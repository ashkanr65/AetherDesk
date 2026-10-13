[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('Toolchain', 'EditorAssets', 'PrepareRunner', 'CheckUI', 'NativeTests', 'TestCore', 'BuildCore', 'BuildUI')][string]$Stage,
    [string]$Repository = ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..')))
)
$ErrorActionPreference = 'Stop'
function Invoke-Tool([string]$Program, [string[]]$Arguments) {
    Get-Command $Program -ErrorAction Stop | Out-Null
    $saved = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        & $Program @Arguments 2>&1 | ForEach-Object {
            if ($_ -is [Management.Automation.ErrorRecord]) { $_.Exception.Message } else { $_.ToString() }
        } | Out-Host
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $saved }
    if ($code -ne 0) { throw "$Program failed with exit code $code" }
}
$project = Join-Path $Repository 'ui\flutter\ad_remote_desktop'
$python = & (Join-Path $PSScriptRoot 'Resolve-Python.ps1')
$release = & (Join-Path $PSScriptRoot 'Release-Version.ps1') -Repository $Repository
if ($Stage -eq 'Toolchain') {
    Invoke-Tool $python @('-X', 'utf8', (Join-Path $Repository 'tools\version.py'))
} else {
    Invoke-Tool $python @('-X', 'utf8', (Join-Path $Repository 'tools\version.py'), '--check')
}
Push-Location -LiteralPath $Repository
try {
    switch ($Stage) {
        Toolchain {
            $settings = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'toolchain.json') -Raw | ConvertFrom-Json
            if ($env:GITHUB_OUTPUT) {
                "flutter=$($settings.flutter)" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
                "version=$($release.version)" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
                "gstreamer=$($settings.gstreamer)" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
            }
            $settings
        }
        EditorAssets {
            $node = & (Join-Path $PSScriptRoot 'Resolve-Node.ps1')
            $env:Path = $node.directory + ';' + $env:Path
            Set-Location -LiteralPath (Join-Path $Repository 'ui\editor')
            Invoke-Tool $node.npm @('ci', '--no-audit', '--no-fund')
            Invoke-Tool $node.npm @('test')
            Invoke-Tool $node.npm @('run', 'build')
            Invoke-Tool $python @('-X', 'utf8', (Join-Path $Repository 'tools\build-kit\Verify-Editor.py'), '--assets', (Join-Path $project 'assets\editor'))
        }
        PrepareRunner {
            $scaffold = Join-Path $env:TEMP ('aetherdesk_scaffold_' + [guid]::NewGuid().ToString('N'))
            Invoke-Tool 'flutter' @('config', '--enable-windows-desktop')
            Invoke-Tool 'flutter' @('create', '--platforms=windows', '--project-name', 'aetherdesk', '--no-pub', $scaffold)
            Get-ChildItem -LiteralPath (Join-Path $scaffold 'windows') -Force |
                Copy-Item -Destination (Join-Path $project 'windows') -Recurse -Force
            Copy-Item -LiteralPath (Join-Path $scaffold '.metadata') -Destination $project -Force
            Copy-Item -LiteralPath (Join-Path $project 'assets\aetherdesk.ico') -Destination (Join-Path $project 'windows\runner\resources\app_icon.ico') -Force
            $runner = Join-Path $project 'windows\runner\main.cpp'
            [IO.File]::WriteAllText($runner, [IO.File]::ReadAllText($runner).Replace('L"aetherdesk"', 'L"AetherDesk"'), [Text.UTF8Encoding]::new($false))
            & (Join-Path $PSScriptRoot 'Configure-Editor-Windows.ps1') -Repository $Repository
        }
        CheckUI {
            Set-Location -LiteralPath $project
            Invoke-Tool 'flutter' @('pub', 'get')
            Invoke-Tool 'flutter' @('analyze', '--fatal-infos')
            Invoke-Tool 'flutter' @('test')
        }
        NativeTests {
            Invoke-Tool 'cmake' @('-S', 'ui/flutter/ad_stream_surface/windows/tests', '-B', 'build/native-desktop-tests', '-G', 'Visual Studio 17 2022', '-A', 'x64')
            Invoke-Tool 'cmake' @('--build', 'build/native-desktop-tests', '--config', 'Release')
            Invoke-Tool 'ctest' @('--test-dir', 'build/native-desktop-tests', '-C', 'Release', '--output-on-failure')
        }
        TestCore {
            Set-Location -LiteralPath (Join-Path $Repository 'core\rust')
            Invoke-Tool 'cargo' @('fmt', '--check', '-p', 'ad-ffi', '-p', 'ad-ssh')
            Invoke-Tool 'cargo' @('test', '--locked', '--release', '--target', 'x86_64-pc-windows-msvc',
                '-p', 'ad-vpn', '-p', 'ad-ssh', '-p', 'ad-display', '-p', 'ad-rdp-bridge',
                '-p', 'ad-stream-protocol', '-p', 'ad-stream-client', '-p', 'ad-display-decoder', '-p', 'ad-remote-session', '-p', 'ad-ffi')
        }
        BuildCore {
            Set-Location -LiteralPath (Join-Path $Repository 'core\rust')
            Invoke-Tool 'cargo' @('build', '--locked', '--release', '--target', 'x86_64-pc-windows-msvc', '-p', 'ad-ffi')
        }
        BuildUI {
            Invoke-Tool $python @('-X', 'utf8', (Join-Path $Repository 'tools\build-kit\Verify-Editor.py'), '--assets', (Join-Path $project 'assets\editor'))
            Set-Location -LiteralPath $project
            Invoke-Tool 'flutter' @('build', 'windows', '--release')
        }
    }
} finally { Pop-Location }
