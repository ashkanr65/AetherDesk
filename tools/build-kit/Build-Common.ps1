$ErrorActionPreference = 'Stop'
# A cmd.exe hop can preserve PowerShell 7's PSModulePath in Windows PowerShell
# 5.1. Resolve this process's built-in modules before using Get-FileHash, etc.
$env:PSModulePath = "$PSHOME\Modules;$env:ProgramFiles\WindowsPowerShell\Modules"
Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Utility\Microsoft.PowerShell.Utility.psd1') -ErrorAction Stop
$kit = $PSScriptRoot
$sourceRepo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$locations = & (Join-Path $kit 'Resolve-Build-Paths.ps1') -Repository $sourceRepo
$repo = $locations.build
$outputRoot = $locations.output
$kitOutput = $locations.kit
$stateDir = Join-Path $kitOutput 'state'
$toolchain = Get-Content -LiteralPath (Join-Path $kit 'toolchain.json') -Raw | ConvertFrom-Json
$versionPython = & (Join-Path $kit 'Resolve-Python.ps1')
$env:AETHERDESK_PYTHON = $versionPython
if (!$env:AETHERDESK_NODE) {
    $nodeCommand = Get-Command node.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($nodeCommand) { $env:AETHERDESK_NODE = $nodeCommand.Source }
}
& $versionPython -X utf8 (Join-Path $sourceRepo 'tools\version.py') | Out-Host
if ($LASTEXITCODE -ne 0) { throw 'Release version synchronization failed.' }
$git = 'C:\Program Files\Git\cmd\git.exe'

function Get-SourceFiles {
    $files = & $git -c core.quotepath=false -C $sourceRepo ls-files --cached --others --exclude-standard
    if ($LASTEXITCODE -ne 0) { throw "Cannot read source files in $sourceRepo" }
    # Cargo.lock is intentionally ignored in older source snapshots, but is
    # mandatory for the kit's locked Linux and Windows builds.
    $files = @($files) + @('core/rust/Cargo.lock')
    [string[]]$snapshot = @($files | Where-Object {
        $_ -notmatch '(^|/)(target|build|native-desktop-tests|\.dart_tool|node_modules)/' -and
        $_ -notlike 'ui/flutter/ad_remote_desktop/assets/editor/*' -and
        (Test-Path -LiteralPath (Join-Path $sourceRepo $_) -PathType Leaf)
    } | Sort-Object -Unique)
    # .NET Framework and modern .NET use different culture-sensitive ordering.
    [Array]::Sort($snapshot, [StringComparer]::Ordinal)
    return $snapshot
}

function Copy-BuildSourceFile([string]$Source, [string]$Destination) {
    if ([IO.Path]::GetExtension($Source) -in @('.sh', '.patch')) {
        # The source checkout may predate .gitattributes or have been edited
        # with Windows line endings. Linux shell scripts and patch input must
        # remain LF even when copying uncommitted working files into the build.
        $contents = [IO.File]::ReadAllText($Source).Replace("`r`n", "`n")
        [IO.File]::WriteAllText($Destination, $contents, [Text.UTF8Encoding]::new($false))
    } else {
        Copy-Item -LiteralPath $Source -Destination $Destination -Force
    }
}

function Get-BuildFingerprint {
    if (!(Test-Path -LiteralPath (Join-Path $sourceRepo 'core\rust\Cargo.lock'))) {
        throw 'The source Cargo.lock is missing. Generate it in the source before starting a locked build.'
    }
    $lines = [Collections.Generic.List[string]]::new()
    # Completion records belong to their actual folders as well as their source.
    # Moving the kit or changing an override must not resume an old output.
    foreach ($location in @($sourceRepo, $repo, $outputRoot, $kitOutput)) {
        $lines.Add('location=' + [IO.Path]::GetFullPath($location).TrimEnd('\').ToLowerInvariant())
    }
    foreach ($relative in Get-SourceFiles) {
        $hash = (Get-FileHash -LiteralPath (Join-Path $sourceRepo $relative) -Algorithm SHA256).Hash
        $lines.Add("source/$relative=$hash")
    }
    [string[]]$kitFiles = @(Get-ChildItem -LiteralPath $kit -File |
        Where-Object { $_.Extension -in '.ps1', '.sh', '.py', '.cmd' } |
        ForEach-Object { $_.FullName })
    [Array]::Sort($kitFiles, [StringComparer]::Ordinal)
    foreach ($path in $kitFiles) {
        $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
        $lines.Add("kit/$([IO.Path]::GetFileName($path))=$hash")
    }
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(($lines -join "`n")))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Read-StepRecord([int]$Number, [string]$Fingerprint) {
    $path = Join-Path $stateDir ('step-{0:D2}.json' -f $Number)
    if (!(Test-Path -LiteralPath $path)) { return $null }
    try { $record = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json } catch { return $null }
    if ($record.fingerprint -ne $Fingerprint) { return $null }
    foreach ($artifact in $record.artifacts) {
        if (!(Test-Path -LiteralPath $artifact.path -PathType Leaf)) { return $null }
        if ((Get-FileHash -LiteralPath $artifact.path -Algorithm SHA256).Hash -ne $artifact.sha256) { return $null }
    }
    if ($record.output -and !(Test-Path -LiteralPath $record.output)) { return $null }
    return $record
}

function Reset-ProcessBuildEnvironment {
    # Do not inherit a previous Developer PowerShell's accumulated tool paths.
    foreach ($item in Get-ChildItem Env: | Where-Object {
        $_.Name -match '^(VSCMD_|__VSCMD|VSINSTALLDIR$|VCINSTALLDIR$|VCTools|WindowsSdk|WindowsSDK|WindowsLibPath$|UniversalCRT|UCRTVersion$|DevEnvDir$|INCLUDE$|LIB$|LIBPATH$|Platform$|VisualStudioVersion$|VS[0-9]+COMNTOOLS$|GSTREAMER_|GST_|PKG_CONFIG|CARGO_TARGET_DIR$|CMAKE_GENERATOR$|CMAKE_GENERATOR_PLATFORM$|CC$|CXX$)'
    }) { [Environment]::SetEnvironmentVariable($item.Name, $null, 'Process') }
    $env:Path = @('C:\Program Files\Git\cmd', "$env:SystemRoot\System32", $env:SystemRoot,
        "$env:SystemRoot\System32\Wbem", "$env:SystemRoot\System32\WindowsPowerShell\v1.0",
        "$env:SystemRoot\System32\OpenSSH", "$env:USERPROFILE\.cargo\bin",
        (Join-Path $env:USERPROFILE 'develop\flutter\bin')) -join ';'
    $env:PATHEXT = '.COM;.EXE;.BAT;.CMD'
    $env:ComSpec = "$env:SystemRoot\System32\cmd.exe"
    $env:CI = 'true'
    $env:FLUTTER_SUPPRESS_ANALYTICS = 'true'
    $env:DART_SUPPRESS_ANALYTICS = 'true'
}
