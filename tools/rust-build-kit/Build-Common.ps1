$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$env:PSModulePath = "$PSHOME\Modules;$env:ProgramFiles\WindowsPowerShell\Modules"
Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Utility\Microsoft.PowerShell.Utility.psd1') -ErrorAction Stop
$recipeRoot = $PSScriptRoot
$sourceRoot = [IO.Path]::GetFullPath((Join-Path $recipeRoot '..\..'))
$outputRoot = [IO.Path]::GetFullPath($(if ($env:AETHERDESK_RUST_OUTPUT_ROOT) {
    $env:AETHERDESK_RUST_OUTPUT_ROOT
} else { Join-Path (Split-Path $sourceRoot -Parent) 'AetherDesk-Rust-output' })).TrimEnd('\')
$frozenFingerprint = $env:AETHERDESK_RUST_FROZEN_FINGERPRINT
$frozenWorker = $frozenFingerprint -match '^[a-f0-9]{64}$' -and
    $sourceRoot.Equals((Join-Path $outputRoot ('source\'+$frozenFingerprint)),[StringComparison]::OrdinalIgnoreCase)
if ($frozenFingerprint -and !$frozenWorker) { throw 'Frozen workers must run from their recorded source snapshot.' }
if ($outputRoot -eq [IO.Path]::GetPathRoot($outputRoot).TrimEnd('\') -or
    $outputRoot.Equals($sourceRoot, [StringComparison]::OrdinalIgnoreCase) -or
    $outputRoot.StartsWith($sourceRoot + '\', [StringComparison]::OrdinalIgnoreCase) -or
    ($sourceRoot.StartsWith($outputRoot + '\', [StringComparison]::OrdinalIgnoreCase) -and !$frozenWorker)) {
    throw 'Choose a Rust output folder separate from the source, not a drive root.'
}
$stateRoot = Join-Path $outputRoot 'state'
$logsRoot = Join-Path $outputRoot 'logs'
$packagesRoot = Join-Path $outputRoot 'packages'
$targetRoot = Join-Path $outputRoot 'cache\target'
$dependencyRoot = Join-Path $outputRoot 'cache\cargo-home'
$release = & (Join-Path $sourceRoot 'tools\build-kit\Release-Version.ps1') -Repository $sourceRoot

function Format-BuildDuration([TimeSpan]$Elapsed) {
    '{0:00}:{1:00}:{2:00}' -f [math]::Floor($Elapsed.TotalHours),$Elapsed.Minutes,$Elapsed.Seconds
}
function Get-BuildStageName([int]$Step) {
    switch ($Step) {
        0 { 'Preflight checks' }
        1 { 'Source snapshot' }
        2 { 'Windows and Linux tests' }
        3 { 'Windows application' }
        4 { 'Runtime packaging and X11 verification' }
        5 { 'Portable ZIP' }
        6 { 'Installer' }
    }
}
function Write-BuildTimingReport([string]$Path,[DateTimeOffset]$Started,[TimeSpan]$Elapsed,[string]$Status,[object[]]$Steps,[bool]$KeepCache) {
    New-Item -ItemType Directory -Force -Path (Split-Path $Path -Parent) | Out-Null
    [ordered]@{
        schemaVersion=1; startedUtc=$Started.ToString('o'); updatedUtc=[DateTimeOffset]::UtcNow.ToString('o')
        status=$Status; elapsed=(Format-BuildDuration $Elapsed); elapsedSeconds=[math]::Round($Elapsed.TotalSeconds,3)
        keepBuildCache=$KeepCache; steps=@($Steps)
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $Path -Encoding UTF8
}

function Reset-BuildOutput([string]$OutputDirectory,[string]$SourceDirectory,[switch]$AfterSuccess,[switch]$KeepCache) {
    $root = [IO.Path]::GetFullPath($OutputDirectory).TrimEnd('\')
    $source = [IO.Path]::GetFullPath($SourceDirectory).TrimEnd('\')
    if ($root -eq [IO.Path]::GetPathRoot($root).TrimEnd('\') -or
        $root.Equals($source,[StringComparison]::OrdinalIgnoreCase) -or
        $root.StartsWith($source + '\',[StringComparison]::OrdinalIgnoreCase) -or
        $source.StartsWith($root + '\',[StringComparison]::OrdinalIgnoreCase)) {
        throw 'Cleanup requires an output folder separate from the source, not a drive root.'
    }
    # Check every existing ancestor before traversing the output. A junction
    # could otherwise redirect an apparently safe output path into the source.
    $ancestor = $root
    while ($ancestor) {
        if (Test-Path -LiteralPath $ancestor) {
            $item = Get-Item -LiteralPath $ancestor -Force
            if (!$item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw "Cleanup refuses a non-directory or linked output ancestor: $ancestor"
            }
        }
        $ancestor = Split-Path -Path $ancestor -Parent
    }
    # Only recognized generated artifacts are disposable. Successful builds also
    # release caches; completed packages/evidence and unknown user files remain.
    $names = if ($AfterSuccess) { @('agent','installer','source','state','cache','preflight.json','model-tests.json') } else { @('agent','installer','logs','packages','reports','source','state',
        'preflight.json','model-tests.json','LATEST-FOLDER.txt',
        'LATEST-ZIP.txt','LATEST-INSTALLER.txt',
        'LATEST-PREVIEW-FOLDER.txt','LATEST-PREVIEW-ZIP.txt','LATEST-PREVIEW-INSTALLER.txt') }
    if ($AfterSuccess -and $KeepCache) { $names = @($names | Where-Object { $_ -ne 'cache' }) }
    $targets = [Collections.Generic.List[string]]::new()
    foreach ($name in $names) {
        $target = [IO.Path]::GetFullPath((Join-Path $root $name))
        if (!$target.StartsWith($root + '\',[StringComparison]::OrdinalIgnoreCase)) {
            throw "Cleanup target is outside the output folder: $target"
        }
        if (Test-Path -LiteralPath $target) { $targets.Add($target) }
    }
    # Validate all trees before deleting any of them; never recurse into links.
    $pending = [Collections.Generic.Stack[string]]::new()
    foreach ($target in $targets) { $pending.Push($target) }
    while ($pending.Count) {
        $item = Get-Item -LiteralPath $pending.Pop() -Force
        if (!$item.FullName.StartsWith($root + '\',[StringComparison]::OrdinalIgnoreCase) -or
            ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Cleanup refuses an outside or linked artifact: $($item.FullName)"
        }
        if ($item.PSIsContainer) {
            foreach ($child in Get-ChildItem -LiteralPath $item.FullName -Force) { $pending.Push($child.FullName) }
        }
    }
    if ($AfterSuccess -and $KeepCache) { Write-Host "Removing staging files: $root (keeping build caches, packages, logs and reports)." }
    elseif ($AfterSuccess) { Write-Host "Removing temporary build files and caches: $root (keeping packages, logs, reports and latest pointers)." }
    else { Write-Host "Cleaning old generated build outputs: $root (keeping dependency and compiler caches until the build succeeds)." }
    foreach ($target in $targets) { Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction Stop }
}

function Clear-NativeAgentBuildCache {
    $distribution = if ($env:AETHERDESK_WSL_DISTRIBUTION) { $env:AETHERDESK_WSL_DISTRIBUTION } else { 'Ubuntu' }
    # The label is applied only by this Rust recipe. Never prune an entire
    # container store or unrelated/old unlabelled images on the user's machine.
    Invoke-StepTool 'wsl.exe' @('-d',$distribution,'-u','root','--exec','podman','--storage-driver=vfs','image','prune','--all','--force','--filter','label=io.aetherdesk.rust-build-cache=true')
}

function Get-RustSourceFiles {
    $paths = [Collections.Generic.List[string]]::new()
    foreach ($path in Get-ChildItem -LiteralPath (Join-Path $sourceRoot 'ui\rust') -Recurse -File) {
        $relative = $path.FullName.Substring($sourceRoot.Length + 1).Replace('\','/')
        if ($relative -notmatch '(^|/)(target|build|\.git)/' -and $path.Extension -in '.rs','.toml','.json','.ps1','.md','.png','.ico') {
            $paths.Add($relative)
        }
    }
    if (Test-Path -LiteralPath (Join-Path $sourceRoot 'ui\rust\Cargo.lock') -PathType Leaf) {
        $paths.Add('ui/rust/Cargo.lock')
    }
    # Native UI path-depends on the production Rust backend. Keep its workspace
    # intact in isolated snapshots, including embedded resources, but no caches.
    $directories = [Collections.Generic.Stack[string]]::new()
    $directories.Push((Join-Path $sourceRoot 'core\rust'))
    while ($directories.Count) {
        foreach ($path in Get-ChildItem -LiteralPath $directories.Pop()) {
            if ($path.PSIsContainer) {
                if ($path.Name -notin @('target','build','.git') -and !($path.Attributes -band [IO.FileAttributes]::ReparsePoint)) { $directories.Push($path.FullName) }
            } else {
                $paths.Add($path.FullName.Substring($sourceRoot.Length + 1).Replace('\','/'))
            }
        }
    }
    # ad-ssh embeds this source even when the current connection does not use X11.
    $paths.Add('tools/x11-icons.py')
    foreach ($relative in @('agent/linux/Dockerfile.bundle','tools/package-linux-agent.sh','tools/build-xwayland.sh','tools/xwayland-monitors.patch','tools/build-kit/Bundle-X11.ps1','tools/build-kit/Release-Version.ps1','tools/build-kit/toolchain.json')) { $paths.Add($relative) }
    foreach ($name in @('release.json','update-config.json','rust-migration.json','RUST-MIGRATION.md','RUST-FEATURE-PARITY.md','rust-feature-inventory.json','rust-regressions.json')) { $paths.Add($name) }
    $paths.Add('tools/test_x11_icons.py')
    foreach ($path in Get-ChildItem -LiteralPath $recipeRoot -File) {
        $paths.Add('tools/rust-build-kit/' + $path.Name)
    }
    [string[]]$result = @($paths | Sort-Object -Unique)
    [Array]::Sort($result,[StringComparer]::Ordinal)
    return $result
}
function Get-SourceFingerprint {
    if ($env:AETHERDESK_RUST_FROZEN_FINGERPRINT) {
        $record = Require-Record 1 $env:AETHERDESK_RUST_FROZEN_FINGERPRINT
        if (!$sourceRoot.Equals($record.output,[StringComparison]::OrdinalIgnoreCase)) { throw 'Frozen snapshot does not match its stage record.' }
        return $env:AETHERDESK_RUST_FROZEN_FINGERPRINT
    }
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
function Get-RustTestWorkspaces {
    # A path dependency is not a member of the UI workspace. Cargo must test
    # backend packages through their own workspace to include dev-dependencies.
    [pscustomobject]@{name='ui';path='ui\rust';packages=@('ad-desktop-model','ad-desktop')}
    [pscustomobject]@{name='core';path='core\rust';packages=@('ad-ssh','ad-vpn','ad-stream-protocol','ad-stream-client','ad-display-decoder','ad-remote-session','ad-qos-controller')}
}
function Initialize-WorkspaceLocks([string]$Snapshot) {
    $dependencyFingerprint = Get-DependencyFingerprint
    foreach ($workspace in Get-RustTestWorkspaces) {
        $manifest = Join-Path $Snapshot ($workspace.path + '\Cargo.toml')
        $lock = Join-Path $Snapshot ($workspace.path + '\Cargo.lock')
        $cachedLock = Join-Path $outputRoot ('cache\locks\' + $dependencyFingerprint + '.' + $workspace.name + '.lock')
        if (!(Test-Path -LiteralPath $lock -PathType Leaf) -and (Test-Path -LiteralPath $cachedLock -PathType Leaf)) {
            Copy-Item -LiteralPath $cachedLock -Destination $lock
        }
        if (!(Test-Path -LiteralPath $lock -PathType Leaf)) {
            Invoke-StepTool 'cargo.exe' @('generate-lockfile','--manifest-path',$manifest)
        }
        Invoke-StepTool 'cargo.exe' @('metadata','--locked','--no-deps','--format-version','1','--manifest-path',$manifest)
        New-Item -ItemType Directory -Force -Path (Split-Path $cachedLock -Parent) | Out-Null
        Copy-Item -LiteralPath $lock -Destination $cachedLock -Force
    }
}
function Invoke-RustWorkspaceTests([string]$Snapshot) {
    $manifest = Join-Path $Snapshot 'ui\rust\Cargo.toml'
    Invoke-StepTool 'cargo.exe' @('fmt','--manifest-path',$manifest,'-p','ad-desktop-model','-p','ad-desktop','--check')
    & (Join-Path $recipeRoot 'Run-Regressions.ps1') -Snapshot $Snapshot -Report (Join-Path $outputRoot 'reports\windows-regressions.json')
}
function Get-Iscc {
    foreach ($path in @(
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
        (Join-Path ([Environment]::GetEnvironmentVariable('ProgramFiles(x86)')) 'Inno Setup 6\ISCC.exe')
    )) { if (Test-Path -LiteralPath $path -PathType Leaf) { return $path } }
    throw 'Inno Setup 6 is required for the Rust installer. Install it or use Build-Portable-Only.cmd.'
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
    $settings = Get-Content -LiteralPath (Join-Path $sourceRoot 'tools\build-kit\toolchain.json') -Raw | ConvertFrom-Json
    $script:gstreamerRoot = if ($env:AETHERDESK_GSTREAMER_ROOT) { $env:AETHERDESK_GSTREAMER_ROOT } else { Join-Path $env:USERPROFILE ('develop\gstreamer\' + $settings.gstreamer) }
    foreach ($relative in @('bin\pkg-config.exe','bin\gst-inspect-1.0.exe','libexec\gstreamer-1.0\gst-plugin-scanner.exe')) {
        if (!(Test-Path -LiteralPath (Join-Path $gstreamerRoot $relative) -PathType Leaf)) { throw "The build machine needs the GStreamer MSVC development/runtime SDK: $gstreamerRoot. Use AETHERDESK_GSTREAMER_ROOT to select its location." }
    }
    $env:GSTREAMER_ROOT = $gstreamerRoot
    $env:PKG_CONFIG_PATH = Join-Path $gstreamerRoot 'lib\pkgconfig'
    $env:PKG_CONFIG = Join-Path $gstreamerRoot 'bin\pkg-config.exe'
    $env:Path = (Join-Path $gstreamerRoot 'bin') + ';' + $env:Path
    foreach ($name in @('cargo.exe','rustc.exe','link.exe','rc.exe')) { Get-Command $name -CommandType Application -ErrorAction Stop | Out-Null }
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
        if ($relative -match '\.(sh|patch)$' -or $relative -like '*Dockerfile*') {
            # Windows checkouts may use CRLF. Linux interprets a trailing CR in
            # shell commands literally; normalize only the isolated snapshot.
            $linuxText = [IO.File]::ReadAllText((Join-Path $sourceRoot $relative)).Replace("`r`n","`n")
            [IO.File]::WriteAllText($target,$linuxText,[Text.UTF8Encoding]::new($false))
        } else {
            Copy-Item -LiteralPath (Join-Path $sourceRoot $relative) -Destination $target -Force
        }
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

function Test-AgentBundle([string]$Directory) {
    foreach ($name in @('ad-agent','runtime-manifest.json','libs\Xwayland','regression-results.json')) {
        if (!(Test-Path -LiteralPath (Join-Path $Directory $name) -PathType Leaf)) { throw "Missing Linux agent file: $name" }
    }
    $results=Get-Content -LiteralPath (Join-Path $Directory 'regression-results.json') -Raw|ConvertFrom-Json
    if($results.schemaVersion -ne 1 -or $results.compilerTestsPassed -ne $true -or $results.required -lt 1 -or $results.registered -lt $results.required -or $results.replacementReady -ne $false -or $results.liveAcceptancePassed -ne $false){throw 'Linux agent regression evidence is missing or invalid.'}
    $manifest = Get-Content -LiteralPath (Join-Path $Directory 'runtime-manifest.json') -Raw | ConvertFrom-Json
    if ($manifest.schema_version -ne 1 -or !$manifest.files) { throw 'Invalid Linux agent runtime manifest.' }
    foreach ($entry in $manifest.files) {
        if ($entry.file_name -notmatch '^[a-zA-Z0-9_.+-]+$' -or $entry.file_name -in @('.','..')) { throw 'Invalid Linux runtime library name.' }
        $file = Join-Path $Directory ('libs\' + $entry.file_name)
        if (!(Test-Path -LiteralPath $file -PathType Leaf) -or (Get-Item -LiteralPath $file).Length -ne $entry.size -or
            (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $entry.sha256) { throw "Linux runtime checksum mismatch: $($entry.file_name)" }
    }
}
function Build-NativeAgent([string]$Snapshot,[string]$Destination) {
    if (Test-Path -LiteralPath $Destination) { throw 'Agent output already exists; use a fresh destination.' }
    if ($env:AETHERDESK_VERIFIED_AGENT_BUNDLE) {
        $bundle = $env:AETHERDESK_VERIFIED_AGENT_BUNDLE
        Test-AgentBundle $bundle
        $evidence=Get-Content -LiteralPath (Join-Path $bundle 'regression-results.json') -Raw|ConvertFrom-Json
        if($evidence.catalogSha256 -ne (Get-FileHash -LiteralPath (Join-Path $Snapshot 'rust-regressions.json')).Hash){throw 'CI agent regressions do not match this source catalog.'}
        $agentVersion=(Get-Content -LiteralPath (Join-Path $bundle 'version.txt') -Raw).Trim()
        if($agentVersion -ne $release.normalized){throw 'CI agent version does not match the Windows client.'}
        New-Item -ItemType Directory -Force -Path $Destination|Out-Null
        Copy-Item -Path (Join-Path $bundle '*') -Destination $Destination -Recurse
        return
    }
    $distribution = if ($env:AETHERDESK_WSL_DISTRIBUTION) { $env:AETHERDESK_WSL_DISTRIBUTION } else { 'Ubuntu' }
    $linuxSource = (& wsl.exe -d $distribution --exec wslpath -u $Snapshot | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or !$linuxSource.StartsWith('/')) { throw 'Could not translate the snapshot path for WSL.' }
    New-Item -ItemType Directory -Force -Path (Split-Path $Destination -Parent) | Out-Null
    $linuxOutput = (& wsl.exe -d $distribution --exec wslpath -u $Destination | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or !$linuxOutput.StartsWith('/')) { throw 'Could not translate the agent output path for WSL.' }
    # The original container recipe compiles the Rust agent, runs its Wayland
    # regression tests, and bundles Xwayland and the encoder's runtime libraries.
    Invoke-StepTool 'wsl.exe' @('-d',$distribution,'-u','root','--exec','podman','--storage-driver=vfs','build','--layers',
        '--from','docker.io/library/almalinux:8','--platform','linux/amd64','--target','artifact','--output',("type=local,dest=$linuxOutput"),
        '-f',($linuxSource + '/agent/linux/Dockerfile.bundle'),$linuxSource)
    Test-AgentBundle $Destination
    $coreManifest = [IO.File]::ReadAllText((Join-Path $Snapshot 'core\rust\Cargo.toml'))
    $version = [regex]::Match($coreManifest,'(?s)\[workspace\.package\].*?version\s*=\s*"([^"]+)"').Groups[1].Value
    if (!$version) { throw 'Agent version missing.' }
    [IO.File]::WriteAllText((Join-Path $Destination 'version.txt'),$version,[Text.UTF8Encoding]::new($false))
}
function Copy-NativeRuntimes([string]$Snapshot,[string]$Agent,[string]$Destination) {
    Test-AgentBundle $Agent
    Copy-Item -Path (Join-Path $gstreamerRoot 'bin\*.dll') -Destination $Destination
    Copy-Item -LiteralPath (Join-Path $gstreamerRoot 'lib\gstreamer-1.0') -Destination $Destination -Recurse
    Copy-Item -LiteralPath (Join-Path $gstreamerRoot 'libexec\gstreamer-1.0\gst-plugin-scanner.exe') -Destination $Destination
    $crt = Join-Path $env:VCToolsRedistDir 'x64\Microsoft.VC143.CRT'
    foreach ($name in @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll')) {
        if (!(Test-Path -LiteralPath (Join-Path $crt $name))) { throw "Missing portable runtime: $name" }
    }
    Copy-Item -Path (Join-Path $crt '*.dll') -Destination $Destination -Force
    $agentDestination = Join-Path $Destination 'agent\linux-x86_64'
    New-Item -ItemType Directory -Force -Path $agentDestination | Out-Null
    Copy-Item -Path (Join-Path $Agent '*') -Destination $agentDestination -Recurse
    if (Test-Path -LiteralPath (Join-Path $gstreamerRoot 'share\licenses')) {
        Copy-Item -LiteralPath (Join-Path $gstreamerRoot 'share\licenses') -Destination (Join-Path $Destination 'gstreamer-licenses') -Recurse
    } else { throw 'GStreamer runtime license files are missing.' }
    & (Join-Path $Snapshot 'tools\build-kit\Bundle-X11.ps1') -Destination $Destination -CacheDirectory (Join-Path $outputRoot 'cache\x11')
}

function Test-NativeX11Runtime([string]$Snapshot,[string]$Package) {
    $previousX11Package = $env:AETHERDESK_X11_PACKAGE
    $evidence=Join-Path $Package 'verification/x11-runtime.json'
    if(Test-Path -LiteralPath $evidence -PathType Leaf){Remove-Item -LiteralPath $evidence -Force}
    try {
        $env:AETHERDESK_X11_PACKAGE = $Package
        $arguments=@('test','--locked','--manifest-path',(Join-Path $Snapshot 'ui\rust\Cargo.toml'),'-p','ad-desktop','--target','x86_64-pc-windows-msvc','x11::tests::bundled_runtime_sharing_authentication_icons_and_cleanup','--','--ignored','--exact','--test-threads=1')
        Write-Host ('Running bundled X11 regression: cargo.exe '+($arguments -join ' '))
        $lines=@(& cargo.exe @arguments|ForEach-Object{Write-Host $_;$_})
        if($LASTEXITCODE -ne 0){throw "cargo.exe failed with exit code $LASTEXITCODE"}
        if(($lines -join "`n") -notmatch 'test result: ok\. 1 passed; 0 failed; 0 ignored;'){throw 'Bundled X11 regression did not run its required test.'}
        New-Item -ItemType Directory -Force -Path (Join-Path $Package 'verification')|Out-Null
        @{schemaVersion=1;bundledX11TestPassed=$true;liveAcceptancePassed=$false;replacementReady=$false}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $Package 'verification/x11-runtime.json') -Encoding UTF8
    } finally { $env:AETHERDESK_X11_PACKAGE = $previousX11Package }
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
function Verify-RustPackage([string]$Package,[switch]$BeforeX11) {
    foreach ($relative in @('x11\vcxsrv.exe','x11\xauth.exe','x11\xkbcomp.exe','x11\fonts\misc\fonts.dir','x11\xkbdata\rules\base','gst-plugin-scanner.exe','gstreamer-1.0','agent\linux-x86_64\version.txt')) {
        if (!(Test-Path -LiteralPath (Join-Path $Package $relative))) { throw "Missing native connection runtime: $relative" }
    }
    Test-AgentBundle (Join-Path $Package 'agent\linux-x86_64')
    $catalog=Join-Path $Package 'verification/regression-catalog.json'
    if(!(Test-Path -LiteralPath $catalog -PathType Leaf)){throw 'Package regression catalog is missing.'}
    $catalogHash=(Get-FileHash -LiteralPath $catalog -Algorithm SHA256).Hash
    foreach($relative in @('verification/windows-regressions.json','agent/linux-x86_64/regression-results.json')){
        $result=Get-Content -LiteralPath (Join-Path $Package $relative) -Raw|ConvertFrom-Json
        if($result.compilerTestsPassed -ne $true -or $result.liveAcceptancePassed -ne $false -or $result.replacementReady -ne $false -or $result.required -lt 1 -or $result.registered -lt $result.required -or $result.catalogSha256 -ne $catalogHash){throw 'Package regression results do not match its catalog.'}
    }
    if(!$BeforeX11){
        $x11=Get-Content -LiteralPath (Join-Path $Package 'verification/x11-runtime.json') -Raw|ConvertFrom-Json
        if($x11.bundledX11TestPassed -ne $true -or $x11.liveAcceptancePassed -ne $false -or $x11.replacementReady -ne $false){throw 'Package bundled X11 test evidence is invalid.'}
    }
    $info = Get-Content -LiteralPath (Join-Path $Package 'build-info.json') -Raw | ConvertFrom-Json
    if ($info.featureEquivalent -ne $false -or $info.runtime -ne 'native-rust' -or $info.product -ne 'AetherDesk Rust') {
        throw 'Package must identify the native Rust client and its acceptance status.'
    }
    $exe = Join-Path $Package 'AetherDesk.exe'
    $stream = [IO.File]::OpenRead($exe)
    try {
        $header = New-Object byte[] 64
        if ($stream.Read($header,0,64) -ne 64 -or $header[0] -ne 77 -or $header[1] -ne 90) {
            throw 'Rust executable has no valid Windows header.'
        }
        $offset = [BitConverter]::ToInt32($header,60)
        if ($offset -lt 64 -or $offset -gt $stream.Length - 6) { throw 'Rust executable has an invalid PE offset.' }
        [void]$stream.Seek($offset,[IO.SeekOrigin]::Begin)
        $signature = New-Object byte[] 6
        if ($stream.Read($signature,0,6) -ne 6 -or $signature[0] -ne 80 -or $signature[1] -ne 69 -or
            $signature[2] -ne 0 -or $signature[3] -ne 0 -or [BitConverter]::ToUInt16($signature,4) -ne 34404) {
            throw 'Rust executable must be Windows x64.'
        }
    } finally { $stream.Dispose() }
    foreach ($name in @('Cargo.lock','rust-migration.json','READ-ME-FIRST.txt','THIRD-PARTY-NOTICES.txt')) {
        if (!(Test-Path -LiteralPath (Join-Path $Package $name) -PathType Leaf)) { throw "Missing Rust package file: $name" }
    }
}

function Get-DependencyFingerprint {
    $lines = @('lock-cache-v2','version=' + $release.normalized)
    foreach ($relative in Get-RustSourceFiles | Where-Object { $_ -like '*Cargo.toml' -or $_ -like '*Cargo.lock' }) {
        $lines += $relative + '=' + (Get-FileHash -LiteralPath (Join-Path $sourceRoot $relative) -Algorithm SHA256).Hash
    }
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(($lines -join [Environment]::NewLine)))).Replace('-','').ToLowerInvariant() }
    finally { $sha.Dispose() }
}
