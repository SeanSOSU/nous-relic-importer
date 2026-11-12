# Puts a release candidate on the test channel. When -Ref (default main) carries a version no release or pre-release
# has yet, it builds that commit (`cargo build --release`, and `cargo test --workspace` for the notes), signs the build
# with the release key, pushes the tag v<version> and publishes it as a GitHub pre-release, then points the test
# channel's feed (the pre-release `test-channel`) at it. Copies started with --test-channel install it within 15
# minutes, the way every copy installs a release. Public copies read only the latest full release, which a pre-release
# never is; promoting a candidate to one (same files) is a release, which only Sean's $release does.
# A candidate is never published twice: every new candidate needs a new version.
#
#   .\tools\publish-candidate.ps1 -DryRun                            # what it would do with main now
#   .\tools\publish-candidate.ps1 -Ref reliquary-v25.0.0 -Worktree <dir>  # patch-check does this for its branches
#   .\tools\publish-candidate.ps1 -Register -Jobs 2 -Affinity FFFF0  # hourly for main, cargo on CPUs 4-19
param(
    [string]$Ref = "main",
    [string]$Worktree = "$(Split-Path -Parent $PSScriptRoot)-candidate",
    # Default: update-signing.key in the folder beside the main checkout, <repo>-keys.
    [string]$Key,
    [ValidateRange(1, 64)][int]$Jobs = 2,
    [ValidatePattern('^[0-9A-Fa-f]{1,16}$')][string]$Affinity,
    [switch]$DryRun,
    [switch]$Register
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$repo = Split-Path -Parent $PSScriptRoot
if (-not $Key) {
    $common = (git -C $repo rev-parse --path-format=absolute --git-common-dir) | Select-Object -First 1
    $Key = Join-Path "$(Split-Path -Parent ([IO.Path]::GetFullPath($common)))-keys" "update-signing.key"
}
$out = Join-Path $repo "target\candidates"
$taskName = "Nous Relic Importer candidates"
$channel = "test-channel"

if ($Register) {
    $arguments = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$PSCommandPath`" -Jobs $Jobs -Worktree `"$Worktree`""
    if ($Affinity) { $arguments += " -Affinity $Affinity" }
    $task = @{
        TaskName = $taskName
        Description = "Publishes each new version on Nous Relic Importer's main as a GitHub pre-release for the test channel. Never makes a full release."
        Action = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument $arguments -WorkingDirectory $repo
        Trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).Date -RepetitionInterval (New-TimeSpan -Hours 1)
        Settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -RunOnlyIfNetworkAvailable -AllowStartIfOnBatteries `
            -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Hours 2)
        Principal = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
    }
    Register-ScheduledTask @task -Force | Out-Null
    Write-Host "Registered `"$taskName`": every hour."
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

function Invoke-Gh {
    $output = gh @args
    if ($LASTEXITCODE) { throw "gh $args failed" }
    $output
}

function Write-Log([string]$Line) {
    Write-Host $Line
    if (-not $DryRun) { Add-Content -LiteralPath (Join-Path $out "candidates.log") -Value "$(Get-Date -Format 'yyyy-MM-dd HH:mm') $Line" }
}

# A Windows notification for the signed-in user (Windows PowerShell has the WinRT types; PowerShell 7 does not).
function Send-Notification([string]$Title, [string]$Body) {
    $xml = { param($Text) [Security.SecurityElement]::Escape($Text) }
    $toast = "<toast><visual><binding template=`"ToastGeneric`"><text>$(& $xml $Title)</text><text>$(& $xml $Body)</text></binding></visual></toast>"
    $show = "`$ProgressPreference = 'SilentlyContinue'`n" +
        "[void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]`n" +
        "[void][Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]`n" +
        "`$toast = New-Object Windows.Data.Xml.Dom.XmlDocument`n" +
        "`$toast.LoadXml('$($toast.Replace("'", "''"))')`n" +
        "[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe').Show((New-Object Windows.UI.Notifications.ToastNotification `$toast))"
    & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -EncodedCommand ([Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($show)))
}

# cargo in the worktree, inside the CPU mask if one was given, its output in $Log; its exit code.
function Invoke-Cargo([string]$Arguments, [string]$Log) {
    $pinned = if ($Affinity) { "start `"`" /affinity $Affinity /wait /b " } else { "" }
    $info = New-Object Diagnostics.ProcessStartInfo "cmd.exe", "/d /v:on /s /c `"${pinned}cargo $Arguments > `"$Log`" 2>&1 & exit !ERRORLEVEL!`""
    $info.UseShellExecute = $false
    $info.WorkingDirectory = $Worktree
    $cargo = [Diagnostics.Process]::Start($info)
    $cargo.WaitForExit()
    $cargo.ExitCode
}

try {
    if (-not $DryRun) { New-Item -ItemType Directory -Force $out | Out-Null }
    $commit = (Invoke-Git -C $repo rev-parse --verify "$Ref^{commit}") | Select-Object -First 1
    $manifest = (Invoke-Git -C $repo show "${commit}:Cargo.toml") -join "`n"
    $package = ([regex]'(?m)^version = "(\d+\.\d+\.\d+)"').Match($manifest, $manifest.IndexOf("[package]"))
    if (-not $package.Success) { throw "$Ref's Cargo.toml has no plain version" }
    $version = $package.Groups[1].Value
    $tag = "v$version"
    # Only the vendored crates may be patched in: a patch from outside the repo is someone's local experiment.
    $patches = [regex]::Matches($manifest, '(?ms)^\[patch\.[^\]]+\]\r?\n(.*?)(?=^\[|\z)') | ForEach-Object { $_.Groups[1].Value }
    if ($patches | Where-Object { $_ -match 'path = "(?!vendor/)' -or $_ -match '\bgit = ' }) { throw "$Ref patches a crate from outside vendor/: not a candidate" }

    $remote = (Invoke-Git -C $repo remote get-url origin) | Select-Object -First 1
    $slug = [regex]::Match($remote, 'github\.com[:/](.+?)(\.git)?$').Groups[1].Value
    if (-not $slug) { throw "origin is not a GitHub repository: $remote" }
    $published = @(Invoke-Gh release list --repo $slug --limit 200 --json tagName --jq '.[].tagName')
    if ($published -contains $tag) { Write-Host "$Ref ($version) is already published."; return }
    $newest = $published | Where-Object { Get-Version $_ } | Sort-Object { Get-Version $_ } | Select-Object -Last 1
    if ($newest -and (Get-Version $tag) -le (Get-Version $newest)) { Write-Host "$Ref ($version) is not newer than ${newest}: nothing to publish."; return }
    if (-not (Test-Path -LiteralPath $Key)) { throw "no release key at $Key" }
    $short = $commit.Substring(0, 7)
    if ($DryRun) {
        Write-Log "Would publish $Ref ($short) as pre-release $tag and point the test channel at it."
        return
    }

    Write-Log "Publishing $Ref ($short) as release candidate $tag."
    if (Test-Path $Worktree) {
        if (Invoke-Git -C $Worktree status --porcelain --untracked-files=no) { throw "$Worktree has uncommitted changes" }
        Invoke-Git -C $Worktree switch --quiet --detach $commit
    } else {
        Invoke-Git -C $repo worktree prune
        Invoke-Git -C $repo worktree add --quiet --detach $Worktree $commit
    }
    $testLog = Join-Path $out "$tag-test.log"
    $buildLog = Join-Path $out "$tag-build.log"
    $signLog = Join-Path $out "$tag-sign.log"
    if (Invoke-Cargo "build --release -j $Jobs" $buildLog) { throw "the release build failed: see $buildLog" }
    $tests = if (Invoke-Cargo "test --workspace --no-fail-fast -j $Jobs -- --test-threads $Jobs" $testLog) {
        $failed = @(Select-String -LiteralPath $testLog -Pattern '^---- (\S+) stdout ----' | ForEach-Object { $_.Matches[0].Groups[1].Value })
        if ($failed) { "$($failed.Count) failed: $($failed -join ', ')" } else { "failed to build: see the log" }
    } else { "all passed" }

    $dist = Join-Path $out $tag
    if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
    $exe = Join-Path $Worktree "target\release\nous-relic-importer.exe"
    if (Invoke-Cargo "run --release -q -p nri-release -- sign --key `"$Key`" --out `"$dist`" --base-url https://github.com/$slug/releases/download/$tag/ windows-x86_64=`"$exe`"" $signLog) {
        throw "signing failed: see $signLog"
    }

    $local = git -C $repo rev-parse --verify --quiet "refs/tags/$tag"
    if ($local -and $local -ne $commit) { throw "a local tag $tag already names another commit" }
    if (-not $local) { Invoke-Git -C $repo tag $tag $commit }
    Invoke-Git -C $repo push --quiet origin "refs/tags/$tag"

    $last = $published | Where-Object { (Get-Version $_) -and (git -C $repo rev-parse --verify --quiet "refs/tags/$_") } |
        Sort-Object { Get-Version $_ } | Select-Object -Last 1
    $changes = if ($last) { Invoke-Git -C $repo log --reverse --format="- %s" "$last..$commit" } else { @() }
    $notes = Join-Path $out "$tag-notes.md"
    [IO.File]::WriteAllText($notes, ((@(
        "A release candidate, for the test channel only: copies started with ``--test-channel`` install it. Every other copy",
        "keeps the latest full release until this one is released.", "",
        "Tests: $tests.") + @("", "Since $last`:" | Where-Object { $last }) + $changes) -join "`n") + "`n")
    $assets = Get-ChildItem -LiteralPath $dist -File | ForEach-Object { $_.FullName }
    Invoke-Gh release create $tag --repo $slug --verify-tag --prerelease --latest=false `
        --title "Nous Relic Importer $version (release candidate)" --notes-file $notes @assets | Out-Null

    $channels = @(Invoke-Gh release list --repo $slug --limit 200 --json tagName --jq '.[].tagName')
    if ($channels -notcontains $channel) {
        Invoke-Gh release create $channel --repo $slug --target $commit --prerelease --latest=false --title "Test channel" `
            --notes "The feed copies started with --test-channel read: always the newest release candidate. Not a release; download a release instead." | Out-Null
    }
    Invoke-Gh release upload $channel (Join-Path $dist "latest.json") --repo $slug --clobber | Out-Null

    Write-Log "Published $tag ($short) as a pre-release; the test channel now offers it. Tests: $tests."
    Send-Notification "Nous Relic Importer $version is on the test channel" "Copies on the test channel install it within 15 minutes. Tests: $tests."
} catch {
    Write-Log "Failed: $_"
    if (-not $DryRun) { Send-Notification "Nous Relic Importer candidate not published" "$_" }
    exit 1
}
