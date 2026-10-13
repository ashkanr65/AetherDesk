# Distributed beside AetherDesk.exe. Installs only the packaged Microsoft runtime.
$ErrorActionPreference = 'Stop'
function Test-EditorRuntime {
    foreach ($key in @(
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
        'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    )) {
        $version = (Get-ItemProperty -LiteralPath $key -Name pv -ErrorAction SilentlyContinue).pv
        try { if ($version -and [version]$version -gt [version]'0.0.0.0') { return $true } } catch { }
    }
    return $false
}
if (Test-EditorRuntime) { Write-Host 'The editor runtime is already installed. Reopen the editor in AetherDesk.'; exit 0 }
$installer = Join-Path $PSScriptRoot 'redist\MicrosoftEdgeWebView2RuntimeInstallerX64.exe'
$manifest = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'redist\webview2-manifest.json') -Raw | ConvertFrom-Json
if ((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash -ne $manifest.sha256) { throw 'The packaged editor runtime has changed. Extract a fresh AetherDesk package.' }
$signature = Get-AuthenticodeSignature -LiteralPath $installer
if ($signature.Status -ne 'Valid' -or !$signature.SignerCertificate -or
    $signature.SignerCertificate.Subject -notmatch '(^|,\s*)O=Microsoft Corporation(,|$)') { throw 'The packaged runtime does not have a valid Microsoft signature.' }
$process = Start-Process -FilePath $installer -ArgumentList @('/silent', '/install') -WindowStyle Hidden -Wait -PassThru
if (!(Test-EditorRuntime)) { throw "The editor runtime could not be installed (exit $($process.ExitCode)). Ask your Windows administrator to install Microsoft Edge WebView2 Runtime." }
Write-Host 'Editor runtime installed. Reopen the editor in AetherDesk.'
