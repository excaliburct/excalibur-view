# The tablet manifest (contract between Fabwire and Excalibur View)

**Schema:** 1 · **Since:** Excalibur View 0.8.0 · **Shared examples:** `docs/tablet-manifest-vectors.json`
(the same file, byte for byte, is in FabwireV2 at `docs/tablet-manifest-vectors.json`; both repos test against it).

This page is the one spec. FabwireV2's `FABWIREV2.md` (the Tablets decision) points here and keeps a copy of it
at `docs/TABLET-MANIFEST.md`. Change both together, and bump `schema` for anything an older reader would get wrong.

## 1. What it is

One small record per job. The office decides in Fabwire (a job's **Tablets** tab) what the shop and the field see on
their tablets; Fabwire turns that into this manifest and pushes it to Excalibur View. Excalibur View's server keeps it
and **holds every shop and field account to it**: it is the only thing such an account is served for that job.

- Fabwire builds it from the job: the sections on or off for Shop and Field, the sheets **released for fabrication**
  (document control, Decision 0424), the documents the office pinned, this week's loads with each ticket box's
  sign-off line, the traveler and loading summaries, and the "do this now" items.
- It only ever **narrows** what the folder rules already allow. A set or file a person's folder rules hide stays
  hidden whatever the manifest says.
- Signing is unchanged: the tablet signs the ticket's existing sign-off line (`POST /signoffs/{id}/sign`), Fabwire
  reads it back and writes RECEIVED BY, Delivered and the signed ticket as before (Decisions 0404, 0410, 0411).
  One rule is added: **who may sign which box** (section 5, "Who signs which box").

## 2. The routes

| Route | Who | What |
|---|---|---|
| `PUT /api/v1/projects/{id}/tablet` | **a sync key only** (Fabwire's program key), on a project it keeps (one with a reference) | Sets the job's manifest. Body: the manifest, at most 8 MiB. |
| `GET /api/v1/projects/{id}/tablet` | a person who may see the project; a sync key | The manifest **as this person's tablet shows it** (section 5). A sync key reads it as stored. |
| `GET /api/v1/tablet` | a person | Their jobs for the tablet: every project they may see that has a manifest, with its card words. |

- `PUT` from a person (even an administrator) or any other kind of key: **403**. Only Fabwire sets it. Among sync
  keys only the one Fabwire makes (its name begins `Fabwire`, as `Fabwire V2`) may; a sync key an administrator
  made for another program is **403** `program_only` too.
- An exact repeat of a sheet, a pinned document or a load in the body is kept once (the first stays, in order); the
  manifest is never refused for it.
- A job that leaves active (completed, on hold, past, cancelled) is pushed as an **empty manifest**: no sheets,
  documents, loads, traveler, loading or to-dos, and no sections for either audience, so its tablets drop it.
- `PUT` answers `{"project": "<id>", "version": N, "changed": true|false}`. A body identical to the one kept
  (the same bytes after the server's own canonical form) changes nothing, writes no audit line and answers
  `changed: false`, so Fabwire may push every minute.
- Every change writes one audit line, `tablet_manifest_set`, under the key's maker, naming the project and saying
  how many sheets, documents and loads it holds and the sections for Shop and Field.
- All three routes are held by the device gate as every other API route (`Door::Gated`): a program key passes as a
  key, a tablet as an approved device.
- A refusal of a bad manifest is **400** `bad_request`; its message starts with the code from section 4
  (`not_released: ...`), so both sides can test the same cases.

## 3. The shape

```jsonc
{
  "schema": 1,                                   // required; this spec
  "job": {                                       // required; words for the cards, each at most 200 characters
    "number": "2601", "name": "TCU Campus 2",    // required
    "town": "Fort Worth", "gc": "Ridgeline Builders",
    "site": "Gate 3, laydown yard", "office": "Jordan (PM) · ext. 214"   // optional, "" when unknown
  },
  "sections": {                                  // required; which tiles each audience gets
    "shop":  ["drawings", "shipping", "traveler", "loading", "documents"],
    "field": ["drawings", "shipping", "documents"]
  },                                             // "drawings" is always on, said or not
  "sheets": [{                                   // released for fabrication only, newest revision only
    "set": "<View set id>", "sheet": "S-101", "title": "Framing plan, level 2",
    "rev": "3", "released": "2026-11-20",        // released: YYYY-MM-DD, required
    "discipline": "structural",                  // anchor_bolts | structural | misc | erection | joist_deck
    "marks": ["B12", "B10"]                      // piece marks on it, for search; may be empty
  }],
  "documents": [{                                // pinned by the office
    "kind": "file",                              // file (a listed file) | set (a drawing set)
    "id": "<View file or set id>", "name": "Site logistics plan",
    "for": ["shop", "field"]                     // one or both
  }],
  "loads": [{                                    // this week's loads (not drafts)
    "number": 2, "ticket": "2601-02", "set": "<the ticket's View set id>",
    "status": "loaded",                          // loading | loaded | in_transit | delivered | returned
    "ships": "2026-11-23",                       // YYYY-MM-DD or ""
    "weight_lb": 18450, "capacity_lb": 48000, "trailer": "Flatbed 48'",
    "destination": "Gate 3, laydown yard",
    "pieces": [{ "mark": "B12", "qty": 3, "description": "W12x26 beam, Grid B", "length": "24'-6\"", "weight_lb": 1911 }],
    "boxes": [                                   // the ticket's three boxes, in the form's order
      { "box": "loaded_by",   "ref": "",                       "signer": "Luis Ramos", "signed": "2026-11-23" },
      { "box": "driver",      "ref": "",                       "signer": "Ben Novak",  "signed": "2026-11-23" },
      { "box": "received_by", "ref": "ship:<job>:2:ReceivedBy", "signer": "",           "signed": "" }
    ]                                            // ref: the sign-off line's reference on the ticket's set ("" once filled)
  }],
  "traveler": { "set": "<View set id>", "pieces": 86, "complete": 61, "open_stations": 18 },   // or null
  "loading":  { "set": "<View set id>", "load": 3, "weight_lb": 22100, "capacity_lb": 48000, "ready_pieces": 9 }, // or null
  "todo": [{ "section": "shipping", "load": 2, "words": "Load 2 is on site: sign the ticket",
             "detail": "Received by is still blank" }]   // most urgent first; at most 20
}
```

Unknown fields are refused (`unknown_field`), so a newer Fabwire never sends something an older View silently drops.

## 4. What the server refuses (400, code first)

| Code | When |
|---|---|
| `schema` | `schema` is missing or not 1. |
| `unknown_field` | a field this spec doesn't name, anywhere, or a field of the wrong type. |
| `bad_section` | a section that isn't drawings, shipping, traveler, loading or documents. |
| `not_released` | a sheet with no `released` date, or one not shaped YYYY-MM-DD. **The released-only rule.** |
| `bad_discipline` | a discipline not in the list. |
| `bad_box` | a box that isn't loaded_by, driver or received_by, or one named twice on a load. |
| `bad_status` | a load status not in the list. |
| `bad_audience` | a document `for` empty, or naming anything but shop and field. |
| `too_long` | text over 200 characters, or with control characters. |
| `too_many` | more than 5,000 sheets, 500 documents, 200 loads, 2,000 pieces on a load, 20 to-dos or 200 marks on a sheet. |
| `not_in_project` | a set or file id that isn't this project's (the server checks; the vectors can't). |
| `out_of_use` | a set that is superseded in View, or in a folder named `` `OUT OF USE (OFFICE ONLY) `` at any depth (the server checks). |

## 5. Who is served what (the enforcement)

A **tablet account** is a person whose role is *viewer* (the folder rules' "not the office"). Administrators,
estimators and keys are never narrowed. A tablet account's **audiences** are its groups named `Shop` and `Field`
(case aside): one, both, or none. A tablet account in neither group reaches nothing of a job that has a manifest.

On a project that **has a manifest**, a tablet account may reach only:

1. **Sets**: each sheet's set (Drawings is always on, for both audiences); each load's ticket set when `shipping` is
   on for one of its audiences; the traveler's set when `traveler` is; the loading log's set when `loading` is; a
   pinned set when `documents` is on and the pin is `for` one of its audiences.
2. **Files**: a pinned file, by the same rule.
3. **Nothing else**: no other set or file, and **no folder listing** (an empty list). Every route that reads a set or a
   file (its bytes, sheets, markups, takeoff, markup lists, sign-offs, stamps, the assistant) goes through the same
   check, so a hidden item is "not found" everywhere, exactly as one that doesn't exist.

And always, whatever the manifest says: the folder rules still apply underneath (an item must pass both); a set
superseded in View, or in an `` `OUT OF USE (OFFICE ONLY) `` folder, is refused.

A project with **no manifest** is served by the folder rules alone, as before 0.8.0.

**Who signs which box.** Fabwire asks each ticket line with a `role` (`loaded_by`, `driver` or `received_by`) on
`POST /sets/{id}/signoffs`; a line asked again with a role it had none for takes it once, and a role is never changed
after. `POST /signoffs/{id}/sign` then refuses (**403**) any account the line's role doesn't allow: **Received by**
and **Driver** are signed by the field or the office; **Loaded by** by the shop, the field or the office. The
office is every audience; a tablet account is its `Shop` and `Field` groups. A line with no role is signed by
anyone who reaches its set, as before. The tablet offers only the Sign buttons its account may use. A role that
isn't one of the three is **400**.

`GET /projects/{id}/tablet` for a tablet account answers the manifest cut to what that person may reach: sections
for its audiences, only reachable sheets, documents and loads, and on each load box the **live** sign-off line
(`line`: its id, `status`, `signer`, `signed`), so a box signed a moment ago shows signed before Fabwire's next pass.

## 6. The shipping fix

Fabwire's starting folder rule for `08 SHIPPING DOCS` was `office+shop`, which hid the tickets from the field crew
that signs them on site. It is `office+shop+field` from this release (an office that kept the old starting rule is
moved to the new one, once, only while the connected View is 0.8.0 or later and only when its saved rule is exactly
the old starting rule; one it changed itself is left alone, and the office is told in a notice when it is moved). The manifest then narrows the field to the tickets of
this week's loads, nothing else in that folder.

## 7. The shared examples

`docs/tablet-manifest-vectors.json`:

- `valid`: manifests both sides must accept, and Fabwire's records must read and write back unchanged (as JSON).
- `invalid`: each with the `code` both validators must give.
- `reaches`: for a valid manifest and an audience (`shop`, `field`, `both`, `none`: a viewer in neither group reaches nothing), the exact sets and files a tablet
  account may reach (before the folder rules), sorted. Fabwire's "What Sam sees" preview and View's server both
  compute it and must agree.

Ids in the examples are symbolic (`set_s101`); View's server tests swap in real ids where the server checks them.
