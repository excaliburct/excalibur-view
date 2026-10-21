# Excalibur Hyperview — build specification

A native Windows drawing viewer, markup and takeoff program for a structural
steel fabricator, built to replace Bluebeam Revu across a six person office.

Everything in this document that describes Revu's behaviour or file format was
decoded from **the user's own exported profiles** (`Ron Quantity Take Off.bpx`,
`Field Issues.bpx`) in `research/bluebeam/`. Where a number or a key name
appears here, it came out of those files, not from memory.

---

## 1. Scope

Agreed with the user on 2026-09-17. Three rings, all in v1. Nothing ships until
all three are done.

**Ring 1 — the daily work.** Revu-shaped UI, document tabs, split views, all
panels. Every markup tool, written as real PDF annotations. All fourteen
measurement tools including Dynamic Fill. Tool Chest imported from his `.bpx`
with its weight data. Markups list with the full column set, custom columns,
totals, legends, CSV export. Text search and VisualSearch. Profiles.

**Ring 2 — production.** Combine, split, extract, insert, delete, rotate, crop,
page labels, headers and footers, slip sheet, flatten and unflatten, reduce file
size, security, Compare Documents, Overlay Pages, and the Batch form of each.

**Ring 3 — the heavy tail.** OCR for scanned drawings. Sign & Seal with
certificates.

**Explicitly out.** 3D PDF. JavaScript scripting. GoCanvas. DMS integration.
Forms are out of v1 unless they turn out to be cheap once the annotation layer
exists.

**Deployment.** Six seats. Shared tool chests and profiles from a common
location, markup attribution by author, a real installer. Studio-style sessions
come later through Fabwire.

### Non-negotiables

1. **Nothing is estimated.** Every quantity traces to a click or a confirmation.
   A sheet with no scale has its measurements left out of totals and says so; it
   never counts them as zero.
2. **The PDF is the file of record.** Markups are written into the PDF as real
   annotations, by incremental update, so the original bytes are never rewritten
   and the file opens correctly in Revu and Acrobat.
3. **His numbers, not mine.** Weights come from his imported tool chest, not from
   a table this program looked up.

---

## 2. What the profile format gave us

`.bpx` is UTF-8 XML with a BOM and CRLF, root `<RevuProfile Version="1"
Name="...">`. Several leaf values are **zlib streams written as lowercase hex**
(they start `789c`): `Title`, `Icon`, `Raw`, `Resources/ID`, `Resources/Data`.

```
value  ->  bytes.fromhex(value)  ->  zlib.decompress  ->  UTF-8
```

`.btx` is the same shape holding a single `BluebeamRevuToolSet`.

### 2.1 Structure

```
RevuProfile
  Record Key="Bookmarks" | "Layers" | "MarkupList" | "Thumbnails" |
             "3D Model Tree" | "Signatures" | "PageSets" | "DockTabMeasure" |
             "FileProperties" | "Hyperlinks" | "Stitching" | "GoCanvas" |
             "ComparisonSettings" | "HatchSetManager" | "LineSetManager" |
             "SecurityPolicy" | "DockContainer" | "ToolSetManager" |
             "HeaderFooter" | "ReduceFileSizeOptions" | "StateModels" |
             "BookmarkTemplates" | "Redaction" | "Script" | "Redline"
  Record Key="Redline"          <- toolbars, nav bar, hover bar, menu/status flags
    ToolStrip*                    Name, Module?, X, Y, Visible, Location, Items/Item*
    NavBarLeft / NavBarMiddle / NavBarRight / HoverBar
  BluebeamRevuToolSet*          <- inside the tool chest records
    Title                         zlib hex -> set name
    ToolChestItem*
      Resources/ID, Resources/Data    zlib hex -> shared objects (Measure dicts, hatch patterns)
      Name                            16-char internal id
      Type                            e.g. Bluebeam.PDF.Annotations.AnnotationLine
      Raw                             zlib hex -> a literal PDF annotation dictionary
      X, Y                            placement offset
      Index                           order in the set
      Mode                            "properties" = apply properties to next drawn markup
```

### 2.2 The tool payload is a PDF annotation

`Raw` decompresses to PDF dictionary syntax, ready to be written into a page's
`/Annots`. Example, his `W12x26`:

```
<< /IC[1 0.7137255 0.5019608]
   /LL 0.04137602 /LLE 2
   /MeasurementTypes 130 /SlopeType 0 /PitchRun 12
   /IT /LineDimension
   /L[8.509204 8.5 174.8593 9.25]
   /DS(font: Helvetica 12pt; text-align:center; line-height:13.8pt; color:#FF0000)
   /RC(<?xml ...><body ...><p>5'-9 1/4"</p></body>)
   /Label()
   /Subj(W12x26)
   /Measure /BBObjPtr_HMOAUKSFBXHVWBVQ
   /DepthUnit[<</Type/NumberFormat/U(')/C 0.001157407/D 100/FD true/SS()>>]
   /BSIColumnData[(26.00)(150.18)()()(0.00)()]
   /Subtype /Line
   /Rect[0 0 183.3593 19.79136]
   /Contents(5'-9 1/4")
   /F 132
   /C[0.5019608 0 0]
   /BS<</W 4/S/S/Type/Border>>
>>
```

`/BBObjPtr_<id>` is a reference into the item's `Resources`, resolved at import.

---

## 3. The annotation dialect

### 3.1 Subtypes in use

| Subtype | Count | Used for |
|---|---|---|
| `Line` | 1177 | length measurements, arrows, dimensions |
| `Polygon` | 343 | area, volume, count symbols, filled shapes |
| `RL` | 245 | Bluebeam's own "rich line"/count group wrapper |
| `Circle` | 35 | ellipse, diameter, radius |
| `PolyLine` | 11 | polylength, angle |
| `FreeText` | 6 | callouts and text boxes |

### 3.2 `/IT` intent

`LineDimension`, `PolyLineDimension`, `Polylength`, `PolygonDimension`,
`PolygonVolume`, `PolygonRadius`, `PolygonCount`, `CircleDimension`,
`PolyLineAngle`, `FreeTextCallout`.

### 3.3 `/MeasurementTypes` is a bit field

Confirmed by correlating every value against its `/IT` across 1,572 tools:

| Bit | Value | Meaning |
|---|---|---|
| 0 | 1 | area |
| 1 | 2 | length |
| 2 | 4 | volume |
| 7 | 128 | count — set on essentially every measurement |
| 8 | 256 | diameter |
| 10 | 1024 | angle |
| 11 | 2048 | radius |

Observed: `130` length, `129` area, `132` volume, `128` count, `384` diameter,
`1152` angle, `2177` area+radius, `2` polylength without count.

### 3.4 The `/Measure` dictionary

Standard PDF 32000-1 §12.9 rectilinear measure, which is why Acrobat reads these
correctly too.

```
<< /Type/Measure /Subtype/RL
   /R(0.25 in = 1 ft' in")            scale, as text
   /X[ NumberFormat ]                 points -> primary unit
   /D[ NumberFormat, NumberFormat ]   distance, multi-part (feet then inches)
   /A[ NumberFormat ]                 area
   /T[ NumberFormat ]                 angle
   /V[ NumberFormat ]                 volume
   /TargetUnitConversion 0.001157407  points -> feet at 1:1 (= 1/864)
>>
```

`NumberFormat`: `/U` unit label, `/C` conversion factor, `/F` fraction format
(`/D` decimal, `/F` fraction, `/R` round, `/T` truncate), `/D` precision or
denominator, `/FD` fixed denominator, `/PS` prefix separator, `/SS` suffix
separator.

Quarter inch scale reads `/C 0.05555556` for feet, which is `4/72` — four feet
per inch of paper. Feet-and-inches is the two element `/D` array with `/SS(-)`
joining them, giving `12'-3 1/2"`.

### 3.5 `/BSIColumnData` — where the tonnage lives

An array of six PDF strings, one per user defined column, in Markups list order
`UserDefined0 .. UserDefined5`. His steel chest carries:

| Tool | `/BSIColumnData` | Reading |
|---|---|---|
| `W12x26` | `(26.00)(150.18)()()(0.00)()` | 26.00 lb/ft; 150.18 lb on the sample pick |
| `HSS6x6x1/4` | `(19.02)(541.46)()()(0.00)()` | 19.02 lb/ft |
| `FB1/2x2` | `(3.40)()(3.400)()()` | 3.40 lb/ft |
| `PL1/2` | `()(0.00)()()(0.00)(20.42)` | 20.42 lb per square foot |
| `Bar Grate 1x3/16` | `()()()()()(7.10)` | 7.10 lb per square foot |

Column 0 is weight per foot for linear shapes; column 5 is weight per square
foot for plate and grating; column 1 is the computed total for that markup.
**Hyperview imports these verbatim and never substitutes its own AISC figures.**

### 3.6 Other keys worth knowing

`/L` line endpoints · `/Vertices` polygon/polyline points · `/LE` line endings
(`/Square`, `/ClosedArrow`, …) · `/LES` ending sizes · `/LL` leader length ·
`/LLE` leader extension · `/IC` interior colour · `/C` stroke colour ·
`/CA` opacity · `/FillOpacity` · `/BS` border style (`/W` width, `/S` style) ·
`/Pattern` + `/PatternName` + `/PatternColor` hatch · `/DS` default text style ·
`/RC` rich caption as XHTML · `/Contents` plain caption · `/Subj` subject —
**this is what the Markups list groups by** · `/Label` · `/CountStyle` count
symbol · `/NumCounts` · `/Depth` and `/DepthUnit` for volume · `/SlopeType` and
`/PitchRun` for sloped true length · `/AlignOnSegment` caption placement ·
`/AP` appearance stream.

---

## 4. The UI contract

Taken from his `Redline` record, so the layout below is literally his.

### 4.1 Toolbars, in his order

| Strip | Contents |
|---|---|
| File | New, Create PDF, Combine, Open, Save, Print, Email, Package, Search, Spell Check *(OCR, Script hidden)* |
| Advanced Text | Edit Text · Review Text · Underline, Squiggly, Strikethrough |
| **Measure** | Measure Tool, Calibrate · Length, Polylength, Area, Perimeter, Diameter, Angle, Radius, Volume, Count · Area Cutout, Ellipse Cutout, **Dynamic Fill** |
| Text | Text Box, Highlight, Pen, Cloud+, Cloud, Callout, Stamp, Image, Snapshot |
| Line | Line, Arrow, Arc, Polyline, Dimension, Rectangle, Ellipse, Polygon |
| Rotate | Rotate CCW, Rotate CW |
| Signature | Sign Document, Digital Signature markup |
| Font | Font, Size, Text Colour, Bold, Italic, Underline, Strikethrough, Align L/C/R |
| Pan & Zoom | Select, Pan, Zoom, Measure Tool, Lasso |
| Annotation | Hyperlink, File Attachment, Flatten, Erase Content |
| Appearance | Fill Colour, Opacity, Hatch Pattern |
| Sketch to Scale | Polygon, Rectangle, Ellipse, Polyline |
| Alignment | Left/Centre/Right, Top/Middle/Bottom, Width/Height/Size, Centre on Document, Spacing H/V, Flip H/V |
| Order | Bring to Front, Send to Back, Bring Forward, Send Backward |
| Redaction | Redaction markup, Apply Redactions |
| Control Point | Add, Subtract, Convert |
| Document | Headers & Footers, Crop Pages, Flatten, Erase Content |
| Edit | Undo, Redo, Cut, Copy, Paste, Format Painter, Delete, Flatten, Snapshot |

### 4.2 Panels

Left rail, his order, 377 px wide, Thumbnails active:
**Measurements · File Manager · Thumbnails · Bookmarks · Layers · Tool Chest ·
Properties · Search · Signatures · Studio · Flags**

Bottom, collapsed by default, Markups active: **Markups · 3D Model tree ·
JavaScript Console**

Thumbnails: box size 132, labels on, scales off.

### 4.3 Navigation bar

- Left — Unsplit, Split, Split Horizontal, One Full Page, Scrolling Pages
- Middle — Pan, Select, Select Text, Zoom, **Scale combo**, First/Prev/Page
  box/Next/Last, Previous View, Next View
- Right — Page Scale, Page Size, Invert Colours

### 4.4 Hover bar

Select, Pan, Escape · Pen, Highlight, Eraser · Line Width, Line Colour ·
Image from Camera · Cut, Copy, Paste, Delete · Full Screen

### 4.5 Markups list columns

Forty two, in this order. He keeps the starred ones visible.

*Subject\** · Label · Layer · Space · Page Index · *Lock\** · *Page\** · Status ·
Checkmark · *Colour\** · *Author\** · Date · Creation Date · X · Y · Width ·
Height · Comments · *Count\** · *Length\** · *Area\** · Volume · Depth ·
Wall Area · Measurement Width · Measurement Height · Measurement · Sequence ·
Capture · Legend · Rise/Drop · Slope · Units · View3D ·
*UserDefined0..5\** · *GoCanvasStatus\** · X Centre · Y Centre

Grouping is by `/Subj`. The user defined columns are where the weights land, so
tonnage is a column total, not a separate feature.

---

## 5. His tool chest

1,533 tools in 26 sets, all imported as-is.

| Set | n | Kind |
|---|---|---|
| Rectangle Tube (HSS) | 296 | length |
| Wide Flange Beams II | 271 | length |
| Flat Bars | 261 | length |
| Angles | 158 | length |
| Square Tube (HSS) | 136 | length |
| Round Bars | 88 | length |
| Plate | 42 | area |
| Misc. Channels | 41 | length |
| Standard Channels | 32 | length |
| Square Bars | 31 | length |
| Standard Beams | 30 | length |
| Steel Pipe | 28 | length |
| Bar Grating | 19 | area |
| Floor Plate | 15 | area |
| Bar Channels | 13 | length |
| FabWire Counters v2 | 12 | count |
| Steel Sheets | 9 | area |
| Misc. Items | 8 | mixed |
| General Measurements | 8 | mixed |
| Embeds | 5 | count |
| Junior Beams | 5 | length |
| Electrical / Mechanical / Concrete / Interior Finishes / Fire & Life Safety | 25 | mixed |

**FabWire Counters v2** — Moment Conn, Shear Conn, Base Plate, Brace/Gusset,
Stiffener, Clip/Tab, Embed, Weld Stud, Roof/Angle Frame, Field Bolt, Joist,
Decking. These are the bridge to his ERP and should eventually post straight
into Fabwire rather than being copied by hand.

A second profile, `Field Issues`, holds 39 punch-list tools across Carpentry,
Electrical, Flooring, Lighting, Paint and Plumbing — circles plus a free text
callout per trade.

---

## 5a. What the drawing files themselves taught us

Found by putting the object layer against the Elm Theater set, and each one
changed the design.

**Sheet numbers are already in the file.** Both the Revit output and the
Bluebeam-stapled combined set carry per-page keys
`revit_metadata_view_sheet_number` (`A0.01`) and `revit_metadata_view_name`
(`A0.01 - INFORMATION SHEET`), and a `/PageLabels` number tree with the same
titles. Both come with trailing NUL bytes and stray direction marks
(`D2.00\u{200f}\0`) which have to be cleaned off. So the order for naming a
sheet is: Revit metadata, then `/PageLabels`, then reading the title block off
the page, then "Page N". Only the last of those is guesswork.

**The scale belongs to the sheet, not to the tool.** Every tool in the chest
carries a `/Measure` from whenever it was made. Placing `W12x26`,
`HSS6x6x1/4` and `L4x4x1/2` along the same 720 point line reported 25'-0",
40'-0" and 80'-0". The sheet's scale — PDF's `/VP` viewport array — is the only
one that means anything, and Hyperview replaces the tool's with it on placement. A
sheet with no `/VP` has no scale, and its markups go down with no measurement
rather than a wrong one. `/VP` also allows several regions, which is how a
quarter inch sheet carries a detail at one inch; the smallest region containing
the point wins.

**The space before a unit comes from the file.** `/PS` defaults to a single
space, and Bluebeam writes `/PS()` explicitly for feet and inches to remove it.
That is the whole difference between `20'-6"` and `20 ' - 6 "`, and between
`203.71 sf` and `203.71sf`.

**Rounding happens once, in the smallest unit.** Rounding feet and then inches
separately turns a hair under twelve feet into `11'-12"`. The total is rounded
in sixteenths first and then broken back up.

**An annotation with no `/AP` is invisible.** Pdfium draws nothing at all for
one. Hyperview therefore generates the appearance stream itself for every markup;
verified by rendering the same file in pdfium and in Poppler and getting the
same picture.

## 6. Architecture

```
crates/
  pdf       done    object model, xref tables and streams, object streams,
                    filters, text encodings, incremental update writer
  chest     done    .bpx / .btx import: 1,533 tools with their weights, plus
                    the toolbars, panels and columns that make up a profile
  annot     done    markups as annotation dictionaries, appearance streams,
                    Measure and NumberFormat, page scale through /VP
  takeoff   done    measurement, the 42 columns, totals, legends, export
  ui        core    egui shell: palette, the mark, commands, icons, menus
  hub       done    the wire model, the HTTP client, the update manifest
  server    core    the self-hosted API: drawings, markups, takeoffs, updates
  release   done    keygen, sign and check: the manifest a seat will accept
  hyperview core    the desktop binary: render worker, canvas, panels
```

`hyperview::render` stays on pdfium for drawing pixels and reading text. `pdf`
owns reading and writing the file itself, because annotation fidelity and the
document operations in ring 2 need object level control that a render engine
does not give.

`render` stays on pdfium for drawing pixels and reading text. `pdf` owns reading
and writing the file itself, because annotation fidelity and the document
operations in ring 2 need object level control that a render engine does not
give.

Markups are saved by **incremental update**: new objects appended, a new xref
section written, original bytes untouched. A crash mid-save leaves the previous
version intact and readable.

## 6a. Where the shell stands

The window is laid out **from his profile**, not from a hard coded design. It
reads the `.bpx` at startup and puts his toolbars in his rows in his order, his
eleven panels down the rail, and his markups columns in the grid. Dropping a
different profile in the `profiles` folder rearranges the program.

Working: menu bar (seven menus, every item either doing something or greyed
with a plain reason), two toolbar rows built from the profile, the panel rail,
Thumbnails with sheet numbers read from the file, Tool Chest with all 1,533
tools searchable and showing each one's weight, Measurements, Properties, the
Markups grid, and the navigation bar with the scale, page box, fit modes and
page size.

The drawing tools are now wired to the annotation layer: a measurement taken on
screen becomes a real PDF annotation, carrying the chest tool's dictionary,
weights and styling, and saves into the file by incremental update. A length
clicked around corners becomes a PolyLine rather than being flattened to a
straight line between its ends. Setting a sheet's scale afterwards rewrites
every caption on it; clearing the scale takes the measurements out of the totals
and says so rather than counting them as zero. A read-only drawing is named as
read-only when it is opened, not when somebody tries to save.

The desktop talks to a server. Sign in from the welcome screen or the Server
panel; the address is checked before anybody types a password into it, and what
is kept between sessions is a session token, never a password. Projects and
drawing sets are browsed in the panel; opening one downloads it into a cache
named by digest, so opening the same set on Tuesday costs nothing. A shared tool
chest pulled with one click lands in the `profiles` folder and takes effect
without a restart — the whole window rearranges into the office's own layout.
The Author column follows the seat, so six people's markups are told apart. An
update waiting is a box in the middle of the window with what is new in it;
Later leaves it to install next time, and it never restarts on its own.

Verified end to end: signed in, pulled the 1,533-tool chest, browsed the job,
downloaded the 12-sheet stamped set and opened it — all from the panel.

**Text search runs across every sheet.** Ctrl+F, or the Search panel. Answers
arrive sheet by sheet so the list fills while a two-hundred-sheet set is still
being read, grouped by sheet number with the words around each one and the match
picked out. Clicking one goes there, zooms enough to read it and puts it in the
middle; every answer on that sheet is shaded and the one being pointed at gets a
ring. F3 and Shift+F3 walk them.

The matching is done over the characters rather than with pdfium's own search,
because a drawing is not prose. `Whole words` treats a letter, a digit and
`/ - _ . #` as part of a word, so `PL1` does not answer with every `PL1/2`, and
`W12x26` is findable without `W12x26A` coming with it. Verified on the Fox
Theater set: `HSS5x5` finds 31 across 3 sheets, and clicking one lands on S-201
with all 24 on that sheet shaded.

**Several drawing sets are open at once**, as tabs. The render worker holds up
to six open at a time and lets the least recently used go, so switching tabs is
instant and the tiles that were already drawn are still there; a set that was
let go is reopened in about fifteen milliseconds without the window noticing.
Every message between the window and the worker names the drawing it is about,
so a tile that arrives while somebody is switching tabs goes to the drawing that
asked for it. Opening a set that is already open brings it forward rather than
opening it twice. A tab shows an amber dot when it has markups not yet in the
file and a hollow one when the file is read-only, and closing a tab saves first.
Ctrl+W closes, Ctrl+Tab walks.

**The window splits**, vertically or across, into two panes on one drawing —
each with its own sheet, its own zoom and its own place on it. Ctrl+2, Ctrl+H,
or the buttons on the navigation bar; F6 or a click moves the work to the other
pane, and Sync Panes ties them together by the middle of the sheet rather than
by the offset, so two panes of different sizes stay on the same part of the
building instead of drifting apart.

The pane being worked in is edged in the accent colour and the other is dimmed,
because which pane a measurement is about to land on should never be a guess.
Markups go only on the pane in use: a click on the other side moves the work
there rather than drawing something where somebody was only looking. Both panes'
tiles are asked for in one list, because the worker keeps the most recent
request and drops the rest — two panes each sending their own would take turns
cancelling each other and neither would finish.

**Dynamic Fill** finds the shape of a bounded area from the lines already on
the drawing. Click inside a room; the fill spreads until it meets something
dark — a wall, a grid line, a boundary somebody drew across a doorway — and the
shape becomes an area measurement.

What makes it trustworthy is knowing when it has gone wrong. A region that is
not quite closed lets the fill out, and what comes back is the area of
everything it could reach: a perfectly good number that is the area of half the
drawing. So a fill that reaches the edge of what it was shown, or swallows the
window, **produces no number at all** and says where to look instead.

Nothing becomes a measurement until somebody applies it. The fill is a proposal
drawn on the sheet; Apply is the confirmation, which is the same rule as
everywhere else here. A single closed path cannot have a hole in it, so every
enclosed opening becomes its own cutout markup, and whatever is too small to cut
out is reported as the percentage the area reads high by. The tracing resolution
is said on its face, because the shape was read off pixels and nothing finer
than a cell was visible.

Verified on S-201: nine openings cut out, 586 counted but too small to draw,
3,501 of 71,026 square points enclosed, traced to about 1.1 points on the sheet.

That work turned up a bug affecting far more than Dynamic Fill: **egui only
fills convex paths correctly**, fanning triangles from the first point, so every
concave area markup — an L-shaped room, a slab with a notch — was painting
wrong. `ui::tess` now cuts a shape into triangles by ear clipping and refuses
one that crosses itself rather than painting a plausible lie.

**Markups sync both ways.** A drawing opened from a server keeps the thread back
to it: saving writes into the PDF first and then sends a copy up, so a server
that is down costs the sync and nothing else. A markup is identified by its
`/NM`, made unique across machines rather than merely within a file — six seats
marking up the same drawing on the same morning must not produce two markups a
sync would mistake for one. The merge is small enough to trust: anything with a
name this machine has not got, take; anything marked removed, remove. No two
seats ever own the same markup, so there is no case where two people's work has
to be reconciled into one. Verified over the wire: two estimators mark up the
same sheet, each ends up holding both lots, and both takeoffs come to the same
number.

## 6b. The mark and the palette

The program is laid out like Revu on purpose — a person who has used it for
years should find things where their hand already goes — but it does not look
like Revu. The identity comes from the shield: navy body, steel cross-bracing
drawn the way a braced bay is drawn on a framing elevation, and a sword.

| | dark | light |
|---|---|---|
| chrome | `#161D28` | `#EDF0F4` |
| bar | `#1C2532` | `#F6F8FA` |
| accent | `#3C6E9F` | `#2B4461` |
| type | `#E3E9F0` | `#141C26` |
| no scale | `#D9A441` | `#A8710F` |

The mark lives in `ui::mark` as polygons, not as an image. The same description
is painted by egui on screen and scan-filled into the window icon at start-up,
so the taskbar and the About box cannot show two different marks. It reads at
sixteen pixels. Rounded corners, a tinted tool with an accent bar under it
rather than a solid block, and navy rather than grey behind a white sheet.

**The Classic look** (View → Classic Look, and in Preferences) is the one other
palette: the grey face of an office computer from the late nineties, raised
buttons, sunken white fields and lists, `#0A246A` navy for anything chosen and
across the window in front, and DejaVu Sans, cut down and renamed in
`crates/ui/fonts`. egui draws a widget's frame as one rectangle in one colour
and a bevel is four edges in four, so the Classic visuals mark each kind of
frame with a colour of its own and `ui::classic::finish` swaps each one for its
bevel at the end of every frame. Anything it does not recognise is drawn as
egui drew it. The chrome's own buttons, the rail, the tabs and the panel titles
draw their classic selves directly.

**Sheets are moved in Thumbnails, in the file.** Dragging a sheet, or several
picked ones, puts them where they are let go; a PDF dropped on the list goes
in there; a sheet's menu adds a blank one, turns sheets or takes them out.
`pdf::pages` does it as one incremental update: the page tree is flattened to
the root in the new order, anything a sheet inherited from a node above it is
written onto the sheet first, and sheets from another file are copied in with
everything they use, once each. Markups are annotations on their sheets, so
they travel with them. Undo writes the root back as it was. Everything the
window keeps by a sheet's place in the set, from its thumbnail to its sheet
number, is carried to the sheet's new place rather than read again.

**Words are typed on the sheet, where they will be.** A text box put down with
a click grows with its words; one dragged out keeps its width and gets taller
as they wrap. The words are typed in "Excalibur Text", Liberation Sans cut down
to the characters a PDF's standard fonts can show and renamed in
`crates/ui/fonts`, because it has Helvetica's widths: a line typed on the
screen wraps where the saved text box will wrap it. The size, colour, weight
and alignment are the markup's own, so the toolbar changes them mid-word. The
first frame of a box ignores the pointer, since a quick click puts its press
and release in one frame and that press is not a click away. A click on the
sheet outside the box finishes it and does nothing else, the words tools hand
back to Select, and a box left empty goes. A callout is two clicks, and its
box, like a Cloud+'s words, grows away from the side its leader meets.
Autosave waits while words are being typed.

## 6c. The server, the API and updates

Hyperview is built to be sold as well as used. A company runs one
`hyperview-server`: one binary, one SQLite file, one folder of drawings, and no
configuration needed to start. Everything the desktop does, any other program
can do, over a documented API at `/api/v1` with its own OpenAPI description at
`/api/v1/openapi.json`.

**A markup on the wire is the annotation dictionary itself**, base64. Not a
summary of one. Anything read from the API can be written back into the drawing
byte for byte, and anything written through it lands as a real annotation that
Revu, Acrobat and everything else will show. There is no lossy middle format for
fidelity to leak out of — the same reason the desktop has no internal markup
type either.

**The takeoff endpoint is the one worth integrating against.** It runs the same
engine the viewer runs, on the same annotations, so a number fetched by an ERP
and a number on somebody's screen cannot disagree. It carries the rule with it:
a measurement from a sheet with no scale comes back `scaled: false` with every
measured field **absent rather than zero**, is counted in `left_out`, and is
excluded from every total. `sheets_without_a_scale` names the sheets somebody
needs to look at. A caller that ignores `left_out` under-reports a bid, so the
warning is in the description at the top of the document, on the `Takeoff`
schema, and on the field itself.

Weights come from the unit weight on the tool that drew the markup, out of the
estimator's own chest. The server looks nothing up in a section table.

### Updates

One place publishes; every company's own server distributes; every seat checks
the signature for itself.

1. **Build.** An installer is built and a release manifest is signed. *How* that
   happens is open — a fleet management app is the intended route — but the
   signing key never leaves the publisher, and this program never generates it
   for them.
2. **Distribute.** `POST /api/v1/admin/releases` takes the installer and its
   signed manifest. The server holds no signing key and signs nothing; it
   checks only that the file matches its manifest and refuses an unsigned one,
   because an unsigned release would be refused by every seat anyway. A shop
   with no internet on the floor can be updated by an administrator uploading
   the file by hand.
3. **Install.** A seat checks on start, downloads in the background, verifies
   the SHA-256 **and** an Ed25519 signature against a key compiled into its own
   binary, and installs on the next launch. Never mid-session: somebody three
   hours into a takeoff does not get restarted.

Three things a shop needs that a consumer program does not:

- **Staging.** A published release is not offered until somebody marks it ready.
  One seat tries it; the other five carry on working.
- **Pinning.** An administrator holds the whole office on one version. A
  fabricator in the middle of a bid package should not have the tool that priced
  the first half behave differently on the second. A pin set through the API is
  the most recent instruction and wins over the environment, including an
  explicit empty one meaning "let them move again".
- **Rollback.** The previous installer stays on the server.

Both halves of the check matter. The digest says the download is whole; the
signature says the publisher meant to publish it. A download that matches its
digest and nothing else proves only that whoever wrote the digest also wrote the
file. A release signed by a key the running program does not know does not
install, whoever is serving it — which is what stops a compromised or merely
misconfigured server pushing anything it likes onto six machines.

### What the API offers

| | |
|---|---|
| `GET /health` | is this a Hyperview server, and what does it speak |
| `POST /auth/token` | sign in |
| `GET /projects`, `POST /projects` | jobs |
| `POST /projects/{id}/place` | where a job sits in the Studio: `Bids/2026`, `Active Jobs`, `Past Jobs/2025` |
| `POST /sets` | upload a drawing set; the sheets are read on the way in |
| `GET /sets/{id}/file` | the PDF, with `If-None-Match` so six seats do not download it six times |
| `GET /sets/{id}/sheets` | sizes and scales; a null scale is not a scale of zero |
| `GET,POST /sets/{id}/markups` | the markup stream, by revision; removals travel as records |
| `GET /sets/{id}/takeoff` | **the quantities** |
| `GET /sets/{id}/takeoff.csv` | the same, for a spreadsheet |
| `GET,POST /chests` | shared tool chests |
| `GET,POST /people` | seats |
| `POST /admin/pin` | hold the office on a version |
| `POST /admin/releases` | publish an installer |
| `GET /update/latest` | is there anything new for this seat |

Roles are `viewer` (read), `estimator` (the ordinary seat), `admin`. Passwords
are Argon2id; session tokens are stored hashed, so a copy of the database is not
a set of live sessions. A wrong password and an unknown address give the same
answer, because telling somebody which half they got right is telling them half
of it.

## 6c-2. The first ten minutes

This is the part a company is actually judged on, and it has one rule: **a
customer never runs a command.** Not to install a server, not to make the first
account, not to add the other five people. If a step needs a terminal, the step
is wrong.

**One file.** `hyperview.exe` is the viewer, the server and the service
installer. Started normally it opens the window; started with `--serve` it is
the company's server; started with `--install-service` it registers itself with
Windows and opens the firewall. A company downloads one thing.

### How a shop gets going

1. **Somebody opens Hyperview and presses Connect.** The program shouts once on
   the local network (UDP 8715) and lists whatever answers, by name and address.
   Nobody types an IP address, and nobody has to find the person who knows one.
2. **If nothing answers**, one button — *Host the drawings on this computer* —
   makes that machine the server. It starts serving immediately, from inside the
   running program, needing nothing of anybody. Keeping it serving after a
   reboot means a Windows service and a firewall rule, which needs one UAC
   prompt; if that is declined, **the server still works** and the program says
   plainly that it serves while Hyperview is open on that machine.
3. **The first person to reach an unclaimed server sets it up**: what to call
   the installation, their own name, email, password. They become its
   administrator and are signed in without a second dialog. What they get back
   is a **join code** to hand round.
4. **Everybody else** opens Hyperview, sees the server already in the list,
   presses *I haven't got an account yet*, types the code and their own details,
   and is working. No administrator types anybody's password for them.

### What this deliberately does not claim

**Nothing here installs software onto somebody else's machine.** Nothing can:
putting software on a server means having an account on that server. A program
that offered to would be lying about what it was doing. What Hyperview does
instead is *be* the thing that needed installing — the server is the same binary
already on the desk — and then make itself findable so nobody has to be told
where it is.

### The discovery protocol, and its limits

A seat broadcasts `HYPERVIEW-FIND-1` on UDP 8715; anything that is a server
replies with its name, address, version, wire version, whether anybody has
claimed it, and an installation id. Three properties matter:

- **It is not a way in.** A reply says only what a server calls itself and where
  it is. Every question after that needs signing in, so answering gives away
  nothing that browsing to the address would not.
- **It never leaves the building.** Broadcast traffic does not cross a router.
  A shop's server does not announce itself to the internet.
- **An answer is an offer, not an instruction.** The list is shown to a person,
  the address is visible on every screen where a password is typed, and a
  machine claiming to be a server is never a reason to trust it with one.

A server listening on `0.0.0.0` has no single address, so it works out which of
its own the asker could reach by routing a socket towards them and reading back
what the operating system chose — the same question the OS answers when
something actually connects. Answers carry an installation id so that one server
heard twice, over broadcast and over loopback, is recognised as one server; where
two addresses are heard for it, the one another desk could also use wins over
loopback.

### Joining

`POST /setup` claims an unclaimed server and works **exactly once** — it is
counted and inserted without letting go in between, so two people who both found
the same fresh server cannot both become its administrator, and from then on it
refuses and changes nothing. `POST /join` makes an ordinary account, gated by a
join code by default. The code is five fabrication words and four hex digits,
compared without case or spacing mattering, because it is read aloud across a
shop; it decides who may *ask* for an account, not what protects one. An
administrator can reissue it — what to do the afternoon somebody leaves — set
joining to open or closed, and cannot arrange for new accounts to be
administrators. A server that has never been set up from the program takes
nobody: joining is `closed` until an administrator turns it on.

## 6c-3. The Fleet channel

Hyperview rides the Excalibur Fleet without the Fleet needing a line of
Hyperview-specific code. It does that by speaking the surface the Fleet already
polls, on the paths it already asks for — deliberately *beside* `/api/v1`, not
in it. One is the product's API, documented and versioned and for customers to
integrate against. The other is the small maintenance channel a shop's server
offers to whoever looks after it.

| | |
|---|---|
| `GET /api/version` | public. What is **running** — never what is staged or on offer, because the Fleet asserts a rollout worked by reading this back. |
| `GET /api/admin/ops` | the 6am questions: days since the drawings were copied off the box, free disk, whether the update channel still refuses unsigned builds, how many drawings and markups are on it. |
| `GET /api/admin/logtail` | the tail of the server's log, clamped, read from the end. |
| `POST /api/admin/update` | a signed release, delivered verbatim. |
| `POST /api/admin/update/rollback` | withdraw the newest offer. |

All but the first are gated on `x-av-update-key`, and **a server with no
maintenance key set answers 404 to all of them** — not 401. A shop that has not
asked to be looked after does not quietly grow a remote channel into its
drawings, and 401 is an invitation to keep trying.

### The Fleet cannot mint a build

This is the property the whole arrangement rests on. What arrives at
`/api/admin/update` is the exact signed manifest the release tool produced, and
the shop's server checks that signature against the same pinned public keys a
seat checks it against. **The maintenance key says who may knock; the signature
says what may come in.** Then every seat checks the same signature again before
it runs anything. Two independent verifications of one signature, neither of
them the hub's — so a Fleet with a compromised shelf can offer only builds that
were already signed by a key the publisher holds on his own machine.

`backupDays` is `null` when the drawings have never been copied off the box, and
null is not zero. The Fleet turns that amber on purpose: a shop holding its own
drawings with no copy anywhere else is one dead disk from losing every takeoff
it has done.

### Two roads for one release

A seat asks **its own company's server** first, and that answer wins — a shop
that has pinned six seats to 1.2 while it finishes a bid has made a decision,
and a program that went round the back of it would be overruling the person who
owns the machines.

A seat with **no** server has nobody to ask: a laptop, the first machine in a
shop, the zip somebody was handed to try. That one asks the publisher's feed
(`trust::HOME_FEED`), and that is the only case it covers. What travels is an
installer and its signed manifest. Nothing about a company goes the other way —
no drawing, no markup, no chest, no name.

So the Fleet publishes twice: to each company's server for attached seats, and
to the publisher's feed for loose ones. Same signed file, verified the same way,
either way.

## 6d. What ships, and what does not

Hyperview ships able to read any tool chest and carrying none.

A chest is the estimator's own work — their sections, their unit weights, the
years of building the thing — and it is worth more than the program that reads
it. Anybody running Hyperview builds their own, exactly as they would in Revu.
The same goes for drawing sets: somebody's project under somebody's contract.

This is enforced rather than remembered:

- `.gitignore` excludes `*.bpx`, `*.btx`, `/profiles/`, `*.pdf`, `*.key`.
- The installer ships an empty `profiles` folder with a note in it, and
  `crates/hyperview/tests/ships_clean.rs` fails if the installer script so much
  as names a `.bpx`.
- A test walks the whole tree and fails if a chest or a drawing has appeared
  anywhere it could be packaged from.
- The program is complete with no chest at all: the panel says where to put one
  rather than looking broken.

A `profiles` folder on a developer's own machine is how they run the thing. It
is ignored and never packaged.

## 6e. An MCP connection — planned

The server already answers, over a documented API, every question about a job
that anybody asks by hand. The same answers through MCP would let an assistant app or
ChatGPT answer them in words, which is worth having for the questions that are
tedious rather than hard:

- *How much steel is on job 6742?* — the takeoff endpoint.
- *Which sheets have no scale set?* — before the bid goes out, not after.
- *Where does HSS5x5 appear in the Elm Theater package?* — the search.
- *What changed between revision 1 and revision 2?* — two takeoffs, differenced.
  Tedious by hand, and the thing a re-issue most often hides.

**The rule that governs the whole design: a model must not be able to turn a
missing scale into a number.** So every tool returns `left_out` and
`sheets_without_a_scale` alongside any total, the tool descriptions say plainly
that a total with `left_out` above zero is short, and nothing in the schema lets
an unscaled measurement come back as a zero that reads like a measurement. The
API already works this way; MCP would inherit it rather than re-state it.

Shape: a small `hyperview-mcp` binary speaking JSON-RPC over stdio, holding a
`hub::Client`, exposing the read endpoints as tools. Writes — placing markups,
setting a scale — stay out of the first version, or are confirmed by a person,
because a quantity nobody clicked is exactly what this program exists not to
produce.

## 7. Build order

1. `pdf` — parse and incremental write. Everything rides on it.
2. `chest` — import his profiles, since they drive both the UI and the tools.
3. `annot` — read and write every markup type, round-trip verified against Revu.
4. `takeoff` — the measurement engine and the column maths.
5. `ui` — the shell, panels and markups grid.
6. Dynamic Fill and VisualSearch.
7. Ring 2 document and batch operations.
8. Ring 3 OCR and signing.
9. Installer, shared chests, author attribution.

## 8. Verification

Round-trip is the test that matters, and it now runs both ways.

**Hyperview to Revu.** Tools taken out of his chest, placed on S-200 by code, saved
by incremental update. The original 4,137,214 bytes are unchanged; qpdf reports
no syntax or stream errors; and pdfium and Poppler — two unrelated engines —
render the result identically. Colours, fills, captions and count symbols all
come out as drawn.

**Revu to Hyperview.** `research/marked.pdf` is a sheet the user marked up in Revu:
46 markups over two sheets at different scales, 1/4" and 3/8". Hyperview reads every
one, and **all 46 captions it computes match the captions Revu wrote, character
for character** — `38'-1 3/4"`, `20'-7"`, `150.12 sf`. The takeoff that falls
out of it:

| subject | kind | picks | quantity | pounds | tons |
|---|---|---|---|---|---|
| HSS5x5x1/4 | length | 27 | 269.80 ft | 4,214 | 2.107 |
| W8x67 | length | 5 | 173.19 ft | 11,604 | 5.802 |
| W8x31 | length | 8 | 57.16 ft | 1,772 | 0.886 |
| PL3/4 | area | 2 | 200.21 sf | 6,133 | 3.066 |
| PL1/4 | area | 3 | 152.74 sf | 1,559 | 0.780 |
| PL1 | area | 1 | 25.92 sf | 1,059 | 0.529 |
| **total** | | **46** | | **26,340** | **13.170** |

Every weight is his own column data, and each one checks out against the
published section weight — 3/4" plate at 30.63 psf, W8x67 at 67 lb/ft.

That test found the last formatting difference: Revu rounds a decimal to the
precision and then **drops trailing zeros**, so 50.098 sf is written `50.1 sf`
and a zero is written `0`. Hyperview does the same.

**End to end, through the program's own data model.** `crates/hyperview/tests`
opens the real stamped set, arms a chest tool, places a measurement, saves,
reopens from disk and reads it back: the caption is right, the `/Measure` is
there so Revu reads the same number, the `/AP` is there so every viewer draws
it, a three-point run keeps all three corners and measures 60'-0" rather than
the 44'-9" a flattened line would give, and deleting it takes it back out of the
file. A sheet with no scale reports its measurement, is counted as left out, and
is never counted as zero.

**Over the wire.** `crates/server/tests/over_the_wire.rs` runs the real server
on a real socket and talks to it with the real client. The 4,137,214-byte
stamped set uploads and its twelve 42 × 30 sheets come back. A markup comes back
as the same base64 dictionary it went up as. A batch with one bad markup in it
saves none of them. A viewer cannot draw and an estimator cannot publish
software. A pinned office stays pinned; an installer that does not match its
manifest is refused at the door.

**The published description cannot drift.** A test walks the router's route list
and the OpenAPI document and fails if either contains anything the other does
not.

Every ring 1 milestone is checked against the Elm Theater set and against
`research/marked.pdf`.
