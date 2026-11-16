<#
WHAT: LatCon's install day as it really is, on a throwaway GitHub Windows runner: the server has NEVER had Citadel
      Server 1.0, and Mark's jobs live only on Mark's PC.
       1. a fresh Windows server: the Office Server installed from its USB kit, the test licence (citadel listed) left in
          its data folder, and the pinned cell installed with --install-citadel-cell from LatCon's IT kit; with no 1.0
          anywhere, the Office Server makes a new key and certificate for the office's cell;
       2. Mark's PC (LatCon Estimating's own Office server page, LatCon's LiveFreshOfficeTests) links, is the admin at once,
          and moves its jobs up through the 47110 relay into the cell: every record, the keyring last, the Data folder
          renamed; Mark signs in with his same account and sees every job;
       3. a second PC asks, Mark approves it, and it signs in with an account from Mark's moved keyring and opens a job;
       4. --forget-citadel-key-copies on a fresh office, before the cell, before any computer, and after: its exact words,
          harmless every time.
      Every message the PCs' screens show is printed from messages.txt, for the install instructions.
WHY:  the other end-to-end run (office-e2e.ps1) starts from Citadel Server 1.0.1 on the server; LatCon never had it, so
      the path IT and Mark will really walk had never run on Windows.

Only on a GitHub runner: it installs a service and opens firewall rules on this machine, all thrown away with the runner.
No repository secret is used; every key here is made in the job and is a test key.

    pwsh -File deploy/windows-test/office-e2e-fresh.ps1 -View VIEW -Latcon LATCON -Server SERVER.exe -Kit KIT -Key KEY -Work WORK
#>
param(
    [Parameter(Mandatory = $true)][string] $View,       # WHAT: the View checkout. WHY: publish.py and the cell's pin.
    [Parameter(Mandatory = $true)][string] $Latcon,     # WHAT: the LatCon Estimating checkout, its app tests built. WHY: Mark's PC and the second PC.
    [Parameter(Mandatory = $true)][string] $Server,     # WHAT: the Office Server program, as on the USB kit. WHY: installed.
    [Parameter(Mandatory = $true)][string] $Kit,        # WHAT: citadel_cell.py kit's folder (it holds the pinned zip). WHY: LatCon's IT kit.
    [Parameter(Mandatory = $true)][string] $Key,        # WHAT: make-test-key.ps1's key file. WHY: signs the licence.
    [Parameter(Mandatory = $true)][string] $Work,       # WHAT: a folder for the stick, the PCs and the logs. WHY: the runner's temp.
    [string] $Python = 'python'                         # WHAT: the Python for publish.py. WHY: the runner's own.
)

$ErrorActionPreference = 'Stop'   # WHAT: any error stops the script. WHY: a test that carries on past a failure proves nothing.
Set-StrictMode -Version Latest     # WHAT: strict variables. WHY: a typo fails rather than reading as empty.

if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This only runs on a GitHub runner: it changes the machine it runs on.' }   # WHAT: refused elsewhere. WHY: it installs a service and changes the firewall.

$OfficeService = 'ExcaliburHyperviewServer'                                   # WHAT: the Office Server's service. WHY: winsvc.rs NAME.
$OfficeHome = Join-Path $env:ProgramData 'Excalibur Hyperview\Server'         # WHAT: where it installs. WHY: winsvc.rs home().
$OfficeExe = Join-Path $OfficeHome 'Hyperview-Server.exe'                     # WHAT: the installed program. WHY: the administrator commands run it.
$OfficeData = Join-Path $OfficeHome 'hyperview-data'                          # WHAT: its data. WHY: the licence is left there; its log is there.
$OfficeLog = Join-Path $OfficeData 'hyperview-server.log'                     # WHAT: the service's log. WHY: alarms are looked for in it.
$OfficeTls = Join-Path $OfficeHome 'citadel-tls'                              # WHAT: the cell's key's name and certificate. WHY: cellkey.rs FOLDER.
$CellExe = Join-Path $OfficeHome 'citadel\Citadel Server.exe'                 # WHAT: the cell's program. WHY: installed from the IT kit.
$CellData = Join-Path $env:ProgramData 'Excalibur Office\citadel'             # WHAT: the cell's data. WHY: its logs are kept.
$Port = 47110                                                                 # WHAT: Citadel's port. WHY: the PCs' store.
$OfficePort = 8714                                                            # WHAT: the Office Server's port. WHY: config DEFAULT_PORT.
$Logs = Join-Path $Work 'logs'                                                # WHAT: what the job keeps. WHY: uploaded, pass or fail.
$Pcs = Join-Path $Work 'pcs'                                                  # WHAT: the two PCs' folders and messages.txt. WHY: LatCon's test writes there.
New-Item -ItemType Directory -Force -Path $Work, $Logs, $Pcs | Out-Null       # WHAT: the folders. WHY: made before anything writes.
$CellZip = (Get-ChildItem -LiteralPath $Kit -Filter '*.zip' | Select-Object -First 1).FullName   # WHAT: the pinned cell's zip in the IT kit. WHY: --install-citadel-cell takes it.
$PinnedCell = ((Get-Content -Raw (Join-Path $View 'packaging/citadel-cell.json') | ConvertFrom-Json).files.'Citadel Server.exe')   # WHAT: the program's pinned SHA-256. WHY: the installed cell must be it.

function Step([string] $text) { Write-Host ""; Write-Host "=== $text" }   # WHAT: a heading in the log. WHY: easy to find a step.

function Check([bool] $ok, [string] $what) {
    if (-not $ok) { throw "FAILED: $what" }   # WHAT: stop. WHY: nothing after a failed check means anything.
    Write-Host "ok   $what"                   # WHAT: said. WHY: the log reads as the list of what was proved.
}

function Sha([string] $path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $path -ErrorAction Stop).Hash.ToLowerInvariant() }   # WHAT: a file's SHA-256. WHY: compared with the pin.

function WaitFor([scriptblock] $condition, [int] $seconds, [string] $what) {
    $until = (Get-Date).AddSeconds($seconds)                                    # WHAT: the limit. WHY: never forever.
    while ((Get-Date) -lt $until) {
        if (& $condition) { return }                                            # WHAT: done. WHY: on.
        Start-Sleep -Seconds 2                                                  # WHAT: a pause. WHY: asked again.
    }
    throw "FAILED: waited $seconds s for $what"                                 # WHAT: stop. WHY: said plainly.
}

function ServiceState([string] $name) {
    $query = (& "$env:SystemRoot\System32\sc.exe" query $name) -join "`n"       # WHAT: Windows' own answer. WHY: as IT reads it.
    if ($query -match 'STATE\s*:\s*\d+\s+(\w+)') { return $Matches[1] }         # WHAT: the state word. WHY: RUNNING, STOPPED...
    return 'MISSING'                                                            # WHAT: no such service. WHY: said plainly.
}

function ListenerPath([int] $port) {
    $listen = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1   # WHAT: the listener. WHY: who holds the port.
    if (-not $listen) { return $null }                                          # WHAT: nobody. WHY: said as null.
    $owner = Get-CimInstance Win32_Process -Filter "ProcessId = $($listen.OwningProcess)" | Select-Object -First 1   # WHAT: its process. WHY: its program is compared.
    if (-not $owner) { return $null }                                           # WHAT: gone in the moment between. WHY: asked again by the caller's wait.
    return $owner.ExecutablePath                                                # WHAT: its program. WHY: compared with a path.
}

function Pin([int] $port) {
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', $port)             # WHAT: a connection. WHY: the certificate comes in the handshake.
    try {
        $tls = [System.Net.Security.SslStream]::new($tcp.GetStream(), $false, [System.Net.Security.RemoteCertificateValidationCallback] { $true })   # WHAT: TLS, anything shown taken. WHY: only read; the pin is the check.
        try {
            $tls.AuthenticateAsClient('citadel-server')                         # WHAT: the handshake. WHY: brings the certificate.
            $cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($tls.RemoteCertificate)   # WHAT: the certificate. WHY: its public key.
            return [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($cert.PublicKey.ExportSubjectPublicKeyInfo())).ToLowerInvariant()   # WHAT: the pin. WHY: as the PCs pin it.
        } finally { $tls.Dispose() }                                            # WHAT: closed. WHY: one connection.
    } finally { $tcp.Dispose() }                                                # WHAT: closed. WHY: as above.
}

function OfficeVersion {
    try { return (Invoke-RestMethod -Uri "http://127.0.0.1:$OfficePort/api/v1/health" -TimeoutSec 5).version }   # WHAT: asked. WHY: an open door.
    catch { return $null }                                                      # WHAT: not answering yet. WHY: waited for by the caller.
}

function OfficeLogText {
    try {
        $stream = [System.IO.File]::Open($OfficeLog, 'Open', 'Read', 'ReadWrite, Delete')   # WHAT: opened beside the writer. WHY: the service holds it open.
        try { return [System.IO.StreamReader]::new($stream).ReadToEnd() } finally { $stream.Dispose() }   # WHAT: all of it. WHY: searched.
    } catch [System.IO.FileNotFoundException], [System.IO.DirectoryNotFoundException] { return '' }   # WHAT: none yet. WHY: nothing to read.
}

function CellUp {
    if ((ListenerPath $Port) -ine $OfficeExe) { return $false }                 # WHAT: the port is the Office Server's. WHY: it ends TLS for the cell.
    try { return (Invoke-WebRequest -Uri "https://127.0.0.1:$Port/v1/hello" -SkipCertificateCheck -TimeoutSec 5 -UseBasicParsing).StatusCode -eq 200 }   # WHAT: relayed and answered. WHY: the cell behind it.
    catch { return $false }                                                     # WHAT: not yet. WHY: waited for by the caller.
}

function NoAlarmSince([int] $from) {
    $text = OfficeLogText                                                       # WHAT: the log now. WHY: searched from $from.
    $new = $text.Substring([Math]::Min($from, $text.Length))                    # WHAT: what was written since. WHY: only new lines count.
    if ($new.Contains('ALARM')) { throw "FAILED: the Office Server raised an alarm:`n$new" }   # WHAT: stop, with the log. WHY: said plainly.
}

# WHAT: --forget-citadel-key-copies, as IT would run it; its exact words and exit code shown and returned. WHY: step 4.
function Forget([string] $when) {
    $said = (& $OfficeExe --forget-citadel-key-copies 2>&1 | Out-String).Trim()   # WHAT: run, as an administrator. WHY: as IT runs it.
    $code = $LASTEXITCODE                                                       # WHAT: its exit code. WHY: shown.
    Write-Host "--forget-citadel-key-copies $when (exit $code) said: $said"     # WHAT: the exact words. WHY: the instructions quote them.
    return [pscustomobject]@{ Said = $said; Code = $code }                      # WHAT: both. WHY: checked by the caller.
}

# WHAT: what the kept key folder holds, by name and SHA-256. WHY: --forget-citadel-key-copies must never touch it.
function TlsFolder { return ((Get-ChildItem -LiteralPath $OfficeTls -File | Sort-Object Name | ForEach-Object { "$($_.Name)=$(Sha $_.FullName)" }) -join ';') }

try {
    Step '1. A fresh Windows server: the Office Server from its USB kit, the licence, the cell from LatCon''s IT kit; no Citadel Server 1.0 anywhere'
    Check ((ServiceState 'CitadelServer') -eq 'MISSING') 'there is no Citadel Server 1.0 service on this server'   # WHAT: none. WHY: LatCon never had it.
    Check (-not (Test-Path -LiteralPath (Join-Path $env:ProgramData 'Citadel Server'))) 'there is no Citadel Server 1.0 data folder'   # WHAT: none. WHY: as above.
    Check (-not (Test-Path -LiteralPath (Join-Path $env:ProgramFiles 'Citadel Server'))) 'there is no Citadel Server 1.0 program folder'   # WHAT: none. WHY: as above.
    $licence = Join-Path $Work 'LatCon.evlicense'                               # WHAT: the test licence. WHY: dropped into hyperview-data.
    & $Python (Join-Path $View 'tools/publish.py') sign-license --key $Key --company 'LatCon' --founding --modules citadel --out $licence   # WHAT: signed with the job's test key. WHY: a real one is signed the same way.
    Check ($LASTEXITCODE -eq 0) 'the test licence (citadel listed) is signed'   # WHAT: made. WHY: the cell runs only with citadel on it.
    New-Item -ItemType Directory -Force -Path $OfficeData | Out-Null            # WHAT: the data folder. WHY: the licence goes in before the first start.
    Copy-Item -LiteralPath $licence -Destination $OfficeData -Force             # WHAT: the .evlicense dropped in hyperview-data. WHY: as IT does.
    $stick = Join-Path $Work 'usb-office'                                       # WHAT: the USB kit. WHY: the program alone.
    New-Item -ItemType Directory -Force -Path $stick | Out-Null                 # WHAT: made. WHY: below.
    Copy-Item -LiteralPath $Server -Destination (Join-Path $stick 'ExcaliburView-Server.exe')   # WHAT: the program on it. WHY: as the kit is laid out.
    & (Join-Path $stick 'ExcaliburView-Server.exe') --install                   # WHAT: installed, as the double-click runs it. WHY: the runner is an administrator.
    $code = $LASTEXITCODE                                                       # WHAT: its exit code. WHY: checked.
    $report = [System.IO.File]::ReadAllText((Join-Path $OfficeHome 'last-install.txt'))   # WHAT: the install's report. WHY: what IT reads.
    Write-Host "install exited $code and said: $report"                        # WHAT: shown. WHY: the log has it.
    Check ($code -eq 0 -and "$report".StartsWith('ok')) 'the Office Server installed as a service from the USB kit'   # WHAT: installed. WHY: step 1.
    WaitFor { (ServiceState $OfficeService) -eq 'RUNNING' -and (OfficeVersion) } 120 'the Office Server answering'   # WHAT: up. WHY: below.
    Write-Host "the Office Server says it is $(OfficeVersion)"                 # WHAT: its version. WHY: the log names the build.
    $before = Forget 'right after the Office Server''s install, before the cell'   # WHAT: asked before there is any cell. WHY: step 4.
    Check (-not (Test-Path -LiteralPath (Join-Path $OfficeHome 'citadel'))) 'the USB kit installed no cell'   # WHAT: none yet. WHY: View's release never carries it.
    $from = (OfficeLogText).Length                                              # WHAT: where the log is now. WHY: only new alarms count.
    $said = & $OfficeExe --install-citadel-cell $CellZip 2>&1 | Out-String    # WHAT: the cell installed from the IT kit, as IT runs it. WHY: the only way in.
    Write-Host $said.TrimEnd()                                                  # WHAT: shown. WHY: the log has it.
    Check ($LASTEXITCODE -eq 0 -and $said.Contains('was checked against this Office Server')) 'the pinned cell installed with --install-citadel-cell'   # WHAT: done. WHY: step 1.
    WaitFor { Test-Path -LiteralPath $CellExe } 60 'the cell put in place at the restart'   # WHAT: swapped in. WHY: the service's start puts it in place.
    Check ((Sha $CellExe) -eq $PinnedCell) "the installed cell is the pinned one ($PinnedCell)"   # WHAT: the pinned program. WHY: only that zip.
    WaitFor { NoAlarmSince $from; CellUp } 300 'the cell answering on 47110'   # WHAT: up, with a new key. WHY: nothing to move.
    $pin = Pin $Port                                                            # WHAT: the new office's pin. WHY: the PCs pin it.
    Write-Host "the new office's cell certificate pin: $pin"                   # WHAT: shown. WHY: the log has it.
    Check (-not (Test-Path -LiteralPath (Join-Path $OfficeTls 'tls.pfx'))) 'no tls.pfx: the new key was made in the machine''s key store, never in a file'   # WHAT: none. WHY: the key never leaves Windows.
    $tls = TlsFolder                                                            # WHAT: the kept key's files. WHY: never touched by the command below.
    $served = Forget 'once the cell is installed, before any computer'          # WHAT: asked before any computer used the cell. WHY: step 4.

    Step '2 and 3. Mark''s PC links, is the admin, moves its jobs up; a second PC asks, Mark approves it, it signs in (LatCon Estimating''s own screens)'
    $env:LATCON_LIVE_FRESH = $Pcs                                               # WHAT: the PCs' folder. WHY: the test does nothing without it.
    try {
        & dotnet test (Join-Path $Latcon 'tests/Citadel.App.Tests/Citadel.App.Tests.csproj') -c Release --no-build -nologo `
            --filter 'FullyQualifiedName~Citadel.App.Tests.LiveFreshOfficeTests' --logger 'console;verbosity=normal'   # WHAT: the live test only. WHY: built before.
        $passed = $LASTEXITCODE -eq 0                                           # WHAT: whether it passed. WHY: checked after the messages are shown.
    } finally { Remove-Item Env:LATCON_LIVE_FRESH -ErrorAction SilentlyContinue }   # WHAT: cleared. WHY: no later run picks it up.
    $messages = Join-Path $Pcs 'messages.txt'                                   # WHAT: what the screens said. WHY: shown whole.
    Write-Host '--- what the PCs'' screens said, in order ---'                  # WHAT: a heading. WHY: the instructions quote these.
    if (Test-Path -LiteralPath $messages) { Get-Content -LiteralPath $messages | ForEach-Object { Write-Host $_ }; Copy-Item -LiteralPath $messages -Destination $Logs }   # WHAT: shown and kept. WHY: as above.
    Check $passed 'LatCon''s fresh-office test passed'                          # WHAT: passed. WHY: steps 2 and 3.
    Check (Test-Path -LiteralPath (Join-Path $Pcs 'fresh.done')) 'LatCon''s fresh-office test really ran'   # WHAT: its done file. WHY: a skipped test can't pass.
    Check ((OfficeLogText).Contains('answered its first computer with the kept key')) 'the cell answered the PCs with the new office''s key'   # WHAT: served. WHY: the relay and the kept key.
    NoAlarmSince 0                                                              # WHAT: no alarm in the whole run. WHY: a quiet install day.

    Step '4. --forget-citadel-key-copies on a fresh office'
    $after = Forget 'after the computers used the cell'                         # WHAT: asked once the cell has served. WHY: what IT would see.
    $again = Forget 'a second time'                                             # WHAT: asked again. WHY: harmless however often.
    Check ($before.Code -ne 0 -and $before.Said.Contains("hasn't answered a computer")) 'before the cell, it refuses in plain words (exit 1)'   # WHAT: refused. WHY: nothing has used the key yet
    Check ($after.Code -eq 0 -and $again.Code -eq 0 -and $after.Said.Contains('no copies') -and $again.Said.Contains('no copies')) 'after the computers used the cell, it says there are no copies (exit 0), however often it is run'   # WHAT: harmless. WHY: a new office's key was never in a file
    Check ((TlsFolder) -eq $tls) 'the kept key''s name and certificate are untouched'   # WHAT: the same files, byte for byte. WHY: the command only ever removes copies.
    Restart-Service -Name $OfficeService -Force                                 # WHAT: restarted. WHY: the cell starts again with its kept key.
    WaitFor { CellUp } 180 'the cell answering again after a restart'           # WHAT: up. WHY: below.
    Check ((Pin $Port) -eq $pin) 'the cell shows the same certificate after the command and a restart: harmless'   # WHAT: unchanged. WHY: the PCs pinned it.

    Step 'Passed'
    Write-Host 'A new office end to end on Windows: the Office Server and the cell installed with no Citadel Server 1.0, a new key made, Mark''s PC linked as the admin and its jobs moved up through the relay (keyring last, Data folder renamed), Mark signing in on both PCs, a second PC approved and signing in with an account from the moved keyring.'
    Write-Host "--forget-citadel-key-copies, 1, before the cell: $($before.Said)"
    Write-Host "--forget-citadel-key-copies, 2, before any computer: $($served.Said)"
    Write-Host "--forget-citadel-key-copies, 3, after: $($after.Said)"
    Write-Host "--forget-citadel-key-copies, 4, again: $($again.Said)"
} catch {
    Write-Host '--- the end of the Office Server''s log ---'                    # WHAT: a heading. WHY: the failure's cause is usually there.
    (OfficeLogText) -split "`n" | Select-Object -Last 80 | ForEach-Object { Write-Host $_ }   # WHAT: its last lines. WHY: read on the run page.
    & "$env:SystemRoot\System32\robocopy.exe" (Join-Path $CellData 'logs') (Join-Path $Logs 'cell-now') /E /B /R:0 /W:0 /NP /NFL /NDL | Out-Null   # WHAT: the cell's logs, in backup mode. WHY: its folder is SYSTEM's and the cell's.
    $cellLog = Get-ChildItem -LiteralPath (Join-Path $Logs 'cell-now') -File -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1   # WHAT: the newest. WHY: why it failed.
    if ($cellLog) { Write-Host "--- the end of the cell's log ($($cellLog.Name)) ---"; Get-Content -LiteralPath $cellLog.FullName -Tail 40 | ForEach-Object { Write-Host $_ } }   # WHAT: its last lines. WHY: as above.
    throw                                                                       # WHAT: the failure, as it was. WHY: the step fails.
} finally {
    foreach ($from in @($OfficeLog, "$OfficeLog.1", (Join-Path $OfficeHome 'last-install.txt'))) {
        if (Test-Path -LiteralPath $from) { Copy-Item -LiteralPath $from -Destination $Logs -ErrorAction SilentlyContinue }   # WHAT: the Office Server's log. WHY: pass or fail.
    }
    & "$env:SystemRoot\System32\robocopy.exe" (Join-Path $CellData 'logs') (Join-Path $Logs 'cell') /E /B /R:0 /W:0 /NP /NFL /NDL | Out-Null   # WHAT: the cell's own logs, in backup mode. WHY: as above.
    $global:LASTEXITCODE = 0                                                    # WHAT: robocopy's codes forgotten. WHY: they are counts, not failures.
}
