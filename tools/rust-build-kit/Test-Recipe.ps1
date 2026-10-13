$ErrorActionPreference = 'Stop'
foreach ($file in Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.ps1') {
    $tokens = $null; $errors = $null
    $null = [Management.Automation.Language.Parser]::ParseFile($file.FullName,[ref]$tokens,[ref]$errors)
    if ($errors.Count) { throw ($errors | Out-String) }
}
# Evaluate selected production functions with no compiler or SDK invocation.
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'Build-Common.ps1'),[ref]$tokens,[ref]$errors)
foreach ($name in @('Invoke-StepTool','Read-Record','Write-Record','Write-Snapshot','Verify-PreviewPackage','Copy-DependencyNotices')) {
    $node = $ast.Find({param($entry) $entry -is [Management.Automation.Language.FunctionDefinitionAst] -and $entry.Name -eq $name},$true)
    if (!$node) { throw "Missing production function: $name" }
    . ([scriptblock]::Create($node.Extent.Text))
}
function recipe_success { $global:LASTEXITCODE = 0 }
function recipe_failure { $global:LASTEXITCODE = 17 }
Invoke-StepTool 'recipe_success' @()
$failed = $false
try { Invoke-StepTool 'recipe_failure' @() } catch {
    if ($_.Exception.Message -notlike '*exit code 17*') { throw }
    $failed = $true
}
if (!$failed) { throw 'A failed command was accepted.' }
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('aetherdesk-rust-kit-test-' + [guid]::NewGuid().ToString('N'))
try {
    $stateRoot = Join-Path $temporary 'state'
    New-Item -ItemType Directory -Path $temporary | Out-Null
    $artifact = Join-Path $temporary 'artifact.txt'
    [IO.File]::WriteAllText($artifact,'verified fixture')
    Write-Record 3 'fingerprint' $artifact @($artifact) 'test.log'
    if (!(Read-Record 3 'fingerprint')) { throw 'Verified completion record was rejected.' }
    if (Read-Record 3 'different') { throw 'A changed-source record was accepted.' }
    [IO.File]::WriteAllText($artifact,'changed')
    if (Read-Record 3 'fingerprint') { throw 'A corrupted artifact was accepted.' }
    Remove-Item -LiteralPath $artifact
    if (Read-Record 3 'fingerprint') { throw 'A missing artifact was accepted.' }
    $sourceRoot = Join-Path $temporary 'source'
    $outputRoot = Join-Path $temporary 'output'
    $manifest = Join-Path $sourceRoot 'ui\rust\Cargo.toml'
    New-Item -ItemType Directory -Force -Path (Split-Path $manifest -Parent) | Out-Null
    [IO.File]::WriteAllText($manifest, "[workspace.package]" + [Environment]::NewLine + 'version = "0.1.2"')
    $lock = Join-Path $sourceRoot 'ui\rust\Cargo.lock'
    $lockText = @('[ [unused] ]','[[package]]','name = "ad-desktop"','version = "0.1.2"',
        '[[package]]','name = "ad-desktop-model"','version = "0.1.2"','[[package]]','name = "other"','version = "0.1.2"') -join [Environment]::NewLine
    [IO.File]::WriteAllText($lock,$lockText)
    function Get-RustSourceFiles { 'ui/rust/Cargo.toml'; 'ui/rust/Cargo.lock' }
    $release = [pscustomobject]@{ normalized='0.2.0' }
    $snapshot = Write-Snapshot 'test-fingerprint'
    if ([IO.File]::ReadAllText((Join-Path $snapshot 'ui\rust\Cargo.toml')) -notmatch 'version = "0\.2\.0"') {
        throw 'Snapshot did not use release.json version.'
    }
    if ([IO.File]::ReadAllText($manifest) -notmatch 'version = "0\.1\.2"') { throw 'Snapshot modified original source.' }
    $snapshotLock = [IO.File]::ReadAllText((Join-Path $snapshot 'ui\rust\Cargo.lock'))
    if ([regex]::Matches($snapshotLock,'version = "0\.2\.0"').Count -ne 2 -or
        [regex]::Matches($snapshotLock,'version = "0\.1\.2"').Count -ne 1) { throw 'Lock version normalization modified external dependencies.' }
    $package = Join-Path $temporary 'package'
    New-Item -ItemType Directory -Path $package | Out-Null
    '{"featureEquivalent":false,"runtime":"native-rust","product":"AetherDesk Rust preview"}' |
        Set-Content -LiteralPath (Join-Path $package 'build-info.json') -Encoding UTF8
    $image = New-Object byte[] 80
    $image[0]=77; $image[1]=90; $image[60]=64
    $image[64]=80; $image[65]=69; $image[68]=100; $image[69]=134
    $exe = Join-Path $package 'aetherdesk-rust-preview.exe'
    [IO.File]::WriteAllBytes($exe,$image)
    foreach ($name in @('Cargo.lock','rust-migration.json','READ-ME-FIRST.txt','THIRD-PARTY-NOTICES.txt')) {
        [IO.File]::WriteAllText((Join-Path $package $name),'offline fixture')
    }
    # Execute the real notices writer, not a hand-created placeholder.
    Remove-Item -LiteralPath (Join-Path $package 'THIRD-PARTY-NOTICES.txt')
    $dependencies = @()
    foreach ($name in @('first-dependency','last-dependency')) {
        $folder = Join-Path $temporary $name
        New-Item -ItemType Directory -Path $folder | Out-Null
        [IO.File]::WriteAllText((Join-Path $folder 'Cargo.toml'),'[package]')
        [IO.File]::WriteAllText((Join-Path $folder 'LICENSE-MIT'),('license for ' + $name))
        $dependencies += [pscustomobject]@{
            name=$name;version='1.0.0';license='MIT';
            repository=('https://example.test/' + $name);
            manifest_path=(Join-Path $folder 'Cargo.toml')
        }
    }
    $fixtureMetadata = [pscustomobject]@{packages=$dependencies} | ConvertTo-Json -Depth 5 -Compress
    function cargo.exe { $global:LASTEXITCODE=0; $fixtureMetadata }
    Copy-DependencyNotices 'offline-manifest' $package
    $noticePath = Join-Path $package 'THIRD-PARTY-NOTICES.txt'
    if (!(Test-Path -LiteralPath $noticePath -PathType Leaf)) {
        throw 'Notices were not written at the package root.'
    }
    $notices = [IO.File]::ReadAllText($noticePath)
    foreach ($name in @('first-dependency','last-dependency')) {
        if (!$notices.Contains($name + ' 1.0.0')) { throw 'Dependency omitted from root notices.' }
        $copied = Join-Path $package ('licenses\' + $name + '-1.0.0\LICENSE-MIT')
        if (!(Test-Path -LiteralPath $copied -PathType Leaf)) { throw 'Dependency license was not copied.' }
        if (Test-Path -LiteralPath (Join-Path (Split-Path $copied -Parent) 'THIRD-PARTY-NOTICES.txt')) {
            throw 'Notices were incorrectly written under a dependency directory.'
        }
    }
    Verify-PreviewPackage $package
    $image[68]=76; $image[69]=1
    [IO.File]::WriteAllBytes($exe,$image)
    $rejected=$false
    try { Verify-PreviewPackage $package } catch {
        if ($_.Exception.Message -ne 'Preview executable must be Windows x64.') { throw }
        $rejected=$true
    }
    if (!$rejected) { throw 'An x86 executable was accepted as x64.' }

} finally {
    $full = [IO.Path]::GetFullPath($temporary)
    $prefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$full.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($full) -notlike 'aetherdesk-rust-kit-test-*') { throw 'Unsafe fixture cleanup.' }
    if (Test-Path -LiteralPath $full) { Remove-Item -LiteralPath $full -Recurse -Force }
}
$worker = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'Build-Worker.ps1'))
foreach ($required in @("'test','--locked'","'build','--locked','--release'","aetherdesk-rust-preview","Check-Migration.ps1","featureEquivalent="+ '$false')) {
    if (!$worker.Contains($required)) { throw "Preview recipe omitted: $required" }
}
if ($worker -match "Invoke-StepTool 'flutter|Invoke-StepTool 'node|Invoke-StepTool 'python|LangGraph") { throw 'Preview recipe must not build the legacy Flutter/Python app.' }
$global:LASTEXITCODE = 0
Write-Host 'PASS: Rust preview recipe syntax, failure propagation, resume hashes, isolated versions/lock, root dependency notices and x64 package validation.'
