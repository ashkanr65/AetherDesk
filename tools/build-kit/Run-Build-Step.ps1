[CmdletBinding(DefaultParameterSetName = 'All')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Single')][ValidateRange(0, 12)][int]$Step,
    [Parameter(ParameterSetName = 'All')][switch]$All,
    [Parameter(ParameterSetName = 'All')][switch]$Rebuild
)
. (Join-Path $PSScriptRoot 'Build-Common.ps1')
$mutex = [Threading.Mutex]::new($false, 'Local\AetherDeskLocalBuild')
$locked = $false
try {
    try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
    if (!$locked) { throw 'Another AetherDesk build is running. Keep only one build window.' }
    $powershell = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
    $fingerprint = Get-BuildFingerprint
    $steps = @(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12)
    if ($PSCmdlet.ParameterSetName -eq 'Single') { $steps = @($Step) }
    # Keep this build awake without changing Windows' saved power settings.
    if (!('AetherBuildPower' -as [type])) {
        Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class AetherBuildPower { [DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint state); }'
    }
    [void][AetherBuildPower]::SetThreadExecutionState(0x80000001L)
    foreach ($number in $steps) {
        if ($PSCmdlet.ParameterSetName -eq 'All' -and !$Rebuild -and $number -ne 0) {
            if (Read-StepRecord $number $fingerprint) {
                Write-Host "STEP $number already complete; saved output verified." -ForegroundColor DarkGreen
                continue
            }
        }
        # No build tools run in the caller. Each step has its own clean process.
        & $powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $kit 'Build-Worker.ps1') -Step $number
        if ($LASTEXITCODE -ne 0) { throw "Step $number failed. Open the newest log in $kitOutput\logs. Double-click Build-Everything.cmd to retry and resume." }
    }
    if ($PSCmdlet.ParameterSetName -eq 'All') {
        Write-Host ''
        Write-Host 'BUILD COMPLETE - portable ZIP and installer are ready.' -ForegroundColor Green
        foreach ($number in @(9, 11, 12)) {
            $record = Read-StepRecord $number $fingerprint
            if (!$record) { throw "Step $number output validation failed." }
            Write-Host $record.output -ForegroundColor Cyan
        }
    }
} finally {
    if ('AetherBuildPower' -as [type]) { [void][AetherBuildPower]::SetThreadExecutionState(0x80000000L) }
    if ($locked) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
