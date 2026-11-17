[CmdletBinding(DefaultParameterSetName='All')]
param(
    [Parameter(Mandatory=$true,ParameterSetName='Single')][ValidateRange(0,6)][int]$Step,
    [Parameter(ParameterSetName='All')][switch]$Rebuild,
    [Parameter(ParameterSetName='All')][switch]$PortableOnly
)
$buildStarted = [DateTimeOffset]::UtcNow
$buildClock = [Diagnostics.Stopwatch]::StartNew()
. (Join-Path $PSScriptRoot 'Build-Common.ps1')
Import-Module (Join-Path $PSScriptRoot 'Release-Retention.psm1') -DisableNameChecking
$mutex = [Threading.Mutex]::new($false,'Local\AetherDeskRustPreviewBuild')
$locked = $false
$timingReady = $false
$cleanupContext = $null
$oldTemp=$env:TEMP;$oldTmp=$env:TMP
$oldTaskId = $env:AETHERDESK_CLEANUP_TASK_ID
$oldTaskWork = $env:AETHERDESK_CLEANUP_TASK_WORK
$buildStatus = 'Failed'
$stepTimings = [Collections.Generic.List[object]]::new()
$keepCache = $env:AETHERDESK_RUST_KEEP_BUILD_CACHE -eq '1'
$timingPath = Join-Path $outputRoot 'reports\build-timing.json'
try {
    try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
    if (!$locked) { throw 'Another Rust build is running.' }
    $steps = if ($PSCmdlet.ParameterSetName -eq 'Single') { @($Step) } elseif ($PortableOnly) { @(0,1,2,3,4,5) } else { @(0,1,2,3,4,5,6) }
    if ($PSCmdlet.ParameterSetName -eq 'All' -and !$PortableOnly) { $null = Get-Iscc }
    # Recover only recorded, proven inactive work from this project. Unknown
    # legacy outputs and another chat's resources are never adopted by age.
    $recoveryPlan=@(Recover-ProjectTasks -Project aetherdesk)
    foreach($entry in $recoveryPlan){Write-Host ($entry|ConvertTo-Json -Depth 8 -Compress)}
    $null = Recover-ProjectTasks -Project aetherdesk -Apply
    $cleanupContext = Start-ProjectTask -Project aetherdesk -ProjectRoot $sourceRoot -OutputRoot $outputRoot -CacheRoot (Join-Path $outputRoot 'cache') -SkipWsl:([bool]$env:AETHERDESK_VERIFIED_AGENT_BUNDLE)
    $env:AETHERDESK_CLEANUP_TASK_ID=$cleanupContext.Task.id
    $env:AETHERDESK_CLEANUP_TASK_WORK=$cleanupContext.Task.work
    $env:TEMP=Join-Path $cleanupContext.Task.work 'tmp';$env:TMP=$env:TEMP
    New-Item -ItemType Directory -Path $env:TEMP -Force|Out-Null
    $ownerFile=Join-Path $cleanupContext.Task.cacheRoot '.project-cache-owner.json'
    if (!(Test-Path -LiteralPath $ownerFile) -and @((Get-ChildItem -LiteralPath $cleanupContext.Task.cacheRoot -Force) | Where-Object {$_.Name -ne '.project-cache.lock'}).Count -eq 0) {
        $null=Adopt-ProjectCache $cleanupContext # Reviewable empty-cache plan first.
        $null=Adopt-ProjectCache $cleanupContext -Apply
    }
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
            $cleanupContext.Task=Get-Content -LiteralPath (Get-ProjectTaskPath $cleanupContext.Task.id) -Raw|ConvertFrom-Json
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
                    if (!$snapshot.StartsWith($cleanupContext.Task.work+'\',[StringComparison]::OrdinalIgnoreCase)) { Add-ProjectArtifact $cleanupContext $snapshot -Role required-runtime }
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
        Write-Host 'Post-build cleanup keeps deliverables and one cache bounded to 10 GB.'
        Write-Host 'AETHERDESK RUST BUILD COMPLETE.' -ForegroundColor Green
        foreach ($output in $completedOutputs) { Write-Host $output }
    }
    if ($PSCmdlet.ParameterSetName -eq 'Single') {
        # Explicit single-stage results are hand-off deliverables for later
        # stages; do not erase them or their canonical source/agent evidence.
        Add-ProjectArtifact $cleanupContext $cleanupContext.Task.work -Role deliverable
        # A separately requested compiler stage hands its binaries to the next
        # invocation; budget enforcement must not erase that hand-off.
        $handoff=Require-Record $Step $fingerprint
        foreach($artifact in $handoff.artifacts){Add-ProjectArtifact $cleanupContext $artifact.path -Role required-runtime}
    }
    $cleanupContext.Task=Get-Content -LiteralPath (Get-ProjectTaskPath $cleanupContext.Task.id) -Raw|ConvertFrom-Json
    foreach ($output in @($completedOutputs)) { if ($output) { Add-ProjectArtifact $cleanupContext $output -Role deliverable } }
    $buildStatus = 'Passed'
    if ($PSCmdlet.ParameterSetName -eq 'All') {
        try {
            # Only complete successful builds can retire a release. Faster and
            # portable-only entry points retain old releases; cache cleanup is
            # independently bounded for every entry point.
            $retainOld=$keepCache -or $PortableOnly
            $releasePlan=Complete-ProjectRelease $cleanupContext $packagesRoot $completedOutputs -BuildSucceeded -KeepOld:$retainOld
            Write-Host ('Release retention dry run: {0} older artifact(s) eligible; newest {1}.' -f @($releasePlan.queued).Count,$releasePlan.newest)
            foreach($reason in $releasePlan.preserved){Write-Warning $reason}
            $retention=Complete-ProjectRelease $cleanupContext $packagesRoot $completedOutputs -BuildSucceeded -KeepOld:$retainOld -Apply
            Write-Host ('Release retention: {0} older artifact(s) queued for task cleanup; keep old releases: {1}.' -f @($retention.queued).Count,$retainOld)
            foreach($reason in $retention.preserved){Write-Warning $reason}
        } catch { Write-Warning ('Release retention deferred safely: '+$_.Exception.Message) }
    }
} finally {
    $buildClock.Stop()
    if ($timingReady) {
        try {
            Write-BuildTimingReport $timingPath $buildStarted $buildClock.Elapsed $buildStatus $stepTimings.ToArray() $keepCache
            Write-Host "Build timings: $timingPath"
        } catch { Write-Warning "Could not save build timings: $($_.Exception.Message)" }
    }
    Write-Host ("TOTAL BUILD TIME: {0} ({1})" -f (Format-BuildDuration $buildClock.Elapsed),$buildStatus) -ForegroundColor Cyan
    if ($cleanupContext) {
        try {
            $cleanupContext.Task=Get-Content -LiteralPath (Get-ProjectTaskPath $cleanupContext.Task.id) -Raw|ConvertFrom-Json
            $plan=Complete-ProjectTask $cleanupContext
            Show-ProjectCleanupReport $plan
            $cleaned=Complete-ProjectTask $cleanupContext -Apply -Failed:($buildStatus -ne 'Passed')
            Show-ProjectCleanupReport $cleaned
        } catch { Write-Warning ('Task cleanup deferred safely: '+$_.Exception.Message) }
        finally {
            if($cleanupContext.CacheLease){$cleanupContext.CacheLease.Dispose()}
            if($cleanupContext.Lease){$cleanupContext.Lease.Dispose()}
        }
    }
    $env:TEMP=$oldTemp;$env:TMP=$oldTmp
    $env:AETHERDESK_CLEANUP_TASK_ID=$oldTaskId
    $env:AETHERDESK_CLEANUP_TASK_WORK=$oldTaskWork
    if ($locked) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
