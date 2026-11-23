# Excalibur View is source-available under the Elastic License 2.0

The license itself is in `LICENSE.txt`. This is the plain-words version, which
is not the license and does not replace it.

## What you may do

Read this source, change it, and build and run it yourself — alone or across
your company.

## What you may not do

- Offer it to others as a hosted or managed service.
- Work around, disable or remove the licensing.
- Strip the copyright and license notices.

## This is not open source

The Elastic License 2.0 is not an OSI-approved open-source license, and
calling it one would be misleading. It is **source-available**. We would
rather say what it is.

## Why publish it at all

So a fabricator's compliance officer can read every line rather than take our
word for what the program does with their drawings, and so an IT department
can build it themselves if their rules say they must — without handing a
competitor the product.

## The Office server

Its source is here too, in `crates/server`, under the same license as the
rest. Read it and build it if you like. Sharing past the trial needs an Office
license, and the Elastic License doesn't allow working around that.

## What is not in here

- **The trade tool chests we sell** — structural steel and misc metals,
  concrete and earthwork, electrical, mechanical, plumbing, general
  contractor — and the tables behind them. The chest *format* is open, and so
  is reading and writing one: build your own, for yourself or for your
  company, and the program will load it like any other. What is sold is the
  research, not the file format.
- **The signing keys.** Releases are signed with a key held offline. The
  public half is in `crates/hyperview/src/trust.rs` — that is how the program
  checks an update before installing it — and the private half has never been
  on a computer that faces a network. Licenses are signed with a separate
  key, and the program trusts that one for licenses and nothing else.

## The PDF engine

The program uses PDFium, which is BSD-licensed and is not included here.
`crates/hyperview/build.rs` picks it up from `third_party/pdfium/embedded/`
if it is there, and the program looks for it beside itself if it is not.

## The model reader

Steel models (IFC) are read and drawn with ifc-lite, the `ifc-lite-core` and
`ifc-lite-geometry` crates at version 19.1.0, under the Mozilla Public License
2.0. They are used as published, unchanged, and their source is where the
build gets it: https://crates.io/crates/ifc-lite-core and
https://crates.io/crates/ifc-lite-geometry.

## Fonts

The program carries three small fonts, each cut down from a free font and
renamed, as their licenses ask of a modified copy. "Excalibur Symbols" and
"Excalibur Classic" come from DejaVu Sans, under the Bitstream Vera license.
"Excalibur Text", used for words typed on a sheet, comes from Liberation Sans
under the SIL Open Font License 1.1. The tablet layout is set in Inter 4.001
(Regular and Bold, unmodified), by The Inter Project Authors, also under the SIL
Open Font License 1.1. The licenses are in `crates/ui/fonts`.

Excalibur Construction Technologies · Pueblo, Colorado · hello@excaliburct.com
