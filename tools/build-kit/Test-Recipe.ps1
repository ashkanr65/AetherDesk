$ErrorActionPreference = 'Stop'
foreach ($file in Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.ps1') {
    $tokens = $null
    $errors = $null
    $null = [Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw ($errors | Out-String) }
    if ([IO.File]::ReadAllText($file.FullName) -match '[\x00-\x08\x0B\x0C\x0E-\x1F]') { throw "Control character in build script: $($file.Name)" }
}
$global:AetherRecipeTestCalls = [Collections.Generic.List[string]]::new()
$global:AetherRecipeTestFailCargo = $false
function cargo { $global:AetherRecipeTestCalls.Add('cargo ' + ($args -join ' ')); $global:LASTEXITCODE = if ($global:AetherRecipeTestFailCargo) { 7 } else { 0 } }
function flutter { $global:AetherRecipeTestCalls.Add('flutter ' + ($args -join ' ')); $global:LASTEXITCODE = 0 }
function cmake { $global:AetherRecipeTestCalls.Add('cmake ' + ($args -join ' ')); $global:LASTEXITCODE = 0 }
function ctest { $global:AetherRecipeTestCalls.Add('ctest ' + ($args -join ' ')); $global:LASTEXITCODE = 0 }
& (Join-Path $PSScriptRoot 'Test-Release-Version.ps1')
& (Join-Path $PSScriptRoot 'Test-Editor-Windows.ps1')
& (Join-Path $PSScriptRoot 'Test-Assistant-Recipe.ps1')
$assistantPython = & (Join-Path $PSScriptRoot 'Resolve-Python.ps1')
& $assistantPython -X utf8 (Join-Path $PSScriptRoot 'test_assistant_bundle.py')
if ($LASTEXITCODE -ne 0) { throw 'Assistant packaging regression checks failed.' }
& $assistantPython -X utf8 (Join-Path $PSScriptRoot '..\test_x11_icons.py')
if ($LASTEXITCODE -ne 0) { throw 'X11 helper lifecycle checks failed.' }
$stages = Join-Path $PSScriptRoot 'Windows-Stages.ps1'
foreach ($stage in @('CheckUI', 'NativeTests', 'TestCore', 'BuildCore', 'BuildUI')) { & $stages -Stage $stage }
if ($global:AetherRecipeTestCalls.Count -ne 10) { throw "Expected ten native commands, got $($global:AetherRecipeTestCalls.Count)" }
if (!($global:AetherRecipeTestCalls | Where-Object { $_ -like 'cargo test --locked --release*ad-display-decoder*ad-ffi*' })) { throw 'Core tests omitted a required package or lockfile check.' }
if (!($global:AetherRecipeTestCalls -contains 'cargo fmt --check -p ad-ffi -p ad-ssh')) { throw 'Missing SSH/FFI formatting gate.' }
if (!($global:AetherRecipeTestCalls -contains 'flutter build windows --release')) { throw 'Missing release UI build.' }
if (!($global:AetherRecipeTestCalls -contains 'ctest --test-dir build/native-desktop-tests -C Release --output-on-failure')) { throw 'Native tests must use the ignored build directory.' }
$global:AetherRecipeTestFailCargo = $true
$failed = $false
try { & $stages -Stage BuildCore } catch {
    if ($_.Exception.Message -notlike '*exit code 7*') { throw }
    $failed = $true
}
if (!$failed) { throw 'A failed compiler must stop the build.' }
$global:AetherRecipeTestFailCargo = $false
$settings = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'toolchain.json') -Raw | ConvertFrom-Json
foreach ($version in @($settings.flutter, $settings.gstreamer)) {
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Invalid toolchain version.' }
}
if ($settings.vcxsrv.version -notmatch '^\d+\.\d+\.\d+\.\d+$') { throw 'Invalid X11 runtime version.' }
foreach ($artifact in @($settings.vcxsrv.archive, $settings.vcxsrv.source, $settings.vcxsrv.license)) {
    if ($artifact.sha256 -notmatch '^[0-9a-f]{64}$' -or $artifact.url -notlike 'https://*') { throw 'X11 downloads must be pinned by SHA-256 over HTTPS.' }
}
$repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
foreach ($relative in @('tools\build-kit\Build-Worker.ps1', '.github\workflows\build-windows.yml')) {
    if ((Get-Content -LiteralPath (Join-Path $repository $relative) -Raw) -notmatch 'Bundle-X11\.ps1') {
        throw "Packaging does not bundle X11: $relative"
    }
    $recipe = Get-Content -LiteralPath (Join-Path $repository $relative) -Raw
    if ($recipe -notmatch 'Bundle-Assistant\.ps1' -or $recipe -notmatch 'Verify-Assistant\.py' -or
        $recipe -notmatch 'Test-Assistant\.ps1') {
        throw "Packaging must bundle and verify the AI Assistant: $relative"
    }
}
if ((Get-Content -LiteralPath (Join-Path $repository '.github\scripts\verify-windows-runtime.py') -Raw) -notmatch 'verify_x11_runtime\(package\)') {
    throw 'The portable package check must exercise bundled X11.'
}

function Expect-X11Failure([scriptblock]$Action, [string]$Expected) {
    try { & $Action } catch {
        if ($_.Exception.Message -notlike $Expected) { throw }
        return
    }
    throw "X11 packaging should have failed: $Expected"
}
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('aetherdesk-x11-recipe-' + [guid]::NewGuid().ToString('N'))
try {
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    $cache = Join-Path $testRoot 'cache'
    New-Item -ItemType Directory -Path $cache | Out-Null
    $destination = Join-Path $testRoot 'package'
    $extractor = Join-Path $testRoot 'fake-extractor.exe'
    [IO.File]::WriteAllText($extractor, 'Never execute this file')
    $bundler = Join-Path $PSScriptRoot 'Bundle-X11.ps1'
    Expect-X11Failure { & $bundler -Destination $destination -CacheDirectory $cache -SevenZip (Join-Path $testRoot 'missing.exe') } '*build machine needs 7-Zip*'
    [IO.File]::WriteAllText((Join-Path $cache "vcxsrv-64.$($settings.vcxsrv.version).installer.exe"), 'corrupt archive')
    Expect-X11Failure { & $bundler -Destination $destination -CacheDirectory $cache -SevenZip $extractor } '*Cached X11 artifact failed SHA-256 verification*'
    if (Test-Path -LiteralPath (Join-Path $destination 'x11')) { throw 'Unverified archive must never be extracted.' }
    New-Item -ItemType Directory -Path (Join-Path $destination 'x11') -Force | Out-Null
    $preserved = Join-Path $destination 'x11\preserved.txt'
    [IO.File]::WriteAllText($preserved, 'keep')
    Expect-X11Failure { & $bundler -Destination $destination -CacheDirectory $cache -SevenZip $extractor } '*Refusing to overlay*'
    if ([IO.File]::ReadAllText($preserved) -ne 'keep') { throw 'Existing package was modified.' }
} finally {
    $resolved = [IO.Path]::GetFullPath($testRoot)
    $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$resolved.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolved) -notlike 'aetherdesk-x11-recipe-*') { throw 'Invalid test cleanup path.' }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
Write-Host 'PASS: recipe syntax, shared Windows commands, pinned X11 packaging, corrupt-cache rejection and failure propagation.'
# Expected failing compiler probes must not become the GitHub step's exit code.
# Clear it only after every assertion and cleanup succeeded.
$global:LASTEXITCODE = 0
