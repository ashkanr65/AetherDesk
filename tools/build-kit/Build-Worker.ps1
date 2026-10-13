[CmdletBinding()]
param([Parameter(Mandatory = $true)][ValidateRange(0, 12)][int]$Step)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
. (Join-Path $PSScriptRoot 'Build-Common.ps1')
Reset-ProcessBuildEnvironment
$revision = (& $git -C $sourceRepo rev-parse HEAD | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { throw 'The source repository is missing or unreadable.' }
$release = & (Join-Path $kit 'Release-Version.ps1') -Repository $sourceRepo
$fingerprint = Get-BuildFingerprint
$project = Join-Path $repo 'ui\flutter\ad_remote_desktop'
$gst = if ($env:AETHERDESK_GSTREAMER_ROOT) { $env:AETHERDESK_GSTREAMER_ROOT } else { Join-Path $env:USERPROFILE "develop\gstreamer\$($toolchain.gstreamer)" }
$python = $versionPython
$inno = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'
$logDir = Join-Path $kitOutput 'logs'
$stamp = Get-Date -Format 'yyyyMMdd_HHmmss_fff'
$logFile = Join-Path $logDir ('step-{0:D2}-{1}.txt' -f $Step, $stamp)
$transcribing = $false
$resultPath = ''
$titles = @('Check the installed build tools', 'Prepare the build folder', 'Build the Linux agent', 'Run the Linux checks',
    'Prepare the Windows runner', 'Check the Flutter interface', 'Test the Windows controls',
    'Test and build the Windows core', 'Build the Windows application', 'Assemble and verify the portable app',
    'Open the new application', 'Create the portable ZIP', 'Create the Windows installer')

function Invoke-Checked {
    param([string]$Program, [string[]]$Arguments)
    Write-Host ''
    Write-Host ('Running: ' + $Program + ' ' + ($Arguments -join ' ')) -ForegroundColor DarkCyan
    Get-Command $Program -ErrorAction Stop | Out-Null
    $savedErrorPreference = $ErrorActionPreference
    try {
        # Windows PowerShell turns redirected native stderr into ErrorRecords.
        # Print those lines into the transcript without treating warnings as
        # terminating errors; the program's exit code determines success.
        $ErrorActionPreference = 'Continue'
        & $Program @Arguments 2>&1 | ForEach-Object {
            if ($_ -is [Management.Automation.ErrorRecord]) { $_.Exception.Message }
            else { $_.ToString() }
        } | Out-Host
        $commandExit = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $savedErrorPreference
    }
    if ($commandExit -ne 0) { throw "Command failed with exit code ${commandExit}: $Program" }
}

function Invoke-WindowsStage([string]$Stage) {
    Invoke-Checked "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $repo 'tools\build-kit\Windows-Stages.ps1'),
        '-Stage', $Stage, '-Repository', $repo)
}

function Get-Revision([string]$Directory) {
    $value = & git -C $Directory rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw "Could not read the Git revision in $Directory" }
    return ($value | Out-String).Trim()
}

function Require-Step([int]$Number) {
    $record = Read-StepRecord $Number $fingerprint
    if (!$record) { throw "Step $Number is missing, changed, or incomplete. Run Build-Everything.cmd to resume safely." }
    return $record
}

function To-WslPath([string]$WindowsPath) {
    # Execute wslpath directly so a Linux shell cannot consume backslashes.
    $nativePath = $WindowsPath.Replace('\', '/')
    $converted = & wsl.exe -d Ubuntu -u root --exec wslpath -a -u $nativePath
    if ($LASTEXITCODE -ne 0) { throw "Could not translate the path for Ubuntu: $WindowsPath" }
    return ($converted | Out-String).Trim()
}

function Configure-WindowsTools {
    $vs = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\Launch-VsDevShell.ps1'
    foreach ($required in @($vs, "$gst\bin\gst-inspect-1.0.exe", "$gst\bin\pkg-config.exe", $python)) {
        if (!(Test-Path -LiteralPath $required)) { throw "Installed tool is missing: $required" }
    }
    & $vs -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Host
    $env:GSTREAMER_ROOT = $gst
    $env:PKG_CONFIG_PATH = "$gst/lib/pkgconfig"
    $env:PKG_CONFIG = "$gst\bin\pkg-config.exe"
    $env:Path = "$gst\bin;$env:USERPROFILE\.cargo\bin;$env:USERPROFILE\develop\flutter\bin;$env:Path"
    $env:Path = (@('C:\Program Files\Git\cmd', "$env:SystemRoot\System32") + ($env:Path -split ';') | Where-Object { $_ } | Select-Object -Unique) -join ';'
    $env:CARGO_TARGET_DIR = Join-Path $repo 'core\rust\target'
    foreach ($tool in @('git.exe', 'cargo.exe', 'rustc.exe', 'flutter.bat', 'cmake.exe', 'ctest.exe', 'cl.exe', 'wsl.exe')) {
        Get-Command $tool -ErrorAction Stop | Out-Null
    }
    # Flutter's MSBuild child uses cmd.exe, not PowerShell's command lookup.
    Invoke-Checked $env:ComSpec @('/d', '/c', 'where git && where flutter && where cmake')
    Write-Host "Fresh build PATH length: $($env:Path.Length)"
}

try {
    New-Item -ItemType Directory -Force -Path $logDir | Out-Null
    New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
    Start-Transcript -LiteralPath $logFile -Force | Out-Null
    $transcribing = $true
    Write-Host "STEP $Step - $($titles[$Step])" -ForegroundColor Cyan
    Write-Host "Log: $logFile"
    if ($Step -le 9) { Configure-WindowsTools }

    if ($Step -gt 1) {
        $null = Require-Step 1
    }
    # Clear only this kit's completion records when rerunning an earlier stage.
    # Compiled files and user/source files are never deleted here.
    if ($Step -gt 0 -and (Test-Path -LiteralPath $stateDir)) {
        $invalidated = @($Step)
        if ($Step -le 9) { $invalidated = @($Step..12) }
        foreach ($number in $invalidated) {
            $oldRecord = Join-Path $stateDir ('step-{0:D2}.json' -f $number)
            if (Test-Path -LiteralPath $oldRecord) { Remove-Item -LiteralPath $oldRecord }
        }
    }

    switch ($Step) {
        0 {
            if (!(Test-Path -LiteralPath (Join-Path $sourceRepo '.git'))) { throw "Source repository is missing: $sourceRepo" }
            $editorTools = & (Join-Path $kit 'Resolve-Node.ps1')
            $nodeVersion = & $editorTools.node --version
            if ($LASTEXITCODE -ne 0 -or $nodeVersion -notmatch '^v([0-9]+)\.' -or [int]$Matches[1] -lt 22) { throw 'Node.js 22 or newer is required for the embedded editor.' }
            Invoke-Checked 'cargo' @('--version')
            Invoke-Checked 'rustc' @('--version')
            $flutterVersionText = & flutter --version --machine
            if ($LASTEXITCODE -ne 0) { throw 'Flutter version check failed' }
            $flutterVersion = ($flutterVersionText | Out-String) | ConvertFrom-Json
            if ($flutterVersion.frameworkVersion -ne $toolchain.flutter) { throw "This source requires Flutter $($toolchain.flutter). Do not upgrade during the build." }
            Invoke-Checked "$gst\bin\pkg-config.exe" @('--modversion', 'gstreamer-1.0')
            Invoke-Checked "$gst\bin\gst-inspect-1.0.exe" @('h264parse')
            Invoke-Checked "$gst\bin\gst-inspect-1.0.exe" @('avdec_h264')
            Invoke-Checked 'wsl.exe' @('-d', 'Ubuntu', '-u', 'root', '--exec', 'podman', '--storage-driver=vfs', '--version')
            Invoke-Checked 'wsl.exe' @('-d', 'Ubuntu', '-u', 'root', '--exec', 'bash', (To-WslPath (Join-Path $kit 'Check-Linux-Tools.sh')))
            if (!(Test-Path -LiteralPath $inno)) { throw "Missing installer compiler: $inno" }
            Invoke-Checked $python @('-X', 'utf8', (Join-Path $kit 'Check-Windows-Tools.py'))
        }
        1 {
            if (!(Test-Path -LiteralPath $repo)) {
                Invoke-Checked 'git' @('clone', '--config', 'core.autocrlf=false', '--config', 'core.longpaths=true', '--no-hardlinks', $sourceRepo, $repo)
            } else {
                Write-Host 'The build folder already exists; verifying and reusing it.'
            }
            if (!(Test-Path -LiteralPath (Join-Path $repo '.git'))) { throw 'The build folder is not a Git checkout. Nothing was deleted.' }
            Invoke-Checked 'git' @('-C', $repo, 'config', 'core.autocrlf', 'false')
            # Copy tracked and new working files, including fixes not yet committed.
            # Keep compiler caches and dated packages. The original source is read-only here.
            $sourceFiles = @(Get-SourceFiles)
            $sourceManifest = Join-Path $repo '.aetherdesk-source-files.json'
            $previousFiles = if (Test-Path -LiteralPath $sourceManifest) {
                @(Get-Content -LiteralPath $sourceManifest -Raw | ConvertFrom-Json)
            } else { @(& $git -c core.quotepath=false -C $repo ls-files) }
            foreach ($relative in $previousFiles) {
                if ($relative -in $sourceFiles) { continue }
                # Older checkouts tracked compiler caches. Preserve those files.
                if ($relative -match '(^|[/\\])(target|build|\.dart_tool)([/\\]|$)') { continue }
                $stale = [IO.Path]::GetFullPath((Join-Path $repo $relative))
                $buildRoot = [IO.Path]::GetFullPath($repo).TrimEnd('\') + '\'
                if (!$stale.StartsWith($buildRoot, [StringComparison]::OrdinalIgnoreCase) -or
                    $relative -match '(^|[/\\])(\.git|target|build|\.dart_tool)([/\\]|$)') {
                    throw "Invalid source manifest entry: $relative"
                }
                if (Test-Path -LiteralPath $stale -PathType Leaf) { Remove-Item -LiteralPath $stale }
            }
            foreach ($relative in $sourceFiles) {
                $destination = Join-Path $repo $relative
                New-Item -ItemType Directory -Force -Path (Split-Path $destination -Parent) | Out-Null
                Copy-BuildSourceFile (Join-Path $sourceRepo $relative) $destination
            }
            ConvertTo-Json -InputObject $sourceFiles | Set-Content -LiteralPath $sourceManifest -Encoding utf8
            New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
            $ignore = @('.git', '**/target', '**/build', '**/.dart_tool', '**/node_modules',
                'embedded-linux-agent*', 'linux-check-bundle*', 'release_package*',
                'native-desktop-tests', '.aetherdesk-local-build', 'installer_output*', '*.zip') -join "`n"
            [IO.File]::WriteAllText((Join-Path $repo '.containerignore'), $ignore + "`n", [Text.UTF8Encoding]::new($false))
            Invoke-Checked $python @('-X', 'utf8', (Join-Path $repo 'tools\version.py'), '--check')
            Write-Host "AetherDesk version $($release.version) (build $($release.build))"
            Write-Host "Source snapshot: $revision / $fingerprint"
            Write-Host 'Checking Rust formatting before building the Linux agent.'
            Invoke-Checked 'cargo' @('fmt', '--manifest-path', (Join-Path $repo 'core\rust\Cargo.toml'), '--check',
                '-p', 'agent-wayland', '-p', 'agent-input', '-p', 'agent-encoder', '-p', 'agent-supervisor',
                '-p', 'ad-stream-protocol', '-p', 'ad-qos-controller', '-p', 'ad-display-decoder',
                '-p', 'ad-display-renderer', '-p', 'ad-stream-client', '-p', 'ad-remote-session')
        }
        2 {
            $resultPath = Join-Path $repo ('embedded-linux-agent-' + $stamp)
            $linuxScript = To-WslPath (Join-Path $kit 'Run-Linux-Step.sh')
            $linuxRepo = To-WslPath $repo
            $linuxOutput = To-WslPath $resultPath
            Invoke-Checked 'wsl.exe' @('-d', 'Ubuntu', '-u', 'root', '--exec', 'bash', $linuxScript, 'bundle', $linuxRepo, $linuxOutput)
        }
        3 {
            $null = Require-Step 2
            $resultPath = Join-Path $repo ('linux-check-bundle-' + $stamp)
            $privateX11 = To-WslPath (Join-Path $kit 'With-Private-X11.sh')
            $linuxScript = To-WslPath (Join-Path $kit 'Run-Linux-Step.sh')
            $linuxRepo = To-WslPath $repo
            $linuxOutput = To-WslPath $resultPath
            Invoke-Checked 'wsl.exe' @('-d', 'Ubuntu', '-u', 'root', '--exec', 'bash', $privateX11, 'bash', $linuxScript, 'checks', $linuxRepo, $linuxOutput)
        }
        4 {
            $null = Require-Step 3
            Invoke-WindowsStage 'EditorAssets'
            Invoke-WindowsStage 'PrepareRunner'
        }
        5 {
            $null = Require-Step 4
            Invoke-WindowsStage 'CheckUI'
        }
        6 {
            $null = Require-Step 5
            Invoke-WindowsStage 'NativeTests'
        }
        7 {
            $null = Require-Step 6
            Invoke-WindowsStage 'TestCore'
            Invoke-WindowsStage 'BuildCore'
        }
        8 {
            $null = Require-Step 7
            Invoke-WindowsStage 'BuildUI'
        }
        9 {
            foreach ($number in 2..8) { $null = Require-Step $number }
            $agent = (Require-Step 2).output
            $resultPath = Join-Path $outputRoot ('AetherDesk-' + $release.version + '-portable_' + $stamp)
            $flutterBuild = Join-Path $project 'build\windows\x64\runner\Release'
            $nativeCore = Join-Path $repo 'core\rust\target\x86_64-pc-windows-msvc\release\ad_ffi.dll'
            foreach ($required in @((Join-Path $flutterBuild 'aetherdesk.exe'), $nativeCore, (Join-Path $agent 'runtime-manifest.json'))) {
                if (!(Test-Path -LiteralPath $required)) { throw "Build output is missing: $required" }
            }
            New-Item -ItemType Directory -Path $resultPath | Out-Null
            Copy-Item -Path (Join-Path $flutterBuild '*') -Destination $resultPath -Recurse
            Copy-Item -LiteralPath $nativeCore -Destination $resultPath
            Copy-Item -Path "$gst\bin\*.dll" -Destination $resultPath -Force
            # Include the compiler runtime locally: a portable app must not rely
            # on the build computer's installed Visual C++ redistributable.
            $crt = Join-Path $env:VCToolsRedistDir 'x64\Microsoft.VC143.CRT'
            foreach ($name in @('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
                if (!(Test-Path -LiteralPath (Join-Path $crt $name))) { throw "Missing redistributable runtime: $crt\$name" }
            }
            Copy-Item -Path (Join-Path $crt '*.dll') -Destination $resultPath -Force
            Copy-Item -LiteralPath "$gst\lib\gstreamer-1.0" -Destination $resultPath -Recurse
            Copy-Item -LiteralPath "$gst\libexec\gstreamer-1.0\gst-plugin-scanner.exe" -Destination $resultPath
            $agentDestination = Join-Path $resultPath 'agent\linux-x86_64'
            New-Item -ItemType Directory -Path $agentDestination -Force | Out-Null
            Copy-Item -Path (Join-Path $agent '*') -Destination $agentDestination -Recurse
            if (Test-Path -LiteralPath "$gst\share\licenses") {
                Copy-Item -LiteralPath "$gst\share\licenses" -Destination $resultPath -Recurse
            }
            & (Join-Path $repo 'tools\build-kit\Bundle-Assistant.ps1') -Destination $resultPath
            Invoke-Checked $python @('-X', 'utf8', (Join-Path $repo 'tools\build-kit\Verify-Assistant.py'), '--package', $resultPath)
            & (Join-Path $repo 'tools\build-kit\Test-Assistant.ps1') -Package $resultPath -Repository $repo
            & (Join-Path $repo 'tools\build-kit\Bundle-X11.ps1') -Destination $resultPath
            & (Join-Path $repo 'tools\build-kit\Bundle-Editor-Runtime.ps1') -Destination $resultPath -Repository $repo
            Invoke-Checked $python @('-X', 'utf8', (Join-Path $repo 'tools\build-kit\Verify-Editor.py'), '--package', $resultPath)
            $env:RUNNER_TEMP = $env:TEMP
            Invoke-Checked $python @('-X', 'utf8', (Join-Path $repo '.github\scripts\verify-windows-runtime.py'), $resultPath)
            [ordered]@{ version = $release.version; build = $release.build; revision = $revision; sourceFingerprint = $fingerprint; builtAt = (Get-Date -Format o) } |
                ConvertTo-Json | Set-Content -LiteralPath (Join-Path $resultPath 'build-info.json') -Encoding utf8
            Set-Content -LiteralPath (Join-Path $kitOutput 'LATEST-PORTABLE-FOLDER.txt') -Value $resultPath -Encoding utf8
        }
        10 {
            $resultPath = (Require-Step 9).output
            $exe = Join-Path $resultPath 'aetherdesk.exe'
            if (!(Test-Path -LiteralPath $exe)) { throw 'The verified portable application is missing. Rerun step 9.' }
            Invoke-Item -LiteralPath $exe
            Write-Host 'Application launch requested. Sign in and test the connection yourself.'
            Write-Host 'This success message confirms the launch request, not the live connection.'
        }
        11 {
            $portable = (Require-Step 9).output
            $resultPath = Join-Path $outputRoot ('AetherDesk-' + $release.version + '-portable-' + $stamp + '.zip')
            Invoke-Checked $python @('-X', 'utf8', (Join-Path $kit 'Create-Portable-Zip.py'), $portable, $resultPath)
            Set-Content -LiteralPath (Join-Path $kitOutput 'LATEST-PORTABLE-ZIP.txt') -Value $resultPath -Encoding utf8
        }
        12 {
            $portable = (Require-Step 9).output
            if (!(Test-Path -LiteralPath $inno)) { throw "Inno Setup compiler is missing: $inno" }
            $installerOutput = $outputRoot
            $installerName = 'AetherDesk-' + $release.version + '-Setup-' + $stamp
            $iss = Join-Path $stateDir ('installer-' + $stamp + '.iss')
            $installerText = @"
[Setup]
AppName=AetherDesk
AppVersion=$($release.version)
VersionInfoVersion=$($release.normalized).$($release.build)
AppId=AetherDesk
DefaultDirName={localappdata}\Programs\AetherDesk
DefaultGroupName=AetherDesk
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2/fast
SolidCompression=yes
OutputDir=$installerOutput
OutputBaseFilename=$installerName
SetupIconFile=$project\assets\aetherdesk.ico
UninstallDisplayIcon={app}\aetherdesk.exe

[Files]
Source: "$portable\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs

[Icons]
Name: "{group}\AetherDesk"; Filename: "{app}\aetherdesk.exe"

#include "$repo\tools\build-kit\Editor-Runtime.iss"
"@
            [IO.File]::WriteAllText($iss, $installerText, [Text.UTF8Encoding]::new($true))
            Invoke-Checked $inno @($iss)
            $resultPath = Join-Path $installerOutput ($installerName + '.exe')
            if (!(Test-Path -LiteralPath $resultPath)) { throw 'The installer compiler did not produce the expected file.' }
            Set-Content -LiteralPath (Join-Path $kitOutput 'LATEST-INSTALLER.txt') -Value $resultPath -Encoding utf8
        }
    }
    if ((Get-BuildFingerprint) -ne $fingerprint) { throw 'Source or build scripts changed while the step was running. Restart Build-Everything.cmd.' }
    New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
    $artifactPaths = @()
    if ($Step -in 2, 3, 9) { $artifactPaths = @(Get-ChildItem -LiteralPath $resultPath -Recurse -File | Select-Object -ExpandProperty FullName) }
    if ($Step -eq 1) {
        $artifactPaths = @(Get-SourceFiles | Where-Object {
            # Step 4 deliberately generates these two incomplete runner scaffolds.
            $_ -notin @('ui/flutter/ad_remote_desktop/windows/CMakeLists.txt', 'ui/flutter/ad_remote_desktop/windows/runner/CMakeLists.txt')
        } | ForEach-Object { Join-Path $repo $_ })
        $artifactPaths += (Join-Path $repo '.aetherdesk-source-files.json')
    }
    if ($Step -eq 4) { $artifactPaths = @(Get-ChildItem -LiteralPath (Join-Path $project 'assets\editor') -File | Select-Object -ExpandProperty FullName) + @((Join-Path $project 'windows\runner\main.cpp'), (Join-Path $project 'windows\runner\flutter_window.cpp'), (Join-Path $project 'windows\CMakeLists.txt'), (Join-Path $project 'windows\runner\resources\app_icon.ico')) }
    if ($Step -eq 7) { $artifactPaths = @((Join-Path $repo 'core\rust\target\x86_64-pc-windows-msvc\release\ad_ffi.dll')) }
    if ($Step -eq 8) { $artifactPaths = @(Get-ChildItem -LiteralPath (Join-Path $project 'build\windows\x64\runner\Release') -Recurse -File | Select-Object -ExpandProperty FullName) }
    if ($Step -in 11, 12) { $artifactPaths = @($resultPath) }
    $artifacts = @($artifactPaths | ForEach-Object { [ordered]@{ path = $_; sha256 = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash } })
    $recordPath = Join-Path $stateDir ('step-{0:D2}.json' -f $Step)
    [ordered]@{ revision = $revision; fingerprint = $fingerprint; step = $Step; completed = (Get-Date -Format o); output = $resultPath; log = $logFile; artifacts = $artifacts } |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath ($recordPath + '.tmp') -Encoding utf8
    Move-Item -LiteralPath ($recordPath + '.tmp') -Destination $recordPath -Force
    Write-Host ''
    Write-Host "STEP $Step COMPLETE" -ForegroundColor Green
    if ($resultPath) { Write-Host "OUTPUT: $resultPath" -ForegroundColor Cyan }
    Write-Host "Log saved: $logFile"
} catch {
    Write-Host ''
    Write-Host "STEP $Step FAILED - do not continue to the next step." -ForegroundColor Red
    Write-Host $_.Exception.Message -ForegroundColor Red
    Write-Host "Send the newest step-$('{0:D2}' -f $Step) log from $logDir for help."
    Write-Host "Log: $logFile"
    throw
} finally {
    Set-Location -LiteralPath $kit
    if ($transcribing) { Stop-Transcript | Out-Null }
}
