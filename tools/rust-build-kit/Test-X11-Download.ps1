$ErrorActionPreference = 'Stop'
$tokens = $null; $errors = $null
$bundler = Join-Path $PSScriptRoot '..\build-kit\Bundle-X11.ps1'
function Expect-X11Failure([scriptblock]$Action, [string]$Expected) {
    try { & $Action } catch {
        if ($_.Exception.Message -notlike $Expected) { throw }
        return
    }
    throw "X11 download should have failed: $Expected"
}
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('aetherdesk-x11-download-' + [guid]::NewGuid().ToString('N'))
$originalLocalAppData = $env:LOCALAPPDATA
try {
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    # Exercise the actual download/verification logic with transport failures,
    # without contacting an external service or running an installer.
    $ast = [Management.Automation.Language.Parser]::ParseFile($bundler, [ref]$tokens, [ref]$errors)
    $node = $ast.Find({param($entry) $entry -is [Management.Automation.Language.FunctionDefinitionAst] -and $entry.Name -eq 'Get-VerifiedDownload'}, $true)
    if (!$node) { throw 'Verified X11 downloader is missing.' }
    . ([scriptblock]::Create($node.Extent.Text))
    $CacheDirectory = Join-Path $testRoot 'download-tests'
    New-Item -ItemType Directory -Path $CacheDirectory | Out-Null
    $env:LOCALAPPDATA = Join-Path $testRoot 'local-appdata'
    $fixtureBytes = [Text.Encoding]::UTF8.GetBytes('pinned X11 test bytes')
    $hash = [Security.Cryptography.SHA256]::Create()
    try { $fixtureHash = -join ($hash.ComputeHash($fixtureBytes) | ForEach-Object { $_.ToString('x2') }) } finally { $hash.Dispose() }
    $artifact = [pscustomobject]@{url='https://example.test/pinned-x11';sha256=$fixtureHash}
    $script:downloadCalls = 0
    $script:downloadMode = 'transient'
    function Wait-X11Retry([int]$Attempt) {}
    function Receive-X11Artifact([string]$Url,[string]$Partial) {
        $script:downloadCalls++
        [IO.File]::WriteAllText($Partial,'incomplete')
        if ($script:downloadMode -eq 'always' -or ($script:downloadMode -eq 'transient' -and $script:downloadCalls -lt 3)) { throw 'Simulated dropped HTTPS connection' }
        if ($script:downloadMode -ne 'corrupt') { [IO.File]::WriteAllBytes($Partial,$fixtureBytes) }
    }
    $verified = Get-VerifiedDownload $artifact 'retry-fixture'
    if ($script:downloadCalls -ne 3 -or (Get-FileHash -LiteralPath $verified -Algorithm SHA256).Hash -ne $fixtureHash) { throw 'Interrupted transfers were not retried and verified.' }
    $null = Get-VerifiedDownload $artifact 'retry-fixture'
    if ($script:downloadCalls -ne 3) { throw 'Verified cache must avoid another download.' }
    $script:downloadCalls = 0; $script:downloadMode = 'always'
    Expect-X11Failure { Get-VerifiedDownload $artifact 'failed-fixture' } '*after 4 attempts*'
    if ($script:downloadCalls -ne 4 -or (Test-Path -LiteralPath (Join-Path $CacheDirectory 'failed-fixture'))) { throw 'Failed transfer was published or retry count was unbounded.' }
    $script:downloadCalls = 0; $script:downloadMode = 'corrupt'
    Expect-X11Failure { Get-VerifiedDownload $artifact 'corrupt-fixture' } '*Downloaded X11 artifact failed SHA-256 verification*'
    if ($script:downloadCalls -ne 1 -or (Test-Path -LiteralPath (Join-Path $CacheDirectory 'corrupt-fixture'))) { throw 'Hash mismatch must never publish the artifact.' }
    [IO.File]::WriteAllText((Join-Path $CacheDirectory 'existing-corrupt'),'preserve for diagnosis')
    Expect-X11Failure { Get-VerifiedDownload $artifact 'existing-corrupt' } '*Cached X11 artifact failed SHA-256 verification*'
    if ([IO.File]::ReadAllText((Join-Path $CacheDirectory 'existing-corrupt')) -ne 'preserve for diagnosis') { throw 'Corrupt existing cache was overwritten.' }
    $legacy = Join-Path $env:LOCALAPPDATA 'AetherDesk\build-cache\x11'
    New-Item -ItemType Directory -Force -Path $legacy | Out-Null
    [IO.File]::WriteAllBytes((Join-Path $legacy 'legacy-fixture'),$fixtureBytes)
    $script:downloadCalls = 0
    $null = Get-VerifiedDownload $artifact 'legacy-fixture'
    if ($script:downloadCalls -ne 0) { throw 'A verified older cache must be reused without network access.' }
    Expect-X11Failure { Get-VerifiedDownload ([pscustomobject]@{url='http://example.test';sha256=$fixtureHash}) 'unsafe-fixture' } '*Invalid pinned X11 artifact*'
    if (@(Get-ChildItem -LiteralPath $CacheDirectory -Filter '*.partial').Count) { throw 'Failed download left an incomplete artifact behind.' }
} finally {
    $env:LOCALAPPDATA = $originalLocalAppData
    $resolved = [IO.Path]::GetFullPath($testRoot)
    $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$resolved.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolved) -notlike 'aetherdesk-x11-download-*') { throw 'Invalid test cleanup path.' }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
Write-Host 'PASS: bounded X11 download retries, pinned hashes, cache reuse, corrupt-cache rejection and partial cleanup (transport simulated).'
$global:LASTEXITCODE = 0
