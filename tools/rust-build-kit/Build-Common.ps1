$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$env:PSModulePath = "$PSHOME\Modules;$env:ProgramFiles\WindowsPowerShell\Modules"
Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Utility\Microsoft.PowerShell.Utility.psd1') -ErrorAction Stop
$recipeRoot = $PSScriptRoot
$sourceRoot = [IO.Path]::GetFullPath((Join-Path $recipeRoot '..\..'))
$outputRoot = [IO.Path]::GetFullPath($(if ($env:AETHERDESK_RUST_OUTPUT_ROOT) {
    $env:AETHERDESK_RUST_OUTPUT_ROOT
} else { Join-Path (Split-Path $sourceRoot -Parent) 'AetherDesk-Rust-output' })).TrimEnd('\')
if ($outputRoot -eq [IO.Path]::GetPathRoot($outputRoot).TrimEnd('\') -or
    $outputRoot.Equals($sourceRoot, [StringComparison]::OrdinalIgnoreCase) -or
    $outputRoot.StartsWith($sourceRoot + '\', [StringComparison]::OrdinalIgnoreCase) -or
    $sourceRoot.StartsWith($outputRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Choose a Rust output folder separate from the source, not a drive root.'
}
$stateRoot = Join-Path $outputRoot 'state'
$logsRoot = Join-Path $outputRoot 'logs'
$packagesRoot = Join-Path $outputRoot 'packages'
$targetRoot = Join-Path $outputRoot 'cache\target'
$dependencyRoot = Join-Path $outputRoot 'cache\cargo-home'
$release = & (Join-Path $sourceRoot 'tools\build-kit\Release-Version.ps1') -Repository $sourceRoot

function Get-RustSourceFiles {
    $paths = [Collections.Generic.List[string]]::new()
    foreach ($path in Get-ChildItem -LiteralPath (Join-Path $sourceRoot 'ui\rust') -Recurse -File) {
        $relative = $path.FullName.Substring($sourceRoot.Length + 1).Replace('\','/')
        if ($relative -notmatch '(^|/)(target|build|\.git)/' -and $path.Extension -in '.rs','.toml','.json','.ps1','.md') {
            $paths.Add($relative)
        }
    }
    if (Test-Path -LiteralPath (Join-Path $sourceRoot 'ui\rust\Cargo.lock') -PathType Leaf) {
        $paths.Add('ui/rust/Cargo.lock')
    }
    foreach ($name in @('release.json','rust-migration.json','RUST-MIGRATION.md')) { $paths.Add($name) }
    foreach ($path in Get-ChildItem -LiteralPath $recipeRoot -File) {
        $paths.Add('tools/rust-build-kit/' + $path.Name)
    }
    [string[]]$result = @($paths | Sort-Object -Unique)
    [Array]::Sort($result,[StringComparer]::Ordinal)
    return $result
}
function Get-SourceFingerprint {
    $lines = [Collections.Generic.List[string]]::new()
    $lines.Add('source=' + $sourceRoot.ToLowerInvariant())
    $lines.Add('output=' + $outputRoot.ToLowerInvariant())
    foreach ($relative in Get-RustSourceFiles) {
        $lines.Add($relative + '=' + (Get-FileHash -LiteralPath (Join-Path $sourceRoot $relative) -Algorithm SHA256).Hash)
    }
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(($lines -join [Environment]::NewLine)))).Replace('-','').ToLowerInvariant() }
    finally { $sha.Dispose() }
}
function Invoke-StepTool([string]$Program,[string[]]$Arguments) {
    Write-Host ("Running: " + $Program + " " + ($Arguments -join ' '))
    & $Program @Arguments | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
function Get-Iscc {
    foreach ($path in @(
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
        (Join-Path ([Environment]::GetEnvironmentVariable('ProgramFiles(x86)')) 'Inno Setup 6\ISCC.exe')
    )) { if (Test-Path -LiteralPath $path -PathType Leaf) { return $path } }
    throw 'Inno Setup 6 is required for the preview installer. Install it or use Build-Portable-Only.cmd.'
}
function Initialize-Toolchain {
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    $env:Path = "$cargoBin;$env:SystemRoot\System32;$env:SystemRoot;$env:Path"
    foreach ($name in @('CARGO_BUILD_TARGET','CARGO_ENCODED_RUSTFLAGS','CARGO_ENCODED_RUSTDOCFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER')) {
        [Environment]::SetEnvironmentVariable($name,$null,'Process')
    }
    $vswhere = Join-Path ([Environment]::GetEnvironmentVariable('ProgramFiles(x86)')) 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (!(Test-Path -LiteralPath $vswhere)) { throw 'Install Visual Studio Build Tools with Desktop development with C++ and a Windows SDK.' }
    $installation = (& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or !$installation) { throw 'No installed x64 MSVC toolchain was found.' }
    $shell = Join-Path $installation 'Common7\Tools\Launch-VsDevShell.ps1'
    if (!(Test-Path -LiteralPath $shell)) { throw 'Visual Studio Developer PowerShell is missing.' }
    & $shell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Host
    $env:CARGO_HOME = $dependencyRoot
    $env:CARGO_TARGET_DIR = $targetRoot
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    foreach ($name in @('cargo.exe','rustc.exe','link.exe')) { Get-Command $name -CommandType Application -ErrorAction Stop | Out-Null }
    $version = (& rustc.exe --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $version -notmatch '^rustc (\d+)\.(\d+)\.') { throw 'The Rust compiler could not be checked.' }
    if ([int]$Matches[1] -lt 1 -or ([int]$Matches[1] -eq 1 -and [int]$Matches[2] -lt 88)) { throw 'Rust 1.88 or newer is required.' }
    Invoke-StepTool 'rustup.exe' @('target','add','x86_64-pc-windows-msvc')
    Write-Host "Rust toolchain: $version"
}
function Read-Record([int]$Step,[string]$Fingerprint) {
    $path = Join-Path $stateRoot ('step-{0:D2}.json' -f $Step)
    if (!(Test-Path -LiteralPath $path -PathType Leaf)) { return $null }
    try {
        $record = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
        if ($record.fingerprint -ne $Fingerprint -or !$record.artifacts) { return $null }
        foreach ($artifact in $record.artifacts) {
            if (!(Test-Path -LiteralPath $artifact.path -PathType Leaf) -or
                (Get-FileHash -LiteralPath $artifact.path -Algorithm SHA256).Hash -ne $artifact.sha256) { return $null }
        }
        return $record
    } catch { return $null }
}
function Require-Record([int]$Step,[string]$Fingerprint) {
    $record = Read-Record $Step $Fingerprint
    if (!$record) { throw "Step $Step is missing or changed. Run Build-Everything.cmd again." }
    return $record
}
function Write-Record([int]$Step,[string]$Fingerprint,[string]$Output,[string[]]$Files,[string]$Log) {
    if (!$Files.Count) { throw 'Cannot record a step without verified artifacts.' }
    $artifacts = @($Files | ForEach-Object {
        if (!(Test-Path -LiteralPath $_ -PathType Leaf)) { throw "Missing artifact: $_" }
        [ordered]@{path=$_;sha256=(Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash}
    })
    New-Item -ItemType Directory -Force -Path $stateRoot | Out-Null
    [ordered]@{step=$Step;fingerprint=$Fingerprint;output=$Output;log=$Log;completed=(Get-Date -Format o);artifacts=$artifacts} |
        ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $stateRoot ('step-{0:D2}.json' -f $Step)) -Encoding UTF8
}
function Write-Snapshot([string]$Fingerprint) {
    $destination = Join-Path $outputRoot ('source\' + $Fingerprint)
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    foreach ($relative in Get-RustSourceFiles) {
        $target = Join-Path $destination $relative
        New-Item -ItemType Directory -Force -Path (Split-Path $target -Parent) | Out-Null
        Copy-Item -LiteralPath (Join-Path $sourceRoot $relative) -Destination $target -Force
    }
    $workspace = Join-Path $destination 'ui\rust\Cargo.toml'
    $text = [IO.File]::ReadAllText($workspace)
    $pattern = '(\[workspace\.package\][\s\S]*?version\s*=\s*)"[^"]+"'
    if ([regex]::Matches($text,$pattern).Count -ne 1) { throw 'Rust workspace version was not found exactly once.' }
    $normalizedVersion = $release.normalized
    $text = [regex]::Replace($text,$pattern,[Text.RegularExpressions.MatchEvaluator]{
        param($match) $match.Groups[1].Value + '"' + $normalizedVersion + '"'
    })
    [IO.File]::WriteAllText($workspace,$text,[Text.UTF8Encoding]::new($false))
    $lock = Join-Path $destination 'ui\rust\Cargo.lock'
    if (Test-Path -LiteralPath $lock -PathType Leaf) {
        $lockText = [IO.File]::ReadAllText($lock)
        $localVersion = '(\[\[package\]\]\s+name\s*=\s*"(?:ad-desktop|ad-desktop-model)"\s+version\s*=\s*)"[^"]+"'
        if ([regex]::Matches($lockText,$localVersion).Count -ne 2) {
            throw 'The supplied lockfile does not contain both native workspace packages.'
        }
        $lockText = [regex]::Replace($lockText,$localVersion,[Text.RegularExpressions.MatchEvaluator]{
            param($match) $match.Groups[1].Value + '"' + $normalizedVersion + '"'
        })
        [IO.File]::WriteAllText($lock,$lockText,[Text.UTF8Encoding]::new($false))
    }
    return $destination
}

function Copy-DependencyNotices([string]$Manifest,[string]$Destination) {
    Write-Host 'Collecting dependency license declarations and supplied license files.'
    $raw = & cargo.exe metadata --locked --format-version 1 --manifest-path $Manifest
    if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata collection failed.' }
    $metadata = ($raw | Out-String) | ConvertFrom-Json
    $notices = [Collections.Generic.List[string]]::new()
    $licenseRoot = Join-Path $Destination 'licenses'
    New-Item -ItemType Directory -Force -Path $licenseRoot | Out-Null
    foreach ($package in $metadata.packages | Sort-Object name,version) {
        $notices.Add($package.name + ' ' + $package.version)
        $notices.Add('Declared license: ' + $package.license)
        $notices.Add('Source: ' + $package.repository)
        $folder = Split-Path $package.manifest_path -Parent
        $packageLicenseDirectory = Join-Path $licenseRoot ($package.name + '-' + $package.version)
        foreach ($file in Get-ChildItem -LiteralPath $folder -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)([._-]|$)' }) {
            if ($file.Length -gt 2097152 -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw 'Unexpected dependency license file.'
            }
            New-Item -ItemType Directory -Force -Path $packageLicenseDirectory | Out-Null
            Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $packageLicenseDirectory $file.Name)
        }
        $notices.Add('')
    }
    [IO.File]::WriteAllLines((Join-Path $Destination 'THIRD-PARTY-NOTICES.txt'),$notices,[Text.UTF8Encoding]::new($false))
}
function Verify-PreviewPackage([string]$Package) {
    $info = Get-Content -LiteralPath (Join-Path $Package 'build-info.json') -Raw | ConvertFrom-Json
    if ($info.featureEquivalent -ne $false -or $info.runtime -ne 'native-rust' -or $info.product -ne 'AetherDesk Rust preview') {
        throw 'Package must be labelled as the incomplete native Rust preview.'
    }
    $exe = Join-Path $Package 'aetherdesk-rust-preview.exe'
    $stream = [IO.File]::OpenRead($exe)
    try {
        $header = New-Object byte[] 64
        if ($stream.Read($header,0,64) -ne 64 -or $header[0] -ne 77 -or $header[1] -ne 90) {
            throw 'Preview executable has no valid Windows header.'
        }
        $offset = [BitConverter]::ToInt32($header,60)
        if ($offset -lt 64 -or $offset -gt $stream.Length - 6) { throw 'Preview executable has an invalid PE offset.' }
        [void]$stream.Seek($offset,[IO.SeekOrigin]::Begin)
        $signature = New-Object byte[] 6
        if ($stream.Read($signature,0,6) -ne 6 -or $signature[0] -ne 80 -or $signature[1] -ne 69 -or
            $signature[2] -ne 0 -or $signature[3] -ne 0 -or [BitConverter]::ToUInt16($signature,4) -ne 34404) {
            throw 'Preview executable must be Windows x64.'
        }
    } finally { $stream.Dispose() }
    foreach ($name in @('Cargo.lock','rust-migration.json','READ-ME-FIRST.txt','THIRD-PARTY-NOTICES.txt')) {
        if (!(Test-Path -LiteralPath (Join-Path $Package $name) -PathType Leaf)) { throw "Missing preview package file: $name" }
    }
}

function Get-DependencyFingerprint {
    $lines = @('lock-cache-v1','version=' + $release.normalized)
    foreach ($relative in Get-RustSourceFiles | Where-Object { $_ -like 'ui/rust/*Cargo.toml' }) {
        $lines += $relative + '=' + (Get-FileHash -LiteralPath (Join-Path $sourceRoot $relative) -Algorithm SHA256).Hash
    }
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(($lines -join [Environment]::NewLine)))).Replace('-','').ToLowerInvariant() }
    finally { $sha.Dispose() }
}
