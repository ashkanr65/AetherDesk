[CmdletBinding()]
param([Parameter(Mandatory=$true)][ValidateRange(0,6)][int]$Step)
. (Join-Path $PSScriptRoot 'Build-Common.ps1')
$fingerprint = Get-SourceFingerprint
$stamp = Get-Date -Format 'yyyyMMdd_HHmmss_fff'
New-Item -ItemType Directory -Force -Path $logsRoot,$packagesRoot,$stateRoot | Out-Null
$log = Join-Path $logsRoot ('step-{0:D2}-{1}.txt' -f $Step,$stamp)
$transcribing = $false
try {
    Start-Transcript -LiteralPath $log | Out-Null
    $transcribing = $true
    Initialize-Toolchain
    $result = ''
    $files = @()
    switch ($Step) {
        0 {
            & (Join-Path $sourceRoot 'ui\rust\Check-Migration.ps1')
            & (Join-Path $PSScriptRoot 'Test-Recipe.ps1')
            $result = Join-Path $outputRoot 'preflight.json'
            [ordered]@{mode='development-preview';version=$release.version;build=$release.build;rustc=(& rustc.exe --version);checked=(Get-Date -Format o)} |
                ConvertTo-Json | Set-Content -LiteralPath $result -Encoding UTF8
            $files = @($result)
        }
        1 {
            $result = Write-Snapshot $fingerprint
            $manifest = Join-Path $result 'ui\rust\Cargo.toml'
            $lock = Join-Path $result 'ui\rust\Cargo.lock'
            $cachedLock = Join-Path $outputRoot ('cache\locks\' + (Get-DependencyFingerprint) + '.lock')
            if (!(Test-Path -LiteralPath $lock) -and (Test-Path -LiteralPath $cachedLock -PathType Leaf)) {
                Copy-Item -LiteralPath $cachedLock -Destination $lock
            }
            if (!(Test-Path -LiteralPath $lock)) { Invoke-StepTool 'cargo.exe' @('generate-lockfile','--manifest-path',$manifest) }
            Invoke-StepTool 'cargo.exe' @('metadata','--locked','--no-deps','--format-version','1','--manifest-path',$manifest)
            New-Item -ItemType Directory -Force -Path (Split-Path $cachedLock -Parent) | Out-Null
            Copy-Item -LiteralPath $lock -Destination $cachedLock -Force
            $files = @(Get-ChildItem -LiteralPath $result -Recurse -File | Select-Object -ExpandProperty FullName)
        }
        2 {
            $snapshot = (Require-Record 1 $fingerprint).output
            $manifest = Join-Path $snapshot 'ui\rust\Cargo.toml'
            Invoke-StepTool 'cargo.exe' @('fmt','--manifest-path',$manifest,'--all','--check')
            Invoke-StepTool 'cargo.exe' @('test','--locked','--manifest-path',$manifest,'-p','ad-desktop-model','--target','x86_64-pc-windows-msvc')
            $result = Join-Path $outputRoot 'model-tests.json'
            [ordered]@{passed=$true;scope='Rust model tests, not full feature parity';checked=(Get-Date -Format o)} |
                ConvertTo-Json | Set-Content -LiteralPath $result -Encoding UTF8
            $files = @($result)
        }
        3 {
            $null = Require-Record 2 $fingerprint
            $snapshot = (Require-Record 1 $fingerprint).output
            Invoke-StepTool 'cargo.exe' @('build','--locked','--release','--manifest-path',(Join-Path $snapshot 'ui\rust\Cargo.toml'),'-p','ad-desktop','--bin','aetherdesk-rust-preview','--target','x86_64-pc-windows-msvc')
            $result = Join-Path $targetRoot 'x86_64-pc-windows-msvc\release\aetherdesk-rust-preview.exe'
            $files = @($result)
        }
        4 {
            $binary = (Require-Record 3 $fingerprint).output
            $snapshot = (Require-Record 1 $fingerprint).output
            $result = Join-Path $packagesRoot ('AetherDesk-Rust-preview-' + $release.version + '-build' + $release.build + '_' + $stamp)
            New-Item -ItemType Directory -Path $result | Out-Null
            Copy-Item -LiteralPath $binary -Destination (Join-Path $result 'aetherdesk-rust-preview.exe')
            foreach ($name in @('release.json','rust-migration.json')) {
                Copy-Item -LiteralPath (Join-Path $snapshot $name) -Destination (Join-Path $result $name)
            }
            Copy-Item -LiteralPath (Join-Path $snapshot 'ui\rust\Cargo.lock') -Destination (Join-Path $result 'Cargo.lock')
            Copy-Item -LiteralPath (Join-Path $snapshot 'RUST-MIGRATION.md') -Destination (Join-Path $result 'RUST-MIGRATION.md')
            @('AetherDesk native Rust development preview.','This preview is NOT the full AetherDesk application.',
              'Profile/favorites preview and read-only import are available.',
              'SSH/SFTP/desktop/editor/AI implementations are still being ported.',
              'Double-click aetherdesk-rust-preview.exe. No Flutter, Node, Python or Rust installation is required to run this preview.',
              'Windows 10/11 x64 and an OpenGL 3.3-capable graphics driver are required.') |
                Set-Content -LiteralPath (Join-Path $result 'READ-ME-FIRST.txt') -Encoding UTF8
            [ordered]@{schemaVersion=1;product='AetherDesk Rust preview';version=$release.version;build=$release.build;fingerprint=$fingerprint;featureEquivalent=$false;runtime='native-rust';built=(Get-Date -Format o)} |
                ConvertTo-Json | Set-Content -LiteralPath (Join-Path $result 'build-info.json') -Encoding UTF8
            Copy-DependencyNotices (Join-Path $snapshot 'ui\rust\Cargo.toml') $result
            Verify-PreviewPackage $result
            $files = @(Get-ChildItem -LiteralPath $result -Recurse -File | Select-Object -ExpandProperty FullName)
            Set-Content -LiteralPath (Join-Path $outputRoot 'LATEST-PREVIEW-FOLDER.txt') -Value $result -Encoding UTF8
        }
        5 {
            $package = (Require-Record 4 $fingerprint).output
            Verify-PreviewPackage $package
            $result = Join-Path $packagesRoot ((Split-Path $package -Leaf) + '_' + $stamp + '.zip')
            Add-Type -AssemblyName System.IO.Compression.FileSystem
            [IO.Compression.ZipFile]::CreateFromDirectory($package,$result,[IO.Compression.CompressionLevel]::Optimal,$true)
            $files = @($result)
            Set-Content -LiteralPath (Join-Path $outputRoot 'LATEST-PREVIEW-ZIP.txt') -Value $result -Encoding UTF8
        }
        6 {
            $package = (Require-Record 4 $fingerprint).output
            Verify-PreviewPackage $package
            $installerName = (Split-Path $package -Leaf) + '-Setup'
            $script = Join-Path $outputRoot ('installer\preview-' + $stamp + '.iss')
            New-Item -ItemType Directory -Force -Path (Split-Path $script -Parent) | Out-Null
            if (@($package,$packagesRoot) | Where-Object { $_ -match '["\r\n]' }) { throw 'Invalid installer path.' }
            @(
                '[Setup]',
                'AppId={{ABE535D8-5D63-4A27-902C-752E22C6E7B5}',
                'AppName=AetherDesk Rust Preview',
                ('AppVersion=' + $release.normalized),
                ('VersionInfoVersion=' + $release.normalized + '.' + $release.build),
                'DefaultDirName={localappdata}\Programs\AetherDesk-Rust-Preview',
                'DefaultGroupName=AetherDesk Rust Preview',
                'PrivilegesRequired=lowest',
                'MinVersion=10.0',
                'ArchitecturesAllowed=x64compatible',
                'ArchitecturesInstallIn64BitMode=x64compatible',
                ('OutputDir=' + $packagesRoot),
                ('OutputBaseFilename=' + $installerName),
                'Compression=lzma2',
                'SolidCompression=yes',
                'UninstallDisplayIcon={app}\aetherdesk-rust-preview.exe',
                '[Files]',
                ('Source: "' + $package + '\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs'),
                '[Icons]',
                'Name: "{group}\AetherDesk Rust Preview"; Filename: "{app}\aetherdesk-rust-preview.exe"',
                '[Run]',
                'Filename: "{app}\aetherdesk-rust-preview.exe"; Description: "Open Rust development preview"; Flags: nowait postinstall skipifsilent'
            ) | Set-Content -LiteralPath $script -Encoding UTF8
            Invoke-StepTool (Get-Iscc) @($script)
            $result = Join-Path $packagesRoot ($installerName + '.exe')
            $files = @($result,$script)
            Set-Content -LiteralPath (Join-Path $outputRoot 'LATEST-PREVIEW-INSTALLER.txt') -Value $result -Encoding UTF8
        }
    }
    if ((Get-SourceFingerprint) -ne $fingerprint) { throw 'Source changed during this step. Retry using the latest source.' }
    Write-Record $Step $fingerprint $result $files $log
    Write-Host "STEP $Step COMPLETE: $result" -ForegroundColor Green
} catch {
    Write-Host "STEP $Step FAILED. Stop; newest log: $log" -ForegroundColor Red
    throw
} finally { if ($transcribing) { Stop-Transcript | Out-Null } }
