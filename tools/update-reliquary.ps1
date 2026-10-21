# Moves the app to a game patch's protocol. Every patch renumbers the game's commands and reshuffles its messages,
# and Reliquary (https://github.com/IceDynamix/reliquary, MIT) tags a version that decodes the new ones. This sets
# that tag on every `reliquary` entry (Cargo.toml, core/Cargo.toml's dependency and build-dependency,
# decoder/Cargo.toml), and Reliquary's own protobuf version on the capture tests' one:
#
#   .\tools\update-reliquary.ps1                       # the newest Reliquary tag
#   .\tools\update-reliquary.ps1 -Reliquary v25.0.0
#
# The inventory exporter in core/src/export is Reliquary Archiver's (MIT). It is copied again only when the archiver
# has released since core/src/export/VENDORED's tag with different exporter files. If the script warns that the
# archiver's build.rs changed, mirror the change in core/build.rs: that is where the game tables the exporter reads
# are downloaded. Needs the GitHub CLI (gh).
param([string]$Reliquary)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = Split-Path -Parent $PSScriptRoot
$archiverRaw = "https://raw.githubusercontent.com/IceDynamix/reliquary-archiver"

function Get-Version([string]$Tag) {
    if ($Tag -match '^v?(\d+)\.(\d+)\.(\d+)$') { [version]"$($Matches[1]).$($Matches[2]).$($Matches[3])" }
}

function Invoke-Gh {
    $output = gh @args
    if ($LASTEXITCODE) { throw "gh $args failed" }
    $output
}

function Get-Raw([string]$Url) { (Invoke-WebRequest -UseBasicParsing $Url).Content }

# Sets the second group of every $Pattern match in $Manifest to $Value; stops unless there are exactly $Count.
function Set-Entry([string]$Manifest, [string]$Pattern, [string]$Value, [int]$Count) {
    $path = Join-Path $root $Manifest
    $text = [IO.File]::ReadAllText($path)
    $found = [regex]::Matches($text, $Pattern).Count
    if ($found -ne $Count) { throw "$Manifest has $found entries matching $Pattern, not $Count" }
    [IO.File]::WriteAllText($path, [regex]::Replace($text, $Pattern, '${1}' + $Value + '${3}'))
}

# The exporter's files at an archiver tag, one "path blob-hash" line each.
function Get-Exporter([string]$Tag) {
    ((Invoke-Gh api "repos/IceDynamix/reliquary-archiver/git/trees/${Tag}?recursive=1") -join "`n" | ConvertFrom-Json).tree |
        Where-Object { $_.type -eq "blob" -and $_.path.StartsWith("src/export/") } |
        ForEach-Object { "$($_.path) $($_.sha)" }
}

if (-not $Reliquary) {
    $Reliquary = Invoke-Gh api --paginate repos/IceDynamix/reliquary/tags --jq '.[].name' |
        Where-Object { Get-Version $_ } | Sort-Object { Get-Version $_ } | Select-Object -Last 1
    if (-not $Reliquary) { throw "Reliquary has no version tags" }
}
$tag ='(?m)^(reliquary = \{[^}]*?\btag = ")([^"]*)(")'
$previous = [regex]::Match([IO.File]::ReadAllText((Join-Path $root "Cargo.toml")), $tag).Groups[2].Value

# The capture tests write Reliquary's messages through protobuf itself, so theirs must be Reliquary's exact version.
$protobuf = [regex]::Match((Get-Raw "https://raw.githubusercontent.com/IceDynamix/reliquary/$Reliquary/Cargo.toml"),
    '(?m)^protobuf = (?:\{[^}]*?\bversion = )?"([^"]+)"').Groups[1].Value
if (-not $protobuf) { throw "Reliquary $Reliquary's Cargo.toml names no protobuf version" }

Set-Entry "Cargo.toml" $tag $Reliquary 1
Set-Entry "core/Cargo.toml" $tag $Reliquary 2
Set-Entry "decoder/Cargo.toml" $tag $Reliquary 1
Set-Entry "Cargo.toml" '(?m)^(protobuf = ")([^"]*)(")' $protobuf 1
Write-Host "reliquary $previous -> $Reliquary on every entry, with protobuf $protobuf."

$vendoredFile = Join-Path $root "core/src/export/VENDORED"
$vendored = [regex]::Match([IO.File]::ReadAllText($vendoredFile), 'at tag (\S+) ').Groups[1].Value
$archiver = Invoke-Gh api repos/IceDynamix/reliquary-archiver/releases/latest --jq .tag_name
if ((Get-Version $archiver) -le (Get-Version $vendored)) {
    Write-Host "Exporter: Reliquary Archiver $vendored's, still the archiver's newest release."
    return
}

$files = @(Get-Exporter $archiver)
if (-not $files) { throw "Reliquary Archiver $archiver has no src/export: its exporter moved, so copy it by hand." }
if (($files -join "`n") -eq (@(Get-Exporter $vendored) -join "`n")) {
    Write-Host "Exporter: Reliquary Archiver $archiver's is the same as $vendored's, so it stays."
} else {
    Get-ChildItem (Join-Path $root "core/src/export") -Recurse -File | Where-Object Name -ne "VENDORED" | Remove-Item
    foreach ($file in $files) {
        $path = $file.Split(" ")[0]
        $target = Join-Path $root "core/$path"
        New-Item -ItemType Directory -Force (Split-Path -Parent $target) | Out-Null
        Invoke-WebRequest -UseBasicParsing "$archiverRaw/$archiver/$path" -OutFile $target
    }
    [IO.File]::WriteAllText($vendoredFile,
        "Copied from https://github.com/IceDynamix/reliquary-archiver at tag $archiver (MIT, IceDynamix and contributors).`n" +
        "Do not edit these files: tools/update-reliquary.ps1 copies them again when the archiver releases different ones.`n")
    $built = [regex]::Match((Get-Raw "$archiverRaw/$archiver/Cargo.toml"), 'reliquary = \{[^}]*tag = "([^"]+)"').Groups[1].Value
    Write-Host "Exporter: copied from Reliquary Archiver $archiver, which builds on reliquary $built."
    if ($built -ne $Reliquary) {
        Write-Warning "Reliquary Archiver $archiver's exporter was written for reliquary $built, not ${Reliquary}: check that it builds."
    }
}

$build = (Invoke-WebRequest -UseBasicParsing "$archiverRaw/$archiver/build.rs").RawContentStream.ToArray()
$hash = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($build)).Replace("-", "").Substring(0, 16)
$hashFile = Join-Path $root "tools/archiver-build-rs.sha"
$known = if (Test-Path $hashFile) { [IO.File]::ReadAllText($hashFile).Trim() } else { "" }
if ($known -and $known -ne $hash) {
    Write-Warning "Reliquary Archiver's build.rs changed since the last update: compare its downloads with core/build.rs ($archiverRaw/$archiver/build.rs)."
}
[IO.File]::WriteAllText($hashFile, "$hash`n")
