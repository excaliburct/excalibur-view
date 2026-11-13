<#
WHAT: the Office Server's install day at LatCon, end to end, on a throwaway GitHub Windows runner:
       a. Citadel Server 1.0.1 installed as the November 6 USB stick had it (built from its commit), and one LatCon PC paired
          with it, which moves its jobs up, so the server has data and a certificate the PCs have pinned;
       b. the Office Server (built with the pinned Citadel cell) installed as a service from the USB kit that
          tools/citadel_cell.py kit makes, with a TEST licence listing the citadel part, signed with a key made in the job;
       c. at its first start: 1.0 stopped and disabled, its data moved into the cell, the certificate's SHA-256 unchanged,
          1.0's service, its 03:30 task and its firewall rule removed, the cell's 47110 rule there, and the cell running in
          its job object; the PC paired with 1.0 opens and saves its job on the cell, through the Office Server's list;
       d. a first computer claims the office as its admin, approves a second, the second reaches a project, and the cell
          answers a device check for the second (office-device, LatCon Estimating's own client code);
       e. a failed move on a fresh install: 1.0 started again and the cell held; then the retry moves it;
       f. an update to the next version through a stand-in feed (the cell with it), --roll-back putting the server and
          its cell back, the version rolled back from not taken again, and a newer one taken.
WHY:  none of this can run on the Linux build server (Windows services, the firewall, Task Scheduler, job objects, a
      restricted token, DPAPI). The Office Server's own tests use fakes for every Windows step.

Only on a GitHub runner: it installs services, opens firewall rules and runs a feed on this machine, all thrown away with
the runner. No repository secret is used; every key here is made in the job and is a test key.

    pwsh -File deploy/windows-test/office-e2e.ps1 -View VIEW -Latcon LATCON -Old OLD.exe -Servers SERVERS -Kit KIT
         -CellPart PART -Key KEY -Device DEVICE.exe -Work WORK
#>
param(
    [Parameter(Mandatory = $true)][string] $View,       # WHAT: the View checkout. WHY: publish.py, test-feed.py.
    [Parameter(Mandatory = $true)][string] $Latcon,     # WHAT: the LatCon Estimating checkout, its client tests built. WHY: the PC paired with 1.0.
    [Parameter(Mandatory = $true)][string] $Old,        # WHAT: the USB's Citadel Server 1.0.1 "Citadel Server.exe". WHY: what the office server has.
    [Parameter(Mandatory = $true)][string] $Servers,    # WHAT: a folder of ExcaliburView-Server-<version>.exe. WHY: the install and the two updates.
    [Parameter(Mandatory = $true)][string] $Kit,        # WHAT: citadel_cell.py kit's folder (it holds citadel\). WHY: the USB kit's cell.
    [Parameter(Mandatory = $true)][string] $CellPart,   # WHAT: citadel_cell.py bundle's part. WHY: signed into each update.
    [Parameter(Mandatory = $true)][string] $Key,        # WHAT: make-test-key.ps1's key file. WHY: signs the licence and the updates.
    [Parameter(Mandatory = $true)][string] $Device,     # WHAT: office-device's program. WHY: the two computers of part d.
    [Parameter(Mandatory = $true)][string] $Work,       # WHAT: a folder for sticks, the feed and the logs. WHY: the runner's temp.
    [string] $Python = 'python',                        # WHAT: the Python for publish.py and the feed. WHY: the runner's own.
    [string] $First = '0.7.0',                          # WHAT: the version installed. WHY: the servers folder names it.
    [string] $Second = '0.7.1',                         # WHAT: the first update. WHY: rolled back from.
    [string] $Third = '0.7.2'                           # WHAT: the next update. WHY: taken after the rollback.
)

$ErrorActionPreference = 'Stop'   # WHAT: any error stops the script. WHY: a test that carries on past a failure proves nothing.
Set-StrictMode -Version Latest     # WHAT: strict variables. WHY: a typo fails rather than reading as empty.

# WHAT: refuses to run anywhere but a GitHub runner. WHY: it installs services and changes the firewall.
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This only runs on a GitHub runner: it changes the machine it runs on.' }

$CitadelService = 'CitadelServer'                                             # WHAT: 1.0's service. WHY: WindowsSetup.ServiceName.
$CitadelTask = 'Citadel Server updates'                                       # WHAT: 1.0's 03:30 task. WHY: cell.rs OLD_UPDATE_TASK.
$CitadelRule = 'Citadel Server'                                               # WHAT: 1.0's firewall rule. WHY: cell.rs OLD_FIREWALL_RULE.
$CitadelFolder = Join-Path $env:ProgramFiles 'Citadel Server'                 # WHAT: 1.0's program folder. WHY: WindowsSetup.ProgramFolder.
$CitadelExe = Join-Path $CitadelFolder 'Citadel Server.exe'                   # WHAT: the copy IT runs. WHY: open-pairing and approve for the PC.
$CitadelData = Join-Path $env:ProgramData 'Citadel Server'                    # WHAT: 1.0's data. WHY: moved into the cell.
$OfficeService = 'ExcaliburHyperviewServer'                                   # WHAT: the Office Server's service. WHY: winsvc.rs NAME.
$OfficeHome = Join-Path $env:ProgramData 'Excalibur Hyperview\Server'         # WHAT: where it installs. WHY: winsvc.rs home().
$OfficeExe = Join-Path $OfficeHome 'Hyperview-Server.exe'                     # WHAT: the installed program. WHY: --roll-back and --retry-citadel-move run it.
$OfficeData = Join-Path $OfficeHome 'hyperview-data'                          # WHAT: its data. WHY: the licence is left there; its log is there.
$OfficeLog = Join-Path $OfficeData 'hyperview-server.log'                     # WHAT: the service's log. WHY: the move's and the updates' lines.
$OfficeKept = Join-Path $OfficeHome 'Hyperview-Server.old'                    # WHAT: the version before an update. WHY: kept.rs kept_program, put back by --roll-back.
$CellExe = Join-Path $OfficeHome 'citadel\Citadel Server.exe'                 # WHAT: the cell's program. WHY: what runs in the job.
$CellData = Join-Path $env:ProgramData 'Excalibur Office\citadel'             # WHAT: the cell's data. WHY: where 1.0's data goes.
$CellRule = 'Excalibur Office Server (Citadel cell)'                          # WHAT: the 47110 rule. WHY: cell.rs FIREWALL_RULE.
$Port = 47110                                                                 # WHAT: Citadel's port. WHY: 1.0's and the cell's.
$OfficePort = 8714                                                            # WHAT: the Office Server's port. WHY: config DEFAULT_PORT.
$FeedPort = 8899                                                              # WHAT: the stand-in feed's port. WHY: free on a runner.
$Logs = Join-Path $Work 'logs'                                                # WHAT: what the job keeps. WHY: uploaded, pass or fail.
$State = Join-Path $Work 'latcon-pc'                                          # WHAT: the LatCon PC's folder. WHY: kept from the pairing to after the move.
$Feed = Join-Path $Work 'feed'                                                # WHAT: the stand-in feed's releases. WHY: one folder each.
New-Item -ItemType Directory -Force -Path $Work, $Logs, $State, $Feed | Out-Null   # WHAT: the folders. WHY: made before anything writes.
$script:feedProcess = $null                                                   # WHAT: the feed's process. WHY: stopped at the end.

# WHAT: Windows' own answer to "is this process in a job?". WHY: the cell must run inside the Office Server's job object.
Add-Type -Namespace E2E -Name Native -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr OpenProcess(uint access, bool inherit, int id);
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool IsProcessInJob(IntPtr process, IntPtr job, out bool inJob);
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool CloseHandle(IntPtr handle);
'@

function Step([string] $text) { Write-Host ""; Write-Host "=== $text" }   # WHAT: a heading in the log. WHY: easy to find a step.

# WHAT: a check that stops the test with plain words. WHY: every claim the test makes is one of these.
function Check([bool] $ok, [string] $what) {
    if (-not $ok) { throw "FAILED: $what" }   # WHAT: stop. WHY: nothing after a failed check means anything.
    Write-Host "ok   $what"                   # WHAT: said. WHY: the log reads as the list of what was proved.
}

function Sha([string] $path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant() }   # WHAT: a file's SHA-256. WHY: compares files byte for byte.

# WHAT: waits for a condition, up to a limit. WHY: services and updates take a moment; never forever.
function WaitFor([scriptblock] $condition, [int] $seconds, [string] $what) {
    $until = (Get-Date).AddSeconds($seconds)                                    # WHAT: the limit. WHY: as given.
    while ((Get-Date) -lt $until) {
        if (& $condition) { return }                                            # WHAT: done. WHY: on.
        Start-Sleep -Seconds 2                                                  # WHAT: a pause. WHY: asked again.
    }
    throw "FAILED: waited $seconds s for $what"                                 # WHAT: stop. WHY: said plainly.
}

# WHAT: sc.exe's word for a service's state. WHY: "RUNNING" is what IT reads.
function ServiceState([string] $name) {
    $query = (& "$env:SystemRoot\System32\sc.exe" query $name) -join "`n"       # WHAT: Windows' own answer. WHY: not a cmdlet's idea of it.
    if ($query -match 'STATE\s*:\s*\d+\s+(\w+)') { return $Matches[1] }         # WHAT: the state word. WHY: RUNNING, STOPPED...
    return 'MISSING'                                                            # WHAT: no such service. WHY: said plainly.
}

# WHAT: sc.exe's start type for a service. WHY: 1.0 must start with Windows again after a failed move.
function StartType([string] $name) {
    $query = (& "$env:SystemRoot\System32\sc.exe" qc $name) -join "`n"          # WHAT: its configuration. WHY: as IT reads it.
    if ($query -match 'START_TYPE\s*:\s*\d+\s+(\w+)') { return $Matches[1] }    # WHAT: AUTO_START, DISABLED... WHY: compared.
    return 'MISSING'                                                            # WHAT: no such service. WHY: said plainly.
}

# WHAT: the program listening on a port, by its full path, or null. WHY: 1.0's program or the cell's must be the one on 47110.
function ListenerPath([int] $port) {
    $listen = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1   # WHAT: the listener. WHY: who holds the port.
    if (-not $listen) { return $null }                                          # WHAT: nobody. WHY: said as null.
    return (Get-CimInstance Win32_Process -Filter "ProcessId = $($listen.OwningProcess)").ExecutablePath   # WHAT: its program. WHY: compared with a path.
}

# WHAT: the processes running a program, by its full path. WHY: whether 1.0 or the cell is running.
function Running([string] $exe) { @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath -ieq $exe }) }

# WHAT: whether a process is inside a job object. WHY: the cell must die with the Office Server.
function InJob([int] $id) {
    $handle = [E2E.Native]::OpenProcess(0x1000, $false, $id)                    # WHAT: the process, to query. WHY: PROCESS_QUERY_LIMITED_INFORMATION.
    if ($handle -eq [IntPtr]::Zero) { throw "FAILED: couldn't open process $id" }   # WHAT: stop. WHY: no answer.
    try {
        $inJob = $false                                                         # WHAT: filled in. WHY: the answer.
        if (-not [E2E.Native]::IsProcessInJob($handle, [IntPtr]::Zero, [ref] $inJob)) { throw "FAILED: IsProcessInJob failed for $id" }   # WHAT: asked. WHY: said if it fails.
        return $inJob                                                           # WHAT: the answer. WHY: checked.
    } finally { [void][E2E.Native]::CloseHandle($handle) }                      # WHAT: let go. WHY: always.
}

# WHAT: whether a firewall rule of this name exists. WHY: netsh exits 0 only when it finds one.
function RuleThere([string] $name) {
    & "$env:SystemRoot\System32\netsh.exe" advfirewall firewall show rule "name=$name" | Out-Null   # WHAT: asked by name. WHY: exact.
    return $LASTEXITCODE -eq 0                                                  # WHAT: found or not. WHY: checked.
}

# WHAT: whether a scheduled task of this name exists. WHY: schtasks exits 0 only when it finds one.
function TaskThere([string] $name) {
    & "$env:SystemRoot\System32\schtasks.exe" /Query /TN $name 2>&1 | Out-Null  # WHAT: asked by name. WHY: exact.
    return $LASTEXITCODE -eq 0                                                  # WHAT: found or not. WHY: checked.
}

# WHAT: the certificate's pin on a port, the way a PC sees it. WHY: the PCs pinned it; it must never change.
function Pin([int] $port) {
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', $port)             # WHAT: a connection. WHY: the certificate comes in the handshake.
    try {
        $accept = [System.Net.Security.RemoteCertificateValidationCallback] { $true }   # WHAT: take whatever it shows. WHY: only read; the pin is the check.
        $tls = [System.Net.Security.SslStream]::new($tcp.GetStream(), $false, $accept)  # WHAT: TLS over it. WHY: as LatCon connects.
        try {
            $tls.AuthenticateAsClient('citadel-server')                         # WHAT: the handshake. WHY: brings the certificate.
            $cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($tls.RemoteCertificate)   # WHAT: the certificate. WHY: its public key.
            return [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($cert.PublicKey.ExportSubjectPublicKeyInfo())).ToLowerInvariant()   # WHAT: the pin. WHY: Pinning.Of's form.
        } finally { $tls.Dispose() }                                            # WHAT: closed. WHY: one connection.
    } finally { $tcp.Dispose() }                                                # WHAT: closed. WHY: as above.
}

# WHAT: the Office Server's version from /health, or null while it doesn't answer. WHY: which build is running.
function OfficeVersion {
    try { return (Invoke-RestMethod -Uri "http://127.0.0.1:$OfficePort/api/v1/health" -TimeoutSec 5).version }   # WHAT: asked. WHY: an open door.
    catch { return $null }                                                      # WHAT: not answering yet. WHY: waited for by the caller.
}

# WHAT: the service's log, read while it is being written. WHY: the move's and the updates' lines are checked in it.
function OfficeLogText {
    if (-not (Test-Path -LiteralPath $OfficeLog)) { return '' }                 # WHAT: none yet. WHY: nothing to read.
    $stream = [System.IO.File]::Open($OfficeLog, 'Open', 'Read', 'ReadWrite, Delete')   # WHAT: opened beside the writer. WHY: the service holds it open.
    try { return [System.IO.StreamReader]::new($stream).ReadToEnd() } finally { $stream.Dispose() }   # WHAT: all of it. WHY: searched.
}

# WHAT: whether a program file carries a version's build mark. WHY: which version a kept .old is (hub::build_mark).
function MarkedAs([string] $path, [string] $version) {
    $text = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($path))   # WHAT: the bytes as text. WHY: the mark is plain ASCII.
    return $text.Contains("hyperview-build-version=$version$([char]2)")         # WHAT: the exact mark. WHY: 0.7.1 never matches 0.7.10.
}

# WHAT: the cell, running and answering on 47110, and its process. WHY: the cell's start is waited for and checked.
function CellUp { (ListenerPath $Port) -ieq $CellExe }
function CellProcess { $found = Running $CellExe; Check ($found.Count -eq 1) "exactly one cell process runs ($($found.Count))"; return $found[0] }

# WHAT: runs one step of the LatCon PC (LatCon's LiveServerTests) and checks it wrote its done file. WHY: as LatCon's own Windows test.
function LatCon([string] $phase, [string] $pin) {
    $env:CITADEL_LIVE_PHASE = $phase                                            # WHAT: which step. WHY: the tests do nothing without it.
    $env:CITADEL_LIVE_SERVER_EXE = $CitadelExe                                  # WHAT: 1.0's program. WHY: open-pairing and approve, as IT runs them.
    $env:CITADEL_LIVE_PIN = $pin                                                # WHAT: 1.0's pin. WHY: every step must see it.
    $env:CITADEL_LIVE_STATE = $State                                            # WHAT: the PC's folder. WHY: kept between steps.
    try {
        & dotnet test (Join-Path $Latcon 'tests/Citadel.Client.Tests/Citadel.Client.Tests.csproj') -c Release --no-build -nologo `
            --filter 'FullyQualifiedName~Citadel.Client.Tests.LiveServerTests' --logger 'console;verbosity=normal'   # WHAT: the live tests only. WHY: built before.
        Check ($LASTEXITCODE -eq 0) "LatCon's $phase step passed"               # WHAT: passed. WHY: the PC's side worked.
    } finally { Remove-Item Env:CITADEL_LIVE_PHASE -ErrorAction SilentlyContinue }   # WHAT: cleared. WHY: no later run picks it up.
    Check (Test-Path -LiteralPath (Join-Path $State "$phase.done")) "LatCon's $phase step really ran"   # WHAT: its done file. WHY: a skipped step can't pass.
}

# WHAT: the USB kit: the server program and the citadel\ folder beside it. WHY: what IT runs on install day.
function Stick([string] $version) {
    $stick = Join-Path $Work "usb-office-$version"                              # WHAT: the stick's folder. WHY: one per version.
    if (-not (Test-Path -LiteralPath $stick)) {                                 # WHAT: made once. WHY: the same stick is used again in (e).
        New-Item -ItemType Directory -Path $stick | Out-Null                    # WHAT: made. WHY: filled below.
        Copy-Item -LiteralPath (Join-Path $Servers "ExcaliburView-Server-$version.exe") -Destination (Join-Path $stick 'ExcaliburView-Server.exe')   # WHAT: the server. WHY: the kit's program.
        Copy-Item -LiteralPath (Join-Path $Kit 'citadel') -Destination (Join-Path $stick 'citadel') -Recurse   # WHAT: citadel\ beside it. WHY: install checks it against the pin.
    }
    return (Join-Path $stick 'ExcaliburView-Server.exe')                        # WHAT: the program to run. WHY: --install.
}

# WHAT: the Office Server installed from the stick, with the test licence left in its data folder first. WHY: as IT does it, licensed for citadel.
function InstallOffice([string] $version) {
    New-Item -ItemType Directory -Force -Path $OfficeData | Out-Null            # WHAT: the data folder. WHY: the licence goes in before the first start.
    Copy-Item -LiteralPath $script:licence -Destination $OfficeData -Force      # WHAT: the licence. WHY: picked up at start (license.rs).
    $exe = Stick $version                                                       # WHAT: the kit. WHY: below.
    & $exe --install                                                            # WHAT: the elevated install, as the double-click runs it. WHY: the runner is an administrator.
    $code = $LASTEXITCODE                                                       # WHAT: its exit code. WHY: checked.
    $report = Get-Content -Raw -LiteralPath (Join-Path $OfficeHome 'last-install.txt') -ErrorAction SilentlyContinue   # WHAT: what it says. WHY: the reason, if it failed.
    Write-Host "install said: $report"                                          # WHAT: shown. WHY: the log has it.
    Check ($code -eq 0 -and "$report".StartsWith('ok')) "the Office Server $version installed as a service from the USB kit"   # WHAT: installed. WHY: (b).
    Check ((ServiceState $OfficeService) -eq 'RUNNING') "the $OfficeService service is RUNNING"   # WHAT: running. WHY: started by install.
    Check ((Sha (Join-Path $OfficeHome 'citadel\Citadel Server.exe')) -eq (Sha (Join-Path $Kit 'citadel\Citadel Server.exe'))) "the kit's pinned cell is installed beside it"   # WHAT: the cell in place. WHY: critical 3.
}

# WHAT: a signed release for the stand-in feed: the server, its cell part and a stand-in desktop app. WHY: (f) updates through it.
function Publish([string] $version) {
    $folder = Join-Path $Feed "v$version"                                       # WHAT: the release's folder. WHY: its tag.
    $build = Join-Path $Work "release-$version"                                 # WHAT: where it is put together. WHY: moved in whole.
    New-Item -ItemType Directory -Force -Path $build | Out-Null                 # WHAT: made. WHY: filled below.
    $app = Join-Path $build 'ExcaliburView.exe'                                 # WHAT: a stand-in desktop app. WHY: a release always names one; the server takes its own part first.
    [System.IO.File]::WriteAllText($app, "MZ a stand-in for the desktop app $version")   # WHAT: written. WHY: signed like any file.
    Copy-Item -LiteralPath (Join-Path $Servers "ExcaliburView-Server-$version.exe") -Destination (Join-Path $build 'ExcaliburView-Server.exe')   # WHAT: the server. WHY: the update.
    Copy-Item -LiteralPath $CellPart -Destination (Join-Path $build 'ExcaliburOffice-Citadel.tar.gz')   # WHAT: the cell part. WHY: the cell updates with the server.
    & $Python (Join-Path $View 'tools/publish.py') sign --key $Key --version $version --channel stable --app $app `
        --server (Join-Path $build 'ExcaliburView-Server.exe') --citadel (Join-Path $build 'ExcaliburOffice-Citadel.tar.gz') --out (Join-Path $build 'release.json')   # WHAT: signed, as a real release is. WHY: publish.py's own sign.
    Check ($LASTEXITCODE -eq 0) "release $version signed with the test key, its cell part checked against the pin"   # WHAT: signed. WHY: (f).
    New-Item -ItemType Directory -Force -Path $folder | Out-Null                # WHAT: the feed's folder. WHY: the files go in first.
    Get-ChildItem -LiteralPath $build | Where-Object Name -ne 'release.json' | Copy-Item -Destination $folder   # WHAT: the files. WHY: before the manifest.
    Copy-Item -LiteralPath (Join-Path $build 'release.json') -Destination $folder   # WHAT: the manifest last. WHY: the feed lists a release only once it has one.
}

# WHAT: stops the wait at once when the Office Server's log has raised an alarm since $from. WHY: a held cell never answers; waiting the whole limit says nothing more.
function NoAlarmSince([int] $from) {
    $text = OfficeLogText                                                       # WHAT: the log now. WHY: searched from $from.
    $new = $text.Substring([Math]::Min($from, $text.Length))                    # WHAT: what was written since. WHY: older alarms were expected.
    if ($new.Contains('ALARM')) { throw "FAILED: the Office Server raised an alarm:`n$new" }   # WHAT: stop, with the log. WHY: said plainly.
    if (([regex]::Matches($new, 'the Citadel cell stopped')).Count -ge 3) { throw "FAILED: the cell keeps stopping:`n$new" }   # WHAT: stop after three restarts. WHY: a cell that can't run won't run on the fourth try either.
}

# WHAT: the cell up on 47110, waited for, failing at once on an alarm. WHY: the move, then the cell's start.
function WaitForCell([int] $seconds, [int] $from) { WaitFor { NoAlarmSince $from; CellUp } $seconds 'the cell answering on 47110' }

# WHAT: the Office Server running a version, with its cell up again. WHY: after a restart, an update or a rollback.
function OfficeAt([string] $version, [int] $seconds) {
    $from = (OfficeLogText).Length                                              # WHAT: where the log is now. WHY: only new alarms count.
    WaitFor { NoAlarmSince $from; (OfficeVersion) -eq $version -and (CellUp) } $seconds "the Office Server $version with its cell answering"   # WHAT: waited for. WHY: below.
    Check ((OfficeVersion) -eq $version) "the Office Server says it is $version"   # WHAT: the version. WHY: the right program runs.
    Check (InJob (CellProcess).ProcessId) "the cell runs inside the Office Server's job object"   # WHAT: in its job. WHY: it dies with the server.
}

try {
    Step 'The test licence: citadel listed, signed with the job''s test key'
    $script:licence = Join-Path $Work 'E2E Test Office.evlicense'               # WHAT: the licence file. WHY: left in the data folder.
    & $Python (Join-Path $View 'tools/publish.py') sign-license --key $Key --company 'E2E Test Office' --founding --modules citadel --out $script:licence   # WHAT: signed, as a real one is. WHY: publish.py's own.
    Check ($LASTEXITCODE -eq 0) 'the test licence is signed'                    # WHAT: made. WHY: the cell runs only with citadel on it.

    Step '(a) Citadel Server 1.0.1 installed from the USB stick, and a LatCon PC paired with it'
    $oldStick = Join-Path $Work 'usb-1.0.1'                                     # WHAT: the first stick. WHY: as the kit was laid out.
    New-Item -ItemType Directory -Force -Path (Join-Path $oldStick 'Citadel Server') | Out-Null   # WHAT: its folder. WHY: as above.
    $oldExe = Join-Path $oldStick 'Citadel Server\Citadel Server.exe'           # WHAT: 1.0.1 on the stick. WHY: run from there, as IT did.
    Copy-Item -LiteralPath $Old -Destination $oldExe                            # WHAT: onto the stick. WHY: as the kit had it.
    $hex = [Convert]::ToHexString([System.Security.Cryptography.RandomNumberGenerator]::GetBytes(40)).ToLowerInvariant()   # WHAT: 80 random hex digits. WHY: a test download key.
    [System.IO.File]::WriteAllText((Join-Path $oldStick 'LatCon Estimating download key.txt'), "lce1_$($hex.Substring(0, 16))_$($hex.Substring(16, 64))`n")   # WHAT: the office's key at the stick's top. WHY: 1.0.1's install looks for it.
    Check ((ServiceState $CitadelService) -eq 'MISSING') 'no Citadel Server on this runner before the test'   # WHAT: a clean machine. WHY: a first install.
    & $oldExe install                                                           # WHAT: 1.0.1's own install. WHY: what IT ran.
    Check ($LASTEXITCODE -eq 0) '1.0.1 installed'                               # WHAT: it worked. WHY: exit 0.
    WaitFor { (ServiceState $CitadelService) -eq 'RUNNING' -and (ListenerPath $Port) } 90 '1.0.1 running on 47110'   # WHAT: up. WHY: its certificate is read off the port.
    $pin = Pin $Port                                                            # WHAT: 1.0.1's certificate. WHY: everything after must keep it.
    $pfx = Sha (Join-Path $CitadelData 'tls.pfx')                               # WHAT: its file's SHA-256. WHY: it must move byte for byte.
    Write-Host "1.0.1's pin $pin, tls.pfx $pfx"                                 # WHAT: shown. WHY: the log has both.
    Check (TaskThere $CitadelTask) "1.0.1's '$CitadelTask' task is there"       # WHAT: its 03:30 task. WHY: it must go after the move.
    Check (RuleThere $CitadelRule) "1.0.1's '$CitadelRule' firewall rule is there"   # WHAT: its rule. WHY: it must go after the move.
    LatCon 'move' $pin                                                          # WHAT: a PC pairs with the 6-digit code and moves its jobs up. WHY: data on 1.0.1.

    Step '(b) the Office Server installed from the USB kit, licensed for citadel'
    InstallOffice $First                                                        # WHAT: installed. WHY: below.

    Step '(c) its first start: 1.0''s data moved into the cell, 1.0 taken away'
    WaitForCell 600 0                                                           # WHAT: the move, then the cell. WHY: the move copies every file first.
    $log = OfficeLogText                                                        # WHAT: the service's log. WHY: checked below.
    Check ($log.Contains('was stopped and disabled for the move')) '1.0 was stopped and disabled before the move'   # WHAT: stopped first. WHY: two servers never run over the same data.
    Check ((Running $CitadelExe).Count -eq 0 -and (Running (Join-Path $CitadelFolder 'service\Citadel Server.exe')).Count -eq 0) '1.0 is not running'   # WHAT: stopped. WHY: the cell holds 47110.
    Check ((ServiceState $CitadelService) -eq 'MISSING') "1.0's service is uninstalled"   # WHAT: gone. WHY: after a good move.
    Check (-not (TaskThere $CitadelTask)) "1.0's 03:30 task is removed"         # WHAT: gone. WHY: it would update a program that no longer runs.
    Check (-not (RuleThere $CitadelRule)) "1.0's firewall rule is removed"      # WHAT: gone. WHY: nothing open for a program that doesn't run.
    Check (RuleThere $CellRule) "the cell's 47110 rule is there"                # WHAT: there. WHY: the PCs reach the cell.
    Check (-not (Test-Path -LiteralPath $CitadelData)) "1.0's data folder isn't where it was (kept renamed)"   # WHAT: renamed. WHY: never read again.
    Check ((Sha (Join-Path $CellData 'tls.pfx')) -eq $pfx) "the certificate file moved byte for byte (SHA-256 $pfx)"   # WHAT: the same file. WHY: the PCs' pins.
    Check ((Pin $Port) -eq $pin) "the cell shows 1.0.1's certificate on 47110 ($pin)"   # WHAT: the same pin on the wire. WHY: no PC pairs again.
    Check (Test-Path -LiteralPath (Join-Path $CellData 'state\moved-from.json')) "the cell's folder is marked as moved"   # WHAT: marked. WHY: never moved twice.
    $cell = CellProcess                                                         # WHAT: the cell's process. WHY: checked below.
    Check (InJob $cell.ProcessId) 'the cell runs inside the Office Server''s job object'   # WHAT: in its job. WHY: it dies with the server.
    Check ($cell.ParentProcessId -eq (Get-CimInstance Win32_Service -Filter "Name = '$OfficeService'").ProcessId) 'the cell was started by the Office Server''s service'   # WHAT: its parent. WHY: supervised.
    LatCon 'after-swap' $pin                                                    # WHAT: the PC paired with 1.0 opens its job on the cell and saves. WHY: carried over through the Office Server's list.

    Step '(d) two computers: the first claims the office, approves the second, which reaches a project and the cell'
    & $Device two-computers (Join-Path $Work 'computers')                       # WHAT: LatCon's own linking code. WHY: (d).
    Check ($LASTEXITCODE -eq 0) 'both computers linked as LatCon Estimating links them'   # WHAT: passed. WHY: its own checks printed above.

    Step '(f) an update through the feed, --roll-back, and a newer update'
    $script:feedProcess = Start-Process -FilePath $Python -ArgumentList @("`"$(Join-Path $View 'deploy/windows-test/test-feed.py')`"", "`"$Feed`"", $FeedPort, "`"$(Join-Path $Logs 'test-feed.log')`"") -PassThru -WindowStyle Hidden   # WHAT: the stand-in feed. WHY: the server reads it.
    Publish $Second                                                             # WHAT: the next version, signed. WHY: the update.
    New-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Services\$OfficeService" -Name Environment -PropertyType MultiString -Value @("HYPERVIEW_RELEASE_FEED=http://127.0.0.1:$FeedPort/releases") -Force | Out-Null   # WHAT: the service reads the stand-in feed. WHY: never the public one.
    Restart-Service -Name $OfficeService -Force                                 # WHAT: restarted with it. WHY: the feed is looked at 20 s after a start.
    OfficeAt $Second 300                                                        # WHAT: updated, cell and all. WHY: (f).
    Check (MarkedAs $OfficeKept $First) "the server before ($First) is kept as Hyperview-Server.old"   # WHAT: kept. WHY: for a rollback.
    Check (Test-Path -LiteralPath (Join-Path $OfficeHome 'citadel.old\Citadel Server.exe')) 'the cell before is kept as citadel.old'   # WHAT: kept. WHY: put back with the server.
    Check ((Pin $Port) -eq $pin) 'the cell still shows the same certificate after the update'   # WHAT: unchanged. WHY: the PCs' pins.
    $back = & $OfficeExe --roll-back 2>&1 | Out-String                          # WHAT: the rollback, as an administrator runs it. WHY: (f).
    Write-Host $back.TrimEnd()                                                  # WHAT: shown. WHY: the log has it.
    Check ($LASTEXITCODE -eq 0 -and $back.Contains("put version $First back")) "--roll-back put $First back"   # WHAT: done. WHY: (f).
    OfficeAt $First 120                                                         # WHAT: running as the version before. WHY: (f).
    Check (MarkedAs $OfficeKept $Second) "the version rolled back from ($Second) is kept beside it"   # WHAT: swapped. WHY: both kept.
    Check ((Test-Path -LiteralPath (Join-Path $OfficeHome 'citadel\Citadel Server.exe')) -and (Test-Path -LiteralPath (Join-Path $OfficeHome 'citadel.old\Citadel Server.exe'))) 'the cell was swapped back with it'   # WHAT: both cells. WHY: put back together.
    WaitFor { (OfficeLogText).Contains("version $Second is not taken") } 180 "the server to look at the feed and leave $Second"   # WHAT: held. WHY: a rollback holds.
    Check ((OfficeVersion) -eq $First) "$Second is not taken again"             # WHAT: still the version before. WHY: (f).
    Publish $Third                                                              # WHAT: a newer one. WHY: taken.
    Restart-Service -Name $OfficeService -Force                                 # WHAT: restarted. WHY: looked at 20 s after.
    OfficeAt $Third 300                                                         # WHAT: the newer one taken. WHY: (f).
    Check (MarkedAs $OfficeKept $First) "$First is kept beside $Third now"   # WHAT: the one before. WHY: one version kept.

    Step '(e) a failed move on a fresh install: 1.0 started again, the cell held; then the retry'
    & $OfficeExe --uninstall                                                    # WHAT: the Office Server taken off. WHY: a fresh install for the failure.
    Check ($LASTEXITCODE -eq 0 -and (ServiceState $OfficeService) -eq 'MISSING') 'the Office Server is uninstalled'   # WHAT: gone. WHY: below.
    WaitFor { (Running $CellExe).Count -eq 0 } 30 'the cell to end with its server'   # WHAT: the job ended. WHY: it dies with the server.
    Copy-Item -LiteralPath $OfficeLog -Destination (Join-Path $Logs 'hyperview-server-before-e.log')   # WHAT: the log so far, kept. WHY: its folder is removed next.
    Remove-Item -LiteralPath (Join-Path $env:ProgramData 'Excalibur Hyperview'), (Join-Path $env:ProgramData 'Excalibur Office') -Recurse -Force   # WHAT: its folders gone. WHY: a fresh office.
    & $oldExe install                                                           # WHAT: 1.0.1 again, fresh. WHY: an office that hasn't moved yet.
    Check ($LASTEXITCODE -eq 0) '1.0.1 installed again'                         # WHAT: it worked. WHY: exit 0.
    WaitFor { (ServiceState $CitadelService) -eq 'RUNNING' -and (ListenerPath $Port) } 90 '1.0.1 running on 47110'   # WHAT: up. WHY: below.
    $pin2 = Pin $Port                                                           # WHAT: its new certificate. WHY: must survive the failure and the retry.
    $pfx2 = Sha (Join-Path $CitadelData 'tls.pfx')                              # WHAT: its file. WHY: likewise.
    $link = Join-Path $CitadelData 'e2e-link'                                   # WHAT: a junction inside 1.0's folder. WHY: the move refuses a folder holding a link.
    New-Item -ItemType Junction -Path $link -Target $env:TEMP | Out-Null        # WHAT: made. WHY: the move fails, as a bad folder would.
    InstallOffice $First                                                        # WHAT: the Office Server again. WHY: its first start tries the move.
    WaitFor { (OfficeLogText).Contains("ALARM: Citadel Server 1.0's data couldn't be moved") } 300 'the failed move''s alarm'   # WHAT: the alarm. WHY: (e).
    WaitFor { (ServiceState $CitadelService) -eq 'RUNNING' -and "$(ListenerPath $Port)" -ilike "$CitadelFolder\*" } 120 '1.0 to answer on 47110 again'   # WHAT: 1.0's program back on its port. WHY: (e).
    Check ((ServiceState $CitadelService) -eq 'RUNNING') '1.0 was started again'   # WHAT: running. WHY: estimating carries on.
    Check ((StartType $CitadelService) -eq 'AUTO_START') '1.0 starts with Windows again'   # WHAT: enabled. WHY: a reboot keeps it.
    Check ((Running $CellExe).Count -eq 0) 'the cell is held, not started'      # WHAT: no cell. WHY: a cell started empty would make a new certificate.
    Check ((Pin $Port) -eq $pin2 -and (Sha (Join-Path $CitadelData 'tls.pfx')) -eq $pfx2) "1.0's data and certificate are untouched"   # WHAT: as it was. WHY: the old data stays.
    Remove-Item -LiteralPath $link -Force                                       # WHAT: the junction gone. WHY: the cause fixed.
    $said = & $OfficeExe --retry-citadel-move 2>&1 | Out-String                 # WHAT: the failure cleared. WHY: the console's retry.
    Write-Host $said.TrimEnd()                                                  # WHAT: shown. WHY: the log has it.
    Check ($LASTEXITCODE -eq 0) 'the failed move is cleared for a retry'        # WHAT: cleared. WHY: below.
    $from = (OfficeLogText).Length                                              # WHAT: where the log is now. WHY: the failure's alarm is older.
    Restart-Service -Name $OfficeService -Force                                 # WHAT: restarted. WHY: the retry runs at the start.
    WaitForCell 600 $from                                                       # WHAT: moved, then the cell. WHY: the retry.
    Check ((ServiceState $CitadelService) -eq 'MISSING') "the retry moved the data and took 1.0's service away"   # WHAT: done. WHY: as a first good move.
    Check ((Pin $Port) -eq $pin2) 'the cell shows the certificate 1.0 had'      # WHAT: the same pin. WHY: no PC pairs again.
    Check (InJob (CellProcess).ProcessId) 'the cell runs inside the Office Server''s job object'   # WHAT: in its job. WHY: as before.

    Step 'Passed'
    Write-Host 'Office Server end to end on Windows: 1.0.1 moved into the cell at first start, two computers linked, a project and the cell reached, an update rolled back and a newer one taken, a failed move held and retried.'   # WHAT: the summary. WHY: one line for the run page.
} catch {
    Write-Host '--- the end of the Office Server''s log ---'                    # WHAT: a heading. WHY: the failure's cause is usually there.
    (OfficeLogText) -split "`n" | Select-Object -Last 80 | ForEach-Object { Write-Host $_ }   # WHAT: its last lines. WHY: read on the run page without downloading.
    & "$env:SystemRoot\System32\robocopy.exe" (Join-Path $CellData 'logs') (Join-Path $Logs 'cell-now') /E /B /R:0 /W:0 /NP /NFL /NDL | Out-Null   # WHAT: the cell's logs, in backup mode. WHY: its folder is the system's and the cell's alone.
    $cellLog = Get-ChildItem -LiteralPath (Join-Path $Logs 'cell-now') -File -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1   # WHAT: the cell's newest log. WHY: why it stopped, if it ran at all.
    if ($cellLog) { Write-Host "--- the end of the cell's log ($($cellLog.Name)) ---"; Get-Content -LiteralPath $cellLog.FullName -Tail 40 | ForEach-Object { Write-Host $_ } }   # WHAT: its last lines. WHY: as above.
    Write-Host '--- who may use the cell''s folder ---'; & "$env:SystemRoot\System32\icacls.exe" $CellData 2>&1 | ForEach-Object { Write-Host $_ }   # WHAT: its permissions. WHY: the cell runs under a restricted token.
    throw                                                                       # WHAT: the failure, as it was. WHY: the step fails.
} finally {
    if ($script:feedProcess) { Stop-Process -Id $script:feedProcess.Id -Force -ErrorAction SilentlyContinue }   # WHAT: the feed stopped. WHY: pass or fail.
    foreach ($from in @($OfficeLog, "$OfficeLog.1", (Join-Path $OfficeHome 'last-install.txt'))) {
        if (Test-Path -LiteralPath $from) { Copy-Item -LiteralPath $from -Destination $Logs -ErrorAction SilentlyContinue }   # WHAT: the Office Server's log. WHY: a failed run says why.
    }
    foreach ($pair in @(@($CellData, 'cell'), @($CitadelData, 'citadel-1.0'))) {
        & "$env:SystemRoot\System32\robocopy.exe" (Join-Path $pair[0] 'logs') (Join-Path $Logs $pair[1]) /E /B /R:0 /W:0 /NP /NFL /NDL | Out-Null   # WHAT: the cell's or 1.0's own logs, in backup mode. WHY: their folders are the system's and the cell's alone; logs only, no keys or data.
    }
    $global:LASTEXITCODE = 0                                                    # WHAT: robocopy's codes forgotten. WHY: they are counts, not failures.
}
