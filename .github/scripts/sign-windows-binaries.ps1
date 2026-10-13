[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Path,

    [Parameter(Mandatory = $true)]
    [string] $CertificateBase64,

    [Parameter(Mandatory = $true)]
    [string] $CertificatePassword
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$resolvedPath = (Resolve-Path -LiteralPath $Path).Path
$binaries = @(Get-ChildItem -LiteralPath $resolvedPath -Recurse -File |
    Where-Object { $_.Extension -in '.exe', '.dll' })

if ($binaries.Count -eq 0) {
    throw "No Windows executables or libraries were found under '$resolvedPath'."
}

$signTool = Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin" `
    -Filter signtool.exe -Recurse -File |
    Where-Object { $_.DirectoryName -match '[\\/]x64$' } |
    Sort-Object FullName -Descending |
    Select-Object -First 1

if ($null -eq $signTool) {
    throw 'The x64 Windows SDK signing tool was not found on this runner.'
}

$certificatePath = Join-Path $env:RUNNER_TEMP 'aetherdesk-signing-certificate.pfx'

try {
    try {
        $certificateBytes = [Convert]::FromBase64String($CertificateBase64)
    } catch {
        throw 'WINDOWS_SIGNING_CERTIFICATE is not valid base64-encoded PFX data.'
    }

    [IO.File]::WriteAllBytes($certificatePath, $certificateBytes)

    foreach ($binary in $binaries) {
        # Retain the publisher signature only for this verified Microsoft
        # redistributable. Re-signing it would invalidate offline installation.
        $webviewInstaller = Join-Path $resolvedPath 'redist\MicrosoftEdgeWebView2RuntimeInstallerX64.exe'
        if ($binary.FullName -eq $webviewInstaller) {
            $signature = Get-AuthenticodeSignature -LiteralPath $binary.FullName
            $manifest = Get-Content -LiteralPath (Join-Path $resolvedPath 'redist\webview2-manifest.json') -Raw | ConvertFrom-Json
            if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or
                !$signature.SignerCertificate -or
                $signature.SignerCertificate.Subject -notmatch '(^|,\s*)O=Microsoft Corporation(,|$)' -or
                (Get-FileHash -LiteralPath $binary.FullName -Algorithm SHA256).Hash -ne $manifest.sha256) {
                throw 'The bundled WebView2 installer must retain its verified Microsoft signature and package checksum.'
            }
            continue
        }
        & $signTool.FullName sign `
            /fd SHA256 `
            /tr 'https://timestamp.digicert.com' `
            /td SHA256 `
            /f $certificatePath `
            /p $CertificatePassword `
            $binary.FullName
        if ($LASTEXITCODE -ne 0) {
            throw "Authenticode signing failed for '$($binary.FullName)'."
        }
    }

    foreach ($binary in $binaries) {
        & $signTool.FullName verify /pa /all /tw $binary.FullName
        if ($LASTEXITCODE -ne 0) {
            throw "Authenticode verification failed for '$($binary.FullName)'."
        }

        $signature = Get-AuthenticodeSignature -LiteralPath $binary.FullName
        if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
            throw "'$($binary.FullName)' has signature status '$($signature.Status)'."
        }
    }
    # Authenticode changes the executable bytes. Reseal the package manifest
    # only after all signatures have been verified, so the application can check
    # the distributed (signed) binary without mistaking signing for corruption.
    $assistantRoot = Join-Path $resolvedPath 'ai\assistant'
    $assistantManifest = Join-Path $assistantRoot 'bundle-info.json'
    if (Test-Path -LiteralPath $assistantManifest -PathType Leaf) {
        $metadata = Get-Content -LiteralPath $assistantManifest -Raw | ConvertFrom-Json
        if ($metadata.engine -ne 'langgraph' -or $metadata.executable -ne 'python/python.exe') {
            throw 'Invalid assistant signing manifest.'
        }
        $runtimePrefix = [IO.Path]::GetFullPath($assistantRoot).TrimEnd('\') + '\'
        foreach ($entry in $metadata.files) {
            $file = [IO.Path]::GetFullPath((Join-Path $assistantRoot $entry.path))
            if (!$file.StartsWith($runtimePrefix, [StringComparison]::OrdinalIgnoreCase)) {
                throw 'Unsafe assistant signing manifest path.'
            }
            if ([IO.Path]::GetExtension($file) -in '.exe', '.dll') {
                $entry.sha256 = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
                $entry.size = (Get-Item -LiteralPath $file).Length
            }
        }
        [IO.File]::WriteAllText($assistantManifest, ($metadata | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
    }
} finally {
    if (Test-Path -LiteralPath $certificatePath) {
        Remove-Item -LiteralPath $certificatePath -Force
    }
}

Write-Host "Signed and verified $($binaries.Count) Windows binaries under '$resolvedPath'."
