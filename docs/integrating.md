# Building on Excalibur View

Written for a developer at a customer's shop, and for whoever is writing the
website's developer pages. Everything here is what the program actually does
today — if something is not built yet it says so.

There are four ways in, and they answer different questions.

| You want to | Use |
|---|---|
| Add a command to the program that reads a drawing and proposes markups | **A plugin** |
| Get quantities out of the office server into your own system | **The API** |
| Let an assistant read your takeoffs | **The MCP endpoint** |
| Ship your own tools, weights and columns | **A tool chest** |

None of them require anything from us. No developer account, no approval, no
store, no revenue share. Your server is yours.

---

## 1. Plugins

A plugin is a **WebAssembly module**. Excalibur View runs it in a sandbox: it
cannot open files, reach the network, see the screen or touch the drawing. It
is handed a description of the sheets and markups as JSON, and hands back what
it found and what it suggests, also as JSON.

**Nothing a plugin says changes the drawing.** It returns *findings*, and a
finding may carry *fixes*. A fix is applied by Excalibur View itself, only when
somebody clicks it. The worst a plugin can do is be wrong.

That shape is deliberate. It is what lets one office run its own tools without
those tools ever being in anybody else's copy of the program.

### What a plugin is for

Real examples: check every sheet is scaled before a bid goes out; find items
counted twice across a set; price a takeoff against your own cost tables;
enforce a company standard for mark numbers; pull a bill of material out in
whatever shape your ERP wants; flag members that changed between issues.

### The contract

A module exports its `memory` and three functions:

```
hv_alloc(len: i32) -> i32            room for len bytes
hv_manifest() -> i64                 the Manifest, as JSON
hv_run(ptr: i32, len: i32) -> i64    runs one command
```

The two that answer return the address in the high 32 bits and the length in
the low 32. In Rust, the `plugin!` macro writes all of that for you.

```rust
use plugin_api::{plugin, Input, Manifest, Output, Command, Level, Finding};

fn manifest() -> Manifest {
    Manifest {
        id: "example-checks".into(),   // short, permanent, letters/digits/dashes
        name: "Example Checks".into(),
        version: "1.0.0".into(),
        publisher: "Example Steel, Inc.".into(),
        description: "Our own checks.".into(),
        abi: plugin_api::ABI,
        commands: vec![Command {
            id: "unscaled".into(),
            name: "Find unscaled sheets".into(),
            description: "Every sheet that has a measurement on it but no scale.".into(),
            scope: plugin_api::Scope::Drawing,
            needs: Default::default(),     // words and line-work both cost time
        }],
        settings: vec![],
    }
}

fn run(input: Input) -> Result<Output, String> {
    let mut out = Output { title: "Unscaled sheets".into(), ..Default::default() };
    for sheet in &input.sheets {
        if sheet.scale.is_none() && input.markups.iter().any(|m| m.page == sheet.page) {
            out.findings.push(Finding::new(
                Level::Warning,
                format!("{} has markups and no scale — every quantity on it is wrong.", sheet.name),
            ));
        }
    }
    Ok(out)
}

plugin!(manifest, run);
```

Build it with `cargo build --target wasm32-unknown-unknown --release`. Any
language that compiles to WebAssembly works; Rust is simply what the helper
macro is written for.

### What it is handed

`Input` carries the command id, the drawing's name, which sheet was on screen,
the sheets, the markups, the person's settings, and the office's custom column
names. A `Command` declares what it `needs` — `words`, `lines`, both or
neither — because reading the line-work of a three-hundred-sheet set takes a
while and a command that only needs text should not pay for it.

**Coordinates are sheet space**: points (1/72 inch of paper), measured right
and down from the top left of the sheet as it is shown. Real lengths are in
feet, real areas in square feet, whatever the sheet is set up in.

### What it hands back

```rust
enum Answer { Done(Output), Failed(String) }
```

An `Output` has a title, a summary, `findings` and `tables`. A `Finding` has a
level, a message, optionally a page and an area (Excalibur View outlines it
while the results are open), the markups it is about, and `fixes`.

A fix carries one or more `Action`s, and this is the whole list:

```
SetSheetScale { page, ratio, text }
SetSubject    { markup, subject }
SetColumn     { markup, column, value }      // custom column, counted from 0
SetQuantity   { markup, quantity }
SetSlope      { markup, rise, run }
AddLength     { page, points, subject }
AddCount      { page, points, subject }
AddArea       { page, points, subject }
AddCloud      { page, area, note }
AddText       { page, area, text }
```

One detail worth knowing: when `AddLength`, `AddCount` or `AddArea` names a
subject the office's tool chest already has a tool for, the markup is made
**with that tool** — colour, columns, unit weight and all. So a plugin that
proposes `W14X38` gets the shop's own W14x38, not a generic line.

A `Table` has columns, rows and an optional totals line, and comes with
`to_csv()`.

### Packaging and signing

A plugin file is one line of magic, one line of JSON header, then the wasm:

```
HVPLUGIN1
{"id":"…","version":"…","name":"…","sha256":"…","bytes":123456,"key":"…","signature":"…"}
<the WebAssembly>
```

The signature is Ed25519 over exactly:

```
hyperview-plugin/1\n<id>\n<version>\n<sha256 lowercase hex>\n
```

`key` names which trusted key signed it. **A plugin that is not signed by a key
the program trusts does not run** — not a warning, it does not run. That is
what stops a plugin arriving by email and being trusted because somebody said
it was fine.

For your own plugins you have two routes, and you should pick deliberately:

- **We sign it**, and it runs in any copy of Excalibur View. Send us the wasm.
- **You add your own key** to `PLUGIN_KEYS` in your own build, and it runs
  only in your shop. A plugin key signs plugins and nothing else.
  Nobody has to ask us for anything, ever. This is the right answer for tools
  that encode your costs or your standards.

### How a plugin reaches a seat

Two ways. An administrator uploads it to the office server — `POST /plugins` —
and every seat gets it automatically on connecting. Or a person loads one on
their own machine, for a tool only they use. Server plugins appear in the
Plugins menu grouped under the office's name.

---

## 2. The API

The office server has an HTTP API, and **its full description lives on your own
server** at `GET /openapi.json`. That document is generated from the routes
themselves and there is a test that fails the build if a route exists without
being described, so it cannot drift out of date.

Point your tooling at it and generate a client. Nothing to read here that the
server will not tell you.

### Getting a key

Studio → Office → Keys → make one, or `POST /me/keys` signed in. Three
purposes:

- **`sync`** — for a program that keeps its own projects in step with this
  server: it files each project under its own reference, puts drawings up,
  archives and brings back those projects, and reads their markups back. That
  is `GET /me`, `POST /projects` (with a `reference`), `POST
  /projects/{id}/archive`, `POST /sets`, `GET /projects/{id}/stamp` and `GET
  /projects/{id}/markuplist`, on projects filed under a reference, and nothing
  else. Every other route refuses it. An administrator makes it, and it acts as
  an estimator. **If your program only does those things, this is the key to
  give it.**
- **`integration`** — the API as the person who made the key. What another
  system uses when it needs more than `sync` covers.
- **`assistant`** — the MCP endpoint only, read only. It sees the jobs its
  person may see and no others.

**No key runs the server**, whoever made it. People, roles, invitations, keys,
remote access, updates, plugins, tool chests, the license and the audit log
take an administrator signed in with their password; a key asking for any of
them is refused with `key_not_allowed`. `GET /me` with a key says which key it
is (`key.purpose`) and `may_administer` (always false for a key), so a program
can refuse to run on a key with more power than it needs.

Keys start `hvk_`, are stored on the server as digests, shown once, and are
revoked one at a time. A key can be given an end with `expires_in_days` (1 to
3650); left out, it never runs out on its own. A key can never be mistaken for
a session because of that prefix.

```
Authorization: Bearer hvk_…
```

### The routes you will actually use

| | |
|---|---|
| `GET /projects` | the jobs |
| `GET /projects/{id}/sets` | the drawing sets in one |
| `GET /sets/{id}/sheets` | sheet numbers, titles, scales |
| `GET /sets/{id}/markups` | every markup as its PDF annotation |
| `GET /sets/{id}/takeoff` | the quantities, totalled |
| `GET /sets/{id}/takeoff.csv` | the same, for a spreadsheet |
| `GET /sets/{id}/markuplist` | the markups list as the program shows it |
| `POST /sets/{id}/copy` | a copy of a set to take off on |
| `POST /sets/{id}/markups` | push markups up |
| `GET /audit`, `/audit.csv` | who did what (administrators) |

A set carries a `revision` that goes up whenever its markups change, so you can
ask for everything since the revision you last saw rather than polling the
whole thing.

**A markup is a PDF annotation dictionary**, not our own invention. That means
a markup Excalibur View drew is a markup every PDF reader shows, and your code
can read it with any PDF library. Everything beside it in the API — subject,
author, quantity, the custom columns — is derived, and is there so you never
have to parse PDF to filter and total.

### Where a job sits, and its drawings' folders

Since 0.6.14 a project has a **place** and a drawing set a **folder**, so the
Studio can show an office's jobs the way it keeps its files: shelves such as
**Active Jobs**, **Bids/2026**, **Lost Bids/2026** and **Past Jobs/2025**, each
folded with how many are on it, and inside a project its drawings in the
folders they came from in the job's own files. A program that keeps jobs (an
estimating or ERP system) says where each one sits and moves it as a bid is
won or lost and a job finishes; the office never moves them by hand.

| | |
|---|---|
| `POST /projects` with `place` | `Bids/2026`. Asked again with the same `reference` and a place, the project moves there; without one, it stays where it is. |
| `POST /projects/{id}/place` | `{"place": "Active Jobs"}` moves it. Null puts it back with the other projects. Nothing on it changes. |
| `POST /sets` with `folder` | The folder the drawing came from, written with `/`. A drawing issued again replaces only the one of the same name **in the same folder** (case and spacing aside), so `S-101` in `10.1 FOR APPROVAL` and in `10.4 FOR FABRICATION` are two drawings. |

A place or folder is folder names between slashes (a backslash counts as one),
none empty, none `.` or `..`, at most 260 characters and twelve deep. A project
with no place is under **Other projects**, and a server where nothing has a
place shows the list it always did.

### Being told instead of asking

Studio → Office → Change notices, or `POST /notices` with a `url`, and the
server tells that address whenever a drawing set's markups change: a couple
of seconds after the change, one small JSON body per set however many pushes
there were.

```json
{"event":"markups.changed","set":"set_…","file":"S-101.pdf","revision":12,
 "project":"prj_…","number":"2640","reference":"fabwire:bid:2640",
 "server":"Example Steel","at":"2026-09-24T19:02:11Z"}
```

`reference` is your own reference for the job, when you filed the project
under one. No drawing or markup travels with a notice; ask the API for what
you want. Each is signed: check `X-Excalibur-Signature: sha256=<hex>` against
HMAC-SHA256 of the raw body, keyed with the secret the server gave you once,
when the address was added. A notice that isn't taken is tried three times,
then dropped, so keep the revision you last saw and catch up by asking.
`POST /notices/{id}/test` sends a `ping`.

### Writing back

`POST /sets/{id}/markups` takes markups and returns the page's new revision.
Every markup carries a stable name (`NM` in the PDF) that survives the round
trip, so you can update one you pushed earlier rather than making a second.

---

## 3. The MCP endpoint

`Hyperview.exe --mcp` speaks Model Context Protocol, so an assistant can read
your takeoffs. Tools available today:

```
list_projects   list_sets   sheets   takeoff
shop_list       counted_twice        revision_cost
takeoff_totals
```

It is **read only**, and an `assistant` key reaches nothing else. An assistant
can tell you what a drawing says; it cannot change one.

In Excalibur View Sealed the assistant is switched off entirely, because the assistant
runs on somebody else's computers and that is the whole of the objection.

---

## 4. Tool chests

A chest is **plain JSON** in a file you own, extension `.evtools`. Not a
database, not a proprietary blob — open it in a text editor.

```json
{
  "format": "excalibur-view-tools",
  "version": 1,
  "name": "Our Steel",
  "columns": [
    { "slot": 0, "name": "LBS Per FT", "kind": "Number", "decimals": 3 },
    { "slot": 1, "name": "Total LBS", "kind": "Formula",
      "formula": "[Length] * [LBS Per FT]", "decimals": 1 }
  ],
  "sets": [
    { "title": "Wide Flange",
      "tools": [
        { "subject": "W14X38", "kind": "Length", "colour": "#1f4fb8",
          "columns": ["38", "", "", "", "", ""],
          "template": "<base64 of the PDF annotation dictionary>" }
      ] }
  ]
}
```

Generate one from your own catalogue and every estimator in the shop has your
sizes, your weights, your naming, your colours. That is how the six chests that
ship with the program are made — the steel one is generated from the AISC
Shapes Database, so a weight can never disagree with the designation above it.

Revu profiles (`.bpx`) and tool sets (`.btx`) are read on the way in and saved
as one of these. After that it is yours.

**One thing worth knowing if you are coming from Bluebeam:** its custom columns
are scoped to the document, not the tool — their own documentation says columns
"operate at the document level, not per individual tool". Open a bid set
without the right profile loaded and every unit weight lands in a column that
does not exist; the weight vanishes and the tonnage is wrong with no error.
Excalibur View keeps the columns with the chest, so this cannot happen.

---

## Which one do I want?

- **Read our quantities into our ERP.** The API, `integration` key, poll
  `revision` and pull `takeoff`.
- **Check our drawings against our own rules.** A plugin, signed with your own
  key.
- **Price a takeoff with our cost tables.** A plugin. It gets the takeoff and
  returns a table, and it never sees the network, so your costs stay in it.
- **Ship our sizes to every estimator.** A tool chest.
- **Let an assistant answer questions about a job.** The MCP endpoint, an
  `assistant` key.

## Things that will not change under you

- A markup is a PDF annotation. Any PDF library reads it.
- A tool chest is JSON you can read.
- The API describes itself at `/openapi.json`, and the build fails if a route
  is missing from it.
- **The API only grows.** A route that is in `/api/v1` stays, under the same
  method and path, and a field that is in it keeps its name and its type. New
  routes and new fields are added; nothing you use is renamed, retyped or
  taken away. The server reads fields it does not know and leaves them, so a
  program written for a later server still talks to an earlier one. A change
  that cannot be made by adding goes into `/api/v2`, beside v1, never in place
  of it. `crates/server/api-v1.json` lists everything v1 has promised, and a
  test fails the build of any release that drops or changes one of them.
- The plugin ABI is versioned. A plugin built for a later contract than the
  program knows is refused with a reason rather than run and misread.
- It all runs on your server. Nothing here needs us to be online, or to exist.

Questions: hello@excaliburct.com.
