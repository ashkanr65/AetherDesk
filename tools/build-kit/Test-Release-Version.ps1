$ErrorActionPreference = 'Stop'
$reader = Join-Path $PSScriptRoot 'Release-Version.ps1'
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('aetherdesk-release-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporary | Out-Null
try {
    foreach ($test in @(@{ version='0.2'; build=1; expected='0.2.0' }, @{ version='0.2.3'; build=65535; expected='0.2.3' })) {
        $test | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $temporary 'release.json') -Encoding UTF8
        $value = & $reader -Repository $temporary
        if ($value.normalized -ne $test.expected -or $value.build -ne $test.build) { throw 'Release normalization failed.' }
    }
    foreach ($invalid in @('{"version":0.2,"build":1}', '{"version":"01.2","build":1}', '{"version":"0.2","build":true}', '{"version":"0.2","build":0}', '{"version":"0.2","build":1.5}', '{"version":"65536.2","build":1}')) {
        Set-Content -LiteralPath (Join-Path $temporary 'release.json') -Value $invalid -Encoding UTF8
        $failed = $false
        try { $null = & $reader -Repository $temporary } catch { $failed = $true }
        if (!$failed) { throw "Accepted an invalid manifest: $invalid" }
    }
} finally {
    $full = [IO.Path]::GetFullPath($temporary)
    $prefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($full) -notlike 'aetherdesk-release-test-*') { throw 'Unsafe test cleanup path.' }
    Remove-Item -LiteralPath $full -Recurse -Force
}
Write-Host 'PASS: PowerShell release normalization and invalid-manifest rejection.'
