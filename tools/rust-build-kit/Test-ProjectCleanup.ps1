[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'Project-Cleanup.psm1') -Force -DisableNameChecking
$testParent=if($env:AETHERDESK_CLEANUP_TASK_WORK){$env:AETHERDESK_CLEANUP_TASK_WORK}else{[IO.Path]::GetTempPath()}
$testRoot=Join-Path $testParent ('aetherdesk-cleanup-test-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
$oldState=$env:AETHER_PROJECT_CLEANUP_STATE
$env:AETHER_PROJECT_CLEANUP_STATE=Join-Path $testRoot 'state'
$contexts=@(); $assertions=0
function Check([bool]$Condition,[string]$Message){if(!$Condition){throw $Message};$script:assertions++}
function Reject([scriptblock]$Action,[string]$Message){$rejected=$false;try{& $Action | Out-Null}catch{$rejected=$true};Check $rejected $Message}
function New-Fixture([string]$Project='aetherdesk',[long]$Budget=10GB){
    $name=[guid]::NewGuid().ToString('N')
    $source=Join-Path $testRoot ('source-'+$name); New-Item -ItemType Directory -Path $source | Out-Null
    [IO.File]::WriteAllText((Join-Path $source 'LICENSE'),'canonical notice')
    $context=Start-ProjectTask -Project $Project -ProjectRoot $source -OutputRoot (Join-Path $testRoot ('output-'+$name)) -CacheRoot (Join-Path $testRoot ('cache-'+$name)) -CacheBudgetBytes $Budget -SkipWsl
    $script:contexts+=@($context)
    return $context
}
try {
    $c=New-Fixture
    Check ($c.Task.wslWork.Length -le 20 -and $c.Task.wslWork.StartsWith('/tmp/ad/')) 'Managed WSL paths exceed nested Unix socket limits.'
    $temp=Join-Path $c.Task.work 'extract'; New-Item -ItemType Directory -Path $temp | Out-Null
    [IO.File]::WriteAllText((Join-Path $temp 'payload'),'temporary')
    Add-ProjectArtifact $c $temp
    $r=Complete-ProjectTask $c
    Check ($r.dryRun -and (Test-Path -LiteralPath $temp)) 'Dry run changed a fixture.'
    Check (@($r.removed).Count -eq 1) 'Dry run did not identify the task workspace.'
    Reject {Get-ProjectCleanupPlan $c.Task 'wrong-token' -Apply} 'Wrong task token accepted.'
    foreach($path in @('',[IO.Path]::GetPathRoot($testRoot),$c.Task.projectRoot,(Split-Path $c.Task.work -Parent),(Join-Path $c.Task.work '..\outside'))){
        Reject {Assert-CleanupTree $path $c.Task.work -Recurse} 'Unsafe path accepted.'
    }
    $shared=New-Fixture openeda
    Add-ProjectArtifact $shared $temp -Role required-runtime
    $r=Complete-ProjectTask $c
    Check (@($r.removed).Count -eq 0) 'Another active task claim was ignored.'
    Check (Test-Path -LiteralPath $temp) 'Shared artifact removed.'
    $shared.Task.resources=@(); Save-ProjectTask $shared.Task
    $link=Join-Path $c.Task.work 'escape'
    New-Item -ItemType Junction -Path $link -Target $c.Task.projectRoot | Out-Null
    Reject {Get-ProjectCleanupPlan $c.Task $c.Task.token -Apply} 'Linked tree accepted.'
    Check ((Get-Content -LiteralPath (Join-Path $c.Task.projectRoot 'LICENSE') -Raw) -eq 'canonical notice') 'Source notice changed.'
    # Remove only the test junction itself, never recurse through its target.
    [IO.Directory]::Delete($link)
    $r=Complete-ProjectTask $c -Apply -Failed
    Check (!(Test-Path -LiteralPath $temp)) 'Failure completion did not remove disposable work.'
    Check (Test-Path -LiteralPath (Join-Path $c.Task.projectRoot 'LICENSE')) 'Canonical notice removed.'
    Check ((Get-Content (Get-ProjectTaskPath $c.Task.id) -Raw | ConvertFrom-Json).status -eq 'failed-cleaned') 'Failure completion not recorded.'
    $keep=New-Fixture
    $path=Join-Path $keep.Task.work 'requested.zip'; [IO.File]::WriteAllText($path,'deliverable')
    Add-ProjectArtifact $keep $path -Role deliverable
    $r=Complete-ProjectTask $keep -Apply
    Check (Test-Path -LiteralPath $path) 'Requested deliverable removed.'
    $keep.Task.resources=@(); Save-ProjectTask $keep.Task
    $r=Get-ProjectCleanupPlan $keep.Task $keep.Task.token -Apply
    Check (!(Test-Path -LiteralPath $path)) 'Released fixture not removed.'
    $cache=New-Fixture -Budget 4
    New-Item -ItemType Directory -Path (Join-Path $cache.Task.cacheRoot 'target') | Out-Null
    [IO.File]::WriteAllText((Join-Path $cache.Task.cacheRoot 'target\object'),'0123456789')
    [IO.File]::WriteAllText((Join-Path $cache.Task.cacheRoot 'LICENSE'),'canonical cache notice')
    $r=Trim-ProjectCache $cache -Apply
    Check (Test-Path -LiteralPath (Join-Path $cache.Task.cacheRoot 'target\object')) 'Unadopted cache was deleted.'
    $null=Adopt-ProjectCache $cache
    Check (!(Test-Path -LiteralPath (Join-Path $cache.Task.cacheRoot '.project-cache-owner.json'))) 'Cache adoption dry run wrote an owner.'
    $null=Adopt-ProjectCache $cache -Apply
    $r=Trim-ProjectCache $cache -Apply
    Check (!(Test-Path -LiteralPath (Join-Path $cache.Task.cacheRoot 'target'))) 'Compiler cache was not bounded.'
    Check (Test-Path -LiteralPath (Join-Path $cache.Task.cacheRoot 'LICENSE')) 'Cache trim deleted a canonical notice.'
    Check (@($r.deferred).Count -gt 0) 'Protected cache budget exception was not reported.'
    Reject { $exclusive=Open-CleanupMaintenanceLease -Exclusive; $exclusive.Dispose() } 'Maintenance ran while a task held a lease.'
    $dead=New-Fixture
    $dead.Task.ownerPid=2147483000; Save-ProjectTask $dead.Task
    $dead.Task.resources+=@([pscustomobject]@{kind='process';pid=$PID;startedUtc=(Get-Process -Id $PID).StartTime.ToUniversalTime().ToString('o');role='active-input'})
    Save-ProjectTask $dead.Task
    Check (Test-CleanupTaskActive $dead.Task) 'A surviving worker was mistaken for an abandoned task.'
    $dead.Task.resources=@();Save-ProjectTask $dead.Task
    $live=New-Fixture
    $r=@(Recover-ProjectTasks -Project aetherdesk)
    Check (Test-Path -LiteralPath $dead.Task.work) 'Recovery dry run removed a workspace.'
    $null=Recover-ProjectTasks -Project aetherdesk -Apply
    Check (!(Test-Path -LiteralPath $dead.Task.work)) 'Proven interrupted task not recovered.'
    Check (Test-Path -LiteralPath $live.Task.work) 'Recovery removed an active task.'
    Check (Test-Path -LiteralPath $shared.Task.work) 'AetherDesk recovery touched OpenEDA.'
    $manual=New-Fixture
    $manual.Task.ownerPid=0;$manual.Task.status='cleanup-deferred';Save-ProjectTask $manual.Task
    Check (Test-CleanupTaskActive $manual.Task) 'Uncertain interrupted chat ownership was guessed inactive.'
    $package=New-Fixture
    $delivery=Join-Path $testRoot 'verified-packages'
    $pending=Join-Path $package.Task.work 'completed.zip'
    [IO.File]::WriteAllText($pending,'verified bytes')
    $published=Publish-ProjectTaskOutput $package $pending $delivery
    Check (([IO.File]::ReadAllText($published)) -eq 'verified bytes') 'Completed output changed during publication.'
    [IO.File]::WriteAllText($pending,'replacement')
    Reject {Publish-ProjectTaskOutput $package $pending $delivery} 'Existing deliverable was overwritten.'
    Reject {Publish-ProjectTaskOutput $package $pending $package.Task.projectRoot} 'Output was moved into canonical source.'
    $null=Complete-ProjectTask $package -Apply -Failed
    Check ((Test-Path -LiteralPath $published) -and !(Test-Path -LiteralPath $package.Task.work)) 'Failure cleanup did not preserve completed output and remove partial output.'
    $git=New-Fixture
    New-Item -ItemType Directory -Path (Join-Path $git.Task.work '.git') | Out-Null
    Reject {Get-ProjectCleanupPlan $git.Task $git.Task.token -Apply} 'A Git repository was treated as a temporary bundle.'
    Check (Test-Path -LiteralPath (Join-Path $git.Task.work '.git')) 'Git repository fixture was deleted.'
    Write-Host "PASS: $assertions project-cleanup safety assertions."
} finally {
    foreach($context in $contexts){
        if($context.CacheLease){$context.CacheLease.Dispose()};if($context.Lease){$context.Lease.Dispose()}
    }
    # This is the newly-created test sandbox, never a project/store directory.
    $null=Assert-CleanupTree $testRoot $testRoot -Recurse
    Remove-Item -LiteralPath $testRoot -Recurse -Force
    $env:AETHER_PROJECT_CLEANUP_STATE=$oldState
}
