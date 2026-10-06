# Setting up the Mac build, once

Ten minutes, once, and then your Mac is out of releases for good.

After this, putting a version out is: press **Run workflow** on GitHub, wait
while it builds and Apple notarizes it, then double-click **Release Excalibur
View** on the PC. That is the whole job.

## What you are handing to GitHub, and what you are not

You are handing over **Apple's** two credentials. They say *this program came
from Excalibur Construction Technologies* to macOS. If one ever leaked, Apple
revokes it and issues another, and the damage is a few bad days.

You are **not** handing over `hyperview-signing.key`. That one says *this
update is safe to install* to every copy of Excalibur View in the world, and
every seat installs what it vouches for by itself. It cannot be revoked and it
cannot be taken back. It stays on the Windows PC and it is used there.

They go on **`excaliburct/excalibur-view`**, which is the public repository.

That used to say the opposite, in bold, and it is worth saying why it changed
rather than quietly editing it. The old advice assumed a private repository was
free. It is not: Actions minutes on a private repository are metered, macOS
runners bill at ten times the rate, and on 23 September the spending limit ran
out in the middle of a release and stopped every build. Public repositories get
Actions free, macOS included.

The security did not change, because it was never the repository being public
that mattered. A secret is not part of the code and is not in the clone.
GitHub never hands a secret to a workflow started by somebody else's fork, and
this workflow refuses to run on a pull request at all -- the `on:` block has no
`pull_request` in it, deliberately, and the comment underneath says so. What
can read these is a workflow run on this repository's own branches, which is
you.

What is still true is everything below about what these are. They are Apple's
credentials. If one leaked, Apple revokes it and issues another and you lose a
few days. They are not `hyperview-signing.key`, which cannot be revoked, never
goes near GitHub, and stays on the Windows PC.

---

## On your Mac

### 1. Export Apple's certificate

1. Open **Keychain Access** (Spotlight, type "Keychain").
2. On the left: **login**, then **My Certificates**.
3. Find **Developer ID Application: Your Name (TEAMID)**.
4. Right-click it → **Export "Developer ID Application: …"**
5. Save it to your Desktop as `excalibur-cert.p12`. File format: **Personal
   Information Exchange (.p12)**.
6. It asks for a password to protect the file. Make one up, and **write it
   down** — you need it in a minute and never again.
7. It then asks for your Mac login password. That is macOS letting the key out
   of the keychain, which is what you want.

### 2. Turn both files into text

GitHub takes text, not files. In Terminal:

```
base64 -i ~/Desktop/excalibur-cert.p12 | tr -d '\n' | pbcopy
```

That put a long line of gibberish on your clipboard. Go and paste it into
GitHub now (step 3 below), then come back for the second one:

```
base64 -i ~/private/AuthKey_*.p8 | tr -d '\n' | pbcopy
```

### 3. Paste them into GitHub

Go to:

**github.com/excaliburct/excalibur-view** → **Settings** → **Secrets and variables**
→ **Actions** → **New repository secret**

Add these five, one at a time. The name has to match exactly.

| Name | What to put in it |
|---|---|
| `MACOS_CERT_P12` | the first long line you copied |
| `MACOS_CERT_PASSWORD` | the password you made up in step 1.6 |
| `MACOS_SIGN_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_API_KEY_P8` | the second long line you copied |
| `APPLE_API_KEY_ID` | the eight characters from the filename — `AuthKey_XXXXXXXX.p8` → `XXXXXXXX` |
| `APPLE_API_ISSUER` | the Issuer ID from App Store Connect → Users and Access → Integrations → Keys. It looks like `69a6de70-…-1f2b3c4d5e6f`. |

### 4. Delete the `.p12` from your Desktop

```
rm ~/Desktop/excalibur-cert.p12
```

It has done its job. The certificate is still in your keychain; this was only a
copy for the trip.

---

## Then, to make a release

1. **github.com/excaliburct/excalibur-view** → **Actions** → **Mac build** → **Run
   workflow**.
   About twenty minutes, most of it Apple. You do not have to watch it.
2. On the PC, double-click **Release Excalibur View**.
   It fetches what GitHub built, runs the tests, builds the Windows programs,
   signs everything with your key and publishes.
3. Try it on one seat.
4. Double-click **Give it to everyone**.

## If the Mac build fails

Open the failed run on GitHub and read the step that is red.

- **"0 valid identities found"** — the certificate did not import. Usually
  `MACOS_CERT_PASSWORD` does not match the password from step 1.6, or the
  `.p12` was exported without its private key (export from **My
  Certificates**, not **Certificates**).
- **notarytool says "Invalid credentials"** — one of the three Apple values is
  wrong. The Key ID is the eight characters in the filename; the Issuer is a
  long UUID and is not the same thing.
- **"No universal macOS build of PDFium … was found"** — the pinned PDFium
  version has no Mac build published. `third_party/pdfium/embedded/VERSION`
  says which one; pick a nearby build that has one.

Nothing in that workflow can publish a release, so a failure there costs you
twenty minutes and nothing else.
