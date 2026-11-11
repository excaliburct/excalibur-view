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
3. **Every app on the same computer shares the pin.** They keep it in one per-user file, `%LocalAppData%\Excalibur\office.json`, holding the server address and pin, or the same path under `~/.config` elsewhere. So the second app on a PC never asks again.

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
- The server's Manager window, on the server PC, can reopen it.

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

A badge or notice tells admins that someone is waiting.

## 4. Signed requests

Every request from an approved device to the Office Server, and to the Citadel cell, carries these headers:

```
X-Excalibur-Device: <device id>
X-Excalibur-Time: <unix seconds>
X-Excalibur-Signature: <base64 Ed25519>
```

The signature is over these lines, each ended by `\n`:

```
excalibur-request/1
<device id>
<METHOD>
<path and query exactly as sent>
<body SHA-256, 64 lowercase hex>
<time>
```

The server checks each of these, and **401 `device_unknown`** answers any failure:

- the time is within ±300 s;
- the signature is not one already seen inside that window (a replay cache);
- the device is approved;
- the signature verifies.

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

- The Office Server's installer carries `Citadel Server.exe`, signed with the update key.
- The Office Server installs the cell into its own folder, locked to the cell's identity, and starts it as a child process:
  - under a restricted token;
  - with its own data folder at `ProgramData\Excalibur Office\citadel\`, ACL'd to the cell alone;
  - HTTPS on 47110, as now, so LatCon Estimating's wire is unchanged.
- The Office Server watches the cell and restarts it if it stops. Fleet updates both together, in one signed update.
- **Device checks:** the cell asks the Office Server over the pipe ("is device X approved, and with what key?") and caches the answer for 60 s.
- **The licence:** the cell runs only when the licence lists the `citadel` module.

## 7. Rules (the guards; each one proven by a test that breaks it)

1. A device request from outside the office network is refused.
2. Claiming the first admin works only while the server is unclaimed and within the 24-hour window.
3. A request signed by an unknown, waiting, refused or removed device is refused. So is one with a bad signature, one outside ±300 s, or a replay.
4. Only an admin device can list, approve, refuse, remove or make admins. The last admin can't be lost.
5. An app never accepts a changed server pin.
6. An app that has seen a verified office certificate never accepts that server unverified.
7. The Citadel cell refuses a device the Office Server hasn't approved, and fails closed when the pipe is down.
8. A seat migrated with a session is approved only if its session was valid at the moment of migration.
