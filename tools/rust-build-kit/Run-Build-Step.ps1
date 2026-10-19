[CmdletBinding(DefaultParameterSetName='All')]
param(
    [Parameter(Mandatory=$true,ParameterSetName='Single')][ValidateRange(0,6)][int]$Step,
    [Parameter(ParameterSetName='All')][switch]$Rebuild,
    [Parameter(ParameterSetName='All')][switch]$PortableOnly
)
$buildStarted = [DateTimeOffset]::UtcNow
$buildClock = [Diagnostics.Stopwatch]::StartNew()
. (Join-Path $PSScriptRoot 'Build-Common.ps1')
$mutex = [Threading.Mutex]::new($false,'Local\AetherDeskRustPreviewBuild')
$locked = $false
$timingReady = $false
$buildStatus = 'Failed'
$stepTimings = [Collections.Generic.List[object]]::new()
$keepCache = $env:AETHERDESK_RUST_KEEP_BUILD_CACHE -eq '1'
$timingPath = Join-Path $outputRoot 'reports\build-timing.json'
try {
    try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
    if (!$locked) { throw 'Another Rust build is running.' }
    $steps = if ($PSCmdlet.ParameterSetName -eq 'Single') { @($Step) } elseif ($PortableOnly) { @(0,1,2,3,4,5) } else { @(0,1,2,3,4,5,6) }
    if ($PSCmdlet.ParameterSetName -eq 'All' -and !$PortableOnly) { $null = Get-Iscc }
    # Clean once, under the build mutex, before any worker/transcript starts.
    # Individual stages keep the snapshots and reports required by later stages.
    if ($PSCmdlet.ParameterSetName -eq 'All') { Reset-BuildOutput $outputRoot $sourceRoot }
    $timingReady = $true
    $fingerprint = Get-SourceFingerprint
    Write-Host 'Building AetherDesk Rust. Native regression checks are required before packaging.' -ForegroundColor Yellow
    $shell = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
    foreach ($number in $steps) {
        $stepStarted = [DateTimeOffset]::UtcNow
        $stepClock = [Diagnostics.Stopwatch]::StartNew()
        $stepStatus = 'Failed'
        $stageName = Get-BuildStageName $number
        Write-Host ("STEP {0}: {1} | started {2} | build elapsed {3}" -f $number,$stageName,(Get-Date -Format 'HH:mm:ss'),(Format-BuildDuration $buildClock.Elapsed)) -ForegroundColor Cyan
        try {
            if ($number -ne 0 -and !$Rebuild -and (Read-Record $number $fingerprint)) {
                $stepStatus = 'Reused'
                Write-Host "STEP $number already complete; saved output verified." -ForegroundColor DarkGreen
                continue
            }
            # Invalidate dependent records before a rerun, so a failed step cannot
            # leave an older downstream package usable as the latest result.
            foreach ($dependent in $(if ($number -eq 0) { @(0) } else { $number..6 })) {
                $path = Join-Path $stateRoot ('step-{0:D2}.json' -f $dependent)
                if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
            }
            # Once stage 1 is sealed, run the recipe itself from that snapshot.
            # Source edits can then be picked up by the next build, without
            # mixing recipe versions or discarding this build's completed work.
            $oldOutputRoot=$env:AETHERDESK_RUST_OUTPUT_ROOT
            $oldFrozen=$env:AETHERDESK_RUST_FROZEN_FINGERPRINT
            try {
                $worker=Join-Path $PSScriptRoot 'Build-Worker.ps1'
                if($number -ge 2){
                    $snapshot=(Require-Record 1 $fingerprint).output
                    $worker=Join-Path $snapshot 'tools/rust-build-kit/Build-Worker.ps1'
                    $env:AETHERDESK_RUST_OUTPUT_ROOT=$outputRoot
                    $env:AETHERDESK_RUST_FROZEN_FINGERPRINT=$fingerprint
                }
                & $shell -NoProfile -ExecutionPolicy Bypass -File $worker -Step $number
                $workerExit=$LASTEXITCODE
            } finally {
                $env:AETHERDESK_RUST_OUTPUT_ROOT=$oldOutputRoot
                $env:AETHERDESK_RUST_FROZEN_FINGERPRINT=$oldFrozen
            }
            if ($workerExit -ne 0) { throw "Step $number failed. See $logsRoot and double-click Build-Everything.cmd to retry." }
            $stepStatus = 'Passed'
        } finally {
            $stepClock.Stop()
            $stepTimings.Add([ordered]@{step=$number;name=$stageName;status=$stepStatus;startedUtc=$stepStarted.ToString('o');elapsed=(Format-BuildDuration $stepClock.Elapsed);elapsedSeconds=[math]::Round($stepClock.Elapsed.TotalSeconds,3)})
            Write-Host ("STEP {0} {1} | step time {2} | total elapsed {3}" -f $number,$stepStatus,(Format-BuildDuration $stepClock.Elapsed),(Format-BuildDuration $buildClock.Elapsed)) -ForegroundColor Cyan
            try { Write-BuildTimingReport $timingPath $buildStarted $buildClock.Elapsed 'Running' $stepTimings.ToArray() $keepCache }
            catch { Write-Warning "Could not save build timings: $($_.Exception.Message)" }
        }
    }
    if ($PSCmdlet.ParameterSetName -eq 'All') {
        try {
            if((Get-SourceFingerprint) -ne $fingerprint){Write-Warning 'Checkout edits were made after the snapshot. This completed build contains the frozen snapshot; build again to include the newer edits.'}
        } catch { Write-Warning 'The completed packages use the verified snapshot. The current checkout changed or could not be compared.' }
        # Capture verified output paths before removing resumable stage records.
        $completedOutputs = @()
        foreach ($number in @(4,5,6) | Where-Object { $steps -contains $_ }) {
            $completedOutputs += (Require-Record $number $fingerprint).output
        }
        if (!$keepCache) {
            try { Clear-NativeAgentBuildCache } catch { Write-Warning "Packages are complete, but the labelled Linux cache could not be cleaned: $($_.Exception.Message)" }
            try { Reset-BuildOutput $outputRoot $sourceRoot -AfterSuccess } catch { Write-Warning "Packages are complete, but temporary Windows build files could not be cleaned: $($_.Exception.Message)" }
        } else {
            Write-Host 'Faster repeat-build mode: Windows and Linux build caches retained.'
            try { Reset-BuildOutput $outputRoot $sourceRoot -AfterSuccess -KeepCache } catch { Write-Warning "Packages are complete, but staging files could not be cleaned: $($_.Exception.Message)" }
        }
        Write-Host 'AETHERDESK RUST BUILD COMPLETE.' -ForegroundColor Green
        foreach ($output in $completedOutputs) { Write-Host $output }
    }
    $buildStatus = 'Passed'
} finally {
    $buildClock.Stop()
    if ($timingReady) {
        try {
            Write-BuildTimingReport $timingPath $buildStarted $buildClock.Elapsed $buildStatus $stepTimings.ToArray() $keepCache
            Write-Host "Build timings: $timingPath"
        } catch { Write-Warning "Could not save build timings: $($_.Exception.Message)" }
    }
    Write-Host ("TOTAL BUILD TIME: {0} ({1})" -f (Format-BuildDuration $buildClock.Elapsed),$buildStatus) -ForegroundColor Cyan
    if ($locked) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
