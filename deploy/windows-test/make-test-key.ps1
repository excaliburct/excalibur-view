<#
WHAT: makes a throwaway Ed25519 release key for the Office Server's Windows end-to-end test on GitHub, and makes it the only
      key in this checkout's KEYS list (crates/hyperview/src/trust.rs), so the servers built next take updates and licences
      signed by that test key and nothing else.
WHY:  the test updates the Office Server, rolls it back and gives it a licence, all of which must be signed by a key built
      into the program. The real keys never come near a test: this checkout only is changed, never committed, and the
      programs built from it are thrown away with the runner. Removing the real key from the list also means a real release
      on the public feed can never be taken by a test build.

    pwsh -File deploy/windows-test/make-test-key.ps1 -View . -Keys "$env:RUNNER_TEMP\test-keys"
#>
param(
    [Parameter(Mandatory = $true)][string] $View,   # WHAT: the View checkout to change. WHY: only its working copy, never a commit.
    [Parameter(Mandatory = $true)][string] $Keys,   # WHAT: where the test key goes. WHY: the runner's temp folder, gone with the runner.
    [string] $Python = 'python'                     # WHAT: the Python to run publish.py with. WHY: the runner's own.
)

$ErrorActionPreference = 'Stop'   # WHAT: any error stops the script. WHY: a half-made key must never be used.
Set-StrictMode -Version Latest     # WHAT: strict variables. WHY: a typo fails rather than reading as empty.

# WHAT: refuses to run anywhere but a GitHub runner. WHY: it replaces the release keys in the checkout, which on a real
# checkout would leave a build that trusts a test key.
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This only runs on a GitHub runner: it replaces the release keys in the checkout.' }

New-Item -ItemType Directory -Force -Path $Keys | Out-Null                       # WHAT: the key's folder. WHY: made fresh in the runner's temp.
$key = Join-Path $Keys 'test-release.key'                                         # WHAT: the key file. WHY: publish.py signs with it.
$said = & $Python (Join-Path $View 'tools/publish.py') make-key --name test-only-office-e2e --out $key   # WHAT: a new key, as a real one is made. WHY: publish.py's own make-key.
if ($LASTEXITCODE -ne 0) { throw "publish.py couldn't make the test key." }       # WHAT: stop on failure. WHY: no key, no test.
$line = ($said | Select-String -Pattern '\("test-only-office-e2e", "[0-9a-f]{64}"\)').Matches.Value   # WHAT: the line it prints for trust.rs. WHY: the public half, nothing else.
if (-not $line) { throw 'publish.py did not print the test key''s public half.' }  # WHAT: stop. WHY: nothing to build in.

$trust = Join-Path $View 'crates/hyperview/src/trust.rs'                          # WHAT: the keys the programs are built with. WHY: KEYS decides what a release and a licence may be signed by.
$text = [System.IO.File]::ReadAllText($trust)                                     # WHAT: the file. WHY: changed below.
$pattern = '(?s)pub const KEYS: &\[\(&str, &str\)\] = &\[.*?\n\];'               # WHAT: the whole KEYS list. WHY: replaced, not added to.
if (-not [regex]::IsMatch($text, $pattern)) { throw "$trust has no KEYS list where it was expected." }   # WHAT: stop. WHY: never a build with the real keys still in.
$replacement = "pub const KEYS: &[(&str, &str)] = &[`n    // A TEST key, made on a GitHub runner by deploy/windows-test/make-test-key.ps1. Never committed, never shipped.`n    $line,`n];"   # WHAT: the test key alone. WHY: as above.
$text = [regex]::Replace($text, $pattern, $replacement.Replace('$', '$$'))        # WHAT: the list replaced. WHY: the servers built next trust only the test key.
[System.IO.File]::WriteAllText($trust, $text)                                     # WHAT: written. WHY: built next.
Write-Host "Test release key built into this checkout: $line"                    # WHAT: the public half. WHY: public, and the log shows which key the job trusted.
