#!/usr/bin/env python3
"""Selling Excalibur View Office through Square, from the publisher's own PC.

Square takes the money. A license can only be made here, beside the signing
key, so this script is the bridge: it makes the payment links the website's
Buy buttons point at, and it turns paid orders into signed `.evlicense`
files with a ready-to-send email.

Nothing about a customer is kept anywhere but this computer, and the key is
never sent anywhere.

    python square.py setup                 make (or update) the payment links
    python square.py orders                what has been paid, and what is issued
    python square.py issue                 sign a license for each new order
    python square.py watch                 the same, quietly, for a scheduled task

Beside the signing key it expects:

    square-token.txt    a Square access token (Developer Console -> your app).
                        Permissions: PAYMENTS_READ, ORDERS_READ, ORDERS_WRITE,
                        PAYMENTS_WRITE, MERCHANT_PROFILE_READ.
    square-links.json   written by `setup`: every payment link it has made.
    square-site.json    written by `setup`: the settings to paste into the
                        website's site.json.
    square-orders.json  written by `issue`: which payments already have a
                        license, so nobody is ever charged into two of them.
    licenses/           where the .evlicense files and their emails land.

`issue` never guesses a company name. Square asks the buyer for it at
checkout; if the answer does not come back with the order, that order waits
and says so, and one command with `--company` finishes it.
"""

import argparse
import datetime
import hashlib
import json
import os
import shutil
import subprocess
import zipfile
import secrets
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

import publish

# What the payment links sell. The name is what the buyer sees on the
# checkout page and on their receipt, and what `issue` reads back to tell a
# first purchase from a renewal.
# The trade chests. Sold per company rather than per user: a chest is a file,
# and charging a shop twice because two estimators open the same file is the
# kind of thing that makes people hate software. So no ladder here - one link
# each, quantity one, and the price is the price however many people use it.
CHEST_PRICE = 14900
CHEST_BUNDLE_PRICE = 49900
# The name is the name of the file the buyer gets. Somebody who pays for
# "Structural steel" and is sent "Structural Steel and Misc Metals.evtools"
# has to stop and work out whether they got the right thing, and that doubt
# is worth more than the tidier name.
CHESTS = [
    ("steel", "Structural Steel and Misc Metals",
     "3,439 tools in 27 sets. 2,485 shapes with their unit weights, plus joists and deck, connections, bolts and welds, and misc metals."),
    ("concrete", "Concrete and Earthwork",
     "523 tools in 18 sets. Footings to flatwork, formwork, rebar and mesh, finishes, earthwork, trenching, paving and haul."),
    ("gc", "General Contractor",
     "491 tools in 16 sets. General conditions, demolition, openings, drywall, finishes, roofing, specialties and punch-walk field issues."),
    ("plumbing", "Plumbing",
     "392 tools in 12 sets. Water, waste, storm, gas and fire protection piping, fittings, valves, fixtures and hangers."),
    ("electrical", "Electrical",
     "385 tools in 10 sets. Conduit, wire, tray, devices, lighting, gear, grounding, fire alarm and low voltage."),
    ("mechanical", "Mechanical",
     "364 tools in 12 sets. Duct by gauge with weights, fittings, dampers, air devices, terminal units, equipment and piping."),
]
CHEST_BUNDLE = ("all", "All six trade chests",
                "5,594 tools. Every trade, one price.")

# Sealed is Office for a shop that is not allowed to let a program reach the
# internet at all, and it is sold the same way: per user, with a year of
# updates in the first price. Double, because what it buys is not a feature
# but a guarantee -- every outward connection refused, not merely switched
# off -- and because the shops that need it are the ones that cannot use
# anything else.
SEALED_ITEM = "Excalibur View Sealed - per user"
SEALED_RENEWAL_ITEM = "Excalibur View Sealed updates - per user, one year"
SEALED_PRICE = 50000  # cents
SEALED_RENEWAL_PRICE = 20000

SEATS_ITEM = "Excalibur View Office - per user"
RENEWAL_ITEM = "Excalibur View Office updates - per user, one year"
SEATS_PRICE = 25000  # cents
RENEWAL_PRICE = 10000
MOST_SEATS = 25   # a link per user count up to here; past it, they email us
COMPANY_FIELD = "Company name, as it should read on the license"
EMAIL_FIELD = "Administrator's email (where the license goes)"

SQUARE_VERSION = "2026-05-20"
LIVE = "https://connect.squareup.com"
SANDBOX = "https://connect.squareupsandbox.com"


# ---- talking to Square -----------------------------------------------------

class SquareSaid(Exception):
    """Square refused something, in its own words."""


class Square:
    TRIES = 4

    def __init__(self, token, base=LIVE, version=SQUARE_VERSION):
        self.token = token.strip()
        self.base = base.rstrip("/")
        self.version = version

    def call(self, method, path, body=None):
        url = self.base + path
        data = json.dumps(body).encode("utf-8") if body is not None else None
        headers = {
            "Authorization": "Bearer " + self.token,
            "Square-Version": self.version,
            "Accept": "application/json",
            "User-Agent": "excalibur-square",
        }
        if data is not None:
            headers["Content-Type"] = "application/json"
        for attempt in range(1, self.TRIES + 1):
            request = urllib.request.Request(url, data=data, method=method, headers=headers)
            try:
                with urllib.request.urlopen(request, timeout=60) as response:
                    raw = response.read()
                    return json.loads(raw) if raw else {}
            except urllib.error.HTTPError as e:
                said = e.read().decode("utf-8", "replace")
                if e.code >= 500 and attempt < self.TRIES:
                    time.sleep(3 * attempt)
                    continue
                raise SquareSaid(f"Square said {e.code} to {method} {path}: {reason(said)}")
            except (urllib.error.URLError, TimeoutError, OSError) as e:
                if attempt < self.TRIES:
                    time.sleep(3 * attempt)
                    continue
                raise SystemExit(f"Could not reach Square after {self.TRIES} tries: {e}")

    def paged(self, path, key, params=None):
        """Every page of a list endpoint, oldest first where Square allows."""
        out = []
        params = dict(params or {})
        while True:
            query = urllib.parse.urlencode({k: v for k, v in params.items() if v not in (None, "")})
            answer = self.call("GET", f"{path}?{query}" if query else path)
            out.extend(answer.get(key) or [])
            cursor = answer.get("cursor")
            if not cursor:
                return out
            params["cursor"] = cursor


def reason(said):
    """The sentence out of a Square error, when there is one."""
    try:
        errors = json.loads(said).get("errors") or []
        return "; ".join(e.get("detail") or e.get("code", "") for e in errors) or said[:300]
    except Exception:
        return said[:300]


# ---- the publisher's folder ------------------------------------------------

class Folder:
    """The signing folder: the key, the token, and what has been issued."""

    def __init__(self, path):
        self.path = os.path.abspath(path)
        # A licence is signed with the key made for licences alone when it is
        # here, so the release key only ever signs releases. Licences signed
        # with the release key before 0.6.6 still check out: the server trusts
        # both for licences, and only the release key for anything it runs.
        licence_key = os.path.join(self.path, "licence-signing.key")
        self.key = licence_key if os.path.exists(licence_key) else os.path.join(self.path, "hyperview-signing.key")
        self.token_file = a_file_called(self.path, "square-token")
        self.links_file = os.path.join(self.path, "square-links.json")
        self.issued_file = os.path.join(self.path, "square-orders.json")
        self.licenses = os.path.join(self.path, "licenses")
        self.log_file = os.path.join(self.path, "square-log.txt")

    def token(self):
        if not os.path.exists(self.token_file):
            raise SystemExit(
                f"{self.token_file} is not there. Make an app in Square's Developer Console, "
                "copy its access token into that file, and run this again. (Notepad adds .txt "
                "to what you type, so a file saved as square-token.txt can end up as "
                "square-token.txt.txt - either name works here.)"
            )
        token = open(self.token_file, encoding="utf-8-sig").read().strip()
        if not token:
            raise SystemExit(f"{self.token_file} is empty.")
        return token

    def square(self, sandbox=False):
        return Square(self.token(), SANDBOX if sandbox else LIVE)

    def links(self):
        return read_json(self.links_file, {})

    def save_links(self, links):
        write_json(self.links_file, links)

    def issued(self):
        return read_json(self.issued_file, {})

    def remember(self, payment_id, record):
        issued = self.issued()
        issued[payment_id] = record
        write_json(self.issued_file, issued)

    def say(self, line, quiet=False):
        if not quiet:
            print(line)
        with open(self.log_file, "a", encoding="utf-8") as f:
            f.write(f"{datetime.datetime.now():%Y-%m-%d %H:%M:%S}  {line}\n")


def a_file_called(folder, stem):
    """`square-token.txt` however Windows spelled it.

    Notepad adds `.txt` to a name that already ends in it, so a file saved as
    square-token.txt is often square-token.txt.txt on disk - and with
    Explorer hiding known extensions, both look the same to the person who
    saved it. Anything starting with the stem counts, and the tidiest
    spelling wins.
    """
    wanted = os.path.join(folder, stem + ".txt")
    try:
        here = os.listdir(folder)
    except OSError:
        return wanted
    likely = sorted(
        (f for f in here if f.lower().startswith(stem.lower()) and os.path.isfile(os.path.join(folder, f))),
        key=lambda f: (f.lower() != stem.lower() + ".txt", len(f), f.lower()),
    )
    return os.path.join(folder, likely[0]) if likely else wanted


def read_json(path, empty):
    if not os.path.exists(path):
        return empty
    try:
        return json.loads(open(path, encoding="utf-8-sig").read() or "null") or empty
    except json.JSONDecodeError as e:
        raise SystemExit(f"{path} is not readable ({e}). It has been left alone.")


def write_json(path, value):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(value, f, indent=2, sort_keys=True)
        f.write("\n")


# ---- setup: the payment links ----------------------------------------------

def a_location(square, wanted=None):
    locations = square.call("GET", "/v2/locations").get("locations") or []
    active = [l for l in locations if l.get("status", "ACTIVE") == "ACTIVE"]
    if wanted:
        for l in locations:
            if wanted in (l.get("id"), l.get("name")):
                return l
        raise SystemExit(f"No location called {wanted}. Yours: " + ", ".join(l.get("name", "?") for l in locations))
    if not active:
        raise SystemExit("This Square account has no active location.")
    return active[0]


def link_body(kind, name, price, users, site, note):
    """One payment link, for exactly this many users.

    Square's hosted checkout has no quantity stepper - the count is fixed when
    the link is made - so we make a link per count and the website's own
    stepper sends the buyer to the right one. The idempotency key is worked
    out rather than random, so making them twice asks Square for the same
    links back instead of a second set.
    """
    return {
        "idempotency_key": f"excalibur-{kind}-{price}-{users}",
        "description": note,
        "name": f"{name} x{users}" if users > 1 else name,
        "order": {
            "location_id": None,  # filled in by the caller
            "line_items": [{
                "name": name,
                "quantity": str(users),
                "base_price_money": {"amount": price, "currency": "USD"},
            }],
        },
        "checkout_options": {
            "allow_tipping": False,
            "ask_for_shipping_address": False,
            "enable_coupon": False,
            "enable_loyalty": False,
            "redirect_url": site.rstrip("/") + "/thanks/",
            "custom_fields": [{"title": COMPANY_FIELD}, {"title": EMAIL_FIELD}],
        },
        "payment_note": f"{note} ({users} user{'s' if users > 1 else ''})",
    }


PRODUCTS_TO_SELL = [
    ("seats", SEATS_ITEM, "price",
     "Excalibur View Office, bought once per user. Updates for the first year are included."),
    ("renewal", RENEWAL_ITEM, "renewal",
     "Another year of updates for Excalibur View Office. Everything keeps working either way."),
    ("sealed", SEALED_ITEM, "sealed",
     "Excalibur View Sealed, bought once per user. Every outward connection refused, "
     "set by your administrator through Group Policy or a managed preference. "
     "Updates for the first year are included, and they arrive as files, by hand."),
    ("sealed_renewal", SEALED_RENEWAL_ITEM, "sealed_renewal",
     "Another year of updates for Excalibur View Sealed. A sealed office never fetches "
     "them, so they come to you as a file. Everything keeps working either way."),
]


def a_ladder(square, kind, name, price, most, site, note, location, had):
    """The links for 1..most users, making only the ones that are missing."""
    rungs, made, kept = {}, 0, 0
    for users in range(1, most + 1):
        before = had.get(str(users)) or {}
        if before.get("url") and before.get("price") == price and before.get("item") == name:
            rungs[str(users)] = before
            kept += 1
            continue
        body = link_body(kind, name, price, users, site, note)
        body["order"]["location_id"] = location["id"]
        link = (square.call("POST", "/v2/online-checkout/payment-links", body).get("payment_link") or {})
        rungs[str(users)] = {
            "id": link.get("id"),
            "url": link.get("url") or link.get("long_url"),
            "item": name,
            "price": price,
            "users": users,
            "made": datetime.date.today().isoformat(),
        }
        made += 1
    return rungs, made, kept


def chest_urls(site, download, key):
    """Where a bought chest lives: the page, and the file the page offers.

    Two addresses under the one unguessable name. The page is what a payment
    link sends somebody to, because a chest that arrives in a download bar
    teaches nobody what to do with it - and on a phone it may not even be
    findable. The file sits beside it.

    The page is at the unguessable name too, not at /f/thanks/steel/. That
    matters: the random name is the whole of the difference between having
    paid and not, so a page anybody could guess that links to the file would
    hand the chest away. It is not a lock either way - the format is open and
    a buyer can pass a chest to anybody - but there is no reason to leave it
    lying on the pricing page.
    """
    suffix = "zip" if key == CHEST_BUNDLE[0] else "evtools"
    site = site.rstrip("/")
    return f"{site}/f/{download}/", f"{site}/f/{download}.{suffix}"


def a_download_name(key, had):
    """The address a bought chest is fetched from, made once and kept.

    Not derived from the chest's name. This scheme is open source, so a name
    anybody could work out is a name anybody could fetch, and the chests are
    the thing being sold. A random one, written down in square-links.json,
    is unguessable and never changes -- which matters, because it is baked
    into a payment link that outlives this script.

    What it is not is a lock. A chest is a file, the format is open, and
    somebody who bought one can pass it to anybody. That is the honest
    position and it is the same one we take when we tell people they can
    build their own.
    """
    before = (had.get(key) or {}).get("download")
    if before:
        return before
    return secrets.token_urlsafe(18)


def one_link(square, kind, name, price, site, note, location, redirect=None):
    """A single payment link for something sold once per company.

    No ladder: a chest costs what it costs whether two people use it or
    twenty, so there is one link and nothing for the website to choose
    between.
    """
    body = link_body(kind, name, price, 1, site, note)
    body["order"]["location_id"] = location["id"]
    if redirect:
        body["checkout_options"]["redirect_url"] = redirect
    # `call`, not `post`. There is no `post`, and there never was: this
    # function was written beside a_ladder and never once run, because no
    # chest link had ever been made. The first time anybody sold a chest it
    # would have stopped here.
    made = square.call("POST", "/v2/online-checkout/payment-links", body)
    link = made.get("payment_link") or {}
    return {"id": link.get("id"), "url": link.get("url") or link.get("long_url"), "price": price}


def chest_links(square, site, location, had):
    """One link per trade chest, and one for all six."""
    rungs, made, kept = {}, 0, 0
    products = [(key, f"Excalibur View tool chest - {name}", CHEST_PRICE, note)
                for key, name, note in CHESTS]
    products.append((CHEST_BUNDLE[0], f"Excalibur View tool chests - {CHEST_BUNDLE[1]}",
                     CHEST_BUNDLE_PRICE, CHEST_BUNDLE[2]))
    for key, name, price, note in products:
        already = had.get(key)
        if already and already.get("price") == price and already.get("download"):
            rungs[key] = already
            kept += 1
            continue
        # Paying takes them straight to their chest. No email to send,
        # nothing for anybody to remember to do, and no gap between somebody
        # paying and somebody getting what they paid for.
        download = a_download_name(key, had)
        page, url = chest_urls(site, download, key)
        rungs[key] = one_link(square, f"chest-{key}", name, price, site,
                              f"{note} One price for the whole office.", location,
                              redirect=page)
        rungs[key]["download"] = download
        rungs[key]["url_file"] = url
        rungs[key]["url_page"] = page
        made += 1
    return rungs, made, kept


def cmd_setup(a):
    folder = Folder(a.folder)
    square = folder.square(a.sandbox)
    location = a_location(square, a.location)
    links = folder.links()
    most = max(1, a.most)
    block = {}
    for kind, name, which, note in PRODUCTS_TO_SELL:
        price = getattr(a, which)
        had = (links.get(kind) or {}).get("by_users") or {}
        print(f"{kind}: {most} link{'s' if most > 1 else ''} at {money(price)} each ...", flush=True)
        rungs, made, kept = a_ladder(square, kind, name, price, most, a.site, note, location, had)
        links[kind] = {"item": name, "price": price, "most": most, "by_users": rungs}
        block[kind] = rungs
        print(f"  {made} made, {kept} already there. One user: {rungs['1']['url']}")
    had_chests = (links.get("chests") or {}).get("by_trade") or {}
    print(f"chests: {len(CHESTS) + 1} links ...", flush=True)
    chests, made, kept = chest_links(square, a.site, location, had_chests)
    links["chests"] = {"price": CHEST_PRICE, "bundle": CHEST_BUNDLE_PRICE, "by_trade": chests}
    print(f"  {made} made, {kept} already there. Steel: {chests['steel']['url']}")

    links["location"] = {"id": location["id"], "name": location.get("name")}
    folder.save_links(links)

    lines = {
        "buy_office_url": block["seats"]["1"]["url"],
        "buy_renewal_url": block["renewal"]["1"]["url"],
        "buy_seats_max": most,
        "buy_office_links": {n: r["url"] for n, r in sorted(block["seats"].items(), key=lambda kv: int(kv[0]))},
        "buy_renewal_links": {n: r["url"] for n, r in sorted(block["renewal"].items(), key=lambda kv: int(kv[0]))},
        "chest_price": f"${CHEST_PRICE // 100}",
        "chest_bundle_price": f"${CHEST_BUNDLE_PRICE // 100}",
        "buy_chest_links": {key: r["url"] for key, r in chests.items()},
        "buy_sealed_url": block["sealed"]["1"]["url"],
        "buy_sealed_renewal_url": block["sealed_renewal"]["1"]["url"],
        "buy_sealed_links": {n: r["url"] for n, r in sorted(block["sealed"].items(), key=lambda kv: int(kv[0]))},
        "buy_sealed_renewal_links": {n: r["url"] for n, r in
                                     sorted(block["sealed_renewal"].items(), key=lambda kv: int(kv[0]))},
    }
    out = os.path.join(a.folder, "square-site.json")
    write_json(out, lines)
    print(f"\nThe website's settings are in {out}.")
    print("Copy those five entries into ExcaliburSite\\site.json, run python build.py, and push.")
    print(f'\n  "buy_office_url": "{lines["buy_office_url"]}",')
    print(f'  "buy_renewal_url": "{lines["buy_renewal_url"]}",')
    print(f'  "buy_seats_max": {most},')
    print(f'  "buy_office_links": {{ ... {most} links ... }},')
    print(f'  "buy_renewal_links": {{ ... {most} links ... }}')
    print(f'  "chest_price": "${CHEST_PRICE // 100}",')
    print(f'  "chest_bundle_price": "${CHEST_BUNDLE_PRICE // 100}",')
    print(f'  "buy_chest_links": {{ ... {len(chests)} links ... }},')
    print(f'  "buy_sealed_url": "{lines["buy_sealed_url"]}",')
    print(f'  "buy_sealed_renewal_url": "{lines["buy_sealed_renewal_url"]}",')
    print(f'  "buy_sealed_links": {{ ... {most} links ... }},')
    print(f'  "buy_sealed_renewal_links": {{ ... {most} links ... }}')


# ---- reading what was bought -----------------------------------------------

def money(cents):
    return f"${cents / 100:,.2f}"


def what_was_bought(order, links):
    """(kind, users) from an order's line items: how many seats, or how many
    users' worth of updates. `None` when it is not one of ours."""
    kind, users = None, 0
    for item in (order.get("line_items") or []):
        name = (item.get("name") or "").lower()
        count = int(float(item.get("quantity") or 1))
        # A chest is checked for first: "Excalibur View tool chest - ..." also
        # contains the word that would otherwise read as a seat.
        if "tool chest" in name:
            kind, users = "chest", 1
        # Sealed before Office, and before the renewal test, because every
        # Sealed line item also contains the words the others are looking
        # for: "Excalibur View Sealed updates - per user" has "updates" and
        # "per user" in it. Checked in the wrong order, a shop that paid for
        # Sealed is issued an ordinary Office license -- which would work,
        # and would be the wrong program for the only reason they bought it.
        elif "sealed" in name:
            sealed_renewal = "renewal" in name or "updates" in name
            kind = "sealed_renewal" if sealed_renewal else "sealed"
            users = users + count
        elif "renewal" in name or "updates" in name:
            kind, users = "renewal", users + count
        elif "office" in name or "per user" in name or "seat" in name:
            kind, users = kind or "seats", users + count
    return kind, users


# The two ways of buying each edition: what it is, and what a renewal of it
# looks like. One place, so nothing has to remember four strings.
BUYING = {
    "seats": ("office", False),
    "renewal": ("office", True),
    "sealed": ("sealed", False),
    "sealed_renewal": ("sealed", True),
}


def chest_ordered(order):
    """Which chest somebody bought, by the name on the line item."""
    for item in (order.get("line_items") or []):
        name = (item.get("name") or "")
        if "tool chest" not in name.lower():
            continue
        after = name.split("-", 1)[-1].strip()
        return after or "a chest"
    return None


def answers_in(order):
    """Whatever the buyer typed at checkout, wherever Square puts it.

    Square has moved these about between versions, so rather than trusting
    one place this walks the order for anything that looks like a question
    and an answer. What it cannot find, it does not invent.
    """
    found = {}

    def walk(thing):
        if isinstance(thing, dict):
            title = thing.get("title") or thing.get("label") or thing.get("name")
            value = thing.get("text") or thing.get("value") or thing.get("answer")
            if isinstance(title, str) and isinstance(value, str) and value.strip():
                found.setdefault(title.strip().lower(), value.strip())
            for v in thing.values():
                walk(v)
        elif isinstance(thing, list):
            for v in thing:
                walk(v)

    walk(order.get("metadata") or {})
    for key in ("custom_fields", "fulfillments", "line_items", "source", "checkout_options"):
        walk(order.get(key))

    # A payment link's custom fields come back as one note on the order's
    # fulfillment, a line per field: "Company name, as it should read on the
    # license: Acme Steel". Seen on a real order in September 2026.
    def notes(thing):
        if isinstance(thing, list):
            for v in thing:
                notes(v)
        elif isinstance(thing, dict):
            for k, v in thing.items():
                if k == "note" and isinstance(v, str):
                    for line in v.splitlines():
                        m = re.match(r"^([^:]{2,200}):\s*(.+)$", line.strip())
                        if m:
                            found.setdefault(m.group(1).strip().lower(), m.group(2).strip())
                else:
                    notes(v)

    notes(order.get("fulfillments"))
    return found


def one_of(answers, *words):
    """The answer to a question whose title has all of these words in it."""
    for title, value in answers.items():
        if all(w in title for w in words):
            return value
    return None


def company_of(order, payment, customer):
    """The company a license is for, or None. Never guessed from an email."""
    answers = answers_in(order)
    company = one_of(answers, "company")
    if company:
        return company.strip()
    if customer and (customer.get("company_name") or "").strip():
        return customer["company_name"].strip()
    note = (payment.get("note") or "").strip()
    if note.lower().startswith("company:"):
        return note.split(":", 1)[1].strip()
    return None


def email_of(order, payment, customer):
    answers = answers_in(order)
    for words in (("administrator",), ("email",)):
        found = one_of(answers, *words)
        if found and "@" in found:
            return found.strip()
    for place in (payment.get("buyer_email_address"), (customer or {}).get("email_address")):
        if place and "@" in place:
            return place.strip()
    return None


def paid_orders(square, folder, days, include_issued=False):
    """Completed payments in the window, each with its order and buyer."""
    since = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=days)).strftime("%Y-%m-%dT%H:%M:%SZ")
    payments = square.paged("/v2/payments", "payments", {"begin_time": since, "sort_order": "ASC", "limit": 100})
    issued = folder.issued()
    links = folder.links()
    out = []
    orders = {}
    customers = {}
    for payment in payments:
        if payment.get("status") != "COMPLETED":
            continue
        if payment["id"] in issued and not include_issued:
            continue
        order = {}
        if payment.get("order_id"):
            if payment["order_id"] not in orders:
                orders[payment["order_id"]] = (square.call("GET", f"/v2/orders/{payment['order_id']}") or {}).get("order") or {}
            order = orders[payment["order_id"]]
        kind, users = what_was_bought(order, links)
        if kind is None:
            continue  # somebody's shop sale, not ours
        customer = None
        if payment.get("customer_id"):
            if payment["customer_id"] not in customers:
                try:
                    customers[payment["customer_id"]] = (square.call("GET", f"/v2/customers/{payment['customer_id']}") or {}).get("customer") or {}
                except SquareSaid:
                    customers[payment["customer_id"]] = {}
            customer = customers[payment["customer_id"]]
        out.append({
            "payment": payment,
            "order": order,
            "kind": kind,
            "users": users,
            "company": company_of(order, payment, customer),
            "email": email_of(order, payment, customer),
            "when": payment.get("created_at", ""),
            "paid": (payment.get("amount_money") or {}).get("amount", 0),
            "issued": issued.get(payment["id"]),
        })
    return out


# ---- the licenses this office already has ----------------------------------

def licenses_here(folder):
    """Every license in the folder, newest first, so a renewal or extra seats
    build on the one the customer already has."""
    out = []
    if not os.path.isdir(folder.licenses):
        return out
    for name in sorted(os.listdir(folder.licenses)):
        if not name.lower().endswith("." + publish.PRODUCTS["view"]["extension"]):
            continue
        try:
            lic = json.loads(open(os.path.join(folder.licenses, name), encoding="utf-8-sig").read())
        except Exception:
            continue
        if isinstance(lic, dict) and lic.get("company"):
            lic["_file"] = os.path.join(folder.licenses, name)
            out.append(lic)
    out.sort(key=lambda l: (l.get("issued", ""), l.get("_file", "")), reverse=True)
    return out


def held_by(folder, company):
    same = simple(company)
    for lic in licenses_here(folder):
        if simple(lic["company"]) == same:
            return lic
    return None


def simple(name):
    """A company name with the noise taken out, for matching one sale to the
    licence a customer already has: "Acme Builders, LLC" and "Acme Builders"."""
    name = re.sub(r"[^a-z0-9 ]", " ", (name or "").lower())
    words = [w for w in name.split() if w not in {"inc", "llc", "ltd", "co", "corp", "company", "the"}]
    return " ".join(words)


def next_license(folder, sale, company, key_path):
    """What this sale means for that company's license: a first one, more
    users on the one they have, or another year of updates."""
    today = datetime.date.today()
    held = held_by(folder, company)
    if held and held.get("updates_through") == publish.FOREVER:
        return None, "that company's license already covers every user and every update, for good. Nothing to issue."
    edition, renewing = BUYING.get(sale["kind"], ("office", False))
    if held and held.get("edition", "office") != edition:
        return None, (f"that company holds a {held.get('edition', 'office')} license and this order is "
                      f"for {edition}. The two are different programs, so this one is issued by hand.")
    if not renewing:
        if held:
            if held.get("users") == 0:
                return None, "that company's license already covers every user. Nothing to issue."
            users = held["users"] + sale["users"]
            through = held["updates_through"]
            note = "Added users"
        else:
            users, through, note = sale["users"], publish.a_year_on(today), ""
    else:
        if not held:
            return None, ("a renewal for a company with no license here. Issue their first license first, "
                          "or name the company with --company.")
        users = held["users"]
        # Another year, from the day their updates run out - or from today,
        # when they renew late. Never a year they already had.
        from_day = today
        if held["updates_through"] > today.isoformat():
            from_day = datetime.date.fromisoformat(held["updates_through"])
        through = publish.a_year_on(from_day)
        note = "Updates renewed"
    lic = publish.make_license(
        # WHAT: the company exactly as the licence already held spells it, when there is one. WHY: the office server takes a renewal by
        # itself only when id and company both match (license::is_a_renewal); "Acme Builders" on a licence held as "Acme Builders, LLC"
        # would be paid for and never arrive. The cloud service does the same (licensing/src/index.js).
        key_path, held["company"] if held else company, edition=edition, users=users, updates_through=through, note=note,
        id=held["id"] if held else None,
    )
    return lic, None


# ---- issuing ---------------------------------------------------------------

def email_text(lic, sale, site):
    users = "every user" if lic["users"] == 0 else f"{lic['users']} user" + ("" if lic["users"] == 1 else "s")
    through = "for good" if lic["updates_through"] == publish.FOREVER else f"through {publish.long_date(lic['updates_through'])}"
    edition, renewing = BUYING.get(sale["kind"], ("office", False))
    named = "Excalibur View Sealed" if edition == "sealed" else "Excalibur View Office"
    what = f"Your renewed {named} license" if renewing else f"Your {named} license"
    return (
        f"Subject: {what} - {lic['company']}\n"
        f"To: {sale.get('email') or '(the administrator)'}\n"
        "\n"
        f"Thanks for buying Excalibur View Office. The license file is attached: {users}, updates {through}.\n"
        "\n"
        "One person does this once, on the computer running the office server. Everybody\n"
        "else gets it by connecting - nobody else ever sees a license.\n"
        "\n"
        "  1. Open Excalibur View there and sign in as an administrator.\n"
        "  2. Studio (on the left) - Office - License.\n"
        "  3. Either Add license file... and pick the attachment, or Paste license... and\n"
        "     paste the block below. Either way it is checked on your own server and\n"
        "     nothing is sent anywhere.\n"
        "\n"
        "The license, if you would rather paste than go looking for the attachment:\n"
        "\n"
        f"{json.dumps(lic, indent=2, sort_keys=True)}\n"
        "\n"
        "Keep the file somewhere safe. If the server is ever rebuilt, add it again.\n"
        "\n"
        f"Anything at all, reply to this email.\n"
        "\n"
        f"Excalibur Construction Technologies · {site}\n"
    )


def a_receipt_name(payment_id):
    """The second address a licence is published at: the buyer's own receipt.

    A customer who loses the file has lost the licence id with it, so the
    first address is no use to them. What they still have is the Square
    receipt, and the payment id on it -- unguessable, theirs, and already in
    their inbox.

    So the same licence is published a second time under the payment id put
    through SHA-256. A page on the website can work that out in the browser
    and fetch the file: no account, no password, no service to run, and
    nothing for anybody to look up by hand. Somebody who has not got a
    receipt cannot guess one.
    """
    return hashlib.sha256(("receipt:" + payment_id.strip()).encode("utf-8")).hexdigest() + ".evlicense"


def publish_into(lic, feed, quiet=False, say=print, receipt=None):
    """Puts a signed licence where the customer's server will fetch it.

    The whole of "they bought three more seats": the server asks for its own
    licence by id on the same half-hourly trip it already makes for updates,
    finds a newer one, and takes it. Nobody emails anybody a file and nobody
    waits on an inbox.

    Published under the id put through SHA-256, so the address cannot be
    worked out from a company's name and the customer list is not walkable.
    """
    os.makedirs(feed, exist_ok=True)
    out = os.path.join(feed, publish.license_filename_for(lic["id"]))
    with open(out, "w", encoding="utf-8") as f:
        json.dump(lic, f, indent=2)
    if not quiet:
        say(f"    published -> {os.path.basename(out)}")
    if receipt:
        # The same file again, at the address the buyer can work out from
        # their own receipt. Losing the licence should not mean waiting on
        # somebody to look it up.
        spare = os.path.join(feed, a_receipt_name(receipt))
        with open(spare, "w", encoding="utf-8") as f:
            json.dump(lic, f, indent=2)
        if not quiet:
            say(f"    and under their receipt -> {os.path.basename(spare)}")
    return out


def push_the_feed(site_repo, what, quiet=False, say=print):
    """Commits and pushes the folder the licences are published from.

    Without this the file sits on the signing machine and the customer's
    server never sees it, which is the same as not having published it at
    all. Run on the machine that holds the key, because that is the only
    machine that ever has a licence to publish.
    """
    if not os.path.isdir(os.path.join(site_repo, ".git")):
        raise SystemExit(f"{site_repo} is not a git checkout.")
    was = os.getcwd()
    try:
        os.chdir(site_repo)
        # Someone else may have pushed to the site since (a page change, the
        # other half of this). Taken first, so our push is not refused.
        subprocess.run(["git", "pull", "-q", "--rebase", "--autostash"], capture_output=True, text=True)
        committed = False
        if subprocess.run(["git", "status", "--porcelain", "--", "static/f"],
                          capture_output=True, text=True).stdout.strip():
            subprocess.run(["git", "add", "-A", "--", "static/f"], check=True,
                           capture_output=True)
            # Deliberately says nothing about who bought what. A commit message
            # is forever and this one is about a customer.
            subprocess.run(["git", "commit", "-q", "-m", what], check=True,
                           capture_output=True)
            committed = True
        # Pushed whenever this checkout is ahead of GitHub, not only when
        # something was committed just now: a push that failed last time left
        # a commit here that nothing would otherwise ever send.
        counted = subprocess.run(["git", "rev-list", "--count", "@{u}..HEAD"],
                                 capture_output=True, text=True)
        ahead = int(counted.stdout.strip()) if counted.returncode == 0 and counted.stdout.strip().isdigit() else 0
        if not committed and not ahead:
            if not quiet:
                say("    nothing to push - the feed was already up to date")
            return False
        pushed = subprocess.run(["git", "push", "-q"], capture_output=True, text=True)
        if pushed.returncode != 0:
            raise SystemExit("could not push the licence feed: " + (pushed.stderr or "").strip())
        if not quiet:
            say("    pushed. Their server has it within the half hour.")
        return True
    finally:
        os.chdir(was)


def issue_one(folder, sale, company, quiet=False, site="https://excaliburct.com", feed=None):
    lic, why = next_license(folder, sale, company, folder.key)
    if why:
        folder.say(f"  {sale['payment']['id']}: {why}", quiet)
        return None
    os.makedirs(folder.licenses, exist_ok=True)
    path = os.path.join(folder.licenses, publish.license_file_name(lic))
    publish.write_license(lic, path)
    letter = os.path.splitext(path)[0] + " - email.txt"
    with open(letter, "w", encoding="utf-8") as f:
        f.write(email_text(lic, sale, site))
    folder.remember(sale["payment"]["id"], {
        "company": lic["company"],
        "license": lic["id"],
        # Kept so a later order can be refused when it is for the other
        # edition. Office and Sealed are different programs.
        "edition": lic.get("edition", "office"),
        "users": lic["users"],
        "updates_through": lic["updates_through"],
        "file": path,
        "kind": sale["kind"],
        "paid": sale["paid"],
        "when": sale["when"],
        "issued": datetime.datetime.now().isoformat(timespec="seconds"),
    })
    users = "every user" if lic["users"] == 0 else f"{lic['users']} users"
    folder.say(f"  {lic['company']}: {users}, updates through {lic['updates_through']} -> {os.path.basename(path)}", quiet)
    if feed:
        publish_into(
            lic, feed, quiet,
            say=lambda line: folder.say(line, quiet),
            receipt=sale.get("payment", {}).get("id"),
        )
    return path


def cmd_files(a):
    """Lays out the folder the website serves, from the chests on this disk.

    A bought chest is the same file for every buyer, so there is nothing to
    generate per order: the payment link already points at a fixed address
    and this puts the file there. Run it again whenever a chest changes; the
    addresses do not move, so nobody's link goes stale.
    """
    folder = Folder(a.folder)
    links = folder.links()
    by_trade = (links.get("chests") or {}).get("by_trade") or {}
    if not by_trade:
        raise SystemExit("no chest links yet - run `python square.py setup` first.")

    source = a.chests or os.path.join(a.folder, "chests")
    into = a.into
    os.makedirs(into, exist_ok=True)

    wanted = {key: f"{name}.evtools" for key, name, _ in CHESTS}
    missing = [f for f in wanted.values() if not os.path.exists(os.path.join(source, f))]
    if missing:
        raise SystemExit(f"{source} is missing: {', '.join(missing)}")

    made = []
    for key, filename in wanted.items():
        rung = by_trade.get(key)
        if not rung or not rung.get("download"):
            print(f"  {key}: no download address - run setup again")
            continue
        out = os.path.join(into, f"{rung['download']}.evtools")
        shutil.copyfile(os.path.join(source, filename), out)
        made.append((key, out))

    bundle = by_trade.get(CHEST_BUNDLE[0])
    if bundle and bundle.get("download"):
        out = os.path.join(into, f"{bundle['download']}.zip")
        with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
            for filename in wanted.values():
                z.write(os.path.join(source, filename), filename)
        made.append((CHEST_BUNDLE[0], out))

    print(f"{len(made)} file(s) in {into}:\n")
    for key, out in made:
        size = os.path.getsize(out) / 1024
        print(f"  {key:<11} {os.path.basename(out):<32} {size:>8.1f} KB")
    print("\nThese go in the site's f/ folder. The addresses never change, so a")
    print("payment link made today still works after the next chest update.")
    print("\nEach one also needs a page beside it, which is where paying sends")
    print("people. The site builds these; here is what they are called:\n")
    for key, _ in made:
        rung = by_trade.get(key) or {}
        page, file_url = chest_urls("", rung.get("download", "?"), key)
        print(f"  {key:<11} f/{page.strip('/').split('/', 1)[-1]}/index.html"
              f"   offers  {file_url.lstrip('/')}")


def cmd_orders(a):
    folder = Folder(a.folder)
    sales = paid_orders(folder.square(a.sandbox), folder, a.days, include_issued=True)
    if not sales:
        print(f"No Excalibur orders in the last {a.days} days.")
        return
    for sale in sales:
        mark = "issued " if sale["issued"] else "WAITING"
        who = sale["company"] or "(no company name)"
        if sale["kind"] == "chest":
            # A chest is a file, not a license. Nothing signs it and nothing
            # issues it; somebody sends it. So it says so rather than sitting
            # in the list looking like work the computer forgot to do.
            which = chest_ordered(sale.get("order") or {}) or "a chest"
            print(f"SEND     {sale['when'][:10]}  {money(sale['paid']):>10}  chest    "
                  f"{which}  {who}  {sale['email'] or ''}")
            continue
        print(f"{mark}  {sale['when'][:10]}  {money(sale['paid']):>10}  {sale['kind']:<7} "
              f"{sale['users']:>3} user(s)  {who}  {sale['email'] or ''}")
        if not sale["issued"] and not sale["company"]:
            print(f"           say who it is:  python square.py issue --payment {sale['payment']['id']} "
                  f"--company \"Their Company\"")


def cmd_issue(a):
    refuse_if_the_cloud_is_issuing(a)
    folder = Folder(a.folder)
    sales = paid_orders(folder.square(a.sandbox), folder, a.days)
    if a.payment:
        sales = [s for s in sales if s["payment"]["id"] == a.payment]
        if not sales:
            raise SystemExit(f"{a.payment} is not a new Excalibur order in the last {a.days} days. "
                             "`orders` lists what there is.")
    if not sales:
        folder.say("Nothing new to issue.", a.quiet)
        return
    folder.say(f"{len(sales)} order(s) to issue:", a.quiet)
    waiting = 0
    published = 0
    for sale in sales:
        if sale["kind"] == "chest":
            which = chest_ordered(sale.get("order") or {}) or "a chest"
            folder.say(f"  {sale['payment']['id']}: {which} for "
                       f"{sale['company'] or 'somebody'} - send them the file. "
                       f"Nothing is signed for a chest.", a.quiet)
            continue
        company = a.company or sale["company"]
        if not company:
            waiting += 1
            folder.say(f"  {sale['payment']['id']} ({money(sale['paid'])}, {sale['when'][:10]}): waiting - "
                       f"Square did not pass on a company name. Run: python square.py issue "
                       f"--payment {sale['payment']['id']} --company \"Their Company\"", a.quiet)
            continue
        if a.dry_run:
            folder.say(f"  would issue: {company}, {sale['kind']}, {sale['users']} user(s)", a.quiet)
            continue
        if issue_one(folder, sale, company, a.quiet, a.site, feed=getattr(a, "publish_into", None)):
            published += 1
    if waiting:
        folder.say(f"{waiting} order(s) waiting on a company name.", a.quiet)
    # The last link in the chain. A licence written to this machine and never
    # pushed has not reached anybody, which looks exactly like success from
    # here and like nothing at all from the customer's server.
    if published and getattr(a, "push", False):
        site_repo = a.site_repo or os.path.dirname(os.path.dirname(os.path.abspath(a.publish_into)))
        push_the_feed(
            site_repo,
            f"Publish {published} licence{'s' if published != 1 else ''}",
            a.quiet,
            say=lambda line: folder.say(line, a.quiet),
        )
    if not a.quiet and not a.dry_run:
        print(f"\nThe files and their emails are in {folder.licenses}. Attach the .evlicense to the email.")


def cmd_thanks(a):
    """Re-points chest links that already exist at their page.

    The first seven chest links were made when paying downloaded the file
    straight away. Changing `chest_links` does not touch them - it leaves a
    link that already exists alone, which is the right thing for prices and
    the wrong thing for this - so the change is made here, to the links
    themselves, and the website keeps the same seven URLs it already has.

    Square wants the version it last gave out with every update, so each
    link is read before it is written.
    """
    folder = Folder(a.folder)
    square = folder.square(a.sandbox)
    links = folder.links()
    by_trade = (links.get("chests") or {}).get("by_trade") or {}
    if not by_trade:
        raise SystemExit("no chest links yet - run `python square.py setup` first.")

    changed, already, stuck = 0, 0, []
    for key, rung in by_trade.items():
        download = rung.get("download")
        link_id = rung.get("id")
        if not download or not link_id:
            stuck.append(f"{key}: no download address yet - run setup again")
            continue
        page, file_url = chest_urls(a.site, download, key)

        got = square.call("GET", f"/v2/online-checkout/payment-links/{link_id}")
        link = got.get("payment_link") or {}
        options = dict(link.get("checkout_options") or {})
        if options.get("redirect_url") == page:
            rung["url_page"], rung["url_file"] = page, file_url
            already += 1
            print(f"  {key:<11} already goes to its page")
            continue
        was = options.get("redirect_url") or "(nowhere)"
        options["redirect_url"] = page
        if a.dry_run:
            print(f"  {key:<11} would change {was} -> {page}")
            continue
        square.call("PUT", f"/v2/online-checkout/payment-links/{link_id}", {
            "payment_link": {"version": link.get("version"), "checkout_options": options},
        })
        rung["url_page"], rung["url_file"] = page, file_url
        changed += 1
        print(f"  {key:<11} {was} -> {page}")

    if not a.dry_run:
        folder.save_links(links)
    for line in stuck:
        print(f"  {line}")
    if a.dry_run:
        print("\nNothing was changed. Run it again without --dry-run.")
        return
    print(f"\n{changed} changed, {already} already right.")
    print("Each page needs to exist before anybody buys that chest. The addresses are")
    print("in square-links.json under chests.by_trade, as url_page and url_file.")


def cmd_tidy(a):
    """Finds payment links nothing points at any more, and offers to delete them.

    They happen honestly: `setup` is run, then run again with a different
    shape -- a ladder where there was a single link -- and the first set is
    left behind. Square keeps them forever and they still take money, which
    is the problem: an old link at an old price, still live, still findable
    by anybody who was sent it once.

    A link is an orphan when its id and its URL both appear nowhere in
    square-links.json, which is the file the website is built from. Nothing
    is deleted without --yes, and what would go is printed first.
    """
    folder = Folder(a.folder)
    square = folder.square(a.sandbox)
    known = json.dumps(folder.links())

    links, cursor = [], None
    while True:
        path = "/v2/online-checkout/payment-links?limit=100"
        if cursor:
            path += "&cursor=" + cursor
        answer = square.call("GET", path)
        links += answer.get("payment_links") or []
        cursor = answer.get("cursor")
        if not cursor:
            break

    orphans = [
        l for l in links
        if (l.get("id") or "") not in known and (l.get("url") or "") not in known
    ]
    print(f"{len(links)} payment links, {len(links) - len(orphans)} still pointed at.\n")
    if not orphans:
        print("Nothing to tidy.")
        return
    print(f"{len(orphans)} nothing points at any more:\n")
    for l in orphans:
        print(f"  {l.get('url')}")
        print(f"    id      {l.get('id')}")
        print(f"    made    {(l.get('created_at') or '')[:10]}")
        print(f"    {(l.get('description') or '')[:66]}")
        print()

    if not a.yes:
        print("Nothing was deleted. Run it again with --yes to delete these.")
        print("Deleting a payment link cannot be undone, and anybody holding it")
        print("gets an error rather than a checkout -- which is the point.")
        return

    gone = 0
    for l in orphans:
        try:
            square.call("DELETE", f"/v2/online-checkout/payment-links/{l['id']}")
            print(f"  deleted {l.get('url')}")
            gone += 1
        except SquareSaid as e:
            print(f"  {l.get('url')}: {e}")
    print(f"\n{gone} deleted.")


def cloud_is_issuing(site):
    """Whether the licence service on the website is signing licences itself.

    From 0.6.6 it is: a licence is signed the moment Square says paid, by the
    service on Cloudflare, whether or not this PC is on. This script stays as
    the manual fallback, and it must not sign what the service already has,
    or a shop that bought three seats would be given six.

    True or False when the service answered; None when it could not be asked.
    It sends a name of its own, because Cloudflare turns away Python's default
    one, and a refusal read as "not issuing" would sign everything twice.
    """
    request = urllib.request.Request(site.rstrip("/") + "/api/licence/health",
                                     headers={"User-Agent": "excalibur-square", "Accept": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=15) as r:
            return bool(json.load(r).get("issuing"))
    except urllib.error.HTTPError as e:
        # No service there at all: this PC is the only thing that signs.
        return False if e.code == 404 else None
    except Exception:
        return None


def refuse_if_the_cloud_is_issuing(a):
    if getattr(a, "force_local", False) or getattr(a, "sandbox", False):
        return
    issuing = cloud_is_issuing(a.site)
    admin = a.site.rstrip("/") + "/api/licence/admin"
    if issuing:
        raise SystemExit(
            "The licence service on the website is issuing licences on its own, so this does not.\n"
            "Anything that needs a person is on its admin page: " + admin + "\n"
            "(--force-local signs here anyway. Only for when the service is down and a customer is waiting.)"
        )
    if issuing is None:
        raise SystemExit(
            "Could not ask the license service whether it is issuing, so this does not sign,\n"
            "in case it is and the customer would be given their seats twice. Look at " + admin + "\n"
            "(--force-local signs here anyway, once you are sure the service has not.)"
        )


def cmd_watch(a):
    """Issues and publishes whatever has been paid for, over and over.

    Meant to be left running, or fired by a scheduled task. With
    `--publish-into` and `--push` this is the whole path from somebody
    pressing Buy to their server holding the licence, with nobody in the
    middle: Square is asked what has been paid, each order is signed against
    the key on this machine, the licence is written under its published name,
    and the folder is pushed. Their server picks it up on the same
    half-hourly trip it already makes.

    Run it once with `--every 0`, which is what a scheduled task wants.
    """
    a.quiet = True
    a.company = None
    a.payment = None
    a.dry_run = False
    if a.every <= 0:
        cmd_issue(a)
        return
    print(f"Watching for paid orders every {a.every}s. Ctrl-C stops it.")
    while True:
        try:
            cmd_issue(a)
        except SystemExit as stop:
            # One bad run must not end the watch: a network hiccup at three in
            # the morning should not mean nobody gets a licence until somebody
            # notices in the morning.
            print(f"[{datetime.datetime.now():%Y-%m-%d %H:%M}] {stop}", flush=True)
        except Exception as e:  # noqa: BLE001 - the same reasoning
            print(f"[{datetime.datetime.now():%Y-%m-%d %H:%M}] {e}", flush=True)
        time.sleep(a.every)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--folder", default=os.path.dirname(os.path.abspath(__file__)),
                   help="the signing folder (where the key and the token are)")
    p.add_argument("--sandbox", action="store_true", help="Square's test account rather than the real one")
    p.add_argument("--site", default="https://excaliburct.com")
    sub = p.add_subparsers(dest="command", required=True)
    fi = sub.add_parser("files", help="lay out the f/ folder the website serves")
    fi.add_argument("--into", required=True, help="where to write them (the site's f/ folder)")
    fi.add_argument("--chests", help="where the .evtools files are (default: <folder>/chests)")

    th = sub.add_parser("thanks", help="point chest links at their page rather than the file")
    th.add_argument("--dry-run", action="store_true", help="say what would change and change nothing")

    s = sub.add_parser("setup", help="make the payment links the website's Buy buttons use")
    s.add_argument("--price", type=int, default=SEATS_PRICE, help="cents per user, one time")
    s.add_argument("--renewal", type=int, default=RENEWAL_PRICE, help="cents per user per year")
    s.add_argument("--sealed", type=int, default=SEALED_PRICE, help="Sealed: cents per user, one time")
    s.add_argument("--sealed-renewal", type=int, default=SEALED_RENEWAL_PRICE,
                   dest="sealed_renewal", help="Sealed: cents per user per year")
    s.add_argument("--most", type=int, default=MOST_SEATS,
                   help="the largest number of users with a link of its own (default %(default)s)")
    s.add_argument("--location", help="which Square location, by name or id")
    o = sub.add_parser("orders", help="what has been paid, and what is issued")
    o.add_argument("--days", type=int, default=60)
    i = sub.add_parser("issue", help="sign a license for each new paid order")
    i.add_argument("--days", type=int, default=60)
    i.add_argument("--payment", help="just this one payment")
    i.add_argument("--company", help="the company name for it, when Square did not pass one on")
    i.add_argument("--dry-run", action="store_true")
    i.add_argument("--publish-into", dest="publish_into",
                   help="the site's static/f folder, so a licence publishes itself")
    i.add_argument("--push", action="store_true",
                   help="commit and push the feed afterwards, so it actually reaches them")
    i.add_argument("--site-repo", dest="site_repo",
                   help="the site checkout, if it is not two levels above --publish-into")
    i.add_argument("--quiet", action="store_true")
    i.add_argument("--force-local", dest="force_local", action="store_true",
                   help="sign here even though the licence service is issuing (only if it is down)")
    td = sub.add_parser("tidy", help="find payment links nothing points at, and delete them")
    td.add_argument("--yes", action="store_true",
                    help="actually delete them (without it, this only lists them)")
    w = sub.add_parser("watch", help="issue and publish, over and over")
    w.add_argument("--days", type=int, default=7)
    w.add_argument("--every", type=int, default=0,
                   help="seconds between checks; 0 runs once and stops (default)")
    w.add_argument("--publish-into", dest="publish_into",
                   help="the site's static/f folder, so a licence publishes itself")
    w.add_argument("--push", action="store_true",
                   help="commit and push the feed afterwards, so it actually reaches them")
    w.add_argument("--site-repo", dest="site_repo",
                   help="the site checkout, if it is not two levels above --publish-into")
    w.add_argument("--force-local", dest="force_local", action="store_true",
                   help="sign here even though the licence service is issuing (only if it is down)")
    a = p.parse_args()
    try:
        {"setup": cmd_setup, "orders": cmd_orders, "issue": cmd_issue,
         "watch": cmd_watch, "files": cmd_files, "thanks": cmd_thanks, "tidy": cmd_tidy}[a.command](a)
    except SquareSaid as e:
        raise SystemExit(str(e))


if __name__ == "__main__":
    main()
