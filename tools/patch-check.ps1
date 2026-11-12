# The weekly check for a game patch. When Reliquary (IceDynamix/reliquary) has a tag newer than the one main builds
# on, or TurnBasedGameData (the game tables core/build.rs compiles in) has a commit newer than the one main's
# core/gamedata.rev pins, it prepares the update on a local branch in a worktree beside the repo (<repo>-patch):
# reliquary-<tag> when the protocol moved (tools/update-reliquary.ps1, and the newest game data with it), otherwise
# gamedata-<commit>. A move some branch already carries is not prepared again, so game data that moves after a
# protocol branch was made gets a gamedata-<commit> of its own. It raises the patch version, runs
# `cargo test --workspace` and `cargo build --release`, and commits whatever they said. When the release build
# passes, tools/publish-candidate.ps1 puts the branch on the test channel as a GitHub pre-release. It reports with a
# Windows notification and target/patch-check/<branch>.md, and logs every run in target/patch-check/patch-check.log.
# It never pushes main and never makes a full release.
#
#   .\tools\patch-check.ps1 -DryRun                            # what it would do now
#   .\tools\patch-check.ps1 -Register -Jobs 2 -Affinity FFFF0  # weekly, cargo with two jobs on CPUs 4-19 (a hex mask)
#   Start-ScheduledTask "Nous Relic Importer patch check"      # the weekly check, now
param(
    [ValidateRange(1, 64)][int]$Jobs = 2,
    [ValidatePattern('^[0-9A-Fa-f]{1,16}$')][string]$Affinity,
    [string]$Worktree = "$(Split-Path -Parent $PSScriptRoot)-patch",
    [switch]$DryRun,
    [switch]$Register
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo "target\patch-check"
$taskName = "Nous Relic Importer patch check"
# What the tests that replay decoder/tests/emulator-login.pcap say when it no longer decodes.
$stale = "needs a new emulator recording"

if ($Register) {
    $arguments = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$PSCommandPath`" -Jobs $Jobs -Worktree `"$Worktree`""
    if ($Affinity) { $arguments += " -Affinity $Affinity" }
    $task = @{
        TaskName = $taskName
        Description = "When Reliquary tags a new game protocol, prepares Nous Relic Importer's update on a branch and puts it on the test channel. Never pushes main."
        # Through conhost --headless: even a hidden PowerShell window flashes on the desktop each run.
        Action = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\conhost.exe" -WorkingDirectory $repo `
            -Argument "--headless `"$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe`" $arguments"
        Trigger = New-ScheduledTaskTrigger -Weekly -DaysOfWeek Thursday -At "10:00"
        Settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -RunOnlyIfNetworkAvailable -AllowStartIfOnBatteries `
            -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Hours 4)
        Principal = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
    }
    Register-ScheduledTask @task -Force | Out-Null
    Write-Host "Registered `"$taskName`": Thursdays at 10:00, or as soon as it can after a missed one."
    return
}

function Get-Version([string]$Tag) {
    if ($Tag -match '^v?(\d+)\.(\d+)\.(\d+)$') { [version]"$($Matches[1]).$($Matches[2]).$($Matches[3])" }
}

function Invoke-Git {
    $output = git @args
    if ($LASTEXITCODE) { throw "git $args failed" }
    $output
}

function Test-Branch([string]$Name) {
    git -C $repo rev-parse --verify --quiet "refs/heads/$Name" | Out-Null
    -not $LASTEXITCODE
}

# The TurnBasedGameData commit $Ref's core/gamedata.rev pins; nothing when it has no such file.
function Get-Pin([string]$Ref) {
    git -C $repo rev-parse --verify --quiet "${Ref}:core/gamedata.rev" | Out-Null
    if (-not $LASTEXITCODE) { (((Invoke-Git -C $repo show "${Ref}:core/gamedata.rev") -join "`n").Trim() -split '\s+')[0] }
}

function Write-Log([string]$Line) {
    Write-Host $Line
    if (-not $DryRun) { Add-Content -LiteralPath (Join-Path $out "patch-check.log") -Value "$(Get-Date -Format 'yyyy-MM-dd HH:mm') $Line" }
}

# A Windows notification for the signed-in user, under Windows PowerShell's name, that opens $Open when clicked.
# Notifications need WinRT, which Windows PowerShell has and PowerShell 7 does not.
function Send-Notification([string]$Title, [string]$Body, [string]$Open) {
    $xml = { param($Text) [Security.SecurityElement]::Escape($Text) }
    $toast = "<toast activationType=`"protocol`" launch=`"$(& $xml ([uri]$Open).AbsoluteUri)`"><visual><binding template=`"ToastGeneric`">" +
        "<text>$(& $xml $Title)</text><text>$(& $xml $Body)</text></binding></visual></toast>"
    $show = "`$ProgressPreference = 'SilentlyContinue'`n" +
        "[void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]`n" +
        "[void][Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]`n" +
        "`$toast = New-Object Windows.Data.Xml.Dom.XmlDocument`n" +
        "`$toast.LoadXml('$($toast.Replace("'", "''"))')`n" +
        "[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe').Show((New-Object Windows.UI.Notifications.ToastNotification `$toast))"
    & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -EncodedCommand ([Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($show)))
    if ($LASTEXITCODE) { Write-Log "Could not show the notification." }
}

# cargo in the worktree, inside the CPU mask if one was given, its output in $Log; its exit code.
function Invoke-Cargo([string]$Arguments, [string]$Log) {
    $pinned = if ($Affinity) { "start `"`" /affinity $Affinity /wait /b " } else { "" }
    # start /wait leaves the exit code in ERRORLEVEL only, which /v:on reads once the line has run.
    $info = New-Object Diagnostics.ProcessStartInfo "cmd.exe", "/d /v:on /s /c `"${pinned}cargo $Arguments > `"$Log`" 2>&1 & exit !ERRORLEVEL!`""
    $info.UseShellExecute = $false
    $info.WorkingDirectory = $Worktree
    $cargo = [Diagnostics.Process]::Start($info)
    $cargo.WaitForExit()
    $cargo.ExitCode
}

# The first error a failed cargo run printed, past the summary of which targets failed.
function Get-FirstError([string]$Log) {
    $first = Select-String -LiteralPath $Log -Pattern '^error(\[E\d+\])?: ' |
        Where-Object { $_.Line -notmatch '^error: (test failed|\d+ targets? failed)' } | Select-Object -First 1
    if ($first) { $first.Line } else { "see the log" }
}

# What a cargo run's log says: passed, its first error, or which tests failed and whether they replay the recording.
function Get-Verdict([int]$Exit, [string]$Log) {
    if (-not $Exit) { return "passed" }
    $failed = [regex]::Matches([IO.File]::ReadAllText($Log), '(?ms)^---- (\S+) stdout ----\r?$(.*?)(?=^---- |^failures:\r?$|\z)')
    if (-not $failed.Count) { return "failed: $(Get-FirstError $Log)" }
    $replays = @($failed | Where-Object { $_.Groups[2].Value.Contains($stale) } | ForEach-Object { $_.Groups[1].Value })
    $others = @($failed | Where-Object { -not $_.Groups[2].Value.Contains($stale) } | ForEach-Object { $_.Groups[1].Value })
    $verdict = @()
    if ($replays) { $verdict += "the $($replays.Count) tests that replay the emulator recording fail, so it $stale" }
    if ($others) { $verdict += "$($others.Count) other tests fail: $($others -join ', ')" }
    $verdict -join "; "
}

try {
    if (-not $DryRun) { New-Item -ItemType Directory -Force $out | Out-Null }
    $manifest = (Invoke-Git -C $repo show main:Cargo.toml) -join "`n"
    $current = [regex]::Match($manifest, '(?m)^reliquary = \{[^}]*?\btag = "([^"]+)"').Groups[1].Value
    $package = ([regex]'(?m)^version = "(\d+\.\d+)\.(\d+)"').Match($manifest, $manifest.IndexOf("[package]"))
    if (-not $current -or -not $package.Success) { throw "main's Cargo.toml has no reliquary tag or no plain version" }
    $pinned = Get-Pin main
    if ($pinned -notmatch '^[0-9a-f]{40}$') { throw "main's core/gamedata.rev does not start with a commit id" }
    $newest = gh api --paginate repos/IceDynamix/reliquary/tags --jq '.[].name' |
        Where-Object { Get-Version $_ } | Sort-Object { Get-Version $_ } | Select-Object -Last 1
    if ($LASTEXITCODE -or -not $newest) { throw "gh could not list Reliquary's tags" }
    $data = @(Invoke-RestMethod -UseBasicParsing -TimeoutSec 60 `
        "https://gitlab.com/api/v4/projects/Dimbreath%2Fturnbasedgamedata/repository/commits?ref_name=main&per_page=1")[0]
    if ($data.id -notmatch '^[0-9a-f]{40}$') { throw "GitLab did not name TurnBasedGameData's newest commit" }
    $protocolMoved = (Get-Version $newest) -gt (Get-Version $current)
    $tablesMoved = $data.id -ne $pinned
    if (-not $protocolMoved -and -not $tablesMoved) {
        Write-Log "Up to date: main builds on reliquary $current and game data $($pinned.Substring(0, 8)), the newest of each."
        return
    }
    $protocolMove = "reliquary $current -> $newest"
    $tablesMove = "game data $($pinned.Substring(0, 8)) -> $($data.short_id) ($($data.title))"
    # A move is prepared once a branch carries it: the protocol on reliquary-<tag>, the game data on a branch whose
    # core/gamedata.rev pins it. A protocol branch made before the data moved leaves that move to gamedata-<commit>.
    $protocolOn = if ($protocolMoved -and (Test-Branch "reliquary-$newest")) { "reliquary-$newest" }
    $carriers = @("gamedata-$($data.short_id)") + @("reliquary-$newest" | Where-Object { $protocolMoved })
    $tablesOn = if ($tablesMoved) { $carriers | Where-Object { (Test-Branch $_) -and (Get-Pin $_) -eq $data.id } | Select-Object -First 1 }
    # What this run prepares: a new protocol carries the newest game data with it.
    $protocol = $protocolMoved -and -not $protocolOn
    $tables = $tablesMoved -and ($protocol -or -not $tablesOn)
    if (-not $protocol -and -not $tables) {
        $where = @()
        if ($protocolOn) { $where += "$protocolMove on branch $protocolOn" }
        if ($tablesOn) { $where += "$tablesMove on branch $tablesOn" }
        Write-Log "Already prepared: $($where -join ' and ')."
        return
    }
    $moves = @()
    if ($protocol) { $moves += $protocolMove }
    if ($tables) { $moves += $tablesMove }
    $branch = if ($protocol) { "reliquary-$newest" } else { "gamedata-$($data.short_id)" }
    $version ="$($package.Groups[1].Value).$($package.Groups[2].Value)"
    $next = "$($package.Groups[1].Value).$([int]$package.Groups[2].Value + 1)"
    if ($DryRun) {
        Write-Log "Would prepare branch $branch in ${Worktree}: $($moves -join ', '), version $version -> $next, then cargo test and the release build."
        return
    }

    Write-Log "$($moves -join ' and '): preparing branch $branch in $Worktree."
    if (Test-Path $Worktree) {
        if (Invoke-Git -C $Worktree status --porcelain) { throw "$Worktree has uncommitted changes: commit or discard them, and the next check prepares $branch." }
        Invoke-Git -C $Worktree switch --quiet --detach main
    } else {
        Invoke-Git -C $repo worktree prune
        Invoke-Git -C $repo worktree add --quiet --detach $Worktree main
    }
    $said = @()
    if ($protocol) {
        try {
            $said = & (Join-Path $Worktree "tools\update-reliquary.ps1") -Reliquary $newest *>&1 |
                ForEach-Object { if ($_ -is [Management.Automation.WarningRecord]) { "WARNING: $_" } else { "$_" } }
        } catch {
            # This run's own changes, in a worktree that was clean: the next check starts over.
            git -C $Worktree reset --quiet --hard
            git -C $Worktree clean --quiet -d --force
            throw "tools/update-reliquary.ps1 could not move to reliquary ${newest}: $_"
        }
    }
    if ($tables) { [IO.File]::WriteAllText((Join-Path $Worktree "core\gamedata.rev"), "$($data.id) $($data.title)`n") }
    $cargoToml = Join-Path $Worktree "Cargo.toml"
    $text = [IO.File]::ReadAllText($cargoToml)
    [IO.File]::WriteAllText($cargoToml, ([regex]'(?m)^version = "[^"]*"').Replace($text, "version = `"$next`"", 1, $text.IndexOf("[package]")))

    $testLog = Join-Path $out "$branch-test.log"
    $buildLog = Join-Path $out "$branch-build.log"
    $tests = Get-Verdict (Invoke-Cargo "test --workspace --no-fail-fast -j $Jobs -- --test-threads $Jobs" $testLog) $testLog
    $build = Get-Verdict (Invoke-Cargo "build --release -j $Jobs" $buildLog) $buildLog
    $results = @(
        "- $($moves -join '; '); version $version -> $next"
        "- cargo test --workspace: $tests"
        "- cargo build --release: $build"
    )
    $said = if ($protocol) { @("", "tools/update-reliquary.ps1 said:", "") + ($said | ForEach-Object { "    $_" }) } else { @() }
    $title = if ($protocol) { "Protocol: reliquary $newest" } else { "Game data: $($data.title)" }
    $message = Join-Path $out "$branch-commit.txt"
    [IO.File]::WriteAllText($message, ((@("$title, version $next", "",
        "Prepared by tools/patch-check.ps1, which never pushes:") + $results + $said) -join "`n") + "`n")
    Invoke-Git -C $Worktree switch --quiet --create $branch
    Invoke-Git -C $Worktree add --all
    Invoke-Git -C $Worktree commit --quiet --file $message
    $commit = Invoke-Git -C $Worktree rev-parse --short HEAD
    # Leave the branch free to check out, merge or delete from the repo itself.
    Invoke-Git -C $Worktree switch --quiet --detach

    $recording = if ($tests.Contains($stale)) {
        "The capture tests replay decoder/tests/emulator-login.pcap, recorded on the old protocol: record a PearlSR login " +
        "on the new one (``--emulator --record``), put it in that file's place and set the counts its tests expect. "
    } else { "" }
    $candidate = "not published: the release build did not pass"
    if ($build -eq "passed") {
        $extra = @{}
        if ($Affinity) { $extra.Affinity = $Affinity }
        & (Join-Path $PSScriptRoot "publish-candidate.ps1") -Ref $branch -Worktree $Worktree -Jobs $Jobs @extra
        $candidate = if ($LASTEXITCODE) { "publishing failed: see target\candidates\candidates.log" } else { "on the test channel as a pre-release" }
    }
    $results += "- release candidate: $candidate"
    $report = Join-Path $out "$branch.md"
    $news = if ($protocol) { "reliquary $newest is out" } else { "the game data moved to $($data.title)" }
    [IO.File]::WriteAllText($report, ((@(
        "# $news", "",
        "Prepared $(Get-Date -Format 'yyyy-MM-dd HH:mm') on branch ``$branch`` (commit $commit), checked out in $Worktree.",
        "Main was not pushed, and nothing was released.", "") + $results + @("",
        "Logs: $testLog and $buildLog.") + $said + @("",
        "Review it with ``git diff main $branch``. $($recording)Merge it into main; promoting its candidate to a " +
        "release is README.md's ""Publishing a release"".")) -join "`n") + "`n")
    Write-Log "Prepared $branch ($commit). Tests: $tests. Release build: $build. Report: $report"
    Send-Notification "Nous Relic Importer: $news" "Branch $branch is prepared in $Worktree. Tests: $tests. Release build: $build. Candidate: $candidate." $report
} catch {
    Write-Log "Failed: $_"
    if (-not $DryRun) { Send-Notification "Nous Relic Importer patch check failed" "$_" (Join-Path $out "patch-check.log") }
    exit 1
}
