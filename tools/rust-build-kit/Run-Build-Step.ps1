[CmdletBinding(DefaultParameterSetName='All')]
param(
    [Parameter(Mandatory=$true,ParameterSetName='Single')][ValidateRange(0,6)][int]$Step,
    [Parameter(ParameterSetName='All')][switch]$Rebuild,
    [Parameter(ParameterSetName='All')][switch]$PortableOnly
)
. (Join-Path $PSScriptRoot 'Build-Common.ps1')
$mutex = [Threading.Mutex]::new($false,'Local\AetherDeskRustPreviewBuild')
$locked = $false
try {
    try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
    if (!$locked) { throw 'Another Rust preview build is running.' }
    $fingerprint = Get-SourceFingerprint
    $steps = if ($PSCmdlet.ParameterSetName -eq 'Single') { @($Step) } elseif ($PortableOnly) { @(0,1,2,3,4,5) } else { @(0,1,2,3,4,5,6) }
    if ($PSCmdlet.ParameterSetName -eq 'All' -and !$PortableOnly) { $null = Get-Iscc }
    Write-Host 'Building the incomplete Rust DEVELOPMENT PREVIEW, not a replacement release.' -ForegroundColor Yellow
    $shell = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
    foreach ($number in $steps) {
        if ($number -ne 0 -and !$Rebuild -and (Read-Record $number $fingerprint)) {
            Write-Host "STEP $number already complete; saved output verified." -ForegroundColor DarkGreen
            continue
        }
        # Invalidate dependent records before a rerun, so a failed step cannot
        # leave an older downstream package usable as the latest result.
        foreach ($dependent in $(if ($number -eq 0) { @(0) } else { $number..6 })) {
            $path = Join-Path $stateRoot ('step-{0:D2}.json' -f $dependent)
            if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
        }
        & $shell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'Build-Worker.ps1') -Step $number
        if ($LASTEXITCODE -ne 0) { throw "Step $number failed. See $logsRoot and double-click Build-Everything.cmd to retry." }
    }
    if ($PSCmdlet.ParameterSetName -eq 'All') {
        Write-Host 'RUST PREVIEW BUILD COMPLETE. Full feature parity is still pending.' -ForegroundColor Green
        foreach ($number in @(4,5,6) | Where-Object { $steps -contains $_ }) {
            Write-Host (Require-Record $number $fingerprint).output
        }
    }
} finally {
    if ($locked) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
