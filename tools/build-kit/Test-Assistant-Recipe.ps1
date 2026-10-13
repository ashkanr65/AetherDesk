# Check the integration launcher without running Flutter or any application build.
$ErrorActionPreference = 'Stop'
$helper = Join-Path $PSScriptRoot 'Test-Assistant.ps1'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('aetherdesk-assistant-recipe-' + [guid]::NewGuid().ToString('N'))
$hadPackageVariable = Test-Path Env:AETHERDESK_ASSISTANT_PACKAGE
$savedPackage = $env:AETHERDESK_ASSISTANT_PACKAGE
$originalLocation = (Get-Location).Path
$global:AetherAssistantRecipeExit = 0
$global:AetherAssistantRecipeCalls = [Collections.Generic.List[string]]::new()
function flutter {
    if (!(Test-Path -LiteralPath (Join-Path $env:AETHERDESK_ASSISTANT_PACKAGE 'ai\assistant\python\python.exe'))) {
        throw 'The integration launcher did not supply its package path.'
    }
    if (!(Test-Path -LiteralPath 'test/assistant_integration_test.dart')) {
        throw 'The integration launcher did not select the Flutter project.'
    }
    $global:AetherAssistantRecipeCalls.Add(($args -join ' '))
    $global:LASTEXITCODE = $global:AetherAssistantRecipeExit
}
try {
    $runtime = Join-Path $testRoot 'package with spaces\ai\assistant'
    New-Item -ItemType Directory -Force -Path $runtime | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $runtime 'python') | Out-Null
    foreach ($name in @('python\python.exe', 'bundle-info.json', 'python\LICENSE.txt', 'server.py')) {
        [IO.File]::WriteAllText((Join-Path $runtime $name), 'Launcher test fixture only; never executed.')
    }
    $package = Join-Path $testRoot 'package with spaces'
    $env:AETHERDESK_ASSISTANT_PACKAGE = 'previous package value'
    & $helper -Package $package
    if ($env:AETHERDESK_ASSISTANT_PACKAGE -ne 'previous package value' -or (Get-Location).Path -ne $originalLocation) {
        throw 'The successful assistant test did not restore the caller environment.'
    }
    if ($global:AetherAssistantRecipeCalls.Count -ne 1 -or
        $global:AetherAssistantRecipeCalls[0] -ne 'test --no-pub test/assistant_integration_test.dart') {
        throw 'The assistant launcher must run only the focused integration test without resolving dependencies.'
    }
    Remove-Item Env:AETHERDESK_ASSISTANT_PACKAGE
    $global:AetherAssistantRecipeExit = 23
    $failed = $false
    try { & $helper -Package $package } catch {
        if ($_.Exception.Message -notlike '*failed with exit code 23*') { throw }
        $failed = $true
    }
    if (!$failed) { throw 'A failed assistant integration test must stop packaging.' }
    if ((Test-Path Env:AETHERDESK_ASSISTANT_PACKAGE) -or (Get-Location).Path -ne $originalLocation) {
        throw 'The failed assistant test did not restore the caller environment.'
    }
    Remove-Item -LiteralPath (Join-Path $runtime 'python\python.exe')
    $rejected = $false
    try { & $helper -Package $package } catch {
        if ($_.Exception.Message -notlike '*packaged runtime file is missing*') { throw }
        $rejected = $true
    }
    if (!$rejected -or $global:AetherAssistantRecipeCalls.Count -ne 2) {
        throw 'An incomplete package must not launch the integration test.'
    }
} finally {
    if ($hadPackageVariable) { $env:AETHERDESK_ASSISTANT_PACKAGE = $savedPackage }
    else { Remove-Item Env:AETHERDESK_ASSISTANT_PACKAGE -ErrorAction SilentlyContinue }
    $resolved = [IO.Path]::GetFullPath($testRoot)
    $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$resolved.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolved) -notlike 'aetherdesk-assistant-recipe-*') { throw 'Invalid assistant recipe cleanup path.' }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
$global:LASTEXITCODE = 0
Write-Host 'PASS: AI integration launcher, failure propagation and environment restoration.'
