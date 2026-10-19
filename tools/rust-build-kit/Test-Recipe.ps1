$ErrorActionPreference = 'Stop'
foreach ($file in Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.ps1') {
    $tokens = $null; $errors = $null
    $null = [Management.Automation.Language.Parser]::ParseFile($file.FullName,[ref]$tokens,[ref]$errors)
    if ($errors.Count) { throw ($errors | Out-String) }
}
# Evaluate selected production functions with no compiler or SDK invocation.
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'Build-Common.ps1'),[ref]$tokens,[ref]$errors)
foreach ($name in @('Format-BuildDuration','Get-BuildStageName','Write-BuildTimingReport','Reset-BuildOutput','Clear-NativeAgentBuildCache','Get-RustSourceFiles','Get-SourceFingerprint','Invoke-StepTool','Read-Record','Require-Record','Write-Record','Write-Snapshot','Verify-RustPackage','Copy-DependencyNotices','Get-RustTestWorkspaces','Initialize-WorkspaceLocks','Invoke-RustWorkspaceTests','Test-AgentBundle','Test-NativeX11Runtime','Get-DependencyFingerprint')) {
    $node = $ast.Find({param($entry) $entry -is [Management.Automation.Language.FunctionDefinitionAst] -and $entry.Name -eq $name},$true)
    if (!$node) { throw "Missing production function: $name" }
    . ([scriptblock]::Create($node.Extent.Text))
}
$sourceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$realSourceRoot=$sourceRoot
. (Join-Path $PSScriptRoot 'Check-Regressions.ps1')
foreach($fixture in @(
    @{phase='windows';attributes='#[cfg(not(target_os = "windows"))]';name='linux_decoder'},
    @{phase='windows';attributes='#[cfg(target_os = "linux")]';name='linux_video'},
    @{phase='linux';attributes='#[cfg(target_os = "windows")]';name='windows_decoder'}
)){
    $rejected=$false
    try{Assert-TestPlatformPhase $fixture.phase $fixture.attributes $fixture.name}catch{
        if($_.Exception.Message -notlike '*platform does not match*'){throw};$rejected=$true
    }
    if(!$rejected){throw 'A platform-only test was assigned to an incompatible build phase.'}
}
Assert-TestPlatformPhase 'linux' '#[cfg(not(target_os = "windows"))]' 'linux_decoder'
Assert-TestPlatformPhase 'windows' '#[cfg(target_os = "windows")]' 'windows_decoder'
Assert-TestPlatformPhase 'package' '#[cfg(windows)]' 'bundled_x11'
Write-Host 'PASS: platform-only regressions reject incompatible Windows/Linux assignments.'
$catalog=Get-RegressionCatalog $sourceRoot $true
foreach($feature in $catalog.features){foreach($check in $feature.checks){
    if($check.test -in @('hardware_decoders_precede_software','decodes_real_h264_across_growing_and_shrinking_resizes','moving_colour_frames_survive_the_real_video_pipeline') -and $check.phase -ne 'linux'){throw 'Linux decoder coverage was assigned to Windows.'}
}}
$linuxRecipe=[IO.File]::ReadAllText((Join-Path $sourceRoot 'agent/linux/Dockerfile.bundle'))
if([regex]::Matches($linuxRecipe,'-p ad-display-decoder -- --(?:list|test-threads=1)').Count -ne 2){throw 'Linux decoder regressions must be registered and executed by the container recipe.'}
if (!$linuxRecipe.Contains('COPY tools/rust-build-kit/test_xwayland_download.py /tmp/test_xwayland_download.py') -or
    !$linuxRecipe.Contains('AETHERDESK_XWAYLAND_SCRIPT=/tmp/build-xwayland.sh python3.9 /tmp/test_xwayland_download.py')) {
    throw 'Linux source-download regressions must run before downloading/building Xwayland.'
}
$fixtureNames=@($catalog.features|ForEach-Object{$_.checks}|Where-Object{$_.phase -eq 'windows'}|Select-Object -ExpandProperty test -Unique)
$missingRegistrationRejected=$false
try {$null=Assert-RegisteredRegressions $catalog 'windows' ('fixture::'+$fixtureNames[0]+': test')}catch{
    if($_.Exception.Message -notlike '*did not register required test*'){throw};$missingRegistrationRejected=$true
}
if(!$missingRegistrationRejected){throw 'A partial compiler registry was accepted.'}
$recipeRoot = $PSScriptRoot
$realFiles = @(Get-RustSourceFiles)
foreach ($required in @('core/rust/Cargo.toml','core/rust/crates/ad-ssh/src/client.rs','core/rust/crates/ad-ssh/src/scoped_command.sh','core/rust/crates/ad-vpn/Cargo.toml','tools/x11-icons.py','tools/rust-build-kit/test_scoped_command.py','tools/rust-build-kit/test_xwayland_download.py','ui/rust/crates/ad-desktop/src/session.rs','ui/rust/crates/ad-desktop/src/desktop_recovery.rs','rust-feature-inventory.json')) {
    if ($realFiles -notcontains $required -or !(Test-Path -LiteralPath (Join-Path $sourceRoot $required) -PathType Leaf)) { throw "Native snapshot omitted its backend dependency: $required" }
}
if (@($realFiles | Where-Object { $_ -match '(^|/)(target|build|\.git)/' }).Count) { throw 'Snapshot includes compiler caches.' }
if ($realFiles -notcontains 'core/rust/crates/agent-wayland/src/terminal_flow_control.rs') {
    throw 'Native snapshot omitted the remote desktop terminal flow-control policy.'
}
# Nested modules in client.rs resolve under src/client unless #[path] says
# otherwise. Checking snapshot presence alone does not validate that layout.
function Assert-SshClientModulePaths([string]$Text) {
    $directory = Join-Path $sourceRoot 'core/rust/crates/ad-ssh/src'
    $declarations = [regex]::Matches($Text,'(?m)^(?:#\[path\s*=\s*"(?<path>[^"]+)"\]\r?\n)?mod\s+(?<name>\w+)\s*;')
    if (!$declarations.Count) { throw 'SSH client module declarations were not found.' }
    foreach ($declaration in $declarations) {
        $name = $declaration.Groups['name'].Value
        $override = $declaration.Groups['path'].Value
        $candidates = if ($override) { @($override) } else { @("client/$name.rs","client/$name/mod.rs") }
        $resolved = @($candidates | Where-Object { Test-Path -LiteralPath (Join-Path $directory $_) -PathType Leaf })
        if ($resolved.Count -ne 1) { throw "Unresolved SSH client module: $name" }
        $relative = 'core/rust/crates/ad-ssh/src/' + $resolved[0].Replace('\','/')
        if ($realFiles -notcontains $relative) { throw "Snapshot omitted resolved SSH client module: $name" }
    }
}
$sshClientText = [IO.File]::ReadAllText((Join-Path $sourceRoot 'core/rust/crates/ad-ssh/src/client.rs'))
Assert-SshClientModulePaths $sshClientText
$badModuleLayout = [regex]::Replace($sshClientText,'(?m)^#\[path = "resumable_file\.rs"\]\r?\n','')
$badLayoutRejected = $false
try { Assert-SshClientModulePaths $badModuleLayout } catch {
    if ($_.Exception.Message -ne 'Unresolved SSH client module: resumable_file') { throw }
    $badLayoutRejected = $true
}
if (!$badLayoutRejected) { throw 'The previously broken transfer-module declaration was accepted.' }
Write-Host 'PASS: SSH client module paths resolve in snapshots; missing sibling-path regression rejected.'
foreach ($required in @('core/rust/crates/ad-ssh/src/resumable_file.rs','ui/rust/crates/ad-desktop/src/terminal_settings.rs','ui/rust/crates/ad-desktop/src/terminal_presets.json','ui/rust/crates/ad-desktop/src/terminal_rules.json','ui/rust/crates/ad-desktop/src/file_workspace.rs','ui/rust/crates/ad-desktop/src/working_copies.rs','ui/rust/crates/ad-desktop/src/comparison.rs','ui/rust/crates/ad-desktop/src/transfers.rs','ui/rust/crates/ad-desktop/src/theme.rs','ui/rust/crates/ad-desktop/src/migration.rs','ui/rust/crates/ad-desktop/src/assistant_history.rs','ui/rust/crates/ad-desktop/src/ai_models.rs')) {
    if ($realFiles -notcontains $required) {throw "Snapshot omitted a requested feature or theme asset: $required"}
}
# Verify the embedded PNG headers without loading a UI or building the application.
foreach ($required in @('ui/rust/crates/ad-desktop/src/dialog.rs','ui/rust/crates/ad-desktop/src/remote_frames.rs','ui/rust/crates/ad-desktop/src/ui_icons.rs','ui/rust/crates/ad-desktop/src/file_picker.rs','ui/rust/crates/ad-desktop/src/terminal_navigation.rs','ui/rust/crates/ad-desktop/build.rs')) {
    if ($realFiles -notcontains $required) {throw "Snapshot omitted a session UI/desktop latency module: $required"}
}
if ($realFiles -notcontains 'ui/rust/crates/ad-desktop/assets/theme/aetherdesk.png') {throw 'Snapshot omitted the native product icon.'}
$binaryManifest = [IO.File]::ReadAllText((Join-Path $sourceRoot 'ui/rust/crates/ad-desktop/Cargo.toml'))
if ($binaryManifest -notmatch '(?m)^name = "aetherdesk"\r?$') {throw 'The product executable must be named aetherdesk.'}
$resourceBuild = [IO.File]::ReadAllText((Join-Path $sourceRoot 'ui/rust/crates/ad-desktop/build.rs'))
foreach ($required in @('1 ICON "{icon}"','assets/theme/aetherdesk.ico','cargo:rustc-link-arg-bin=aetherdesk=','AetherDesk.exe')) {
    if (!$resourceBuild.Contains($required)) {throw "Windows product icon/resource recipe omitted: $required"}
}
$themeSource = [IO.File]::ReadAllText((Join-Path $sourceRoot 'ui/rust/crates/ad-desktop/src/theme.rs'))
if ($themeSource -match 'dark-reference|light-reference|load_texture|painter\.image') {throw 'Theme must use solid colours, with no background image loading.'}
foreach ($required in @('ui/rust/crates/ad-desktop/src/editor_tools.rs','ui/rust/crates/ad-desktop/src/editor_transport.rs','ui/rust/crates/ad-desktop/src/editor_language.rs')) {
    if ($realFiles -notcontains $required) {throw "Snapshot omitted an R5 editor module: $required"}
}
Write-Host 'PASS: R5 editor modules included; themes use solid colours without background textures.'
foreach ($required in @('remote_view.rs','remote_presentation.rs','remote_keyboard.rs','remote_geometry.rs','remote_toolbar.rs','remote_input.rs')) {
    if ($realFiles -notcontains ('ui/rust/crates/ad-desktop/src/' + $required)) {
        throw "Snapshot omitted an R6 desktop module: $required"
    }
}
# remote_presentation.rs is a sibling of remote_view.rs, not a file under
# src/remote_view/. Preserve the explicit child-module path in the snapshot.
function Assert-RemotePresentationModulePath([string]$Text) {
    if ($Text -notmatch '(?m)^#\[path = "remote_presentation\.rs"\]\r?\nmod presentation;') {
        throw 'Unresolved remote presentation sibling module'
    }
    if (!(Test-Path -LiteralPath (Join-Path $sourceRoot 'ui/rust/crates/ad-desktop/src/remote_presentation.rs') -PathType Leaf)) {
        throw 'Missing remote presentation sibling module'
    }
}
$remoteViewText = [IO.File]::ReadAllText((Join-Path $sourceRoot 'ui/rust/crates/ad-desktop/src/remote_view.rs'))
Assert-RemotePresentationModulePath $remoteViewText
$missingPathRejected = $false
try { Assert-RemotePresentationModulePath ([regex]::Replace($remoteViewText,'(?m)^#\[path = "remote_presentation\.rs"\]\r?\n','')) } catch {
    if ($_.Exception.Message -ne 'Unresolved remote presentation sibling module') { throw }
    $missingPathRejected = $true
}
if (!$missingPathRejected) { throw 'Missing remote presentation path was accepted.' }
Write-Host 'PASS: R6 input/monitor/toolbar snapshot modules and presentation sibling path.'
foreach ($required in @('ui/rust/crates/ad-desktop/src/x11.rs','ui/rust/crates/ad-desktop/assets/theme/aetherdesk.ico','tools/x11-icons.py')) {
    if ($realFiles -notcontains $required) { throw "Snapshot omitted an R9 X11 dependency: $required" }
}
$iconBytes = [IO.File]::ReadAllBytes((Join-Path $sourceRoot 'ui/rust/crates/ad-desktop/assets/theme/aetherdesk.ico'))
if ($iconBytes.Length -lt 22 -or [BitConverter]::ToUInt16($iconBytes,0) -ne 0 -or
    [BitConverter]::ToUInt16($iconBytes,2) -ne 1 -or [BitConverter]::ToUInt16($iconBytes,4) -eq 0) {
    throw 'The X11 fallback icon has no valid ICO header.'
}
$x11Source = [IO.File]::ReadAllText((Join-Path $sourceRoot 'ui/rust/crates/ad-desktop/src/x11.rs'))
if ($x11Source -notmatch 'async fn bundled_runtime_sharing_authentication_icons_and_cleanup\(') {
    throw 'The package X11 runtime test filter no longer resolves to a source test.'
}
if ((Test-Path -LiteralPath (Join-Path $sourceRoot 'core\rust\Cargo.lock')) -and $realFiles -notcontains 'core/rust/Cargo.lock') {
    throw 'Snapshot omitted the existing backend lockfile.'
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
    if ((Format-BuildDuration ([TimeSpan]::Zero)) -ne '00:00:00' -or
        (Format-BuildDuration ([TimeSpan]::FromSeconds(183845))) -ne '51:04:05') {
        throw 'Build duration formatting wraps or loses elapsed hours.'
    }
    $timingFixture = Join-Path $temporary 'timing\build-timing.json'
    $timingSteps = @(@{step=2;name=(Get-BuildStageName 2);status='Failed';elapsed='00:00:03';elapsedSeconds=3.25})
    $timingEntries = [Collections.Generic.List[object]]::new()
    $timingEntries.Add($timingSteps[0])
    foreach ($timingStatus in @('Running','Failed','Passed')) {
        Write-BuildTimingReport $timingFixture ([DateTimeOffset]::UtcNow) ([TimeSpan]::FromSeconds(5.5)) $timingStatus $timingEntries.ToArray() $true
        $savedTiming = Get-Content -LiteralPath $timingFixture -Raw | ConvertFrom-Json
        if ($savedTiming.status -ne $timingStatus -or $savedTiming.elapsedSeconds -ne 5.5 -or
            $savedTiming.elapsed -ne '00:00:05' -or !$savedTiming.keepBuildCache -or
            @($savedTiming.steps).Count -ne 1 -or $savedTiming.steps[0].status -ne 'Failed') {
            throw 'Build timing report lost status, duration, mode or failed-stage information.'
        }
    }
    Write-Host 'PASS: elapsed timing supports long builds and persists running/success/failure details without a compiler.'
    $cleanupRoot = Join-Path $temporary 'cleanup-output'
    $cleanupSource = Join-Path $temporary 'cleanup-source'
    $oldDirectories = @('agent','installer','logs','packages','reports','source','state')
    $oldFiles = @('preflight.json','model-tests.json','LATEST-FOLDER.txt','LATEST-ZIP.txt','LATEST-INSTALLER.txt','LATEST-PREVIEW-FOLDER.txt','LATEST-PREVIEW-ZIP.txt','LATEST-PREVIEW-INSTALLER.txt')
    foreach ($name in $oldDirectories + @('cache\target','cache\cargo-home','cache\locks','cache\x11')) {
        $directory = Join-Path $cleanupRoot $name
        $null = New-Item -ItemType Directory -Path $directory -Force
        [IO.File]::WriteAllText((Join-Path $directory 'sentinel.txt'),'fixture')
    }
    $null = New-Item -ItemType Directory -Path $cleanupSource
    [IO.File]::WriteAllText((Join-Path $cleanupSource 'sentinel.txt'),'source')
    [IO.File]::WriteAllText((Join-Path $cleanupRoot 'my-file.txt'),'user file')
    foreach ($name in $oldFiles) { [IO.File]::WriteAllText((Join-Path $cleanupRoot $name),'old build') }
    Reset-BuildOutput $cleanupRoot $cleanupSource
    foreach ($name in $oldDirectories + $oldFiles) {
        if (Test-Path -LiteralPath (Join-Path $cleanupRoot $name)) { throw "Cleanup kept an old generated artifact: $name" }
    }
    foreach ($name in @('cache\target','cache\cargo-home','cache\locks','cache\x11')) {
        if ([IO.File]::ReadAllText((Join-Path $cleanupRoot "$name\sentinel.txt")) -ne 'fixture') { throw 'Cleanup removed a reusable cache.' }
    }
    if ([IO.File]::ReadAllText((Join-Path $cleanupRoot 'my-file.txt')) -ne 'user file' -or
        [IO.File]::ReadAllText((Join-Path $cleanupSource 'sentinel.txt')) -ne 'source') { throw 'Cleanup modified user or source files.' }
    Reset-BuildOutput $cleanupRoot $cleanupSource
    Reset-BuildOutput (Join-Path $temporary 'not-created') $cleanupSource
    foreach ($unsafe in @([IO.Path]::GetPathRoot($temporary),$cleanupSource,(Join-Path $cleanupSource 'output'),$temporary)) {
        $rejected = $false
        try { Reset-BuildOutput $unsafe $cleanupSource } catch {
            if ($_.Exception.Message -notlike 'Cleanup requires*') { throw }; $rejected = $true
        }
        if (!$rejected) { throw "Cleanup accepted an unsafe output root: $unsafe" }
    }
    # Junctions do not require elevation on Windows. The external target must
    # survive and all generated artifacts must remain until validation succeeds.
    $link = Join-Path $cleanupRoot 'packages'
    $null = New-Item -ItemType Junction -Path $link -Target $cleanupSource
    $null = New-Item -ItemType Directory -Path (Join-Path $cleanupRoot 'agent')
    $rejected = $false
    try {
        try { Reset-BuildOutput $cleanupRoot $cleanupSource } catch {
            if ($_.Exception.Message -notlike 'Cleanup refuses*linked artifact*') { throw }; $rejected = $true
        }
        if (!$rejected -or !(Test-Path -LiteralPath (Join-Path $cleanupRoot 'agent')) -or
            [IO.File]::ReadAllText((Join-Path $cleanupSource 'sentinel.txt')) -ne 'source') { throw 'Cleanup followed a junction or deleted before validating all targets.' }
        $rejected = $false
        try { Reset-BuildOutput (Join-Path $link 'nested') $cleanupSource } catch {
            if ($_.Exception.Message -notlike 'Cleanup refuses*linked output ancestor*') { throw }; $rejected = $true
        }
        if (!$rejected) { throw 'Cleanup followed an output ancestor junction.' }
    } finally { [IO.Directory]::Delete($link) }
    $nestedLink = Join-Path $cleanupRoot 'agent\linked-files'
    $null = New-Item -ItemType Junction -Path $nestedLink -Target $cleanupSource
    try {
        $rejected = $false
        try { Reset-BuildOutput $cleanupRoot $cleanupSource } catch {
            if ($_.Exception.Message -notlike 'Cleanup refuses*linked artifact*') { throw }; $rejected = $true
        }
        if (!$rejected -or [IO.File]::ReadAllText((Join-Path $cleanupSource 'sentinel.txt')) -ne 'source') { throw 'Cleanup followed a nested artifact junction.' }
    } finally { [IO.Directory]::Delete($nestedLink) }
    Write-Host 'PASS: build cleanup removes generated artifacts, preserves caches/user/source files, is repeatable, and rejects unsafe roots/junctions before deletion.'
    # Completed packages and evidence survive post-success cleanup. All local
    # staging and compiler/download caches become disposable only at this point.
    foreach ($name in @('agent','installer','source','state','cache\target','cache\cargo-home','packages','logs','reports','acceptance')) {
        $directory = Join-Path $cleanupRoot $name
        $null = New-Item -ItemType Directory -Path $directory -Force
        [IO.File]::WriteAllText((Join-Path $directory 'sentinel.txt'),'completed fixture')
    }
    foreach ($name in @('preflight.json','model-tests.json','LATEST-FOLDER.txt','LATEST-ZIP.txt','LATEST-INSTALLER.txt')) {
        [IO.File]::WriteAllText((Join-Path $cleanupRoot $name),'completed fixture')
    }
    $cacheLink = Join-Path $cleanupRoot 'cache\linked-files'
    $null = New-Item -ItemType Junction -Path $cacheLink -Target $cleanupSource
    try {
        $rejected=$false
        try { Reset-BuildOutput $cleanupRoot $cleanupSource -AfterSuccess } catch {
            if ($_.Exception.Message -notlike 'Cleanup refuses*linked artifact*') { throw }; $rejected=$true
        }
        if (!$rejected -or !(Test-Path -LiteralPath (Join-Path $cleanupRoot 'agent'))) { throw 'Post-success cleanup followed a cache junction or deleted before validation.' }
    } finally { [IO.Directory]::Delete($cacheLink) }
    Reset-BuildOutput $cleanupRoot $cleanupSource -AfterSuccess
    Reset-BuildOutput $cleanupRoot $cleanupSource -AfterSuccess
    foreach ($name in @('agent','installer','source','state','cache','preflight.json','model-tests.json')) {
        if (Test-Path -LiteralPath (Join-Path $cleanupRoot $name)) { throw "Post-success cleanup retained temporary files: $name" }
    }
    foreach ($name in @('packages','logs','reports','acceptance')) {
        if ([IO.File]::ReadAllText((Join-Path $cleanupRoot "$name\sentinel.txt")) -ne 'completed fixture') { throw "Post-success cleanup removed completed outputs/evidence: $name" }
    }
    foreach ($name in @('LATEST-FOLDER.txt','LATEST-ZIP.txt','LATEST-INSTALLER.txt')) {
        if ([IO.File]::ReadAllText((Join-Path $cleanupRoot $name)) -ne 'completed fixture') { throw 'Post-success cleanup removed latest package pointers.' }
    }
    if ([IO.File]::ReadAllText((Join-Path $cleanupRoot 'my-file.txt')) -ne 'user file' -or
        [IO.File]::ReadAllText((Join-Path $cleanupSource 'sentinel.txt')) -ne 'source') { throw 'Post-success cleanup changed source or unrelated files.' }
    # Fast mode retains only reusable caches, not unnecessary staging files.
    foreach ($name in @('agent','installer','source','state','cache\target')) {
        $fixtureDirectory = Join-Path $cleanupRoot $name
        New-Item -ItemType Directory -Force -Path $fixtureDirectory | Out-Null
        [IO.File]::WriteAllText((Join-Path $fixtureDirectory 'sentinel.txt'),'fast fixture')
    }
    Reset-BuildOutput $cleanupRoot $cleanupSource -AfterSuccess -KeepCache
    foreach ($name in @('agent','installer','source','state')) {
        if (Test-Path -LiteralPath (Join-Path $cleanupRoot $name)) { throw 'Fast build mode kept unnecessary staging.' }
    }
    if ([IO.File]::ReadAllText((Join-Path $cleanupRoot 'cache\target\sentinel.txt')) -ne 'fast fixture' -or
        [IO.File]::ReadAllText((Join-Path $cleanupRoot 'packages\sentinel.txt')) -ne 'completed fixture') {
        throw 'Fast build mode removed reusable caches or completed packages.'
    }
    Write-Host 'PASS: optional fast builds preserve compiler caches while deleting staging files.'
    # Exercise the production Linux cleanup with a captured command, never WSL.
    $realStepTool = ${function:Invoke-StepTool}
    $savedDistribution = $env:AETHERDESK_WSL_DISTRIBUTION
    try {
        $env:AETHERDESK_WSL_DISTRIBUTION='fixture-distro'
        function Invoke-StepTool([string]$Program,[string[]]$Arguments) {
            if ($Program -ne 'wsl.exe' -or ($Arguments -join '|') -ne '-d|fixture-distro|-u|root|--exec|podman|--storage-driver=vfs|image|prune|--all|--force|--filter|label=io.aetherdesk.rust-build-cache=true') { throw 'Linux cleanup was not restricted to labelled Rust build images.' }
            $script:cleanupCommandObserved=$true
        }
        $script:cleanupCommandObserved=$false
        Clear-NativeAgentBuildCache
        if (!$script:cleanupCommandObserved) { throw 'Linux cleanup did not invoke its labelled command.' }
    } finally {
        ${function:Invoke-StepTool}=$realStepTool
        $env:AETHERDESK_WSL_DISTRIBUTION=$savedDistribution
    }
    if ([regex]::Matches($linuxRecipe,'(?m)^LABEL io\.aetherdesk\.rust-build-cache="true"').Count -ne 2) { throw 'Rust build stages must carry the cache ownership label.' }
    Write-Host 'PASS: post-success cleanup removes staging/caches, keeps completed packages/evidence, rejects cache junctions, and restricts Linux pruning to owned labels.'
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
    $coreManifest = Join-Path $sourceRoot 'core\rust\Cargo.toml'
    $coreLock = Join-Path $sourceRoot 'core\rust\Cargo.lock'
    New-Item -ItemType Directory -Force -Path (Split-Path $coreManifest -Parent) | Out-Null
    [IO.File]::WriteAllText($coreManifest,'[workspace]')
    [IO.File]::WriteAllText($coreLock,'backend source lock')
    New-Item -ItemType Directory -Force -Path (Join-Path $sourceRoot 'tools') | Out-Null
    $shellFixture=Join-Path $sourceRoot 'tools/test-agent.sh'
    [IO.File]::WriteAllText($shellFixture,"#!/bin/sh`r`necho fixture`r`n",[Text.UTF8Encoding]::new($false))
    function Get-RustSourceFiles { 'ui/rust/Cargo.toml'; 'ui/rust/Cargo.lock'; 'core/rust/Cargo.toml'; 'core/rust/Cargo.lock'; 'tools/test-agent.sh' }
    $release = [pscustomobject]@{ normalized='0.2.0' }
    $snapshot = Write-Snapshot 'test-fingerprint'
    if ([IO.File]::ReadAllText((Join-Path $snapshot 'tools/test-agent.sh')).Contains("`r")) { throw 'Linux snapshot retained Windows line endings.' }
    if (![IO.File]::ReadAllText($shellFixture).Contains("`r`n")) { throw 'Linux normalization changed the source checkout.' }
    if ([IO.File]::ReadAllText((Join-Path $snapshot 'ui\rust\Cargo.toml')) -notmatch 'version = "0\.2\.0"') {
        throw 'Snapshot did not use release.json version.'
    }
    if ([IO.File]::ReadAllText($manifest) -notmatch 'version = "0\.1\.2"') { throw 'Snapshot modified original source.' }
    $snapshotLock = [IO.File]::ReadAllText((Join-Path $snapshot 'ui\rust\Cargo.lock'))
    if ([regex]::Matches($snapshotLock,'version = "0\.2\.0"').Count -ne 2 -or
        [regex]::Matches($snapshotLock,'version = "0\.1\.2"').Count -ne 1) { throw 'Lock version normalization modified external dependencies.' }
    if ([IO.File]::ReadAllText((Join-Path $snapshot 'core\rust\Cargo.lock')) -ne 'backend source lock') { throw 'Snapshot changed the backend lock.' }
    $liveSource=$sourceRoot
    $oldFrozen=$env:AETHERDESK_RUST_FROZEN_FINGERPRINT
    $snapshotFiles=@(Get-ChildItem -LiteralPath $snapshot -Recurse -File|Select-Object -ExpandProperty FullName)
    $frozenId='a'*64
    Write-Record 1 $frozenId $snapshot $snapshotFiles 'snapshot-fixture.log'
    try {
        $env:AETHERDESK_RUST_FROZEN_FINGERPRINT=$frozenId
        $sourceRoot=$snapshot
        [IO.File]::WriteAllText((Join-Path $liveSource 'tools/live-edit.txt'),'An edit made after the source snapshot')
        if((Get-SourceFingerprint) -ne $frozenId){throw 'Live edits invalidated a sealed build snapshot.'}
        $snapshotShell=Join-Path $snapshot 'tools/test-agent.sh'
        $savedShell=[IO.File]::ReadAllText($snapshotShell)
        [IO.File]::WriteAllText($snapshotShell,'tampered snapshot')
        $rejected=$false
        try{$null=Get-SourceFingerprint}catch{$rejected=$true}
        if(!$rejected){throw 'A changed frozen source snapshot was accepted.'}
        [IO.File]::WriteAllText($snapshotShell,$savedShell,[Text.UTF8Encoding]::new($false))
    } finally {
        $sourceRoot=$liveSource
        $env:AETHERDESK_RUST_FROZEN_FINGERPRINT=$oldFrozen
    }
    Write-Host 'PASS: frozen build ignores later checkout edits and rejects changes inside its recorded snapshot.'
    $dependencyFingerprint = Get-DependencyFingerprint
    [IO.File]::WriteAllText($coreLock,'updated backend source lock')
    if ((Get-DependencyFingerprint) -eq $dependencyFingerprint) { throw 'A changed backend source lock reused a stale cache key.' }
    [IO.File]::WriteAllText($coreLock,'backend source lock')

    # Mock only Cargo: exercise the production workspace/lock/test orchestration.
    $cargoCalls = [Collections.Generic.List[object]]::new()
    $failBackendTest = $false
    $failX11Test = $false
    $emptyRegistry=$false
    $emptyResults=$false
    $tooFewResults=$false
    $x11PackagesSeen = [Collections.Generic.List[string]]::new()
    function cargo.exe {
        $cargoCalls.Add(@($args))
        $manifestArgument = $args[[Array]::IndexOf($args,'--manifest-path') + 1]
        if ($args[0] -eq 'generate-lockfile') {
            [IO.File]::WriteAllText((Join-Path (Split-Path $manifestArgument -Parent) 'Cargo.lock'),('generated for ' + $manifestArgument))
        }
        $global:LASTEXITCODE = 0
        if($args[0] -eq 'test' -and $args -contains '--list'){
            if(!$emptyRegistry){foreach($name in $fixtureNames){Write-Output ('fixture::'+$name+': test')}}
        } elseif($args[0] -eq 'test') {
            $count=if($args -contains 'x11::tests::bundled_runtime_sharing_authentication_icons_and_cleanup'){1}else{$fixtureNames.Count}
            if($tooFewResults){$count=1}
            Write-Output $(if($emptyResults){'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out'}else{'test result: ok. '+$count+' passed; 0 failed; 0 ignored; 0 measured; 0 filtered out'})
        }
        if ($failBackendTest -and $args[0] -eq 'test' -and $args -contains 'ad-ssh') { $global:LASTEXITCODE = 101 }
        if ($args -contains 'x11::tests::bundled_runtime_sharing_authentication_icons_and_cleanup') {
            $x11PackagesSeen.Add($env:AETHERDESK_X11_PACKAGE)
            if ($failX11Test) { $global:LASTEXITCODE = 101 }
        }
    }
    Initialize-WorkspaceLocks $snapshot
    if (@($cargoCalls | Where-Object { $_[0] -eq 'generate-lockfile' }).Count) { throw 'Source locks were regenerated.' }
    if ([IO.File]::ReadAllText((Join-Path $snapshot 'ui\rust\Cargo.lock')) -ne $snapshotLock) { throw 'UI source lock was overwritten.' }
    if ([IO.File]::ReadAllText((Join-Path $snapshot 'core\rust\Cargo.lock')) -ne 'backend source lock') { throw 'Backend source lock was overwritten.' }
    # Restore missing snapshot locks from distinct caches without resolving again.
    Remove-Item -LiteralPath (Join-Path $snapshot 'ui\rust\Cargo.lock'),(Join-Path $snapshot 'core\rust\Cargo.lock')
    Initialize-WorkspaceLocks $snapshot
    if (@($cargoCalls | Where-Object { $_[0] -eq 'generate-lockfile' }).Count) { throw 'Cached workspace locks were not reused.' }
    if ([IO.File]::ReadAllText((Join-Path $snapshot 'ui\rust\Cargo.lock')) -ne $snapshotLock -or
        [IO.File]::ReadAllText((Join-Path $snapshot 'core\rust\Cargo.lock')) -ne 'backend source lock') { throw 'Workspace lock caches were mixed.' }
    # A new dependency fingerprint with no cached/source locks resolves both.
    [IO.File]::WriteAllText($coreManifest,'[workspace] # changed dependencies')
    Remove-Item -LiteralPath (Join-Path $snapshot 'ui\rust\Cargo.lock'),(Join-Path $snapshot 'core\rust\Cargo.lock')
    $cargoCalls.Clear()
    Initialize-WorkspaceLocks $snapshot
    if (@($cargoCalls | Where-Object { $_[0] -eq 'generate-lockfile' }).Count -ne 2) { throw 'Missing locks were not generated for both workspaces.' }
    $metadataCalls = @($cargoCalls | Where-Object { $_[0] -eq 'metadata' })
    if ($metadataCalls.Count -ne 2 -or @($metadataCalls | Where-Object { $_ -notcontains '--locked' }).Count) { throw 'Workspace lock validation was skipped.' }
    $cargoCalls.Clear()
    Invoke-RustWorkspaceTests $realSourceRoot
    $testCalls = @($cargoCalls | Where-Object { $_[0] -eq 'test' })
    $expected = [Collections.Generic.List[object]]::new()
    foreach($suffix in @('--list','--test-threads=1')){
        foreach($workspace in Get-RustTestWorkspaces){
            $arguments=@('test','--locked','--manifest-path',(Join-Path $realSourceRoot ($workspace.path+'\Cargo.toml')),'--target','x86_64-pc-windows-msvc')
            foreach($packageName in $workspace.packages){$arguments+=@('-p',$packageName)}
            $expected.Add($arguments+@('--',$suffix))
        }
    }
    if ($testCalls.Count -ne 4) { throw 'Both workspace registries and test suites must run.' }
    for ($index=0; $index -lt 4; $index++) {
        if (($testCalls[$index] -join '|') -ne ($expected[$index] -join '|')) { throw 'Tests used the wrong workspace, packages or lock/target options.' }
    }
    $report=Join-Path $outputRoot 'reports/windows-regressions.json'
    $record=Get-Content -LiteralPath $report -Raw|ConvertFrom-Json
    if(!$record.compilerTestsPassed -or $record.liveAcceptancePassed -or $record.replacementReady -or $record.required -ne $fixtureNames.Count){throw 'Regression report confuses automated and live acceptance.'}
    $emptyRegistry=$true;$failed=$false
    try {Invoke-RustWorkspaceTests $realSourceRoot} catch {if($_.Exception.Message -notlike '*registry is empty*'){throw};$failed=$true}
    if(!$failed){throw 'An empty compiler registry was accepted.'};$emptyRegistry=$false
    $emptyResults=$true;$failed=$false
    try {Invoke-RustWorkspaceTests $realSourceRoot} catch {if($_.Exception.Message -notlike '*nonempty passing test result*'){throw};$failed=$true}
    if(!$failed){throw 'Zero-test success was accepted.'};$emptyResults=$false
    $tooFewResults=$true;$failed=$false
    try {Invoke-RustWorkspaceTests $realSourceRoot} catch {if($_.Exception.Message -notlike '*Fewer behavioral tests*'){throw};$failed=$true}
    if(!$failed){throw 'Insufficient passing baseline counts were accepted.'};$tooFewResults=$false
    $failBackendTest = $true
    $failed = $false
    try { Invoke-RustWorkspaceTests $realSourceRoot } catch {
        if ($_.Exception.Message -notlike '*exit code 101*') { throw }
        $failed = $true
    }
    if (!$failed) { throw 'Backend test failures were ignored.' }
    Write-Host 'PASS: R11 registered-test and nonempty-result gates, separate live status, report output and failure propagation (Cargo mocked).'
    $cargoCalls.Clear()
    $savedX11Package = $env:AETHERDESK_X11_PACKAGE
    try {
        $env:AETHERDESK_X11_PACKAGE = 'previous-fixture-package'
        Test-NativeX11Runtime $snapshot $temporary
        $expectedX11 = @('test','--locked','--manifest-path',(Join-Path $snapshot 'ui\rust\Cargo.toml'),'-p','ad-desktop','--target','x86_64-pc-windows-msvc','x11::tests::bundled_runtime_sharing_authentication_icons_and_cleanup','--','--ignored','--exact','--test-threads=1')
        if ($cargoCalls.Count -ne 1 -or ($cargoCalls[0] -join '|') -ne ($expectedX11 -join '|') -or
            $x11PackagesSeen[0] -ne $temporary -or $env:AETHERDESK_X11_PACKAGE -ne 'previous-fixture-package') {
            throw 'Package X11 verification used the wrong source, test, package or environment.'
        }
        $failX11Test = $true
        $failed = $false
        try { Test-NativeX11Runtime $snapshot $temporary } catch {
            if ($_.Exception.Message -notlike '*exit code 101*') { throw }; $failed = $true
        }
        if (!$failed -or $env:AETHERDESK_X11_PACKAGE -ne 'previous-fixture-package') {
            throw 'Package X11 failure propagation or environment restoration failed.'
        }
        $failX11Test=$false;$emptyResults=$true;$failed=$false
        try {Test-NativeX11Runtime $snapshot $temporary} catch {if($_.Exception.Message -notlike '*did not run its required test*'){throw};$failed=$true}
        if(!$failed -or (Test-Path -LiteralPath (Join-Path $temporary 'verification/x11-runtime.json'))){throw 'Zero-test X11 success reused stale evidence.'}
        $emptyResults=$false
    } finally { $env:AETHERDESK_X11_PACKAGE = $savedX11Package }
    Write-Host 'PASS: R9 embedded icon, shared-runtime package test routing, failure propagation and environment restoration (Cargo mocked).'
    $package = Join-Path $temporary 'package'
    New-Item -ItemType Directory -Path $package | Out-Null
    foreach ($relative in @('x11\vcxsrv.exe','x11\xauth.exe','x11\xkbcomp.exe','x11\fonts\misc\fonts.dir','x11\xkbdata\rules\base','gst-plugin-scanner.exe','agent\linux-x86_64\version.txt','agent\linux-x86_64\ad-agent','agent\linux-x86_64\libs\Xwayland')) {
        $fixture = Join-Path $package $relative
        New-Item -ItemType Directory -Force -Path (Split-Path $fixture -Parent) | Out-Null
        [IO.File]::WriteAllText($fixture,'fixture')
    }
    New-Item -ItemType Directory -Path (Join-Path $package 'gstreamer-1.0') | Out-Null
    $agentRoot = Join-Path $package 'agent\linux-x86_64'
    $library = Join-Path $agentRoot 'libs\Xwayland'
    $agentManifest = @{schema_version=1;files=@(@{file_name='Xwayland';size=(Get-Item $library).Length;sha256=(Get-FileHash $library).Hash})} | ConvertTo-Json -Depth 4
    [IO.File]::WriteAllText((Join-Path $agentRoot 'runtime-manifest.json'),$agentManifest)
    New-Item -ItemType Directory -Path (Join-Path $package 'verification')|Out-Null
    $fixtureCatalog=Join-Path $package 'verification/regression-catalog.json'
    [IO.File]::WriteAllText($fixtureCatalog,'{}')
    $fixtureReport=@{schemaVersion=1;compilerTestsPassed=$true;liveAcceptancePassed=$false;replacementReady=$false;required=1;registered=1;catalogSha256=(Get-FileHash $fixtureCatalog).Hash}|ConvertTo-Json
    [IO.File]::WriteAllText((Join-Path $package 'verification/windows-regressions.json'),$fixtureReport)
    [IO.File]::WriteAllText((Join-Path $agentRoot 'regression-results.json'),$fixtureReport)
    [IO.File]::WriteAllText((Join-Path $package 'verification/x11-runtime.json'),(@{bundledX11TestPassed=$true;liveAcceptancePassed=$false;replacementReady=$false}|ConvertTo-Json))
    Test-AgentBundle $agentRoot
    [IO.File]::WriteAllText($library,'corrupted')
    $failed = $false
    try { Test-AgentBundle $agentRoot } catch { if ($_.Exception.Message -notlike '*checksum mismatch*') { throw }; $failed=$true }
    if (!$failed) { throw 'A corrupted agent runtime was accepted.' }
    [IO.File]::WriteAllText($library,'fixture')
    '{"featureEquivalent":false,"runtime":"native-rust","product":"AetherDesk Rust"}' |
        Set-Content -LiteralPath (Join-Path $package 'build-info.json') -Encoding UTF8
    $image = New-Object byte[] 80
    $image[0]=77; $image[1]=90; $image[60]=64
    $image[64]=80; $image[65]=69; $image[68]=100; $image[69]=134
    $exe = Join-Path $package 'AetherDesk.exe'
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
    Verify-RustPackage $package
    $windowsEvidence=Join-Path $package 'verification/windows-regressions.json'
    $badEvidence=$fixtureReport|ConvertFrom-Json;$badEvidence.catalogSha256='wrong-catalog'
    [IO.File]::WriteAllText($windowsEvidence,($badEvidence|ConvertTo-Json))
    $rejected=$false
    try {Verify-RustPackage $package}catch{if($_.Exception.Message -notlike '*do not match its catalog*'){throw};$rejected=$true}
    if(!$rejected){throw 'Mismatched packaged regression evidence was accepted.'}
    [IO.File]::WriteAllText($windowsEvidence,$fixtureReport)
    $x11Evidence=Join-Path $package 'verification/x11-runtime.json'
    [IO.File]::WriteAllText($x11Evidence,(@{bundledX11TestPassed=$false;liveAcceptancePassed=$false;replacementReady=$false}|ConvertTo-Json))
    $rejected=$false
    try {Verify-RustPackage $package}catch{if($_.Exception.Message -notlike '*X11 test evidence is invalid*'){throw};$rejected=$true}
    if(!$rejected){throw 'A failed packaged X11 result was accepted.'}
    [IO.File]::WriteAllText($x11Evidence,(@{bundledX11TestPassed=$true;liveAcceptancePassed=$false;replacementReady=$false}|ConvertTo-Json))
    Write-Host 'PASS: R11 packages reject mismatched catalog hashes and failed bundled-X11 evidence.'
    $image[68]=76; $image[69]=1
    [IO.File]::WriteAllBytes($exe,$image)
    $rejected=$false
    try { Verify-RustPackage $package } catch {
        if ($_.Exception.Message -ne 'Rust executable must be Windows x64.') { throw }
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
foreach ($required in @('Invoke-RustWorkspaceTests $snapshot','Test-NativeX11Runtime $snapshot $result','Initialize-WorkspaceLocks $result',"'build','--locked','--release'","aetherdesk","Check-Migration.ps1","featureEquivalent="+ '$false')) {
    if (!$worker.Contains($required)) { throw "Rust recipe omitted: $required" }
}
if ($worker -match "Invoke-StepTool 'flutter|Invoke-StepTool 'node|Invoke-StepTool 'python|LangGraph") { throw 'Rust recipe must not build the legacy Flutter/Python app.' }
$global:LASTEXITCODE = 0
Write-Host 'PASS: Rust recipe syntax, workspace test routing, separate dependency locks, failure propagation, resume hashes, isolated versions, root dependency notices and x64 package validation.'
