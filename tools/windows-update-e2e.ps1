# The Windows updater end to end, with no admin and nothing outside a scratch folder:
#   tools\windows-update-e2e.ps1 -Old <older build> -Feed <signed folder> [-Phase quiet|active]
# <signed folder> holds latest.json and the newer build (nri-release sign --base-url http://127.0.0.1:8765/), signed
# with the key the older build trusts; it is served on that address. The older build runs from the install folder of
# a scratch LOCALAPPDATA, so it counts as installed, with --emulator (no admin, no live game) and --headless.
#   quiet:  no game traffic: it downloads the newer build, verifies it and hands over within seconds
#   active: a game session through the emulator relay: it waits for the session and five quiet minutes after it
# Either way the newer build must replace the installed file, start, find itself up to date and quit when asked.
param(
    [Parameter(Mandatory)][string]$Old,
    [Parameter(Mandatory)][string]$Feed,
    [ValidateSet('quiet', 'active')][string]$Phase = 'quiet',
    [int]$GameSeconds = 90
)
$ErrorActionPreference = 'Stop'
# The agent CPUs on the workstation (CLAUDE.md); everything started from here inherits them.
(Get-Process -Id $PID).ProcessorAffinity = 0xFFFF0
$latest = Get-Content (Join-Path $Feed 'latest.json') -Raw | ConvertFrom-Json
$newBuild = Join-Path $Feed 'nous-relic-importer.exe'
$run = Join-Path ([IO.Path]::GetTempPath()) "nri-update-e2e-$Phase"
Remove-Item -Recurse -Force $run -ErrorAction SilentlyContinue
$la = Join-Path $run 'LocalAppData'
$dir = Join-Path $la 'Programs\Nous Relic Importer'
New-Item -ItemType Directory -Force $dir, (Join-Path $run 'AppData') | Out-Null
$installed = Join-Path $dir 'nous-relic-importer.exe'
Copy-Item $Old $installed
$log = Join-Path $la 'Nous Relic Importer\sync.log'
$env:LOCALAPPDATA = $la
$env:APPDATA = Join-Path $run 'AppData'
$t0 = Get-Date
$server = $null

function Lines { if (Test-Path $log) { Get-Content $log } else { @() } }
function Stamp($what) { Write-Output ('[{0,6:N1}s] {1}' -f ((Get-Date) - $t0).TotalSeconds, $what) }
function Stop-Server { if ($server -and -not $server.HasExited) { Stop-Process -Id $server.Id } }
function Fail($why) { Stop-Server; Write-Output "FAIL: $why"; Write-Output '--- log:'; Lines; exit 1 }
function WaitFor($pattern, $seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        if ((Lines) -match $pattern) { return $true }
        Start-Sleep -Milliseconds 500
    }
    $false
}
function Quit {
    Add-Type -Namespace SyncE2E -Name Kernel -MemberDefinition @'
[DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr OpenEventW(uint access, bool inherit, string name);
[DllImport("kernel32.dll")] public static extern bool SetEvent(IntPtr handle);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr handle);
'@
    $handle = [SyncE2E.Kernel]::OpenEventW(0x0002, $false, 'Local\NousRelicImporterQuit')
    if ($handle -eq [IntPtr]::Zero) { return }
    [SyncE2E.Kernel]::SetEvent($handle) | Out-Null
    [SyncE2E.Kernel]::CloseHandle($handle) | Out-Null
}

if (Get-Process nous-relic-importer -ErrorAction SilentlyContinue) { Fail 'Nous Relic Importer is already running; quit it first' }
$server = Start-Process -FilePath python -ArgumentList '-m', 'http.server', '8765', '--bind', '127.0.0.1', '--directory', $Feed -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 2
$app = Start-Process -FilePath $installed -ArgumentList '--emulator', '--headless', '--update-feed', 'http://127.0.0.1:8765/latest.json' -PassThru
Stamp "started the installed older build as pid $($app.Id); the feed offers $($latest.version)"

if ($Phase -eq 'active') {
    # The relay forwards 127.0.0.1:23301 to the emulator on 127.0.0.2:23301: stand in for both ends.
    $game = Start-ThreadJob -ArgumentList $GameSeconds -ScriptBlock {
        param($seconds)
        $emulator = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Parse('127.0.0.2'), 23301)
        $emulator.Start()
        $client = [Net.Sockets.TcpClient]::new()
        foreach ($try in 1..100) { try { $client.Connect('127.0.0.1', 23301); break } catch { Start-Sleep -Milliseconds 200 } }
        $session = $emulator.AcceptTcpClient()
        $chunk, $buffer, $received = [byte[]]::new(64), [byte[]]::new(4096), 0
        $end = (Get-Date).AddSeconds($seconds)
        while ((Get-Date) -lt $end) {
            $session.GetStream().Write($chunk, 0, $chunk.Length)
            Start-Sleep -Seconds 1
            while ($client.GetStream().DataAvailable) { $received += $client.GetStream().Read($buffer, 0, $buffer.Length) }
        }
        $session.Close(); $client.Close(); $emulator.Stop()
        "$received bytes went through the relay in $seconds s"
    }
    Stamp "a game session through the relay for $GameSeconds s"
}

if (-not (WaitFor 'update downloaded and verified' 60)) { Fail 'no verified download' }
Stamp 'downloaded and verified the newer build'

if ($Phase -eq 'active') {
    $quietUntil = $t0.AddSeconds($GameSeconds + 295)
    while ((Get-Date) -lt $quietUntil) {
        if ((Lines) -match 'handing over') { Fail ('handed over {0:N0} s in, during the session or the quiet minutes after it' -f ((Get-Date) - $t0).TotalSeconds) }
        if ($app.HasExited) { Fail 'the running copy exited during the game session' }
        Start-Sleep -Seconds 2
    }
    Stamp ('no hand-over during the session or the five quiet minutes after it (game: {0})' -f (Receive-Job $game -Wait))
    if (-not (WaitFor 'handing over' 60)) { Fail 'it never installed once the game was quiet' }
    Stamp 'handed over once the game had been quiet'
}

if (-not (WaitFor 'update installed' 60)) { Fail 'the newer build never installed itself' }
if (-not (WaitFor "started version=`"?$([regex]::Escape($latest.version))" 30)) { Fail 'the newer build did not start' }
Stamp "the installed copy is now $($latest.version) and running"
$app.WaitForExit(15000) | Out-Null
if (-not $app.HasExited) { Fail 'the older copy is still running' }
if ((Get-FileHash $installed).Hash -ne (Get-FileHash $newBuild).Hash) { Fail 'the installed file is not the newer build' }
$running = @(Get-CimInstance Win32_Process -Filter "Name = 'nous-relic-importer.exe'")
if ($running.Count -ne 1 -or $running[0].ExecutablePath -ne $installed) {
    Fail ('expected one copy, running from the install folder: ' + ($running | ForEach-Object { "$($_.ProcessId) $($_.CommandLine)" }))
}
Stamp ('running as pid {0}: {1}' -f $running[0].ProcessId, $running[0].CommandLine)

# Started in the background, the new copy checks 90 s in: up to date, and the download it ran from cleared away.
if (-not (WaitFor "up to date latest=`"?$([regex]::Escape($latest.version))" 120)) { Fail 'the newer build did not find itself up to date' }
if (Test-Path (Join-Path $dir 'update\nous-relic-importer.exe')) { Fail 'the downloaded build was left behind' }
Stamp 'it checked the feed again: up to date, download cleared'
Quit
$deadline = (Get-Date).AddSeconds(15)
while ((Get-Process -Id $running[0].ProcessId -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
if (Get-Process -Id $running[0].ProcessId -ErrorAction SilentlyContinue) { Fail 'it did not quit when asked' }
Stop-Server
Stamp 'quit when asked'
Lines | Select-String 'update|handing over|started|up to date'
Write-Output "PASS ($Phase)"
