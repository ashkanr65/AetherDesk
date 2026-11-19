Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$script:LinuxHelper = Join-Path $PSScriptRoot 'project_cleanup_linux.py'

function Get-ProjectCleanupStateRoot {
    if ($env:AETHER_PROJECT_CLEANUP_STATE) { return [IO.Path]::GetFullPath($env:AETHER_PROJECT_CLEANUP_STATE) }
    return Join-Path $env:LOCALAPPDATA 'AetherProjectMaintenance'
}
function Assert-ProjectName([string]$Project) {
    if ($Project -notin @('aetherdesk','openeda')) { throw 'Project must be aetherdesk or openeda.' }
}
function Write-CleanupJson([string]$Path,$Value) {
    $temporary = $Path + '.' + [guid]::NewGuid().ToString('N') + '.tmp'
    [IO.File]::WriteAllText($temporary,($Value | ConvertTo-Json -Depth 30),[Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $temporary -Destination $Path -Force
}
function Assert-CleanupTree([string]$Path,[string]$Root,[switch]$Recurse,[switch]$ForDeletion) {
    if ([string]::IsNullOrWhiteSpace($Path) -or [string]::IsNullOrWhiteSpace($Root) -or
        ![IO.Path]::IsPathRooted($Path) -or ![IO.Path]::IsPathRooted($Root)) { throw 'Absolute nonempty cleanup paths required.' }
    $full = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    $base = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    if ($base -eq [IO.Path]::GetPathRoot($base).TrimEnd('\') -or
        !($full.Equals($base,[StringComparison]::OrdinalIgnoreCase) -or $full.StartsWith($base+'\',[StringComparison]::OrdinalIgnoreCase))) { throw 'Cleanup path escapes its managed task directory.' }
    $ancestor = $full
    while ($ancestor) {
        if (Test-Path -LiteralPath $ancestor) {
            $item = Get-Item -LiteralPath $ancestor -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Cleanup refuses a link or mount point: $ancestor" }
        }
        $ancestor = Split-Path -Path $ancestor -Parent
    }
    if ($Recurse -and (Test-Path -LiteralPath $full)) {
        $pending = [Collections.Generic.Stack[string]]::new(); $pending.Push($full)
        while ($pending.Count) {
            $item = Get-Item -LiteralPath $pending.Pop() -Force
            if($ForDeletion -and $item.Name -eq '.git'){throw 'A Git repository/worktree inside the task directory must be retained.'}
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Cleanup refuses a link or mount point: $($item.FullName)" }
            if ($item.PSIsContainer) { foreach ($child in Get-ChildItem -LiteralPath $item.FullName -Force) { $pending.Push($child.FullName) } }
        }
    }
    return $full
}
function Get-CleanupBytes([string]$Root) {
    if (!(Test-Path -LiteralPath $Root)) { return [long]0 }
    $null = Assert-CleanupTree $Root $Root -Recurse
    [long]$bytes = 0
    foreach ($file in Get-ChildItem -LiteralPath $Root -Recurse -File -Force) { $bytes += $file.Length }
    return $bytes
}
function Open-CleanupMaintenanceLease([switch]$Exclusive) {
    $root = Get-ProjectCleanupStateRoot
    $null = Assert-CleanupTree $root $root
    New-Item -ItemType Directory -Path $root -Force | Out-Null
    $share = if ($Exclusive) { [IO.FileShare]::None } else { [IO.FileShare]::ReadWrite }
    # Task leases take byte-range locks through FileShare: maintenance needs
    # exclusive access, while normal tasks can coexist across both projects.
    return [IO.File]::Open((Join-Path $root 'maintenance.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,$share)
}
function Read-ProjectTasks {
    $root = Join-Path (Get-ProjectCleanupStateRoot) 'tasks'
    if (!(Test-Path -LiteralPath $root)) { return @() }
    $null = Assert-CleanupTree $root $root -Recurse
    return @(Get-ChildItem -LiteralPath $root -File -Filter '*.json' | ForEach-Object {
        $value = Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json
        if ($value.schema -ne 1) { throw "Unknown task manifest: $($_.FullName)" }
        $value
    })
}
function Test-CleanupTaskActive($Task) {
    foreach($resource in $Task.resources){
        if($resource.kind -eq 'process' -and (Test-ProjectProcessAlive $resource.pid $resource.startedUtc)){return $true}
    }
    if ($Task.status -notin @('active','cleanup-deferred')) { return $false }
    if (!$Task.ownerPid) { return $true } # Manual/chat tasks require explicit completion.
    return Test-ProjectProcessAlive $Task.ownerPid $Task.ownerStartedUtc
}
function Test-ProjectProcessAlive([int]$ProcessId,$StartedUtc) {
    if (!$StartedUtc) { return $true }
    try {
        $expected = ([DateTime]$StartedUtc).ToUniversalTime()
        $process = Get-Process -Id $ProcessId -ErrorAction Stop
        return $process.StartTime.ToUniversalTime().Ticks -eq $expected.Ticks
    } catch [Microsoft.PowerShell.Commands.ProcessCommandException] {
        return $false
    } catch {
        # Get-Process cannot read StartTime for some Windows services. CIM can
        # distinguish a recycled PID while keeping unknown processes protected.
        try {
            $process = Get-CimInstance -ClassName Win32_Process -Filter "ProcessId = $ProcessId" -ErrorAction Stop
            if (!$process.CreationDate) { return $true }
            $started = $process.CreationDate.ToUniversalTime()
            return [Math]::Abs(($started - $expected).Ticks) -lt [TimeSpan]::TicksPerMillisecond
        } catch {
            return $true
        }
    }
}
function Register-ProjectProcess {
    param([string]$TaskId,[string]$Token)
    $task=Get-Content -LiteralPath (Get-ProjectTaskPath $TaskId) -Raw|ConvertFrom-Json
    Assert-ProjectTaskToken $task $Token
    $task.resources+=@([pscustomobject]@{kind='process';pid=$PID;startedUtc=(Get-Process -Id $PID).StartTime.ToUniversalTime().ToString('o');role='active-input'})
    Save-ProjectTask $task
}
function Get-ProjectTaskPath([string]$Id) {
    if ($Id -notmatch '^(aetherdesk|openeda)-[a-zA-Z0-9-]{8,100}$') { throw 'Invalid task ID.' }
    return Join-Path (Join-Path (Get-ProjectCleanupStateRoot) 'tasks') ($Id+'.json')
}
function Get-ProjectWslWorkPath([string]$Id) {
    $null=Get-ProjectTaskPath $Id
    $sha=[Security.Cryptography.SHA256]::Create()
    try { return '/tmp/ad/'+([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::ASCII.GetBytes($Id))).Replace('-','').ToLowerInvariant().Substring(0,12)) } finally { $sha.Dispose() }
}
function Save-ProjectTask($Task) { Write-CleanupJson (Get-ProjectTaskPath $Task.id) $Task }
function Assert-ProjectTaskToken($Task,[string]$Token) {
    if (!$Token -or $Task.token -cne $Token) { throw 'Task ownership token does not match.' }
    Assert-ProjectName $Task.project
    $null = Get-ProjectTaskPath $Task.id
}
function Invoke-CleanupLinux($Task,[string]$Action,[bool]$Apply=$false,[string[]]$Protected=@(),[string]$Image='', [string[]]$Command=@()) {
    if ($Task.skipWsl) { return [pscustomobject]@{resources=@();removed=@();deferred=@();remainingImageCacheBytes=0} }
    $keepWork=$Action -ne 'inspect' -and @($Task.resources | Where-Object {$_.kind -eq 'wsl-path' -and $_.role -ne 'temporary'}).Count -gt 0
    $windowsWork=if($Action -eq 'inspect'){''}else{$Task.work}
    $wslWork=if($Action -eq 'inspect'){''}else{$Task.wslWork}
    $request = @{project=$Task.project; task=$Task.id; action=$Action; apply=$Apply; protected=$Protected; store=$Task.store;keepWork=$keepWork;windowsWork=$windowsWork;wslWork=$wslWork}
    if($Action -eq 'run'){$request.image=$Image;$request.command=$Command}
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(($request | ConvertTo-Json -Depth 20 -Compress)))
    $path = (& wsl.exe -d $Task.distribution -u $Task.user --exec wslpath -u $script:LinuxHelper | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or !$path.StartsWith('/')) { throw 'Cannot translate cleanup helper path.' }
    $output = & wsl.exe -d $Task.distribution -u $Task.user --exec python3 $path $encoded
    if ($LASTEXITCODE -ne 0) { throw 'Scoped WSL cleanup/inspection failed; unknown resources were preserved.' }
    return ($output | Out-String | ConvertFrom-Json)
}
function Start-ProjectTask {
    param([ValidateSet('aetherdesk','openeda')][string]$Project='aetherdesk',
          [Parameter(Mandatory=$true)][string]$ProjectRoot,[Parameter(Mandatory=$true)][string]$OutputRoot,
          [string]$CacheRoot,[long]$CacheBudgetBytes=10000000000,[switch]$Manual,[switch]$SkipWsl)
    $projectPath=[IO.Path]::GetFullPath($ProjectRoot).TrimEnd('\')
    $outputPath=[IO.Path]::GetFullPath($OutputRoot).TrimEnd('\')
    if ($outputPath -eq [IO.Path]::GetPathRoot($outputPath).TrimEnd('\') -or $outputPath.Equals($projectPath,[StringComparison]::OrdinalIgnoreCase) -or $outputPath.StartsWith($projectPath+'\',[StringComparison]::OrdinalIgnoreCase) -or $projectPath.StartsWith($outputPath+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Task outputs must be separate from source.' }
    $lease=Open-CleanupMaintenanceLease
    $cacheLease=$null; $task=$null
    try {
        $id=$Project+'-'+[DateTime]::UtcNow.ToString('yyyyMMddTHHmmss')+'-'+[guid]::NewGuid().ToString('N')
        $work=Join-Path (Join-Path $outputPath '.cleanup-work') ([guid]::NewGuid().ToString('N').Substring(0,16))
        $null=Assert-CleanupTree $work $work
        New-Item -ItemType Directory -Path $work | Out-Null
        $root=Join-Path (Get-ProjectCleanupStateRoot) 'tasks'; New-Item -ItemType Directory -Force -Path $root | Out-Null
        $task=[pscustomobject]@{schema=1;id=$id;project=$Project;token=[guid]::NewGuid().ToString('N');projectRoot=$projectPath;work=$work;wslWork=(Get-ProjectWslWorkPath $id);status='active';ownerPid=$(if($Manual){0}else{$PID});ownerStartedUtc=$(if($Manual){''}else{(Get-Process -Id $PID).StartTime.ToUniversalTime().ToString('o')});startedUtc=[DateTime]::UtcNow.ToString('o');resources=@();retained=@();cacheRoot=$CacheRoot;cacheBudgetBytes=$CacheBudgetBytes;distribution='Ubuntu';user='root';store=@{uid=0;driver='vfs';graphRoot='/var/lib/containers/storage'};skipWsl=[bool]$SkipWsl}
        Write-CleanupJson (Join-Path $work '.project-owner.json') @{project=$Project;task=$id;token=$task.token}
        Save-ProjectTask $task
        $cacheLease=$null
        if ($CacheRoot) {
            foreach($other in Read-ProjectTasks){
                if($other.id -ne $task.id -and (Test-CleanupTaskActive $other) -and $other.cacheRoot -and
                    ([IO.Path]::GetFullPath($other.cacheRoot).TrimEnd('\')).Equals([IO.Path]::GetFullPath($CacheRoot).TrimEnd('\'),[StringComparison]::OrdinalIgnoreCase)) {
                    throw 'Another active or manual task claims this cache.'
                }
            }
            $null=Assert-CleanupTree $CacheRoot $CacheRoot
            New-Item -ItemType Directory -Force -Path $CacheRoot | Out-Null
            $cacheLease=[IO.File]::Open((Join-Path $CacheRoot '.project-cache.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
        }
        if (!$SkipWsl) { $null=Invoke-CleanupLinux $task 'begin' }
        return [pscustomobject]@{Task=$task;Lease=$lease;CacheLease=$cacheLease}
    } catch {
        if($cacheLease){$cacheLease.Dispose()}
        if($task){$task.status='cleanup-deferred';Save-ProjectTask $task}
        $lease.Dispose(); throw
    }
}
function Add-ProjectArtifact {
    param($Context,[Parameter(Mandatory=$true)][string]$Path,[ValidateSet('temporary','deliverable','required-runtime')][string]$Role='temporary')
    $task=$Context.Task
    if ($Role -eq 'temporary') { $null=Assert-CleanupTree $Path $task.work }
    $task.resources+=@([pscustomobject]@{kind='windows-path';path=[IO.Path]::GetFullPath($Path);role=$Role})
    Save-ProjectTask $task
}
function Add-ProjectPodmanResource {
    param($Context,[ValidateSet('container','image')][string]$Kind,[string]$Id,[ValidateSet('temporary','deliverable','required-runtime')][string]$Role='temporary')
    $Id=$Id -replace '^sha256:',''
    if ($Id -notmatch '^[a-f0-9]{64}$') { throw 'Register a full exact Podman ID.' }
    $Context.Task.resources+=@([pscustomobject]@{kind=$Kind;id=$Id;role=$Role}); Save-ProjectTask $Context.Task
}
function Publish-ProjectTaskOutput {
    param($Context,[Parameter(Mandatory=$true)][string]$Path,[Parameter(Mandatory=$true)][string]$DeliveryRoot)
    $source=Assert-CleanupTree $Path $Context.Task.work -Recurse
    if (!(Test-Path -LiteralPath $source)) { throw 'Completed task output is missing.' }
    $root=Assert-CleanupTree $DeliveryRoot $DeliveryRoot
    if ($root.Equals($Context.Task.projectRoot,[StringComparison]::OrdinalIgnoreCase) -or $root.StartsWith($Context.Task.projectRoot+'\',[StringComparison]::OrdinalIgnoreCase) -or $root.Equals($Context.Task.work,[StringComparison]::OrdinalIgnoreCase) -or $root.StartsWith($Context.Task.work+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Deliverables require a separate explicit output directory.' }
    $destination=Join-Path $root ([IO.Path]::GetFileName($source))
    $null=Assert-CleanupTree $destination $root
    if (Test-Path -LiteralPath $destination) { throw 'Never overwrite an existing deliverable.' }
    New-Item -ItemType Directory -Path $root -Force | Out-Null
    Add-ProjectArtifact $Context $destination -Role deliverable
    Move-Item -LiteralPath $source -Destination $destination -ErrorAction Stop
    return $destination
}
function Add-ProjectWslArtifact {
    param($Context,[string]$RelativePath,[ValidateSet('temporary','deliverable','required-runtime')][string]$Role='temporary')
    if (!$RelativePath -or $RelativePath.StartsWith('/') -or $RelativePath -match '(^|/)\.\.(/|$)|\\') { throw 'Use a relative path inside the WSL task directory.' }
    $Context.Task.resources+=@([pscustomobject]@{kind='wsl-path';path=($Context.Task.wslWork+'/'+$RelativePath);role=$Role}); Save-ProjectTask $Context.Task
}
function Invoke-ProjectTestContainer {
    param($Context,[Parameter(Mandatory=$true)][string]$Image,[string[]]$Command=@('true'))
    if($Context.Task.skipWsl){throw 'This task has no WSL resources.'}
    $result=Invoke-CleanupLinux $Context.Task 'run' $false @() -Image $Image -Command $Command
    if($result.containerId){Add-ProjectPodmanResource $Context container $result.containerId}
    if($result.tail){Write-Host $result.tail}
    if($result.exitCode -ne 0){throw "Disposable test container failed (exit $($result.exitCode)); log: $($result.log)"}
    return $result
}
function Adopt-ProjectCache {
    param($Context,[switch]$Apply)
    $task=$Context.Task
    if (!$task.cacheRoot -or !$Context.CacheLease) { throw 'A held project cache lease is required.' }
    $null=Assert-CleanupTree $task.cacheRoot $task.cacheRoot -Recurse
    $path=Join-Path $task.cacheRoot '.project-cache-owner.json'
    if (Test-Path -LiteralPath $path) {
        $owner=Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
        if ($owner.project -ne $task.project -or $owner.projectRoot -ne $task.projectRoot) { throw 'Cache belongs to another project.' }
    } elseif ($Apply) { Write-CleanupJson $path @{project=$task.project;projectRoot=$task.projectRoot;purpose='bounded reusable build cache'} }
    [pscustomobject]@{action='adopt-project-cache';path=$task.cacheRoot;dryRun=(!$Apply);bytes=(Get-CleanupBytes $task.cacheRoot)}
}
function Get-ProjectCleanupPlan {
    param($Task,[string]$Token,[switch]$Apply,[switch]$AllowInterrupted)
    Assert-ProjectTaskToken $Task $Token
    if ($Task.status -eq 'active' -and (Test-CleanupTaskActive $Task) -and $AllowInterrupted) { throw 'Another task is active or its ownership is uncertain.' }
    $others=@(Read-ProjectTasks | Where-Object {$_.id -ne $Task.id -and (Test-CleanupTaskActive $_)})
    $protected=@(); foreach($other in $others){foreach($resource in $other.resources){if($resource.kind -in @('container','image')){$protected+=@($resource.id)}}}
    foreach($resource in $Task.resources){if($resource.kind -in @('container','image') -and $resource.role -ne 'temporary'){$protected+=@($resource.id)}}
    $removed=@(); $deferred=@()
    # Only the task's freshly created, marker-bound workspace is removable.
    # User deliverables are retained even if explicitly placed inside it.
    $work=$Task.work
    $liveChildren=@($Task.resources | Where-Object {$_.kind -eq 'process' -and $_.pid -ne $PID -and (Test-ProjectProcessAlive $_.pid $_.startedUtc)})
    if($liveChildren.Count){
        foreach($resource in $Task.resources){if($resource.kind -in @('container','image')){$protected+=@($resource.id)}}
    }
    if (Test-Path -LiteralPath $work) {
        $null=Assert-CleanupTree $work $work -Recurse -ForDeletion
        if ($work.Equals($Task.projectRoot,[StringComparison]::OrdinalIgnoreCase)) { throw 'Never remove a project root.' }
        $owner=Get-Content -LiteralPath (Join-Path $work '.project-owner.json') -Raw | ConvertFrom-Json
        if ($owner.project -ne $Task.project -or $owner.task -ne $Task.id -or $owner.token -cne $Token) { throw 'Managed directory ownership mismatch.' }
        $claims=@($others | Where-Object {$_.work -eq $work -or @($_.resources | Where-Object {$_.kind -eq 'windows-path' -and ($_.path -eq $work -or $_.path.StartsWith($work+'\',[StringComparison]::OrdinalIgnoreCase) -or $work.StartsWith($_.path.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase))}).Count})
        $retained=@($Task.resources | Where-Object {$_.kind -eq 'windows-path' -and $_.role -ne 'temporary' -and ($_.path -eq $work -or $_.path.StartsWith($work+'\',[StringComparison]::OrdinalIgnoreCase))})
        if ($claims.Count -or $retained.Count -or $liveChildren.Count) { $deferred+=@('Task workspace retained for active claims/processes or requested stage/deliverable outputs.') }
        else {
            $removed+=@([pscustomobject]@{kind='windows-directory';path=$work;dryRun=(!$Apply)})
            # WSL discovery/ownership checks happen before any Windows deletion.
        }
    }
    $linux=Invoke-CleanupLinux $Task 'clean' $false $protected
    foreach($resource in $linux.resources){
        if (!@($Task.resources | Where-Object {$_.kind -eq $resource.kind -and $_.id -eq $resource.id}).Count) {
            $Task.resources+=@([pscustomobject]@{kind=$resource.kind;id=$resource.id;role='temporary'})
        }
    }
    if ($Apply) {
        Save-ProjectTask $Task # Discovery is persisted before exact-ID mutation.
        $linux=Invoke-CleanupLinux $Task 'clean' $true $protected
        if (@($linux.deferred).Count) {
            $removed=@(); $deferred+=@('Windows task workspace retained because container ownership/use is uncertain.')
        } else { foreach($item in $removed){Remove-Item -LiteralPath $item.path -Recurse -Force -ErrorAction Stop} }
    }
    $deferred+=@($linux.deferred)
    return [pscustomobject]@{project=$Task.project;task=$Task.id;dryRun=(!$Apply);removed=@($removed)+@($linux.removed);deferred=$deferred;imageCacheBytes=$linux.remainingImageCacheBytes}
}
function Trim-ProjectCache {
    param($Context,[switch]$Apply)
    $task=$Context.Task; $removed=@(); $deferred=@(); [long]$bytes=0
    if (!$task.cacheRoot) { return [pscustomobject]@{bytes=0;removed=@();deferred=@()} }
    try {$bytes=Get-CleanupBytes $task.cacheRoot}
    catch {return [pscustomobject]@{bytes=$null;removed=@();deferred=@('Cache size unknown; unsafe/unreadable cache preserved: '+$_.Exception.Message)}}
    try {
        if ($bytes -gt $task.cacheBudgetBytes) {
            $ownerPath=Join-Path $task.cacheRoot '.project-cache-owner.json'
            if (!$Context.CacheLease -or !(Test-Path -LiteralPath $ownerPath)) { throw 'Cache is not explicitly adopted under an exclusive project cache lease.' }
            $owner=Get-Content -LiteralPath $ownerPath -Raw | ConvertFrom-Json
            if ($owner.project -ne $task.project -or $owner.projectRoot -ne $task.projectRoot) { throw 'Cache owner mismatch.' }
            if (Test-ProjectCompilersActive) { throw 'A compiler is active; cache eviction deferred.' }
            # Evict only regenerable compiler output. Preserve dependency sources,
            # Git repositories, canonical licence notices and all deliverables.
            $target=Join-Path $task.cacheRoot 'target'
            foreach($record in Read-ProjectTasks){
                foreach($resource in $record.resources){
                    if($resource.kind -eq 'windows-path' -and ($resource.role -in @('deliverable','required-runtime') -or (Test-CleanupTaskActive $record)) -and
                        ($resource.path -eq $target -or $resource.path.StartsWith($target+'\',[StringComparison]::OrdinalIgnoreCase))) {
                        throw 'A retained result or active task still uses compiler output; cache eviction deferred.'
                    }
                }
            }
            if (Test-Path -LiteralPath $target) {
                $null=Assert-CleanupTree $target $task.cacheRoot -Recurse -ForDeletion
                $size=Get-CleanupBytes $target
                if ($Apply) { Remove-Item -LiteralPath $target -Recurse -Force }
                $removed+=@([pscustomobject]@{kind='compiler-cache';path=$target;dryRun=(!$Apply);bytes=$size})
                $bytes-=$size
            }
            if ($bytes -gt $task.cacheBudgetBytes) { throw 'Protected dependency/download cache exceeds budget; report a cache-budget exception.' }
        }
    } catch { $deferred+=@('Cache budget exception: '+$_.Exception.Message) }
    return [pscustomobject]@{bytes=$bytes;removed=$removed;deferred=$deferred}
}
function Test-ProjectCompilersActive { return @((Get-Process -Name 'cargo','rustc' -ErrorAction SilentlyContinue)).Count -gt 0 }
function Complete-ProjectTask {
    param($Context,[switch]$Apply,[switch]$Failed)
    $task=$Context.Task
    try {
        $report=Get-ProjectCleanupPlan $task $task.token -Apply:$Apply
        if(@($report.deferred).Count -and $task.cacheRoot){
            $cache=[pscustomobject]@{bytes=(Get-CleanupBytes $task.cacheRoot);removed=@();deferred=@('Cache eviction deferred while task resource use is uncertain.')}
        }else{$cache=Trim-ProjectCache $Context -Apply:$Apply}
        $report.removed+=@($cache.removed); $report.deferred+=@($cache.deferred)
        $report | Add-Member NoteProperty cacheBytes $(if($null -eq $cache.bytes){$null}else{$cache.bytes+$report.imageCacheBytes})
        if($null -ne $report.cacheBytes -and $report.cacheBytes -gt $task.cacheBudgetBytes){$report.deferred+=@('Combined project cache exceeds 10 GB; protected/in-use resources were preserved.')}
        $report | Add-Member NoteProperty windowsFreeBytes (Get-PSDrive C).Free
        $report | Add-Member NoteProperty compaction 'Deferred: requires fresh confirmation that both chats and every other WSL job are idle, plus the shared exclusive maintenance lock.'
        if ($Apply) {
            $task.status=if($Failed){'failed-cleaned'}else{'complete'}
            if ($report.deferred.Count) { $task.status='cleanup-deferred' }
            Save-ProjectTask $task
            $root=Join-Path (Get-ProjectCleanupStateRoot) 'reports'; New-Item -ItemType Directory -Force -Path $root | Out-Null
            Write-CleanupJson (Join-Path $root ($task.id+'.json')) $report
        }
        return $report
    } catch {
        if($Apply){$task.status='cleanup-deferred';Save-ProjectTask $task}
        throw
    } finally {
        if ($Apply) {
            if ($Context.CacheLease) { $Context.CacheLease.Dispose() }
            if ($Context.Lease) { $Context.Lease.Dispose() }
        }
    }
}
function Recover-ProjectTasks {
    param([ValidateSet('aetherdesk','openeda')][string]$Project='aetherdesk',[switch]$Apply)
    $lease=Open-CleanupMaintenanceLease
    try {
        foreach($task in Read-ProjectTasks | Where-Object {$_.project -eq $Project -and $_.status -in @('active','cleanup-deferred')}) {
            if (Test-CleanupTaskActive $task) { Write-Warning "Preserved active/uncertain task $($task.id)"; continue }
            # A per-task lease excludes another recovery process. Never treat
            # age, an absent tag, or an unnamed resource as ownership proof.
            $lock=[IO.File]::Open((Get-ProjectTaskPath $task.id)+'.lock',[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
            try {
                $report=Get-ProjectCleanupPlan $task $task.token -Apply:$Apply -AllowInterrupted
                if ($Apply) { $task.status=if($report.deferred.Count){'cleanup-deferred'}else{'complete'}; Save-ProjectTask $task }
                $report
            } finally {$lock.Dispose()}
        }
    } finally {$lease.Dispose()}
}
function Show-ProjectCleanupReport($Report) {
    $size=if($null -eq $Report.cacheBytes){'unknown (see exception)'}else{('{0:N2} GiB' -f ($Report.cacheBytes/1GB))}
    Write-Host ('Cleanup {0}: {1} artifact(s) {2}; cache {3}; Windows free {4:N1} GiB.' -f $Report.task,@($Report.removed).Count,$(if($Report.dryRun){'would be removed'}else{'removed'}),$size,($Report.windowsFreeBytes/1GB))
    foreach($message in $Report.deferred){Write-Warning $message}
    Write-Host $Report.compaction
}
Export-ModuleMember -Function *-Project*,Assert-CleanupTree,Get-CleanupBytes,Invoke-CleanupLinux,Open-CleanupMaintenanceLease,Read-ProjectTasks,Test-CleanupTaskActive
