# Excalibur View

A native Windows and macOS drawing viewer, markup and takeoff program, by
Excalibur Construction Technologies. Free for one person; offices add
Excalibur View Office for the server that lets their people work together.

*Hyperview* is the name it was built under, and it is still the name inside
it: the program files (`Hyperview.exe`, `Hyperview-Server.exe`), their
folders, `hyperview://` links, the API's paths and the update feed all keep it,
so every office that already has it keeps updating without anybody touching
anything. What people see — window, menus, shortcuts, Settings → Apps — says
Excalibur View.

## What it looks like

![Excalibur View with a second floor framing plan open: the tool chest down the left, every beam measured and labelled, and 52.020 tons on the status bar](https://excaliburct.com/assets/img/screens/view-main.webp)

Takeoff on a real set. The tools carry the weights, so measuring a beam puts
tonnage on the status bar as it is drawn.

![The Shop List: a cut list by shape with pieces, total length and weight, footing up to 87 pieces and 104,040 lb](https://excaliburct.com/assets/img/screens/view-shoplist.webp)

The same markups read as a cut list: stock length, saw kerf, what is worth
keeping as a drop, and what to buy. Cutting lengths round up, never down.

A thirty-five second demo of the same job runs at
[excaliburct.com](https://excaliburct.com/), unnarrated and nothing sped up.

## What a company is given

Two files. Nothing else — no installer, no DLLs, no readme, no scripts.

| | |
|---|---|
| `Hyperview.exe` | Double-click it on each computer. It installs itself for that person (their own app folder, Start menu, desktop, Settings → Apps; no administrator), opens, and finds the office server by itself. |
| `Hyperview-Server.exe` | Double-click it once on the office server. Windows asks for permission once; it installs itself as a proper Windows service, opens the firewall for the office network only, and serves with nobody signed in. The first person to open Hyperview sets it up and gets the join code for everybody else. |

Both keep themselves up to date. The server reads the publisher's release feed
every half hour, and again whenever a seat asks and its last look is ten
minutes old; it checks each new build against the signing key compiled into
it, offers the viewer to every seat — each of which checks it again before
installing — and replaces itself. A seat asks its server every quarter of an
hour (a seat with no server asks the feed hourly), fetches a new version in
the background, and says so in a box in the middle of the window with what
is new in it: **Restart now**, or **Later** and it takes over the next time
Hyperview opens. Nothing restarts on its own in the middle of anybody's
work. **Help → Check for Updates** asks there and then.

If a seat cannot put a new version in place itself, the box says why and
**Download** opens the releases page. What each seat did is written to
`hyperview.log` in `%LOCALAPPDATA%\Excalibur Hyperview`.

## Where it runs

- **Hyperview.exe**: Windows 10 and 11, and Windows Server 2016, 2019, 2022
  and 2025 with the Desktop Experience, including over Remote Desktop. A
  server with no graphics card draws with Windows' own software Direct3D
  rather than failing to start. Each person on a Remote Desktop server gets
  their own copy, their own window and their own settings.
- **Hyperview-Server.exe**: the same, plus Server Core — it is a console
  program and a service, and needs no desktop.
- Neither needs PowerShell, which a hardened server or a Remote Desktop host
  may not let an ordinary user start: elevation, shortcuts, the Settings entry
  and the firewall are all asked of Windows directly.
- Windows Server 2012 R2 and older are not supported: the toolchain's
  programs need calls that arrived with Windows 10.

## Printing

Ctrl+P opens Hyperview's own Print dialog over the drawing: the printers and
paper sizes Windows has, the printer's own Properties dialog, orientation
(Auto turns each sheet to suit the paper), fit-to-paper or actual size,
copies, and a preview of the sheet on the paper. The sheets go straight to the
printer, drawn by the PDF engine as vectors with their markups. No other
program is opened.

## Snapping

With a measuring or drawing tool in hand, the point being put down is pulled
onto the drawing's own line-work: ends of lines, where lines cross, circle
centres, midpoints, and anywhere along a line — in that order of preference —
with Revu's marks on screen saying which it caught. Markup corners and a grid
can be snapped to as well. Each can be turned off under View; holding Ctrl
turns snapping off for as long as it is held; Shift squares a line up.

## Measuring how fast it is

```
Hyperview.exe --benchmark "C:\path\to\a drawing set.pdf"
```

opens the drawing, sits still, drags, spins the wheel and flips through the
sheets, then writes `hyperview-benchmark.txt` beside the drawing: frame times,
how long the screen takes to go sharp again after each of those, tile times,
and what snapping costs. `HYPERVIEW_TILE_THREADS=0` and
`HYPERVIEW_NO_PREFETCH=1` switch off the parallel tile drawing and the
drawing-ahead, to see what each is worth.

## Publishing a release

The signing key is the publisher's and lives only on the publisher's own
computer, beside `tools/publish.py`. Its public half is compiled into every
build, in `crates/hyperview/src/trust.rs`, which the server reads too.

```
python3 publish.py sign    --key hyperview-signing.key --version 0.5.1 \
                           --channel preview --notes-file notes.txt \
                           --app Hyperview.exe --server Hyperview-Server.exe
python3 publish.py upload  --token-file github-token.txt \
                           --repo excaliburct/hyperview-releases --version 0.5.1 \
                           --notes-file notes.txt \
                           release.json Hyperview.exe Hyperview-Server.exe
```

A release goes up as `preview` — a GitHub prerelease, taken only by an office
whose administrator chose "get them early". Once it has been tried there:

```
python3 publish.py promote --key hyperview-signing.key \
                           --token-file github-token.txt \
                           --repo excaliburct/hyperview-releases --version 0.5.1
```

and every office takes it. The channel is part of what is signed, so a
manifest edited by hand is refused everywhere.

From 0.6.2 the programs go up as `ExcaliburView.exe` and
`ExcaliburView-Server.exe` — the names the website's download buttons look for
(excaliburct.com reads the latest release straight from GitHub). Every copy
already installed finds its update by the name `release.json` gives, never by
one of its own, so the rename needs nothing from them
(`server/tests/kept_up_to_date.rs` checks it); and the program installs itself
under its own name whatever the downloaded file was called. `upload` matches
each file to the manifest by fingerprint, so the files on the publisher's PC
can be called anything.

`upload` is patient with a home connection: a dropped or reset connection, or a
5xx from GitHub, is tried again; a draft left by a run that stopped part way is
found and finished rather than a second one made; every file is checked on the
release, whole, before the release is made public. The kit's PowerShell script
keeps everything it prints in `publish-log.txt` beside it.
`crates/hub/tests/signed_by_the_publishing_script.rs` checks a manifest the
script really made, so the script and the programs cannot drift apart.

## Building it

You need Rust (https://rustup.rs). From Linux, `cargo build --release --target
x86_64-pc-windows-gnu` with mingw-w64 installed builds the Windows programs.

```
cargo build --release
```

What comes out:

| | |
|---|---|
| `hyperview.exe` | the viewer — ship it as `Hyperview.exe` |
| `hyperview-server.exe` | the server — ship it as `Hyperview-Server.exe` |
| `hyperview-release.exe` | signing and checking releases by hand |

A Windows build carries the PDF engine inside it when
`third_party/pdfium/embedded/pdfium.dll` is present at build time (the
`pdfium-win-x64` archive from https://github.com/bblanchon/pdfium-binaries,
chromium/7881). Without it the program needs `pdfium.dll` beside it. On Linux,
set `PDFIUM_DIR` to the folder holding `libpdfium.so`.

```
cargo test --release --workspace
```

Everything should pass. If something does not, that is the bug — the tests are
the specification.

## Running it

Double-clicking a PDF with Hyperview opens it; several open as tabs; dragging a
PDF onto the window opens it. `HYPERVIEW_PORTABLE=1` stops a copy installing
itself, for running a build straight out of `target`.

### Your tool chest

Hyperview ships with none, on purpose — a tool chest is the estimator's own
work. An administrator shares one with the whole office from the server panel
(**Shared tool chests → Share one**), and every seat can use it. A `.bpx` or
`.btx` put in a `profiles` folder beside the program is read at start-up too, or
set `HYPERVIEW_PROFILE` to one file.

## The server

Installed by double-clicking, as above. It keeps everything in
`C:\ProgramData\Excalibur Hyperview\Server\hyperview-data` — the database, the
drawings, and `hyperview-server.log` — and that folder is the one to back up.
`Hyperview-Server.exe --uninstall` (as administrator) removes the service and
firewall rules and leaves the drawings where they are.

Elsewhere, `hyperview-server --console` runs it in a window, keeping
`hyperview-data` beside the program; `HYPERVIEW_FIRST_ADMIN=1` prints a
password for a first administrator on a box nobody will open Hyperview near.

It serves the API on port 8714 and answers on the local network on 8715, so
seats find it without anybody typing an address —
`crates/server/tests/found_on_the_network.rs` is the end-to-end proof of that,
because it is the one thing in the product that cannot be worked around by
hand. The description of that API is at `/api/v1/openapi.json`, and
`docs/SPEC.md` explains what it is for.

Everything the desktop does is on that API, so anything else — an ERP, a
spreadsheet, a fleet tool — can ask the same questions and get the same answers.
The one worth knowing about is `GET /api/v1/sets/{id}/takeoff`.

## The markups list

A bar along the bottom of the window says how many markups there are, what they
weigh and whether anything was left out for want of a scale. Drag it up, or
click it, and it is every markup — on this sheet, this drawing, or every open
drawing — grouped by subject, sheet, label, type, author, status, colour or
drawing, each group with its own count, length, area and tons. **Export**
writes exactly what it is showing as Excel (numbers as numbers, a Summary
sheet), CSV, the PDF summary, XML, HTML, or JSON shaped like Bluebeam's markup
list. The tool chest's own columns ("LBS Per FT", "TTL Length LBS") appear by
name, with formula columns worked out; the unit-weight columns are found by
name, so another company's chest weighs its own steel.

## Quantity and slope

**Properties** on a selected measurement has a **Quantity** and, on a length, a
**Slope**. Draw one line for six identical beams, set ×6, and every total,
weight, cut list and export counts six; the sheet shows `×6` beside it. A
slope turns a rafter or stringer drawn in plan into its length up the pitch.
Both are written in the markup — the slope in Revu's own keys, so Revu reads
it too — and travel with the drawing.

## Plugins

An office's own tools, kept out of everybody else's copy. A plugin is a
signed `.hvplugin` file; an administrator adds it from **Plugins → Add
Plugin…** and the office server hands it to every seat the way it hands out
the tool chest. Another company's server never has it, so another company's
Hyperview never shows it.

A plugin runs sealed off, in an interpreter: no files, no network, no screen,
and a fixed budget of work and memory. It is handed the sheets — the words
printed on them, their line-work, their scales — and the takeoff, and hands
back what it found, outlined on the sheet, with fixes offered as buttons.
Nothing changes the drawing until somebody clicks one, and each click is one
step on the Undo list. A plugin that is not signed by a key this program was
built with is refused by the server and again by every seat. See
`docs/plugins.md`.

An office's own checks — scales, sizes read off the drawing, missed members,
takeoff coverage, connections — are what a plugin is for: each is built on its
own, kept by the office that wrote it, and never part of the program.

## Excalibur View Office: the license

The desktop app is free for one person, for any work, and never asks for a
license. Office is the server, and is licensed by a small signed file
(`.evlicense`) the administrator adds under **Studio → Office → License**, or
drops in the server's data folder. The server checks it against the keys built
into the program; nothing is sent anywhere to do it (`crates/server/src/license.rs`,
`crates/hub/src/license.rs`).

- **Trial:** a new server runs everything for 30 days.
- **No license after that:** everything already there can still be opened,
  printed and exported; sharing new work through the server pauses (`403
  license_needed`, never a sign-out). Nothing is deleted.
- **Users:** a license covers a number of accounts; at the limit the next
  person waits, and nobody already on is turned away.
- **Updates:** the server only replaces itself with versions published on or
  before the license's *updates through* date. The desktop app is free and is
  always offered. A newer server put in **by hand** — built after the updates
  ended — pauses sharing like an ended trial, and says the two ways out: renew
  and add the new file, or put back a version the license covers. Administrators
  are told 30 days before updates end, and after; nothing stops working.
- **Founding installs:** a server that was already in use before licensing
  (people on it from before 1 October 2026 the first time a licensing version
  starts) is Office for good — every user, every update, no file needed. A
  founding license file (`publish.py sign-license --founding`) covers a fresh
  install on new hardware.

Licenses are signed on the publisher's PC with `tools/publish.py sign-license`,
by the same key as releases; the signed text starts differently, so one can
never pass for the other. Only administrators ever see licensing messages.

## Tool chests: Excalibur View's own format

Tools live in `.evtools` files: plain JSON — each tool's subject, label,
colour, the office's column values (weights), and the PDF annotation it stamps
— with nothing in them that needs anybody else's program to open
(`crates/chest/src/native.rs`). **File → Load Tool Chest…** — or dropping the file on the
window — takes one of these, or imports a Revu profile (`.bpx`) or tool set
(`.btx`) once and keeps it as `.evtools`; **File → Save Tool Chest As…** writes one; a chest shared with the
office goes up as `.evtools` whatever it was picked as. Revu's toolbar layout
does not come across: Excalibur View lays out its own.

## Studio

The **Studio** button on the left is the office's shared drawings: projects,
drawing sets put up from any seat, and shared tool chests. A drawing opened from
Studio keeps in step with everybody else's copy every few seconds — markups
somebody else draws appear on your screen without anybody saving or reopening.
A tool chest an administrator shares arrives on every seat by itself.

## FabWire, and other programs

An administrator makes a key under **Studio → Office → Other programs** (or
FabWire makes its own when an administrator signs in once from its Settings).
With it, another system uses the API as that administrator: FabWire keeps one
project per bid, puts the drawings up — a reissued drawing supersedes the old
one and brings its takeoff across, marked for checking — watches a stamp that
moves when the takeoff does, and reads the takeoff back from
`/api/v1/projects/{id}/markuplist` in the shape its Bluebeam importer already
reads, with exact counts beside it. A finished bid is archived, never deleted.
See `docs/integrating.md`.

## The four things Revu does not have

Under the Document menu, and all built on the same rule as everything else:

| | |
|---|---|
| **Shop List** | the takeoff as a cut list, rounded **up**, weighed from the chest's own pounds-per-foot, and — once somebody types a stock length — nested into sticks with the kerf taken off every cut. No default mill length, ever. |
| **Counted Twice?** | markups sitting on top of each other measuring the same thing. It reports and never removes: a doorway measured once for the frame and once for the opening is an ordinary takeoff. |
| **Revision Cost** | two issues in, and a line per subject saying what moved and what it is worth. A renamed subject is one gone and one arrived, not silently matched up. |
| **Grid locations** | every markup knows it is at B-4, read off the sheet's own grid bubbles, and it goes on the cut list. A sheet with no grid gets no locations rather than the nearest label. |

## What to read

- `docs/SPEC.md` — what this is, what it does, how it was verified, and the
  rules it will not break.
- `docs/RELEASING.md` — signing a build, and what an install checks before it
  will run one.
- `docs/plugins.md` — writing, signing and handing out a plugin.

## The rule the whole thing is built on

**Nothing is estimated.** Every quantity traces back to a click or a
confirmation. A sheet with no scale has its measurements reported and left out
of the totals — never counted as zero. Weights come from the unit weights in the
tool chest, never from a section table this program looked up. A Dynamic Fill
that escapes a room produces no number at all rather than the area of half the
drawing.

A number that is quietly wrong is worse than no number, because somebody prices
a job with it.
