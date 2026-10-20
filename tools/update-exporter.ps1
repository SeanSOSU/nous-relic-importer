# Copies Reliquary Archiver's inventory exporter (src/export, MIT) into core/src/export at a release tag.
# Run once per game patch, after the archiver has released for it:
#
#   .\tools\update-exporter.ps1 v0.20.0
#
# then set the `reliquary` tag the script prints on every `reliquary` entry (Cargo.toml, core/Cargo.toml,
# decoder/Cargo.toml) and rebuild. If the script reports that the archiver's build.rs changed, mirror the
# change in core/build.rs: that is where the game tables the exporter reads are downloaded.
param([Parameter(Mandatory = $true)][string]$Tag)
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$raw = "https://raw.githubusercontent.com/IceDynamix/reliquary-archiver/$Tag"
$files = @(
    "src/export/mod.rs",
    "src/export/database.rs",
    "src/export/fribbels/mod.rs",
    "src/export/fribbels/converters.rs",
    "src/export/fribbels/exporter.rs",
    "src/export/fribbels/handlers.rs",
    "src/export/fribbels/models.rs",
    "src/export/fribbels/utils.rs"
)

foreach ($file in $files) {
    $target = Join-Path $root "core/$file"
    New-Item -ItemType Directory -Force (Split-Path -Parent $target) | Out-Null
    Invoke-WebRequest -UseBasicParsing "$raw/$file" -OutFile $target
}
[System.IO.File]::WriteAllText((Join-Path $root "core/src/export/VENDORED"), @"
Copied from https://github.com/IceDynamix/reliquary-archiver at tag $Tag (MIT, IceDynamix and contributors).
Do not edit these files: re-run tools/update-exporter.ps1 <tag> instead.

"@)

$cargo = (Invoke-WebRequest -UseBasicParsing "$raw/Cargo.toml").Content
$reliquary = [regex]::Match($cargo, 'reliquary = \{[^}]*tag = "([^"]+)"').Groups[1].Value
Write-Host "Archiver $Tag builds on reliquary ${reliquary}: set that tag on both reliquary entries in Cargo.toml."

$build = (Invoke-WebRequest -UseBasicParsing "$raw/build.rs").Content
$hash = [System.BitConverter]::ToString(
    [System.Security.Cryptography.SHA256]::Create().ComputeHash([System.Text.Encoding]::UTF8.GetBytes($build))
).Replace("-", "").Substring(0, 16)
$hashFile = Join-Path $root "tools/archiver-build-rs.sha"
$previous = if (Test-Path $hashFile) { (Get-Content $hashFile -Raw).Trim() } else { "" }
if ($previous -and $previous -ne $hash) {
    Write-Warning "The archiver's build.rs changed since the last update: compare its downloads with ours ($raw/build.rs)."
}
[System.IO.File]::WriteAllText($hashFile, "$hash`n")
