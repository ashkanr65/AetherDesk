[CmdletBinding()]
param([switch]$Release)
$ErrorActionPreference = 'Stop'
$repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$manifest = Get-Content -LiteralPath (Join-Path $repository 'rust-migration.json') -Raw | ConvertFrom-Json
if ($manifest.schemaVersion -ne 1 -or !$manifest.features -or !$manifest.referenceFiles) {
    throw 'The migration parity manifest is invalid.'
}
foreach ($entry in $manifest.referenceFiles) {
    if ([IO.Path]::IsPathRooted($entry.path) -or $entry.path -match '(^|/)\.\.(/|$)') {
        throw 'Invalid migration reference path.'
    }
    $path = Join-Path $repository $entry.path
    if (!(Test-Path -LiteralPath $path -PathType Leaf) -or
        (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $entry.sha256) {
        throw "Migration reference changed: $($entry.path)"
    }
}
foreach ($feature in $manifest.features) {
    if (!(Test-Path -LiteralPath (Join-Path $repository $feature.reference) -PathType Leaf)) {
        throw "Missing feature reference: $($feature.id)"
    }
}
$dart = Get-Content -LiteralPath (Join-Path $repository 'ui\flutter\ad_remote_desktop\lib\remote_desktop_view.dart') -Raw
$rust = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'crates\ad-desktop-model\src\input.rs') -Raw
$dartMap = [regex]::Match($dart, 'const codes = <int, int>\{(?<map>[\s\S]*?)\};').Groups['map'].Value
$original = @([regex]::Matches($dartMap, '(\d+):\s*(\d+)'))
$ported = @([regex]::Matches($rust, '(\d+)\s*=>\s*Some\((\d+)\)'))
if ($original.Count -ne $ported.Count) { throw 'Rust physical key map lost entries.' }
foreach ($item in $original) {
    $matched = @($ported | Where-Object {
        $_.Groups[1].Value -eq $item.Groups[1].Value -and
        $_.Groups[2].Value -eq $item.Groups[2].Value
    })
    if ($matched.Count -ne 1) { throw 'Rust physical key map differs from the reference.' }
}
$inventory = Get-Content -LiteralPath (Join-Path $repository $manifest.featureInventory) -Raw | ConvertFrom-Json
if ($inventory.schemaVersion -ne 1 -or !$inventory.features -or !$inventory.sourceFiles) { throw 'The detailed feature inventory is missing.' }
$ids = @{}
foreach ($feature in $inventory.features) {
    if (!$feature.id -or $ids.ContainsKey($feature.id)) { throw 'Feature inventory contains a duplicate or missing ID.' }
    $ids[$feature.id] = $true
    if (!$feature.acceptance) { throw "Feature has no acceptance criteria: $($feature.id)" }
    foreach ($path in @($feature.references) + @($feature.implementation)) {
        if ([IO.Path]::IsPathRooted($path) -or $path -match '(^|[\\/])\.\.([\\/]|$)' -or
            !(Test-Path -LiteralPath (Join-Path $repository $path) -PathType Leaf)) { throw "Invalid feature evidence path: $path" }
    }
}
if ($Release -and (!$manifest.replacementReady -or !$inventory.replacementReady -or
    @($inventory.features | Where-Object { $_.status -notin @('verified','preserved_unavailable') }).Count -or
    @($manifest.features | Where-Object { $_.status -ne 'verified' }).Count)) {
    throw 'Rust release blocked: full feature parity has not been verified.'
}
Write-Host "PASS: frozen references and $($original.Count) physical key mappings. Replacement ready: $($manifest.replacementReady)."
Write-Host "PASS: $($inventory.features.Count) behavior groups and $($inventory.sourceFiles.Count) original source files inventoried."
& (Join-Path $repository 'tools/rust-build-kit/Check-Regressions.ps1') -Repository $repository -CheckReferences
