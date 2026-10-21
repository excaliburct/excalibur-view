# Excalibur View Sealed — where your data goes

For the compliance officer, the IT department, and whoever has to sign the
System Security Plan. Written to be handed to an auditor as it stands.

**Scope.** Excalibur View Sealed, and the Excalibur View Office server it runs
against. Covers where drawing data goes, what connections the software makes,
and what is recorded.

**The short version.** Nothing leaves your network. The desktop program and
the server both run on your computers. Excalibur Construction Technologies
never receives your drawings, your markups, your takeoff, your project names
or your people. In Sealed, the software is prevented from reaching the outside
world at all, by a control your administrator sets and nobody in the office can
undo.

**What this document is not.** No software makes a company compliant. NIST SP
800-171 compliance depends on how your whole organisation handles Controlled
Unclassified Information — your people, your facilities, your network, your
procedures. What follows tells you truthfully what this one piece of software
does, so you can write it into your own plan. Where a control is yours rather
than ours, it says so.

---

## 1. Where data lives

| What | Where it is kept | Leaves your network? |
|---|---|---|
| Drawings (PDF) | Your office server's data folder, and a cache on each person's computer | No |
| Markups, measurements, takeoff | Inside the PDF, and in the server's database | No |
| Tool chests and profiles | Each computer, and the server when the office shares one | No |
| People, roles, sessions | The server's database | No |
| The license file | The server's database and wherever your administrator keeps the file | No |
| The audit log | The server's database | No |

There is no Excalibur cloud service. There is no account with us. There is no
telemetry, no analytics, no crash reporting to us, and no license check that
phones home — a license is a signed file your own server verifies offline.

The program's own settings live under the signed-in user's profile
(`%APPDATA%\Excalibur Hyperview`); the cache of downloaded drawings lives under
`%LOCALAPPDATA%`. Both are ordinary per-user folders covered by whatever your
existing policy does to them.

## 2. Every connection the software can make

**Inside your network, always:**

| Connection | From | To | Purpose |
|---|---|---|---|
| The office server | Each desktop | Your server, HTTPS | Drawings, markups, takeoff, people, chests |

**Outside your network — every one of these is off in Sealed:**

| Connection | From | To | Purpose when not sealed |
|---|---|---|---|
| Update check | Desktop and server | The release feed you configure (GitHub by default) | Look for a newer version |
| Update download | Desktop and server | The same | Fetch it |
| The assistant | Desktop | The assistant's provider | An assistant app working alongside the person |
| Excalibur Fleet | Server | The Fleet service | Watching a server on your behalf |
| Change notices | Server | The address an administrator gives it | Telling another program a takeoff changed, only once an administrator sets one up. Sealed sends them only to addresses inside your network |
| Website links | Desktop | excaliburct.com | Pricing, documentation |

That is the complete list. In Sealed none of them is made, and the software
offers no setting that makes them.

## 3. How Sealed is enforced

Two things can seal an installation, and **neither is a checkbox inside the
program**:

1. **A Sealed license.** An `edition: sealed` license seals the server and
   every seat that signs in to it. Nobody has to remember to switch anything
   on; nobody in the office can switch it off.
2. **Machine policy.** Your administrator enables *Seal this computer* under
   Computer Configuration → Administrative Templates → Excalibur Construction
   Technologies → Excalibur View. It writes
   `HKLM\Software\Policies\Excalibur\View` → `Sealed` (DWORD 1).

Policy outranks everything. A computer sealed by policy stays sealed on an
ordinary Office license, on no license at all, and while signed out — which
matters, because the computer you are most concerned about is the one that has
never reached your server. **Nothing in the software unseals a machine that
policy sealed**, and there is an automated test in our source that asserts it.

The `HKLM\Software\Policies` branch is writable only by administrators and is
the branch Group Policy owns, so this is not a setting a user can reach.

The ADMX and ADML templates are in `deploy/policy/` and install the usual way:
`ExcaliburView.admx` into `PolicyDefinitions`, `en-US\ExcaliburView.adml` into
`PolicyDefinitions\en-US`.

**What Sealed does not restrict.** Everything inside your own network. A
private address (10.x, 172.16–31.x, 192.168.x, 169.254.x, localhost), a bare
hostname such as `drawings`, and the suffixes `.local`, `.lan`, `.internal`,
`.intranet` and `.home.arpa` are all treated as inside and never refused.
Opening, marking up, measuring, printing and exporting are untouched. A sealed
seat is the same program with the doors to the internet shut, not a reduced
one.

## 4. Updating a sealed installation

A sealed installation never fetches its own updates. Instead:

1. You obtain the release from us by whatever route your policy allows.
2. Every release is signed with an Ed25519 key held offline on the publisher's
   own computer. The software verifies that signature against a key compiled
   into it before anything is installed — so the download path is never what
   makes an update safe.
3. Your administrator installs it.

You can verify a release yourself: each one carries a SHA-256 and a detached
signature, and the trusted public key is published on our website and built
into every copy of the program.

## 5. Accounts and access

- Each person has their own account on your server. Passwords are stored
  hashed with Argon2id, each with its own salt; the plaintext is never stored
  and never leaves the moment it is typed.
- Three roles: **viewer** (read and export only), **estimator** (mark up,
  measure, save), **administrator** (also people, chests, licenses, settings).
- Sessions are bearer tokens, stored on the server as SHA-256 digests — what
  the client holds is not what the server keeps. They expire after 30 days of
  **no use**, and every use pushes that out, so a laptop left unused goes dead
  on its own while somebody working daily is not made to retype a password.
- Keys for other programs (an assistant, or an integration such as FabWire)
  are separate from sessions, prefixed `hvk_`, stored as digests, shown once,
  and revocable one at a time.
- The first account on a new server is claimed once, by whoever sets it up,
  and that route closes permanently the moment an account exists.

## 6. The audit log

Kept on your server, in its database, and never sent anywhere.

**Recorded:** signing in; a sign-in refused; reaching or downloading a drawing
set; uploading one; exporting a takeoff; adding or removing a person; changing
what a person may do; changing a password; adding a license; making or
revoking a key for another program; changing a server setting.

**Each line carries:** the time in UTC, the person's id, their email as it read
at the time, the action, what it was done to, any detail, and the address it
came from when the server can tell.

**Not recorded:** every markup as it is drawn. A log nobody can read is a log
nobody reads, and the markups are themselves a record of who drew what.

**Append only.** No route in the software edits or deletes a line.
Administrators read it at `GET /audit`, or export it at `GET /audit.csv`.

**Honest limitation, state it in your plan:** whoever administers the server
machine can reach the database file directly. That is true of every log ever
kept on a system, and is why an auditor asks who administers the machine
rather than trusting the software. If your plan requires a log the local
administrator cannot alter, ship these entries to your SIEM or a
write-once store and cite that as the control.

## 7. How this maps to NIST SP 800-171 Rev. 3

Our contribution to each control, and what remains yours. This is a starting
point for your System Security Plan, not a substitute for it, and a qualified
assessor should review it against your environment.

| Family | What the software contributes | Yours |
|---|---|---|
| **3.1 Access Control** | Per-person accounts, three roles, least privilege by role, sessions that expire on idle, revocable keys, first-account claim that closes | Who gets an account; role assignment; network access to the server; remote access policy |
| **3.3 Audit and Accountability** | The audit log in §6: actions traceable to a person, time-stamped in UTC, exportable | Retention period; review cadence; protecting the log from the local administrator; time synchronisation |
| **3.4 Configuration Management** | Sealed enforced by Group Policy, not by user setting; signed releases; the version in use is visible on every seat | Baseline configuration; change control; approving updates before they are installed |
| **3.5 Identification and Authentication** | Argon2id password hashing with per-password salts; hashed session tokens; separate program keys | Password policy; multi-factor at the OS or network layer — the software does not provide MFA |
| **3.8 Media Protection** | Drawings stay on your server and in per-user caches; no cloud copy exists | Disk encryption; media handling; disposal; clearing the local cache on decommission |
| **3.13 System and Communications Protection** | HTTPS to your own server; in Sealed, no outbound connection is made at all; no telemetry ever | TLS certificates on your server; network segmentation; boundary protection |
| **3.14 System and Information Integrity** | Every release verified against a publisher key built into the program before installing; a license verified offline | Patch management; malware protection; monitoring |

**Not provided, and you should plan for it elsewhere:** multi-factor
authentication, FIPS 140-validated cryptography, encryption at rest for the
server's database, and automated log forwarding. If a contract requires any of
these, tell us — it helps us decide what to build next.

## 8. Cryptography in use

| Where | What |
|---|---|
| Release signatures | Ed25519, verified against a key compiled into the program |
| License signatures | Ed25519, domain-separated payload, verified offline on your server |
| Passwords | Argon2id, per-password salt |
| Session tokens | Random, stored as SHA-256 digests |
| File identity | SHA-256 |
| Transport to your server | TLS via rustls, trusting both the built-in certificate authority list and Windows' own |

None of this is FIPS 140-validated. If your contract requires validated
modules, say so before you buy.

## 9. Questions an assessor usually asks

**Does any drawing data leave the network?** No. There is no cloud service and
no account with us.

**Can a user turn Sealed off?** No. It is set by a Group Policy machine policy
under `HKLM\Software\Policies`, or by the license. Neither is reachable from
inside the program.

**What happens if the license expires on a sealed server?** Nothing is deleted
or locked away. Everyone can still open and export every drawing and markup;
sharing new work through the server pauses. A sealed machine stays sealed
regardless.

**Does the software call home to check the license?** No. The license is a
signed file; your server verifies the signature offline, every time, against
keys built into it.

**How do we add seats, or renew?** By hand, and that is the trade. An ordinary
Office server fetches its own renewal: buy three more seats and the new license
is on the server within the hour, because the server asks for it. A sealed
server never asks anything of anybody, so nothing arrives by itself. You buy
the seats, we sign a license at the new count, and it reaches you as a file —
by email, on a stick, however your shop takes files in. An administrator drops
it on the server the same way the first one was installed.

This is not an oversight and it will not be fixed, because fixing it would mean
a sealed server reaching out, which is the one thing the edition exists to
refuse. Plan for it: if a renewal matters on a date, ask for the file before
that date rather than on it. Nothing stops working while you wait — see the
expiry answer above.

**Where is the source of these claims?** Sealed is `crates/hub/src/sealed.rs`;
every outbound call passes `web::outbound`; the audit log is
`crates/server/src/audit.rs`. We will walk your assessor through them.

---

*Excalibur Construction Technologies, Pueblo, Colorado. Questions:
hello@excaliburct.com.*
