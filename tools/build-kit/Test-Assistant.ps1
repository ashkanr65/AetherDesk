[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Package,
    [string]$Repository
)
$ErrorActionPreference = 'Stop'
if (!$Repository) { $Repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..')) }
$packageRoot = (Resolve-Path -LiteralPath $Package -ErrorAction Stop).Path
$project = Join-Path $Repository 'ui\flutter\ad_remote_desktop'
foreach ($relative in @('ai\assistant\python\python.exe', 'ai\assistant\bundle-info.json', 'ai\assistant\python\LICENSE.txt', 'ai\assistant\server.py')) {
    if (!(Test-Path -LiteralPath (Join-Path $packageRoot $relative) -PathType Leaf)) {
        throw "Cannot test the AI Assistant: packaged runtime file is missing: $relative"
    }
}
if (!(Test-Path -LiteralPath (Join-Path $project 'test\assistant_integration_test.dart') -PathType Leaf)) {
    throw "The AI Assistant integration test is missing from $project"
}
$hadPackageVariable = Test-Path Env:AETHERDESK_ASSISTANT_PACKAGE
$previousPackage = $env:AETHERDESK_ASSISTANT_PACKAGE
Push-Location -LiteralPath $project
try {
    $env:AETHERDESK_ASSISTANT_PACKAGE = $packageRoot
    Get-Command flutter -ErrorAction Stop | Out-Null
    $savedPreference = $ErrorActionPreference
    try {
        # Preserve useful native stderr on Windows PowerShell without treating
        # ordinary progress messages as errors; Flutter's exit code is decisive.
        $ErrorActionPreference = 'Continue'
        & flutter test --no-pub test/assistant_integration_test.dart 2>&1 | ForEach-Object {
            if ($_ -is [Management.Automation.ErrorRecord]) { $_.Exception.Message } else { $_.ToString() }
        } | Out-Host
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $savedPreference }
    if ($code -ne 0) { throw "AI Assistant integration test failed with exit code $code" }
} finally {
    if ($hadPackageVariable) { $env:AETHERDESK_ASSISTANT_PACKAGE = $previousPackage }
    else { Remove-Item Env:AETHERDESK_ASSISTANT_PACKAGE -ErrorAction SilentlyContinue }
    Pop-Location
}
Write-Host 'PASS: bundled AI Assistant chat, file access and permission checks.'
