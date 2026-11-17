[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'Project-Cleanup.psm1') -DisableNameChecking
Import-Module (Join-Path $PSScriptRoot 'Release-Retention.psm1') -Force -DisableNameChecking
$module=Get-Module Release-Retention
# Substitute only the OS inventory. Production path/ownership/hash/quarantine
# operations and active-task claims are exercised against isolated fixtures.
& $module {
    $script:processPaths=@();$script:installPaths=@();$script:inventoryFails=$false
    function script:Get-ReleaseProcessPaths {if($script:inventoryFails){throw 'Inventory unavailable'};return $script:processPaths}
    function script:Get-ReleaseInstallPaths {return $script:installPaths}
}
$parent=if($env:AETHERDESK_CLEANUP_TASK_WORK){$env:AETHERDESK_CLEANUP_TASK_WORK}else{[IO.Path]::GetTempPath()}
$testRoot=Join-Path $parent ('aetherdesk-retention-test-'+[guid]::NewGuid().ToString('N'))
$null=Assert-CleanupTree $testRoot $parent
New-Item -ItemType Directory -Path $testRoot|Out-Null
$oldState=$env:AETHER_PROJECT_CLEANUP_STATE;$env:AETHER_PROJECT_CLEANUP_STATE=Join-Path $testRoot 'ledger'
$contexts=@();$assertions=0
function Check([bool]$Condition,[string]$Message){if(!$Condition){throw $Message};$script:assertions++}
function Reject([scriptblock]$Action,[string]$Message){$rejected=$false;try{& $Action|Out-Null}catch{$rejected=$true};Check $rejected $Message}
function New-Fixture {
    $base=Join-Path $testRoot ([guid]::NewGuid().ToString('N'));$source=Join-Path $base 'source'
    New-Item -ItemType Directory -Path $source -Force|Out-Null
    [IO.File]::WriteAllText((Join-Path $source 'LICENSE'),'canonical notice')
    $context=Start-ProjectTask -Project aetherdesk -ProjectRoot $source -OutputRoot (Join-Path $base 'output') -SkipWsl
    $script:contexts+=@($context)
    return $context
}
function Packages($Context){return Join-Path (Split-Path (Split-Path $Context.Task.work -Parent) -Parent) 'packages'}
function New-Release($Context,[int]$Number,[switch]$Portable) {
    $root=Packages $Context;$stem='AetherDesk-0.1.3-build10_20261116_120000_'+$Number.ToString('000');$package=Join-Path $root $stem
    New-Item -ItemType Directory -Path (Join-Path $package 'runtime') -Force|Out-Null
    @{schemaVersion=1;product='AetherDesk';version='0.1.3';build=10;runtime='native-rust';fingerprint=('a'*64);built='2026-11-16T12:00:00Z'}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $package 'build-info.json')
    @{version='0.1.3';build=10}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $package 'release.json')
    [IO.File]::WriteAllText((Join-Path $package 'AetherDesk.exe'),'verified fixture binary')
    [IO.File]::WriteAllText((Join-Path $package 'LICENSE'),'generated copy of canonical notice')
    [IO.File]::WriteAllText((Join-Path $package 'runtime\data'),'bundled runtime')
    $zip=Join-Path $root ($stem+'_20261116_120001_000.zip')
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory($package,$zip,[IO.Compression.CompressionLevel]::Fastest,$true)
    $outputs=@($package,$zip)
    if(!$Portable){$setup=Join-Path $root ($stem+'-Setup.exe');[IO.File]::WriteAllText($setup,'verified fixture installer');$outputs+=@($setup)}
    return $outputs
}
function Register-Release($Context,[string[]]$Outputs){return Complete-ProjectRelease $Context (Packages $Context) $Outputs -BuildSucceeded -KeepOld -Apply}
try {
    $c=New-Fixture;$old=@(New-Release $c 1);$null=Register-Release $c $old;$current=@(New-Release $c 2)
    $recordRoot=Join-Path (Split-Path (Packages $c) -Parent) 'reports\completed-releases'
    $r=Complete-ProjectRelease $c (Packages $c) $current -Apply
    Check ($r.queued.Count -eq 0 -and (Test-Path -LiteralPath $old[0])) 'A failed build retired an old release.'
    Check (@(Get-ChildItem -LiteralPath $recordRoot -File).Count -eq 1) 'A failed build registered a new completed release.'
    $dry=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded
    Check ($dry.queued.Count -eq 3 -and (Test-Path -LiteralPath $old[0])) 'Dry run failed to preview the whole old release or changed it.'
    Check (@(Get-ChildItem -LiteralPath $recordRoot -File).Count -eq 1) 'Dry run wrote a registry record.'
    $r=Register-Release $c $current
    Check ($r.queued.Count -eq 0 -and (Test-Path -LiteralPath $old[0])) 'Build-Faster retention removed an old release.'
    $latest=Join-Path (Split-Path (Packages $c) -Parent) 'LATEST-ZIP.txt'
    [IO.File]::WriteAllText($latest,$current[1])
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ($r.queued.Count -eq 3 -and !(Test-Path -LiteralPath $old[0])) 'Full-build retention did not retire all old outputs.'
    Check (@(Get-ChildItem -LiteralPath (Join-Path $c.Task.work 'obsolete-releases') -Recurse -File).Count -eq 7) 'Old outputs bypassed task-owned quarantine.'
    Check (@($current|Where-Object {Test-Path -LiteralPath $_}).Count -eq 3) 'Newest release outputs were removed.'
    Check (([IO.File]::ReadAllText($latest)) -eq $current[1]) 'Latest-package pointer changed during retention.'
    Check (Test-Path -LiteralPath (Join-Path $c.Task.projectRoot 'LICENSE')) 'Canonical source notice removed.'
    $cleanup=Complete-ProjectTask $c
    Check (Test-Path -LiteralPath $c.Task.work) 'Cleanup dry run removed quarantine.'
    $null=Complete-ProjectTask $c -Apply
    Check (!(Test-Path -LiteralPath $c.Task.work) -and (Test-Path -LiteralPath $current[0])) 'Task completion did not remove obsolete data while retaining newest outputs.'

    $c=New-Fixture;$old=@(New-Release $c 1);$null=Register-Release $c $old;$current=@(New-Release $c 2)
    & $module {param($p) $script:processPaths=@($p)} (Join-Path $old[0] 'AetherDesk.exe')
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ($r.queued.Count -eq 0 -and (Test-Path -LiteralPath $old[0])) 'A running portable copy was retired.'
    & $module {param($p) $script:processPaths=@();$script:installPaths=@($p)} $old[0]
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ($r.queued.Count -eq 0 -and (Test-Path -LiteralPath $old[0])) 'Installed copy was retired.'
    & $module {$script:installPaths=@();$script:inventoryFails=$true}
    Reject {Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply} 'Unknown process state allowed release deletion.'
    Check (Test-Path -LiteralPath $old[0]) 'Inventory failure removed an old release.'
    & $module {$script:inventoryFails=$false}
    $other=New-Fixture;Add-ProjectArtifact $other (Join-Path $old[0] 'runtime') -Role required-runtime
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ($r.queued.Count -eq 0 -and (Test-Path -LiteralPath $old[0])) 'Another active task release claim was ignored.'
    $other.Task.resources=@();Save-ProjectTask $other.Task
    [IO.File]::WriteAllText((Join-Path $old[0] 'settings.json'),'user data')
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ($r.queued.Count -eq 0 -and (Test-Path -LiteralPath (Join-Path $old[0] 'settings.json'))) 'User-modified package was deleted.'
    Remove-Item -LiteralPath (Join-Path $old[0] 'settings.json')
    New-Item -ItemType Directory -Path (Join-Path $old[0] 'empty-user-folder')|Out-Null
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded
    Check ($r.queued.Count -eq 0) 'An added empty user directory was ignored.'
    Remove-Item -LiteralPath (Join-Path $old[0] 'empty-user-folder')
    $link=Join-Path $old[0] 'escape';New-Item -ItemType Junction -Path $link -Target $c.Task.projectRoot|Out-Null
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ($r.queued.Count -eq 0 -and (Test-Path -LiteralPath $link)) 'Release symlink/junction was followed or removed.'
    [IO.Directory]::Delete($link)
    [IO.File]::AppendAllText($old[1],'changed')
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ($r.queued.Count -eq 0) 'Changed release archive was deleted.'
    $outside=Join-Path $c.Task.projectRoot 'LICENSE'
    $realRecord=Join-Path (Join-Path (Split-Path (Packages $c) -Parent) 'reports\completed-releases') ((Split-Path $old[0] -Leaf)+'.json')
    $record=Get-Content -LiteralPath $realRecord -Raw|ConvertFrom-Json;$record.artifacts[0].path=$outside
    $record|ConvertTo-Json -Depth 16|Set-Content -LiteralPath $realRecord
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ((Test-Path -LiteralPath $outside) -and $r.queued.Count -eq 0) 'A manipulated manifest escaped the packages directory.'

    $c=New-Fixture;$old=@(New-Release $c 1 -Portable);$current=@(New-Release $c 2)
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded
    Check ($r.queued.Count -eq 2 -and (Test-Path -LiteralPath $old[0])) 'Exact legacy package/ZIP was not safely adopted in dry run.'
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ($r.queued.Count -eq 2 -and !(Test-Path -LiteralPath $old[1])) 'Verified legacy release did not retire.'
    $unknown=Join-Path (Packages $c) 'my-project';New-Item -ItemType Directory -Path (Join-Path $unknown '.git') -Force|Out-Null
    $modified=@(New-Release $c 3 -Portable);[IO.File]::WriteAllText((Join-Path $modified[0] 'private-key'),'irreplaceable fixture')
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ((Test-Path -LiteralPath $unknown) -and (Test-Path -LiteralPath $modified[0])) 'Untracked project/user files were adopted by age or name.'
    $newestInfo=Join-Path $current[0] 'build-info.json';[IO.File]::WriteAllText($newestInfo,'invalid')
    Reject {Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply} 'Invalid newest package allowed old-release cleanup.'
    Check (Test-Path -LiteralPath $modified[0]) 'Newest verification failure removed an old release.'

    $c=New-Fixture;$first=@(New-Release $c 1);$null=Register-Release $c $first
    $second=@(New-Release $c 2);$null=Register-Release $c $second
    $current=@(New-Release $c 3)
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ($r.queued.Count -eq 6 -and @((Get-ChildItem -LiteralPath (Packages $c))).Count -eq 3) 'A full build did not retire multiple older faster-build releases as complete sets.'
    Reject {Complete-ProjectRelease $c $c.Task.projectRoot $current -BuildSucceeded -Apply} 'Source root was accepted as the packages directory.'

    $c=New-Fixture;$old=@(New-Release $c 1);$null=Register-Release $c $old;$current=@(New-Release $c 2)
    # A copy can become active between the preview and the actual move.
    & $module {
        param($p) $script:racePath=$p;$script:inventoryCalls=0
        function script:Get-ReleaseProcessPaths {$script:inventoryCalls++;if($script:inventoryCalls -gt 1){return $script:racePath};return @()}
    } (Join-Path $old[0] 'AetherDesk.exe')
    $r=Complete-ProjectRelease $c (Packages $c) $current -BuildSucceeded -Apply
    Check ($r.queued.Count -eq 0 -and (Test-Path -LiteralPath $old[0])) 'A copy launched during retention was not rechecked.'

    $runner=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'Run-Build-Step.ps1'))
    Check ($runner.IndexOf("`$buildStatus = 'Passed'") -lt $runner.IndexOf('$releasePlan=Complete-ProjectRelease')) 'Retention occurs before full build success.'
    Check ($runner.Contains('$retainOld=$keepCache -or $PortableOnly')) 'Faster or portable-only builds lost old-release protection.'
    Check ($runner.Contains('-BuildSucceeded -KeepOld:$retainOld -Apply')) 'Build runner omitted the successful-build/retention gate.'
    Write-Host "PASS: $assertions completed-release retention assertions (real filesystem, isolated OS inventory)."
} finally {
    foreach($context in $contexts){if($context.CacheLease){$context.CacheLease.Dispose()};if($context.Lease){$context.Lease.Dispose()}}
    $null=Assert-CleanupTree $testRoot $parent -Recurse
    Remove-Item -LiteralPath $testRoot -Recurse -Force
    $env:AETHER_PROJECT_CLEANUP_STATE=$oldState
    Remove-Module Release-Retention
}
