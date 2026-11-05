[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Destination,
    [string]$CacheDirectory = (Join-Path $env:LOCALAPPDATA 'AetherDesk\build-cache\x11'),
    [string]$SevenZip
)
$ErrorActionPreference = 'Stop'
$settings = (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'toolchain.json') -Raw | ConvertFrom-Json).vcxsrv
if (!$SevenZip) {
    $command = Get-Command '7z.exe' -ErrorAction SilentlyContinue
    $SevenZip = if ($command) { $command.Source } else { Join-Path $env:ProgramFiles '7-Zip\7z.exe' }
}
if (!(Test-Path -LiteralPath $SevenZip -PathType Leaf)) {
    throw 'The build machine needs 7-Zip to extract the private X11 runtime. End users do not need 7-Zip or VcXsrv.'
}
$package = [IO.Path]::GetFullPath($Destination)
$x11 = Join-Path $package 'x11'
if (Test-Path -LiteralPath $x11) { throw "Refusing to overlay an existing X11 bundle: $x11. Use a fresh package directory." }
New-Item -ItemType Directory -Force -Path $CacheDirectory | Out-Null

function Receive-X11Artifact([string]$Url, [string]$Partial) {
    $curl = Get-Command 'curl.exe' -ErrorAction SilentlyContinue
    if ($curl) {
        & $curl.Source --fail --location --silent --show-error --proto '=https' --proto-redir '=https' --connect-timeout 20 --max-time 600 --speed-limit 1024 --speed-time 60 --output $Partial $Url
        if ($LASTEXITCODE -ne 0) { throw "HTTPS transfer stopped (curl exit $LASTEXITCODE)." }
    } else {
        $previous = [Net.ServicePointManager]::SecurityProtocol
        try {
            [Net.ServicePointManager]::SecurityProtocol = $previous -bor [Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -Uri $Url -OutFile $Partial -UseBasicParsing -TimeoutSec 600
        } finally { [Net.ServicePointManager]::SecurityProtocol = $previous }
    }
}
function Wait-X11Retry([int]$Attempt) { Start-Sleep -Seconds ([Math]::Min(8, 2 * $Attempt)) }
function Get-VerifiedDownload($Artifact, [string]$Name) {
    if ($Artifact.sha256 -notmatch '^[0-9a-f]{64}$' -or $Artifact.url -notlike 'https://*') {
        throw "Invalid pinned X11 artifact: $Name"
    }
    $file = Join-Path $CacheDirectory $Name
    if (!(Test-Path -LiteralPath $file -PathType Leaf)) {
        $partial = $file + '.' + [guid]::NewGuid().ToString('N') + '.partial'
        try {
            $legacy = Join-Path (Join-Path $env:LOCALAPPDATA 'AetherDesk\build-cache\x11') $Name
            if ([IO.Path]::GetFullPath($legacy) -ne [IO.Path]::GetFullPath($file) -and
                (Test-Path -LiteralPath $legacy -PathType Leaf) -and
                (Get-FileHash -LiteralPath $legacy -Algorithm SHA256).Hash -eq $Artifact.sha256) {
                Write-Host "Reusing verified X11 cache: $Name"
                Copy-Item -LiteralPath $legacy -Destination $partial
            } else {
                for ($attempt = 1; $attempt -le 4; $attempt++) {
                    Write-Host "Downloading X11 artifact $Name (attempt $attempt/4)"
                    try { Receive-X11Artifact $Artifact.url $partial; break } catch {
                        if ($attempt -eq 4) { throw "Cannot download X11 artifact $Name after 4 attempts. Verified cache files were kept. Retry the build when the connection is available. Last error: $($_.Exception.Message)" }
                        Write-Warning "X11 download interrupted: $Name. Retrying."
                        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial }
                        Wait-X11Retry $attempt
                    }
                }
            }
            if ((Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash -ne $Artifact.sha256) {
                throw "Downloaded X11 artifact failed SHA-256 verification: $Name"
            }
            Move-Item -LiteralPath $partial -Destination $file -Force
        } finally {
            if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial }
        }
    }
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $Artifact.sha256) {
        throw "Cached X11 artifact failed SHA-256 verification: $file"
    }
    return $file
}

$version = $settings.version
$archive = Get-VerifiedDownload $settings.archive "vcxsrv-64.$version.installer.exe"
$source = Get-VerifiedDownload $settings.source "vcxsrv-source-$version.tar.gz"
$license = Get-VerifiedDownload $settings.license "vcxsrv-COPYING-$version"
New-Item -ItemType Directory -Force -Path $x11 | Out-Null
# Read the NSIS archive as data. Never run its installer, change the registry,
# install services, create shortcuts, or require elevation on a user's PC.
& $SevenZip x '-y' '-bso0' "-o$x11" '-xr!$PLUGINSDIR' '-xr!uninstall.exe' $archive
if ($LASTEXITCODE -ne 0) { throw "Cannot extract bundled X11 runtime (7-Zip exit $LASTEXITCODE)." }
foreach ($relative in @('vcxsrv.exe', 'xauth.exe', 'xkbcomp.exe', 'libX11.dll', 'libXau.dll',
    'libxcb.dll', 'swrast_dri.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll',
    'fonts\misc\fonts.dir', 'fonts\misc\fonts.alias', 'xkbdata\rules\base', 'locale\locale.alias')) {
    if (!(Test-Path -LiteralPath (Join-Path $x11 $relative) -PathType Leaf)) { throw "Incomplete X11 archive: $relative" }
}
$licenses = Join-Path $x11 'licenses'
New-Item -ItemType Directory -Force -Path $licenses | Out-Null
Copy-Item -LiteralPath $license -Destination (Join-Path $licenses 'COPYING')
Copy-Item -LiteralPath $source -Destination (Join-Path $licenses 'vcxsrv-source.tar.gz')
@"
AetherDesk bundles the unmodified VcXsrv $version Windows runtime.
Upstream: https://github.com/marchaesen/vcxsrv
Binary archive: $($settings.archive.url)
Matching source archive: $($settings.source.url)
The matching source is included as licenses/vcxsrv-source.tar.gz, with its
component copyright/license notices and build scripts. See licenses/COPYING.
This private runtime is extracted at build time; no VcXsrv installation is run.
"@ | Set-Content -LiteralPath (Join-Path $licenses 'README.txt') -Encoding utf8
[ordered]@{
    version = $version
    archiveUrl = $settings.archive.url
    archiveSha256 = $settings.archive.sha256
    sourceUrl = $settings.source.url
    sourceSha256 = $settings.source.sha256
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $x11 'bundle-info.json') -Encoding utf8
Write-Host "Bundled private X11 runtime $version in $x11"
