# Plugins

An office's own tools, in its own seats and nobody else's.

## What a plugin is

A WebAssembly module with three exports, sealed into a `.hvplugin` file with
the publisher's signature. The contract — every type a plugin is handed and
hands back — is `crates/plugin-api`. In Rust a plugin is two functions and a
macro:

```rust
plugin_api::plugin!(manifest, run);

fn manifest() -> plugin_api::Manifest { /* id, name, version, commands, settings */ }
fn run(input: plugin_api::Input) -> Result<plugin_api::Output, String> { /* ... */ }
```

That is a whole one: it needs nothing from this repository but the contract.

### What it is handed

For each sheet in the command's scope (the sheet on screen, or every sheet):
its name, size, the scale set on it, and — when the command asks for them —
the words printed on it and its straight line-work with each line's pen
width, all in sheet space (points, from the top left of the sheet as shown).
Then every markup on those sheets: kind, subject, label, points, length in
feet up any slope, area, count, quantity, custom columns and pounds. And the
command's settings, as the person running it left them.

### What it hands back

A title, a summary, findings and tables. A finding has a level (note, check,
problem), a sheet and an area to outline, and fixes. A fix is a label and a
list of actions from a short, fixed list:

| Action | |
|---|---|
| `set_sheet_scale` | a sheet's scale, by ratio (48 for 1/4" = 1'-0") |
| `set_subject` | a markup's subject |
| `set_column` | one of a markup's six custom columns |
| `set_quantity` | how many pieces a markup stands for |
| `set_slope` | a pitch, rise over run |
| `add_length` | a new length along given points, made with the office's tool of that subject when the chest has one |

Hyperview carries a fix out only when somebody clicks it, as one undo step.

### What it cannot do

Import anything. A module with any import is refused, so it has no file
system, no network, no clock and no way to reach the program. It gets
sixty billion instructions' worth of work and a gigabyte and a half of
memory per run, and is thrown away after each.

## Writing one somewhere else

A plugin doesn't need this repository, only the contract:

```toml
[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
plugin-api = { git = "https://github.com/cngmoney88/excalibur-view", branch = "main", features = ["guest"] }
serde_json = "1"
```

Keep `Cargo.lock` so the contract only moves when you move it.

## Testing it before it is signed

A plugin is two ordinary Rust functions, so most of its testing is ordinary
`cargo test`. For real data, open a drawing in Excalibur View and use
**Plugins → Save This Sheet for a Plugin Test…**: it writes the sheet on screen
as the plugin would be handed it (words, line-work, scale, markups, the
office's column names) to a JSON file, which a test reads with
`serde_json::from_str::<plugin_api::Input>`.

Then, to run the built module in the same sandbox and limits a signed one
gets:

```sh
Hyperview.exe --plugin-check target/wasm32-unknown-unknown/release/my_plugin.wasm "S-201.json"
```

It reads the manifest, runs every command against the saved sheet (or
against nothing, when no sheet is given), says whether it is ready to be
signed, and writes each command's answer to `my_plugin-check.json` beside
it. Nothing is installed and nothing appears in the Plugins menu.

## Who signs it

A plugin loads only when a key this program was built with signed exactly
its bytes, the same rule as an update. A plugin written outside Excalibur is
signed by us: send the `.wasm`, its id and its version to
hello@excaliburct.com. What comes back is the `.hvplugin`.

Plugins are signed with a plugin key, listed in `PLUGIN_KEYS` in
`crates/hyperview/src/trust.rs`. A plugin key seals plugins and nothing else:
no release or license is checked against it, so it can be used as often as
plugins need signing without the release key coming out. Plugins signed with
a release key before plugin keys existed still load. A company that writes
plugins regularly can have a plugin key of its own in that list.

`publish.py make-key --name <name> --out <file>` makes a key on the computer
that will sign with it and prints the line for `trust.rs`.

## Building and signing

```sh
cargo build --release --target wasm32-unknown-unknown   # -> target/wasm32-unknown-unknown/release/acme_estimating.wasm
```

On the publisher's PC, beside the signing key:

```powershell
py -3 publish.py sign-plugin --key plugin-signing.key --id acme-estimating `
      --version 1.0.0 --name "Acme Estimating" acme_estimating.wasm
```

That writes `acme-estimating-1.0.0.hvplugin`. An administrator adds it
with **Plugins → Add Plugin…**; it goes to the office server, which checks
the signature and hands it to every seat within a few minutes. A newer
version with the same id replaces the older.

## Handing it out

The office server keeps plugins in its database and gives them to its own
seats only (`/api/v1/plugins`). **Plugins → Manage Plugins…** shows what a
seat has, where it came from and who signed it; an administrator can take
one off the office's list, and each seat drops it the next time it looks.
