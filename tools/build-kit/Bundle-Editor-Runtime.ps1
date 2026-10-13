[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Destination,
    [string]$Repository = ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))),
    [string]$Installer = $env:AETHERDESK_WEBVIEW2_INSTALLER,
    [string]$CacheDirectory = (Join-Path $env:LOCALAPPDATA 'AetherDesk\build-cache\webview2')
)
$ErrorActionPreference = 'Stop'
$destinationRoot = [IO.Path]::GetFullPath($Destination)
$redist = Join-Path $destinationRoot 'redist'
if (Test-Path -LiteralPath $redist) { throw "Refusing to overlay the editor runtime directory: $redist" }
$settings = (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'toolchain.json') -Raw | ConvertFrom-Json).webview2
if (!$Installer) {
    New-Item -ItemType Directory -Path $CacheDirectory -Force | Out-Null
    $Installer = Join-Path $CacheDirectory 'MicrosoftEdgeWebView2RuntimeInstallerX64.exe'
    if (!(Test-Path -LiteralPath $Installer)) {
        $partial = $Installer + '.partial'
        Write-Host 'Downloading the Microsoft WebView2 offline installer for inclusion in the package.'
        Invoke-WebRequest -UseBasicParsing -Uri $settings.url -OutFile $partial
        Move-Item -LiteralPath $partial -Destination $Installer -Force
    }
}
if (!(Test-Path -LiteralPath $Installer -PathType Leaf)) { throw "WebView2 standalone installer is missing: $Installer" }
if ((Get-Item -LiteralPath $Installer).Length -lt $settings.minimumBytes) {
    throw 'WebView2 requires the full x64 standalone installer; a bootstrapper cannot support offline installation.'
}
$signature = Get-AuthenticodeSignature -LiteralPath $Installer
if ($signature.Status -ne 'Valid' -or !$signature.SignerCertificate -or
    $signature.SignerCertificate.Subject -notmatch '(^|,\s*)O=Microsoft Corporation(,|$)') {
    throw 'WebView2 installer must have a valid Microsoft Corporation Authenticode signature. Remove an invalid cached file and retry.'
}
$plugin = Join-Path $destinationRoot 'webview_windows_plugin.dll'
if (!(Test-Path -LiteralPath $plugin -PathType Leaf)) { throw 'The Flutter package is missing webview_windows_plugin.dll.' }
$loader = Join-Path $destinationRoot 'WebView2Loader.dll'
if (!(Test-Path -LiteralPath $loader)) {
    $matchingLoader = Join-Path $Repository 'ui\flutter\ad_remote_desktop\build\windows\x64\packages\Microsoft.Web.WebView2\build\native\x64\WebView2Loader.dll'
    if (!(Test-Path -LiteralPath $matchingLoader)) { throw "The matching WebView2 loader is missing: $matchingLoader" }
    Copy-Item -LiteralPath $matchingLoader -Destination $loader
}
New-Item -ItemType Directory -Path $redist -Force | Out-Null
$packaged = Join-Path $redist 'MicrosoftEdgeWebView2RuntimeInstallerX64.exe'
Copy-Item -LiteralPath $Installer -Destination $packaged
[ordered]@{
    schemaVersion = 1
    distribution = 'Microsoft WebView2 Evergreen Standalone x64'
    source = $settings.url
    sha256 = (Get-FileHash -LiteralPath $packaged -Algorithm SHA256).Hash.ToLowerInvariant()
    fileVersion = (Get-Item -LiteralPath $packaged).VersionInfo.FileVersion
    signer = $signature.SignerCertificate.Subject
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $redist 'webview2-manifest.json') -Encoding utf8
foreach ($name in @('Install-Editor-Runtime.cmd', 'Install-Editor-Runtime.ps1')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $destinationRoot
}
Write-Host 'Bundled the offline editor runtime, matching loader and portable installer helper.'
