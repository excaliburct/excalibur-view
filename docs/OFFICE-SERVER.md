# Excalibur Office Server

One server for every office, whatever Excalibur programs it runs. It is View's office server, grown: every View server
already out there becomes an Office Server through its own signed update, keeping its drawings, accounts, join codes,
certificate and settings. Nobody reinstalls anything and no computer connects again.

Decided by Creede on 2026-11-10: "one server that handles literally everything from the Excalibur ecosystem", with
security tight, everything updating itself, and room to add capabilities over time.

## What an office gets

One program, one Windows service, one Manager, one certificate and fingerprint, one pairing of each computer, one set of
firewall rules, one nightly update, one backup, one set of folders for IT to allow. Inside it, **modules**, each switched
on by the office's signed licence:

| Module | What it does | Where it comes from |
|---|---|---|
| `view` | drawings, takeoff, markups, sign-offs, the field package | View's server (always on) |
| `cad` | Excalibur CAD's office drawings: check-out, check-in, versions | Excalibur CAD's server, moved in |
| `archetype` | Archetype's shared jobs and releases | Archetype's server (a cut-down View server already) |
| `citadel` | Citadel / LatCon Estimating's jobs, kept **blind** | Citadel Server, run alongside (below) |

A module that isn't licensed isn't served: its routes answer 404, as if it didn't exist.

## How it stays safe

- **Signed licence decides the modules.** The licence gains an optional `modules` line, signed with the rest. A licence
  without one (every licence today) means View only, so old licences keep working and still verify.
- **One update channel, signed on the YubiKey,** with the rollback the parts already have. Nothing updates without a
  version Creede signed.
- **Each part in its own limited account.** Citadel keeps its own Windows service account and its own locked folders; the
  Office Server installs it, updates it and watches it, and never reads its data. Citadel stays blind: the bids are sealed
  on the office's computers and the server only ever holds sealed bytes.
- **Encrypted, pinned connections everywhere.** View's server moves to the same HTTPS with a pinned fingerprint that
  Citadel and CAD already use (today it is plain HTTP on the office network).
- **Nothing on the internet** except through the office's tunnel.
- **Every change to the server core** gets an independent review and a live attack test before it merges.

## How existing installs move over

| Install | How it becomes an Office Server |
|---|---|
| Mesa Fab's View server | the next View server update (Fleet push), in place |
| Mesa Fab's Archetype server | its next update moves its jobs into the Office Server's `archetype` module, then the old service is retired |
| The separate CAD server | a one-click move of its drawings into the `cad` module, then it is retired |
| Latcon's Citadel Server | the Office Server takes it over as its `citadel` part: same service, same certificate, no re-pairing |

## Citadel and every company

Citadel is one program for every company (Creede, 2026-11-01). LatCon Estimating becomes **Citadel + the Latcon pack**:
a signed file with the company's name, rates, items, bid templates and switched-on abilities. The `citadel` module is
company-blind already: one Office Server build serves any company's Citadel, because the server never sees inside it.

## Build order

1. **Licence modules** (this branch): the `modules` line, read, verified, and the old licences unchanged.
2. **The module registry**: which modules are on, `GET /api/v1/office` for the Manager, and every module's health in the
   maintenance report Fleet reads, so Fleet sees one server.
3. **The `citadel` part**: the Office Server finds, watches and reports Citadel Server (service state, version, port, its
   update status); then installs and updates it from the same signed channel.
4. **HTTPS with a pinned fingerprint** for View's own API (the seats pin it on first connect, as Citadel's PCs do).
5. **The `cad` module**: CAD's check-out/check-in store in the server; CAD's app signs in to the Office Server; the move tool.
6. **The `archetype` module**, then **one installer and Manager** for everything, and the new USB kit.

Each step merges on its own, with tests, the guards proven, and a review; each ships as a normal signed update.

## Where it stands (2026-11-10, branch office-server-20261110)

| Step | State |
|---|---|
| 1. Licence modules | done: signed `modules` line; old licences unchanged; the publishing script signs it (`--modules`) |
| 2. Module registry | done: `GET /api/v1/office`, and every part's state in the maintenance report |
| 3. Citadel part | watching done: Citadel Server's version read every 5 minutes on this machine, by its certificate pin, never its data. Next: installing and updating it from the same signed channel |
| 4. HTTPS | server side done: its own certificate, HTTPS on TCP 8716 beside HTTP on 8714 (UDP 8715 stays the finding port), `secure_port` in /health. Next: the seats move to it and pin it (the View app), then plain HTTP can be turned off |
| 5. CAD part | server side done: CAD's check-out/check-in store under `/api/v1/cad/...` with the office's own accounts; `hyperview-server --move-cad "<old CAD data folder>"` moves a CAD server's drawings in (every file checked, the old folder only read). Next: Excalibur CAD signs in to the Office Server |
| 6. Archetype part, one installer and Manager, the USB kit | not started |

Existing servers get the HTTPS port opened to the office network by the Manager's next install/repair (the
service account can't change the firewall); until then HTTPS answers on the server itself and through the
tunnel.

### Moving a CAD server in (once CAD signs in to the Office Server)
1. Stop the old Excalibur CAD server's service.
2. On the server, in an administrator's Command Prompt:
   `"C:\Program Files\Excalibur View Server\hyperview-server.exe" --move-cad "C:\ProgramData\Excalibur CAD Server\data"`
   (the paths as installed there).
3. It says how many drawings, versions and files came. The old folder isn't changed, so the old server can be
   started again if anything looks wrong.
4. Each person signs in to CAD with their Office Server account. Drawings that were checked out on the old server
   are checked in; whoever had one checks it out again.
