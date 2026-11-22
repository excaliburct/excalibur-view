# Linking computers to the Excalibur Office Server, without codes

Status: design, 2026-11-11. Creede's requirements:

- Every app links without anybody reading a code: View, CAD, Archetype, and LatCon Estimating (Citadel).
- The first person to set up the office's server is its admin. At LatCon, that is Mark.
- An admin approves each other computer, and can make other people admins.
- It is as easy as possible, because LatCon's IT can't be relied on.
- It ships in the Office Server's first release (0.7.0), with Citadel inside it.

## 1. The pieces

- **The Office Server** (Rust, one Windows service) keeps the office's **device list**. It is the only authority on which computers may connect, for every app.
- **A device** is one app on one computer for one Windows user. It has an **Ed25519 device key**, made on first run:
  - Windows keeps the private half with DPAPI (CurrentUser); on a Mac or Linux it is a 0600 file.
  - The private half never leaves the computer.
- **Each app keeps its own sign-in on top**, as today:
  - View's accounts, on the server;
  - Citadel's sealed store accounts and keyring, on the PC.

  A computer must be **approved and** its person signed in.
- **Citadel runs inside the Office Server** as a supervised child process, its "cell":
  - it runs under its own restricted identity, with its own locked data folder;
  - it does not keep its own device list: it asks the Office Server, over a local named pipe whose ACL lets only the Office Server's account and the Citadel cell's account connect;
  - it stays a blind store: jobs are still sealed on each PC.

## 2. Is this the real server? (no codes)

The baseline needs nothing from anyone:

1. **Found on the office network.** A computer finds the server through discovery (UDP 8715, which answers local networks only), or through a typed address.
2. **First contact, pinned for good.** A computer that has never linked takes the server's certificate pin, the SPKI SHA-256 of its 8714 HTTPS certificate, on its first connection. It accepts this only when the address is on a local network: a private IPv4 range, fc00::/7, fe80::/10 or loopback. From then on it accepts only that pin, never plain HTTP. This is how View's seats already pin.
3. **Every app on the same computer shares the pin.** They keep it in one per-user file, `%LocalAppData%\Excalibur\office.json`, holding the server address and pin, or the same path under `~/.config` elsewhere (on an iPad or iPhone, under the app's own `Library/Application Support`, the only place iOS lets it keep such a file; a record an earlier build kept under `~/.config` is copied across once, never over a newer one). So the second app on a PC never asks again.

Stronger, when Fleet can reach the server:

4. **The office certificate.** The Office Server makes a signing request for a statement:

   ```
   excalibur-office-cert/1
   company <name>
   license <licence id>
   pin <8714 SPKI sha256 hex>
   citadel-pin <47110 SPKI sha256 hex, or none>
   issued <YYYY-MM-DD>
   ```

   - Creede signs it on the YubiKey (a new "office" slot) from Fleet.
   - The server serves it at `GET /api/v1/office/identity`.
   - Every app carries the office public key built in, the same way update keys are.
   - An app that sees a valid certificate whose pin matches the connection marks the server **verified**.
   - Once an app has seen a verified server, it never accepts that server unverified again.

   This closes the only gap the baseline leaves: someone already on the office network, impersonating the server at the very moment a brand-new computer links for the first time.

## 3. Is this computer allowed in? (no codes)

### Asking

`POST /api/v1/devices/request`. It is open, but **only from local networks**: a request marked as coming from outside is refused (403). It is rate-limited per address with the JOINING rule.

```json
{ "public_key": "<base64, 44 chars>", "app": "view|cad|archetype|latcon-estimating",
  "app_version": "0.7.0", "machine": "DANA-LAPTOP", "os_user": "dana", "display_name": "Dana",
  "platform": "windows-x64", "time": 1794410000,
  "proof": "<base64 Ed25519 signature>" }
```

The proof signs these lines, each ended by `\n`:

```
excalibur-enroll/1
<server pin>
<public_key base64>
<app>
<machine>
<os_user>
<time>
```

Binding the request to the server's pin means a request made to an impostor can't be relayed to the real server.

### What the server keeps

- `device_id` = the first 16 bytes of SHA-256("excalibur-device/1\n" + public key), as 32 lowercase hex characters.
- The device is stored in the `devices` table, `waiting`.
- The answer is `{ "device_id": "...", "state": "waiting" }`.

### The first device

When the server has **no approved device** yet, it is unclaimed. The first local request is then **approved as an admin** at once. That is Mark at LatCon, or the person who sets the server up.

- To stop a stranger racing the owner, claiming is open only for **24 hours after the service first starts**.
- "Unclaimed" means **no admin device approved for every app**: computers carried over from Citadel Server never count, and making one an admin approves it for every app.
- A request showing any sign of a proxy (`Forwarded`, `X-Forwarded-For`, `X-Real-IP`, `Via`) is treated as outside, and never claims.
- On the server PC itself: `hyperview-server --reopen-claim` (only while there is no admin), `--make-admin <device id>`, and `--reset-admins` (every admin unmade, claiming open for a day). All three are in the audit log.

### Carrying seats over from before 0.7.0

- A session issued before the device list began, and still valid, carries **exactly one** device over, once; then it is spent.
- Carrying over closes **30 days** after the device list began.
- An administrator's seat becomes an admin device only from the office network; from outside it comes over as an ordinary one.
- A refused or removed device is never brought back by asking again, by a session or by a join: only an admin's Approve does that.
- The open update routes never keep a session alive.

## 3a. An upgrade never locks an office out (0.7.2)

What happened at Mesa Fab on 2026-11-20, and what each part of 0.7.2 changes:

1. 0.7.1 was published as an early release (a GitHub prerelease, `preview` in its `release.json`). Mesa's Office Server, set
   to take them early, updated itself. Every desktop follows the stable channel, which skips prereleases
   (`hub::feed::choose`), so none could update except through that server. **Fixed:** `publish.py sign` and `release` and
   `release_now.py` make a full, stable release unless `--channel preview` / `--preview` is asked for
   (`tools/test_release_channel.py`).
2. The 0.6.30 and 0.6.31 desktops (and the App Store iPads on 0.6.31) can't sign requests, so the device gate turned
   them all away. **Fixed:** the upgrade grace, below.
3. An upgraded server never opened the first-admin claim, so a new 0.7.1 computer waited with no admin anywhere.
   **Fixed:** the claim opens after an upgrade too, below.
4. **Fixed:** the update routes answer any seat on the office network, signed in or not; a 0.7.2 desktop that its server
   turns away asks the publisher's feed itself, and shows "Update Excalibur View" where it would say only "Not connected".

### The upgrade

The server counts as **upgraded** when any account in its `people` table was made before the device list began
(`devices_began`): accounts on a fresh install are all made after its first start. On the first start of 0.7.2 or later
that finds the office upgraded (`devices::open_for_the_upgrade`), **once** (it writes `device_upgrade_seen`, and a restart
never does it again), and only when the device list began less than 30 days before:

- **the grace opens** for 14 days (`device_grace_until`), written in the audit log;
- **the claim opens** for 24 hours from that start, the same rule as a fresh install, but only when the office has **no**
  admin computer (§3, "The first device"). An office with an admin computer is never opened to a claim.

Mesa Fab, already on 0.7.1 with its admin set: its accounts predate its device list (2026-11-20), so the grace opens on
0.7.2's first start; the claim does not. A fresh install (LatCon): nothing changes.

### The grace, while it lasts

Every rule below is held to the office network as §3 defines it (no tunnel mark, no sign of a proxy, a local socket
address). From outside, nothing changes.

- **A computer from before 0.7.0** (a request with no `X-Excalibur-Device` header at all) is let past the device gate:
  - with a session that is valid now, of an account made before the device list began; the route's own sign-in checks
    run as always, and the session is never extended by the gate;
  - or to sign in (`POST /api/v1/auth/token`); the sign-in takes only an account made before the device list began
    (anything else is 401 `device_unknown`, as the gate says);
  - a session that has run out is answered 401 `unauthorised` by the gate itself, the word an old seat signs in again on;
  - any other request, signed in as nobody (`/join`, `/setup`...), is held by the gate as ever.
- **A computer on 0.7.0 or later that is waiting** may sign in. When the password matches and the account was made before
  the device list began, the sign-in approves that computer in the same step (`how = grace`), as an ordinary computer, or
  as the admin computer when the account is an administrator's and the office has **no** admin computer. A refused or
  removed computer stays so, whoever signs in on it; only an admin's Approve brings it back. A 0.7.2 desktop that is
  told to wait tries its sign-in once, so this needs nothing from the person.
- Every computer let in is written in the audit log: an old seat at most every six hours a person and address ("a
  computer from before 0.7.0 was let in during the upgrade grace"), each approval once ("a computer was approved by its
  person's sign-in during the upgrade grace").
- On the server PC: `hyperview-server --upgrade-grace <days>` sets the grace to that many days from now (0 to 30; 0 ends it
  now), written in the audit log. For an iPad still waiting on the App Store's review, say.

When the grace ends, the device gate holds exactly as in 0.7.1. A request with no device at all is then answered 401
`device_unknown` with words saying to update Excalibur View.

### The update, always

`GET /api/v1/update/latest` and `GET /api/v1/update/{version}/download` answer any seat **on the office network**, signed
in or not, approved or not, in the grace or after it. From outside they still need a session or a key (security review
H2e: they never extend one). What they hand out is the same signed build that is public on the release page, and every
seat checks its signature itself before installing it.

A 0.7.2 desktop that its server turns away (`device_unknown`, `unauthorised` or `clock_behind` on the update question)
asks the publisher's feed for its own channel instead, and so does one with no server (as before). One signed in whose
server's gate refuses it after letting it in shows the server's words and **Update Excalibur View** in the Studio panel.

### Threats, and what holds them

| Threat | What holds it |
|---|---|
| A stranger on the office network uses the grace to get in | They need a valid session or the password of an account the office had before the upgrade. That is exactly the bar every computer met before 0.7.0, and only for 14 days. Wrong passwords are counted and slowed as every sign-in is. |
| The same from the internet, through the tunnel or a forwarded port | The grace, the open update routes and the claim are all office-network only; a tunnel mark or any sign of a proxy counts as outside. |
| **Any existing account, not only an administrator's, approves a new computer by signing in on it** (a deliberate choice, for review) | Without it, every desktop that updates during the grace would wait for the admin, which is the lockout this fixes. It needs a password the office had before the upgrade, typed on the office network, inside the 14 days; that is the same bar the grace already sets for old seats, and the bar every computer met before 0.7.0. Such a computer is approved as an ordinary one, never an admin unless the account is an administrator's and the office has no admin computer. Each one is in the audit log and marked `how = grace` in Computers, where an admin can remove it. |
| A stolen old password gets a computer approved for good | Only during the grace, only from the office network, and every approval is written in the audit log with `how = grace` on the device, so an admin sees and removes it in Computers. A removed computer never comes back by signing in. |
| A newly made account (or one made by a stranger with a join code) uses the grace | Only accounts made before the device list began get it. |
| An attacker strips a 0.7 computer's device headers to look old | Then it is held to the old rule (password or session, office network, 14 days), never more. |
| A stranger races the owner for the reopened claim | The claim reopens only for an office with no admin computer, for the same 24 hours as a fresh install, from the office network, never through a proxy. `--make-admin <code>` at the server is still the sure way. |
| A restart, a rollback or a later update stretches the grace or reopens the claim | `device_upgrade_seen` is written before either opens; nothing opens them again but the console. |
| The grace is used forever on an office that never noticed | It ends by itself after 14 days; the console can set at most 30; none opens more than 30 days after the device list began. |
| The open update routes leak something or are abused | They serve the public signed build and its manifest only; seats verify the signature against keys built into them. The feed is looked at at most once a minute however often they are asked. |
| A man in the middle offers an old build to roll a seat back | Unchanged: a seat takes a build that is not newer only from its own pinned server's administrator's hold (audit, 2026-11-11). |

### What each office sees when 0.7.2 rolls out (for the owner)

**Mesa Fab** (on 0.7.1, the admin computer set with `--make-admin`):

1. Within half an hour of the release, the office server updates itself to 0.7.2. Nothing to do.
2. The computers still on 0.6.30 and 0.6.31, and the iPads, connect again as they used to: open View and, if asked, sign
   in with the same email and password. They keep working for two weeks.
3. Each desktop updates itself to 0.7.2 (from the office server, or from the internet if it isn't connected), usually
   the next time View is opened. When it restarts on 0.7.2 it asks to be approved; the person's own sign-in approves it.
   The owner doesn't have to do anything, and can see each one in Studio, Office, Computers.
4. The iPads stay on 0.6.31 until the App Store approves the new build. If that takes longer than two weeks, run
   `Hyperview-Server.exe --upgrade-grace 14` on the server for two more.
5. The admin computer stays the admin computer. Nobody new can claim the office.

**LatCon** (installed fresh on 0.7.1):

1. The office server updates itself to 0.7.2. Nothing else changes: there is no grace and no new claim, because every
   computer there was set up on 0.7.1 and is already approved. Mark approves new computers as before.
2. The desktops update themselves to 0.7.2, from the office server or from the internet.

### Waiting

`GET /api/v1/devices/me` is signed by the device (§4). It answers `{ state: waiting|approved|refused|removed, admin: bool, approved_by, approved_at }`. The app checks every 5 s while its "Waiting for approval" screen is open.

### Approving

These calls are signed by an **admin device**:

- `GET /api/v1/devices`: every device, newest waiting first.
- `POST /api/v1/devices/{id}/approve`, with `{ "admin": false }`.
- `POST /api/v1/devices/{id}/refuse`.
- `POST /api/v1/devices/{id}/remove`: a removed device must ask again.
- `POST /api/v1/devices/{id}/admin`, with `{ "on": true|false }`: make or unmake an admin.

The last admin can't be removed, refused or unmade; the answer is 409 "make someone else an admin first".

Every call is written in the audit log.

### What an admin sees

> **Dana (DANA-LAPTOP, signed in to Windows as dana) wants to use LatCon Estimating. Asked 2 minutes ago from 192.168.1.44.**
> [Approve] [Approve as admin] [Refuse]

Where the approval screen appears:

- in every app, for admin devices: LatCon Estimating (Settings → Office server → Computers), View (Studio → Office → Computers), CAD, Archetype;
- in the server's Manager window;
- in Fleet.

A badge or notice tells admins that someone is waiting. Both the asking computer and the admin's list show the computer's code, the first twelve hex digits of its device id in three groups of four ("29AC 6B9D FEA9"), so a person can match them.

## 4. Signed requests

Every request from an approved device to the Office Server carries these headers:

```
X-Excalibur-Device: <device id>
X-Excalibur-Time: <unix seconds>
X-Excalibur-Signature: <base64 Ed25519>
X-Excalibur-Body-SHA256: <body SHA-256, 64 lowercase hex>
```

The signature is over these lines, each ended by `\n` (changed 2026-11-11 after the security review: the server's pin is now the second line):

```
excalibur-request/1
<server pin: the 8714 SPKI SHA-256 hex, or none>
<device id>
<METHOD>
<path and query exactly as sent>
<body SHA-256, 64 lowercase hex>
<time>
```

`X-Excalibur-Body-SHA256` repeats the body line, so the server checks the signature **before it reads the body**, then holds the body to that hash. **A request with a body must send it**; without it, or on a GET or HEAD, any body is refused unread. A computer not yet approved never has a body read at all, and a proven body is given the time its size allows (half a minute, plus a second per 256 KiB the route takes, at most an hour). `docs/office-enrollment-vectors.json` has worked examples.

The server checks each of these, and **401 `device_unknown`** answers any failure:

- the time is within ±300 s, and not before the server's process started (so a restart opens no replay window; that refusal is 401 `clock_behind`, saying the computer's clock is behind and it will work in a few minutes);
- the signature is not one already seen inside that window (a replay cache, with a share per device);
- the device is approved for every app (a computer carried over from Citadel Server is approved for the cell only, until an admin approves it);
- the signature verifies, for this server's pin.

A HEAD request meets the same door as its GET.

Citadel's existing `citadel-request/1` signing stays exactly as it is, for LatCon Estimating's traffic to the Citadel cell. Only the device list moves.

## 5. How each app uses it

- **View, CAD, Archetype:**
  - Signing in, joining and every API call also need an approved device (§4).
  - The join code and invitations stay as a second way in for computers outside the office network.
  - A seat that already holds a valid session when it first meets 0.7.0 **enrolls itself and is approved automatically**: it was admitted before. Mesa Fab's seats change nothing.
  - The office's View admins are made device admins on the same rule.
- **LatCon Estimating:**
  - "Link to the office server" asks no code. It finds the server, sends the request, shows "Waiting for Mark (or another admin) to approve this computer", and links when approved.
  - Computers already paired with Citadel Server 1.0.x are carried into the device list approved, with the person who paired them recorded.
- **Admins:** in every app, the Computers list approves, refuses, removes and makes admins.

## 6. Citadel inside the Office Server

- View's public release does **not** carry the cell: it carries only its pin (`packaging/citadel-cell.json`, the SHA-256 of
  LatCon's cell zip and of `Citadel Server.exe`). The cell reaches only offices whose licence lists `citadel` (§6c).
- The Office Server installs the cell into its own folder, locked to the cell's identity, and starts it as a child process:
  - under a restricted token;
  - with its own data folder at `ProgramData\Excalibur Office\citadel\`, ACL'd to the cell alone;
  - HTTPS on 47110, as now, so LatCon Estimating's wire is unchanged: the Office Server answers TLS there with the same
    certificate and relays each connection to the cell over a locked pipe (§6b).
- The Office Server watches the cell and restarts it if it stops. Fleet updates both together: the cell part first, over the
  maintenance channel, then the server, which takes only the cell part of its own version.
- **Its TLS key:** held by the Office Server in Windows' own key store, usable by the system only; the cell never sees it (§6b).
- **Device checks:** the cell asks the Office Server over the pipe ("is device X approved, and with what key?") and caches the answer for 60 s.
- **The licence:** the cell runs only when the licence lists the `citadel` module.

## 6a. The cell contract (exact)

**How the Office Server starts the cell** (Windows):

```
"<office program folder>\citadel\Citadel Server.exe" --cell --pipe excalibur-office-<install id> --data "C:\ProgramData\Excalibur Office\citadel" --certificate <certificate, base64 DER> --relay excalibur-cell-relay-<32 hex> --relay-sid <cell SID>
```

From Citadel Server 1.1.7: `--certificate` is public (the cell works out its own pin from it); `--relay` is the relay pipe's
name, new and random at each start, never the control pipe; `--relay-sid` is the cell's own SID, written into the relay
pipe's permissions. There is no `--port`, `--tls-key` or `--tls-cert`; the cell refuses them (§6b).

- `--cell` means:
  - no Manager window and no Windows service of its own;
  - no own pairing, and its `/v1/pair*` routes answer 410 `pairing_moved`;
  - its devices are the Office Server's;
  - it exits when the pipe closes.
- The Office Server restarts the cell after 5 s if it exits, at most 5 times in 10 minutes, then writes it in the log and shows it on `/api/v1/office` as "not answering".

**The pipe:** `\\.\pipe\excalibur-office-<install id>`, the install id written in letters, digits and dashes only (its `_` as `-`, at most 64), the form the cell accepts.

- Created by the Office Server.
- ACL: SYSTEM, plus the cell's own SID (read and write data only, never make an instance). Nobody else may connect.
- The Office Server makes the next instance before the old one closes, so the name is never free; a name someone else holds is an alarm ("pipe-taken" on `/api/v1/office`), not a restart.
- Messages are one JSON object per line, UTF-8, at most 4 KiB per line. Each request gets exactly one answer line.

The messages:

- Cell to server: `{"op":"device","id":"<32 hex>"}`
  - Server to cell: `{"state":"approved","public_key":"<base64>","admin":false,"name":"Dana","machine":"DANA-LAPTOP","app":"latcon-estimating"}`
  - or `{"state":"waiting"|"refused"|"removed"|"unknown"}`
- Cell to server: `{"op":"hello","version":"1.1.0"}`
  - Server to cell: `{"ok":true,"office":"<company>","modules":["view","citadel",...]}`
- Cell to server: `{"op":"migrate","devices":[{"public_key":"...","name":"...","machine":"...","paired_at":"...","paired_by":"..."}]}`
  - Server to cell: `{"ok":true,"added":N}`, or `{"ok":false,"error":"migrate_sealed"}`
  - Sent once, on the first start of a cell over an old Citadel Server's data. It brings its paired computers into the device list, approved **for the cell only**.
  - Taken only after a `hello`, until a `migrate` has succeeded; then never again. A cell with nothing to carry over sends `{"op":"migrate","devices":[]}` to say so. A cell that stops between its hello and its migrate migrates on its next connection.

**Device ids for Citadel traffic:** the cell takes the Office Server's answer for the public key and works out Citadel's own id from it (`citadel-device/1` label). Both ids name the same key.

**Failing closed:** if the pipe doesn't answer within 2 s, the cell refuses the request with 503 `office_unreachable`. It never falls back to an old list.

## 6b. TLS on 47110: ended by the Office Server, relayed to the cell (Citadel Server 1.1.7, View 0.7.1)

Windows' TLS (SChannel) gives no credentials at all to a process whose token has restricting SIDs, which the cell's has:
`AcquireCredentialsHandle` returned `0x8009030E` "No credentials are available" even with no certificate (the Windows
end-to-end test, try 21, 2026-11-15). So the cell can't serve TLS, whatever key it holds. Creede chose (2026-11-15): the
Office Server, which runs as the system, ends TLS on 47110 itself and relays each connection to the cell.

**On 47110 (the Office Server, `crate::cellrelay`):**

- rustls, TLS 1.3 and 1.2 only (Citadel's own), ALPN `http/1.1` (what Citadel served), the certificate every computer
  pinned. The handshake signs through the cell's key in Windows' key store (`NCryptSignHash`, ECDSA P-256 SHA-256); the
  key never leaves Windows.
- At most 500 connections, 32 from one machine (an IPv6 /64 counts as one), Citadel's own numbers, held across the IPv4 and
  IPv6 listeners together (one set of caps for both). A handshake must finish within 10 s; a connection that carries
  nothing either way for 90 s is closed (the cell's own keep-alive is 60 s). Every chunk is flushed on as soon as it is
  written, so TLS never holds the end of an answer back.
- Listened on from the cell's first start (IPv4, and IPv6 where there is one), each socket bound with
  `SO_EXCLUSIVEADDRUSE`, so no other program can share or take the port (the Office Server's own 8714 is bound the same
  way). With no cell running, a connection is closed at once.
- Just after a restart, Windows may refuse to bind an exclusive port again while the last process's connections are
  still closing. The Office Server then tries again with a growing wait (from a quarter second up to 5 s) for up to a
  minute, and logs each wait ("... is still held; trying again in ... ms"). Any other bind failure is reported at once.
- 47110's IPv6 socket is IPv6 only (IPv4 has its own socket). If 8714 is set to listen on `[::]`, that one socket takes
  IPv4 as well (dual-stack), so the office is reached either way.
- If the key ever fails to sign a handshake, the log says ALARM (at most once a minute) and `/api/v1/office` shows the
  cell as not answering for two minutes after the last failure. The firewall rule "Excalibur Office Server (Citadel cell)" is for the Office Server's program, TCP 47110,
  local subnet only, every profile; it is replaced at each start of the cell so an older rule never lingers.

**The relay pipe (`cell::relaywin`, Citadel's `ServerHost.ListenRelay`):**

- `\\.\pipe\excalibur-cell-relay-<32 hex>`, a new random name at each start of the cell, passed on its command line, apart from
  the control pipe.
- Served by the cell (Kestrel's named-pipe transport), plain HTTP/1.1, the cell's only listener. Its permissions are
  `D:P(A;;GA;;;SY)(A;;GA;;;<cell SID>)`: the system (the Office Server connects as it) and the cell (it makes each next
  instance); nobody else may open it, and remote clients are refused.
- Before a byte is sent, the Office Server checks with `GetNamedPipeServerProcessId` that the pipe is served by the cell
  process it started; a pipe served by anyone else is an alarm and gets nothing.
- Each connection's first bytes are a PROXY protocol v2 header (version 2, PROXY, TCP over IPv4 or IPv6) with the
  caller's real address and port and the address it reached. The cell reads it within 5 s and sets the connection's
  remote and local ends from it, so the per-address cap, the Guard's per-address limits and the logs see the real
  computer. A connection without a good header is closed unanswered; the header is trusted only because this pipe is the
  cell's only listener and only the system may open it.
- Bytes are relayed in each direction on its own, so one waiting to write never holds up the other: 16 KiB at a time,
  each write waiting for the other side (backpressure) for at most 90 s. A direction with nothing to read ends the
  connection only when the other has been quiet as long, so a long upload or answer is never cut. Any failure or the idle
  limit ends both directions at once and both are closed together, so a connection's slot is given back within 90 s of
  its last byte. When the caller ends its sending, the cell's side is only flushed (Windows named pipes have no
  half-close in tokio): the cell sees the end when the relay ends and the pipe closes.

**The key:**

- **Where:** a persisted, **non-exportable** machine key in the Microsoft Software Key Storage Provider.
- **Who may use it:** its permissions are exactly `D:P(A;;FA;;;SY)`: protected, the system in full, nobody else, the cell
  included. (Windows' key store writes the system's entry back with generic bits added, `0xD01F01FF`; the check reads
  generic bits as the file rights they grant.)
- **What the cell gets:** the certificate only, public, for its pin.
- **What the Office Server keeps** in `<service folder>\citadel-tls\` (the system's and administrators' only):
  `key-name.txt`, `certificate.der`, and 1.0's moved `tls.pfx`.
- **Copies of the key as files**, until an administrator removes them: `citadel-tls\tls.pfx` (and any
  `citadel-tls\quarantine*.pfx` or `tls-<n>.pfx`), and the `tls.pfx` left in Citadel Server 1.0's renamed data folder
  (`C:\ProgramData\Citadel Server (moved to the Excalibur Office Server <date>)\`). Each is DPAPI-sealed for this machine,
  readable by SYSTEM and administrators. `hyperview-server --forget-citadel-key-copies` removes them, refused until the
  cell has answered a computer through the relay with the key in Windows' store (noted as `citadel_cell_served`).

At every start of the cell (`crate::cellkey` decides; `cell::tlskey` hands it to Windows):

| What is there | What happens |
|---|---|
| The first start after the move: 1.0's `tls.pfx` in the cell's folder, nothing kept | It is imported once (DPAPI opened as the system with Citadel's purpose; PFX import as a non-exportable CNG machine key). Its certificate is kept, so every computer's pin still matches. `tls.pfx` is moved out of the cell's reach into `citadel-tls\`. |
| A new office: nothing moved, nothing kept, and the cell never served | A new P-256 key is made with its permissions set **before** it is finished, and a certificate made for it in Citadel's own shape (`CN=Citadel Server`, this computer's name and `localhost`, ten years). |
| A restart, a new cell version, an update, a rollback: the key kept | The kept key is opened by name. |
| A `tls.pfx` appears in the cell's folder after a key is kept | Not used: moved into quarantine (`citadel-tls\quarantine.pfx`), its read-only flag cleared first, with an alarm in the log. The cell can write its own folder, so nothing there replaces the office's key. |
| A `tls.pfx` in the cell's folder can't be moved out (at import or quarantine) | The cell isn't started until the Office Server restarts; an alarm in the log, and `/api/v1/office` shows it not answering. A copy of the key never stays with the cell. |
| The key's permissions were changed (an older build's read entry for the cell included) | Set back to the system only, with an alarm in the log, and read back; if they still aren't exactly ours, the cell isn't started. |
| The key is owned by anyone but the system or administrators | Refused: whoever owns it could always give itself access. |
| Half the kept pair, or nothing kept although the cell has served before (`state\moved-from.json`) | Refused, with an alarm: a new key would change the pin every computer holds. Put `citadel-tls` back from a backup, or delete the mark to start again with a new certificate (every computer pairs afresh). |
| The key and the certificate don't match | Refused. |

## 6c. How the cell reaches an office (only one licensed for citadel)

- **At install:** the office's private IT kit (from LatCon) carries LatCon's cell zip. After the Office Server is
  installed with the office's licence, an administrator runs
  `hyperview-server --install-citadel-cell "<zip>"`. It is refused unless the Office Server is installed, its licence
  lists `citadel`, and the zip's SHA-256 is the pinned one; then only the pinned files are taken out and checked, and the
  service restarts, checks the cell once more, puts it in place and starts it.
- **On updates:** Fleet signs the cell part of each version (`tools/citadel_cell.py bundle`, then
  `hyperview-release prepare|finish|bundle --platform windows-x64-citadel`) and pushes it over `POST /api/admin/update`
  (the maintenance key) **only to installs licensed for citadel**. The server checks the signature, refuses it when its
  licence doesn't list `citadel`, and keeps it in `citadel-parts\` (never among the seats' releases, so no seat is sent
  it). When the server updates itself to that version, it checks the part's signature again and unpacks it; the new
  server checks it against its own pin before the cell starts. A licensed server running the cell doesn't update until
  Fleet has delivered that version's cell.
- **Never:** View's public release, `publish.py sign` and `publish.py release` refuse `--citadel`. An office without
  `citadel` (Mesa Fab) never downloads or holds the cell.

## 7. Rules (the guards; each one proven by a test that breaks it)

1. A device request from outside the office network is refused.
2. Claiming the first admin works only while the server is unclaimed and within the 24-hour window.
3. A request signed by an unknown, waiting, refused or removed device is refused. So is one with a bad signature, one outside ±300 s, or a replay.
4. Only an admin device can list, approve, refuse, remove or make admins. The last admin can't be lost.
5. An app never accepts a changed server pin.
6. An app that has seen a verified office certificate never accepts that server unverified.
7. The Citadel cell refuses a device the Office Server hasn't approved, and fails closed when the pipe is down.
8. A seat migrated with a session is approved only if its session was valid at the moment of migration.
9. The upgrade grace (§3a) is for the office network only, for accounts made before the device list began, opens once,
   and never brings back a refused or removed computer; the claim reopens after an upgrade only when there is no admin
   computer. The update routes need a sign-in from outside.
