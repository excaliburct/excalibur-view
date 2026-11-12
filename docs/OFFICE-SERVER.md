# Excalibur Office Server

One server for every office, whatever Excalibur programs it runs. **It is the only Excalibur server:** there is no
separate CAD server, Archetype server or Citadel Server to install any more (Creede, 2026-11-11). Each of those is
moved into the Office Server once, then retired. It is View's office server, grown: every View server
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
| `cad` | Excalibur CAD's office drawings: check-out, check-in, versions | built in; an old CAD server's drawings are moved in (`--move-cad`) |
| `archetype` | Archetype's shared jobs and releases | built in; an old Archetype server's jobs are moved in (`--move-archetype`) |
| `citadel` | Citadel's estimating jobs, kept **blind** | the Citadel cell: `citadel\Citadel Server.exe`, shipped inside the Office Server and run by it as a supervised child process under a restricted token (docs/OFFICE-ENROLLMENT.md §6a). This is the only way Citadel runs |

A module that isn't licensed isn't served: its routes answer 404, as if it didn't exist.

## How it stays safe

- **Signed licence decides the modules.** The licence gains an optional `modules` line, signed with the rest. A licence
  without one (every licence today) means View only, so old licences keep working and still verify.
- **One update channel, signed on the YubiKey,** with the rollback the parts already have. Nothing updates without a
  version Creede signed.
- **Each part in its own limited account.** The Citadel cell runs under a restricted token with its own SID and its own
  locked data folder (`ProgramData\Excalibur Office\citadel`); the Office Server starts it, restarts it and answers its
  device questions over a locked pipe, and never reads its data. Citadel stays blind: the bids are sealed
  on the office's computers and the server only ever holds sealed bytes.
- **Encrypted, pinned connections everywhere.** View's server moves to the same HTTPS with a pinned fingerprint that
  Citadel and CAD already use (today it is plain HTTP on the office network).
- **Nothing on the internet** except through the office's tunnel.
- **Every change to the server core** gets an independent review and a live attack test before it merges.

**Remote access through a connector set up by hand.** Remote access turned on in the program runs Cloudflare's
connector itself, and the server then counts each internet caller by its own address. An office that runs its own
`cloudflared` instead must set `HYPERVIEW_BEHIND_PROXY=1` on the service. Without it every internet caller looks
like this machine, so one stranger's wrong guesses can hold up everyone else's sign-ins, Fleet's included. The
server never believes a forwarded address unless a proxy is expected, so a program on the server can't claim a new
address with each guess.

**HTTPS only, once every seat has moved.** The API's port answers HTTPS and plain HTTP together, so seats from
before 0.7.0 keep working through an update. Once every computer in the office runs 0.7.0 or newer, set
`HYPERVIEW_HTTPS_ONLY=on` on the service and restart it. Plain HTTP from any other computer is then refused before it
is read, with a short "426, HTTPS only" answer saying to update Excalibur. This machine itself (the tunnel connector, a
script on the server) may still use plain HTTP, because that never leaves the box. The server will not start with
any value but `on` or `off`, or with the switch on while `HYPERVIEW_TLS=off`.

## How existing installs move over

Every separate server is **moved in, then retired**. Nothing is retired until the move has been checked.

| Install | Moved in by | Then retired by Creede |
|---|---|---|
| Mesa Fab's View server | the next server update (Fleet push), in place: it *becomes* the Office Server | nothing to retire |
| Mesa Fab's Archetype server | `hyperview-server --move-archetype "<old Archetype data folder>"` on the Office Server's machine | once the moved jobs are checked in Archetype: stop the old Archetype server's service, then remove it (`sc stop` / `sc delete`, or its own uninstaller); keep its data folder until the next verified backup |
| A separate Excalibur CAD server | `hyperview-server --move-cad "<old CAD data folder>"` (below) | once the moved drawings are checked in CAD: stop the old CAD server's service, then remove it; keep its data folder until the next verified backup |
| An office's Citadel Server 1.0.x | the Office Server's update carries `citadel\Citadel Server.exe`; on its first start over the old Citadel data the cell sends its paired computers to the Office Server once (`migrate`), approved, with who paired them | once LatCon Estimating links to the Office Server and the computers show approved: stop the old Citadel Server service and remove it, so only the cell answers on 47110 |

## How every part stays up to date

| What | How it updates | Proven by |
|---|---|---|
| The Office Server itself (View, CAD and Archetype parts in one program) | The same signed update channel View's server has today: pushed by the maintenance channel or fetched from the publisher's feed, checked against the keys built in, the old version kept for a rollback. Nothing else to install | `kept_up_to_date` tests; every release since 0.6 |
| Its data across an update | New tables are made as the server opens, if not there yet; nothing is rewritten. The certificate and key stay in the data folder, so every seat's pin still matches | the server suites open a fresh and an existing database every run |
| Ports, firewall, tunnel | Unchanged: HTTPS answers on the API's own port 8714 | `served_encrypted` |
| View on each computer | From the office server, as today | `kept_up_to_date` |
| View on iPads and Android | From the stores (Release workflow) | the store release scripts' tests |
| Excalibur CAD | From its own signed feed (GitHub), whatever server it signs in to | CAD's update tests |
| Archetype | From its own Archetype server, or from its own signed feed when its server is an Office Server (an Office Server hands out View's releases, which Archetype would refuse) | ExcaliburArchetype branch archetype-office-client-20261110 |
| The Citadel cell | Inside the Office Server's own signed update: both update together | the cell supervisor's tests (`crate::cell`) and Citadel's own |
| The estimating program on each computer | Its own signed feed, as today | its own tests |
| Licences | The office's licence renews itself from the publisher when bought seats, parts or years are added; a renewal can never take a part, seat or year away | the licence tests |

**Never put a server program older than 0.7.0 back by hand once seats have moved to HTTPS.** A seat that holds the
server's pin never falls back to plain HTTP, and an older server has no HTTPS: every seat would refuse to connect
until the server is updated again. The maintenance channel's rollback is safe: it withdraws the newest View offered
to the seats (an older View doesn't pin, and the server still answers plain HTTP), and a server between 0.7.x
versions keeps the same certificate.

## Citadel and every company

Citadel is one program for every company (Creede, 2026-11-01). Each company's build becomes **Citadel + that company's pack**:
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
| 3. Citadel part | watching done: Citadel Server's version read every 5 minutes on this machine, by its certificate pin, never its data; an administrator can forget the pin after a reinstall (`POST /api/v1/admin/citadel/forget-pin`). Updates: Fleet keeps sending Citadel Server its own signed updates directly, as today; the Office Server reports the version that results |
| 4. HTTPS | server side done: its own certificate, and HTTPS on the API's own port 8714, beside plain HTTP on the same port (each connection's first byte says which), `secure_port` in /health. Next: the seats move to it and pin it (the View app), then plain HTTP can be turned off |
| 5. CAD part | done on both sides: CAD's check-out/check-in store under `/api/v1/cad/...` with the office's own accounts, kept in the server's own database and file store (so the nightly off-site backup and its drill carry CAD's drawings); `hyperview-server --move-cad "<old CAD data folder>"` moves a CAD server's drawings in. Excalibur CAD signs in to an Office Server (branch excad-office-server-20261110) |
| 6. Archetype part | done on both sides: Archetype's jobs under `/api/v1/archetype/...` with the office's accounts, in the server's own database and file store (backed up with the rest); only files sent to Archetype are handed out by it, so a View drawing's fingerprint opens nothing; `hyperview-server --move-archetype "<old Archetype data folder>"` moves an Archetype server's jobs in. Archetype finds its jobs on an Office Server (ExcaliburArchetype branch archetype-office-client-20261110). Branch office-archetype-20261110 |
| One installer and Manager | the server program installs itself as one Windows service carrying View, CAD, Archetype and the Citadel cell; there is no separate Citadel Server service any more (branch office-enroll-20261111: the cell is started, restarted and answered by the Office Server, under a restricted token). Studio > Office in View shows every part and how it is (running and version, not licensed, needs an update, not found, not answering, a changed certificate) |
| The USB kit | after **C** signs the releases: the Office Server (View Server 0.7.0, carrying the Citadel cell), the estimating program, View, the checklists and the sums |

Ports: TCP 8714 (the API, HTTPS and plain HTTP on the one port) and UDP 8715 (seats find the server). Nothing
new to open: an office's firewall and its tunnel's rules stay exactly as they are when the server updates to the
Office Server. The program's own "host the drawings on this computer" server stays plain HTTP for now.

Independent security review (2026-11-10): one high (CAD read drawings before its checks), one medium (a seat's
fallback address could take its sign-in after HTTPS was blocked) and nine lows, all fixed on this branch with tests;
see the commit "Review fixes".

### Moving an Archetype server in
1. Stop the old Archetype server's service.
2. On the server, in an administrator's Command Prompt:
   `"C:\Program Files\Excalibur View Server\hyperview-server.exe" --move-archetype "C:\ProgramData\Excalibur Archetype\Server\data"`
3. It says how many jobs, check-ins and files came. The old folder isn't changed.
4. Once the jobs are checked in Archetype, Creede removes the old service (see the table above).

### Moving a CAD server in
1. Stop the old Excalibur CAD server's service. (On Linux, run step 2 as the account the Office Server runs as,
   so the copied files are the server's.)
2. On the server, in an administrator's Command Prompt:
   `"C:\Program Files\Excalibur View Server\hyperview-server.exe" --move-cad "C:\ProgramData\Excalibur CAD Server\data"`
   (the paths as installed there).
3. It says how many drawings, versions and files came. The old folder isn't changed, so the old server can be
   started again if anything looks wrong.
4. Each person signs in to CAD with their Office Server account. Drawings that were checked out on the old server
   are checked in; whoever had one checks it out again.
5. Once the drawings are checked, Creede removes the old CAD server's service (see the table above).
