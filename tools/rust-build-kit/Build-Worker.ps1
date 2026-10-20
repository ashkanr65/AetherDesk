[CmdletBinding()]
param([Parameter(Mandatory=$true)][ValidateRange(0,6)][int]$Step)
. (Join-Path $PSScriptRoot 'Build-Common.ps1')
$fingerprint = Get-SourceFingerprint
$stamp = Get-Date -Format 'yyyyMMdd_HHmmss_fff'
New-Item -ItemType Directory -Force -Path $logsRoot,$packagesRoot,$stateRoot | Out-Null
$log = Join-Path $logsRoot ('step-{0:D2}-{1}.txt' -f $Step,$stamp)
$transcribing = $false
$stepClock = [Diagnostics.Stopwatch]::StartNew()
try {
    Start-Transcript -LiteralPath $log | Out-Null
    $transcribing = $true
    Write-Host ("STEP {0}: {1} | started {2}" -f $Step,(Get-BuildStageName $Step),(Get-Date -Format o))
    Initialize-Toolchain
    $result = ''
    $files = @()
    switch ($Step) {
        0 {
            & (Join-Path $sourceRoot 'ui\rust\Check-Migration.ps1')
            & (Join-Path $PSScriptRoot 'Test-Recipe.ps1')
            $result = Join-Path $outputRoot 'preflight.json'
            [ordered]@{mode='native-rust';version=$release.version;build=$release.build;rustc=(& rustc.exe --version);checked=(Get-Date -Format o)} |
                ConvertTo-Json | Set-Content -LiteralPath $result -Encoding UTF8
            $files = @($result)
        }
        1 {
            $result = Write-Snapshot $fingerprint
            Initialize-WorkspaceLocks $result
            $files = @(Get-ChildItem -LiteralPath $result -Recurse -File | Select-Object -ExpandProperty FullName)
        }
        2 {
            $snapshot = (Require-Record 1 $fingerprint).output
            Invoke-RustWorkspaceTests $snapshot
            $agent = Join-Path $outputRoot ('agent\linux-x86_64-' + $stamp)
            Build-NativeAgent $snapshot $agent
            $result = Join-Path $outputRoot 'model-tests.json'
            [ordered]@{passed=$true;agent=$agent;regressions=(Join-Path $outputRoot 'reports\windows-regressions.json');scope='Cataloged native UI/core and Linux agent regressions; live feature parity remains unverified';checked=(Get-Date -Format o)} |
                ConvertTo-Json | Set-Content -LiteralPath $result -Encoding UTF8
            $files = @($result,(Join-Path $outputRoot 'reports\windows-regressions.json')) + @(Get-ChildItem -LiteralPath $agent -Recurse -File | Select-Object -ExpandProperty FullName)
        }
        3 {
            $null = Require-Record 2 $fingerprint
            $snapshot = (Require-Record 1 $fingerprint).output
            Invoke-StepTool 'cargo.exe' @('build','--locked','--release','--manifest-path',(Join-Path $snapshot 'ui\rust\Cargo.toml'),'-p','ad-desktop','--bin','aetherdesk','--target','x86_64-pc-windows-msvc')
            $result = Join-Path $targetRoot 'x86_64-pc-windows-msvc\release\AetherDesk.exe'
            $files = @($result)
        }
        4 {
            $binary = (Require-Record 3 $fingerprint).output
            $snapshot = (Require-Record 1 $fingerprint).output
            $result = Join-Path $packagesRoot ('AetherDesk-' + $release.version + '-build' + $release.build + '_' + $stamp)
            New-Item -ItemType Directory -Path $result | Out-Null
            Copy-Item -LiteralPath $binary -Destination (Join-Path $result 'AetherDesk.exe')
            foreach ($name in @('release.json','LICENSE','LICENSE-MIT','LICENSE-APACHE')) {
                Copy-Item -LiteralPath (Join-Path $snapshot $name) -Destination (Join-Path $result $name)
            }
            Copy-Item -LiteralPath (Join-Path $snapshot 'ui\rust\Cargo.lock') -Destination (Join-Path $result 'Cargo.lock')
            @('AetherDesk - secure remote workspace.',
              'Includes native SSH/SFTP, bundled X11 and remote desktop paths, popup editor, AI/Tools and protected native profiles.',
              'Configure appearance and behaviour from the gear menu > Preferences. See release notes for known limitations.',
              'Double-click AetherDesk.exe. All required application runtimes are bundled.',
              'Windows 10/11 x64 and an OpenGL 3.3-capable graphics driver are required.') |
                Set-Content -LiteralPath (Join-Path $result 'READ-ME-FIRST.txt') -Encoding UTF8
            [ordered]@{schemaVersion=1;product='AetherDesk';version=$release.version;build=$release.build;fingerprint=$fingerprint;featureEquivalent=$false;runtime='native-rust';built=(Get-Date -Format o)} |
                ConvertTo-Json | Set-Content -LiteralPath (Join-Path $result 'build-info.json') -Encoding UTF8
            Copy-DependencyNotices (Join-Path $snapshot 'ui\rust\Cargo.toml') $result
            $tests = Get-Content -LiteralPath (Require-Record 2 $fingerprint).output -Raw | ConvertFrom-Json
            Copy-NativeRuntimes $snapshot $tests.agent $result
            New-Item -ItemType Directory -Path (Join-Path $result 'verification') | Out-Null
            Copy-Item -LiteralPath $tests.regressions -Destination (Join-Path $result 'verification\windows-regressions.json')
            Copy-Item -LiteralPath (Join-Path $snapshot 'rust-regressions.json') -Destination (Join-Path $result 'verification\regression-catalog.json')
            Verify-RustPackage $result -BeforeX11
            # Exercise the production native manager against the newly bundled
            # server before either distributable can be marked complete.
            Test-NativeX11Runtime $snapshot $result
            Verify-RustPackage $result
            $files = @(Get-ChildItem -LiteralPath $result -Recurse -File | Select-Object -ExpandProperty FullName)
            Set-Content -LiteralPath (Join-Path $outputRoot 'LATEST-FOLDER.txt') -Value $result -Encoding UTF8
        }
        5 {
            $package = (Require-Record 4 $fingerprint).output
            Verify-RustPackage $package
            $result = Join-Path $packagesRoot ((Split-Path $package -Leaf) + '_' + $stamp + '.zip')
            Add-Type -AssemblyName System.IO.Compression.FileSystem
            # Runtime DLLs/agent bundles dominate this archive. Fast compression
            # avoids spending minutes on a slightly smaller distributable.
            [IO.Compression.ZipFile]::CreateFromDirectory($package,$result,[IO.Compression.CompressionLevel]::Fastest,$true)
            $files = @($result)
            Set-Content -LiteralPath (Join-Path $outputRoot 'LATEST-ZIP.txt') -Value $result -Encoding UTF8
        }
        6 {
            $package = (Require-Record 4 $fingerprint).output
            Verify-RustPackage $package
            $installerName = (Split-Path $package -Leaf) + '-Setup'
            $script = Join-Path $outputRoot ('installer\rust-' + $stamp + '.iss')
            New-Item -ItemType Directory -Force -Path (Split-Path $script -Parent) | Out-Null
            if (@($package,$packagesRoot) | Where-Object { $_ -match '["\r\n]' }) { throw 'Invalid installer path.' }
            @(
                '[Setup]',
                'AppId={{ABE535D8-5D63-4A27-902C-752E22C6E7B5}',
                'AppName=AetherDesk',
                ('AppVersion=' + $release.normalized),
                ('VersionInfoVersion=' + $release.normalized + '.' + $release.build),
                'DefaultDirName={localappdata}\Programs\AetherDesk-Rust',
                'DefaultGroupName=AetherDesk',
                'UsePreviousGroup=no',
                'UsePreviousAppDir=yes',
                'PrivilegesRequired=lowest',
                'CloseApplications=no',
                'RestartApplications=no',
                'MinVersion=10.0',
                'ArchitecturesAllowed=x64compatible',
                'ArchitecturesInstallIn64BitMode=x64compatible',
                ('OutputDir=' + $packagesRoot),
                ('OutputBaseFilename=' + $installerName),
                'Compression=lzma2',
                'SolidCompression=yes',
                'UninstallDisplayIcon={app}\AetherDesk.exe',
                ('SetupIconFile=' + (Join-Path ((Require-Record 1 $fingerprint).output) 'ui\rust\crates\ad-desktop\assets\theme\aetherdesk.ico')),
                '[InstallDelete]',
                'Type: files; Name: "{app}\aetherdesk-rust.exe"',
                'Type: files; Name: "{app}\aetherdesk-rust-preview.exe"',
                'Type: files; Name: "{app}\RUST-MIGRATION.md"',
                'Type: files; Name: "{app}\RUST-FEATURE-PARITY.md"',
                'Type: files; Name: "{app}\rust-migration.json"',
                'Type: files; Name: "{userprograms}\AetherDesk Rust Preview\AetherDesk Rust Preview.lnk"',
                'Type: dirifempty; Name: "{userprograms}\AetherDesk Rust Preview"',
                'Type: files; Name: "{userprograms}\AetherDesk Rust\AetherDesk Rust.lnk"',
                'Type: dirifempty; Name: "{userprograms}\AetherDesk Rust"',
                '[Files]',
                ('Source: "' + $package + '\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs'),
                '[Icons]',
                'Name: "{group}\AetherDesk"; Filename: "{app}\AetherDesk.exe"',
                '[Run]',
                'Filename: "{app}\AetherDesk.exe"; Description: "Open AetherDesk"; Flags: nowait postinstall skipifsilent'
            ) | Set-Content -LiteralPath $script -Encoding UTF8
            Invoke-StepTool (Get-Iscc) @($script)
            $result = Join-Path $packagesRoot ($installerName + '.exe')
            $files = @($result,$script)
            Set-Content -LiteralPath (Join-Path $outputRoot 'LATEST-INSTALLER.txt') -Value $result -Encoding UTF8
        }
    }
    if ((Get-SourceFingerprint) -ne $fingerprint) { throw 'Source changed during this step. Retry using the latest source.' }
    Write-Record $Step $fingerprint $result $files $log
    Write-Host "STEP $Step COMPLETE: $result" -ForegroundColor Green
} catch {
    Write-Host "STEP $Step FAILED. Stop; newest log: $log" -ForegroundColor Red
    throw
} finally {
    $stepClock.Stop()
    Write-Host ("STEP {0} WORKER TIME: {1}" -f $Step,(Format-BuildDuration $stepClock.Elapsed)) -ForegroundColor Cyan
    if ($transcribing) { Stop-Transcript | Out-Null }
}
