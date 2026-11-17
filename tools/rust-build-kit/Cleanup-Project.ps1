[CmdletBinding()]
param(
    [ValidateSet('Status','Begin','DryRun','Complete','Recover','AdoptCache','Compact')][string]$Action='Status',
    [ValidateSet('aetherdesk','openeda')][string]$Project='aetherdesk',
    [string]$ProjectRoot=([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))),
    [string]$OutputRoot,[string]$TaskId,[string]$Token,[string]$CacheRoot,
    [switch]$Apply,[switch]$IdleConfirmed,[string]$ReviewedHelperSha256,
    [string]$CompactionHelper=(Join-Path $env:USERPROFILE 'Documents\Codex\2026-10-19\run-x20\work\Compact-UbuntuDisk.ps1')
)
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'Project-Cleanup.psm1') -Force -DisableNameChecking
if (!$OutputRoot) { $OutputRoot=Join-Path (Split-Path $ProjectRoot -Parent) ($Project+'-cleanup-output') }
if ($Action -eq 'Begin') {
    $context=Start-ProjectTask -Project $Project -ProjectRoot $ProjectRoot -OutputRoot $OutputRoot -CacheRoot $CacheRoot -Manual
    try { $context.Task | ConvertTo-Json -Depth 20 } finally { if($context.CacheLease){$context.CacheLease.Dispose()};$context.Lease.Dispose() }
    exit 0
}
if ($Action -eq 'Recover') { Recover-ProjectTasks -Project $Project -Apply:$Apply | ConvertTo-Json -Depth 20; exit 0 }
if ($Action -eq 'Compact') {
    if (!$IdleConfirmed) { Write-Output 'Compaction deferred: freshly confirm BOTH chats and ALL other WSL jobs are idle.'; exit 0 }
    $lock=$null
    try { $lock=Open-CleanupMaintenanceLease -Exclusive }
    catch { Write-Output 'Compaction deferred: another task holds the shared maintenance lock.'; exit 0 }
    try {
        if (@(Read-ProjectTasks | Where-Object { Test-CleanupTaskActive $_ }).Count) { throw 'Compaction deferred: an active or uncertain task remains.' }
        $helper=Assert-CleanupTree $CompactionHelper (Split-Path $CompactionHelper -Parent)
        $text=[IO.File]::ReadAllText($helper)
        $matches=[regex]::Matches($text,'(?m)^\$ubuntuVhdPath\s*=\s*''([^'']+)''\s*$')
        if ($matches.Count -ne 1) { throw 'Compaction helper has no single reviewable Ubuntu disk target.' }
        $target=[IO.Path]::GetFullPath($matches[0].Groups[1].Value)
        $registered=@(Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss' | Get-ItemProperty | Where-Object {$_.DistributionName -eq 'Ubuntu'})
        if ($registered.Count -ne 1) { throw 'Cannot verify the current Ubuntu registration.' }
        $diskName='ext4.vhdx'
        if ($registered[0].PSObject.Properties['VhdFileName'] -and $registered[0].VhdFileName) { $diskName=$registered[0].VhdFileName }
        $expected=[IO.Path]::GetFullPath((Join-Path $registered[0].BasePath $diskName))
        if (!$target.Equals($expected,[StringComparison]::OrdinalIgnoreCase)) { throw 'Helper target does not match Ubuntu registered disk.' }
        $null=Assert-CleanupTree $target (Split-Path $target -Parent)
        $hash=(Get-FileHash -LiteralPath $helper -Algorithm SHA256).Hash
        $probe=[pscustomobject]@{project=$Project;id='';distribution='Ubuntu';user='root';store=@{uid=0;driver='vfs';graphRoot='/var/lib/containers/storage'};skipWsl=$false}
        $before=Invoke-CleanupLinux $probe 'inspect'
        if (@($before.containers).Count) { throw 'Compaction deferred: container/build storage remains; verify all WSL jobs and ownership first.' }
        if (!$Apply) { [pscustomobject]@{action='compact';dryRun=$true;disk=$target;helperSha256=$hash;windowsFreeBytes=(Get-PSDrive C).Free} | ConvertTo-Json; exit 0 }
        if (!$ReviewedHelperSha256 -or $hash -ne $ReviewedHelperSha256) { throw 'Read and review the helper, then pass its reviewed SHA256 from the compaction dry run.' }
        foreach($arguments in @(
            @('-d','Ubuntu','-u','root','--exec','sync'),
            @('-d','Ubuntu','-u','root','--exec','/usr/sbin/fstrim','-v','/'),
            @('--shutdown')
        )) {
            & wsl.exe @arguments
            if ($LASTEXITCODE -ne 0) { throw 'A compaction preparation command failed; stopped before the next command.' }
        }
        if ((Get-FileHash -LiteralPath $helper -Algorithm SHA256).Hash -ne $hash) { throw 'Compaction helper changed since review.' }
        & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File $helper
        if ($LASTEXITCODE -ne 0) { throw 'Ubuntu compaction helper failed.' }
        & wsl.exe -d Ubuntu -u root --exec true
        if ($LASTEXITCODE -ne 0) { throw 'Ubuntu did not reopen after compaction.' }
        $after=Invoke-CleanupLinux $probe 'inspect'
        $afterIds=@($after.images | ForEach-Object {$_.Id})
        foreach($image in $before.images){if($image.Id -notin $afterIds){throw 'A pre-existing project/runtime image is missing after maintenance.'}}
        [pscustomobject]@{compaction='complete';windowsFreeBytes=(Get-PSDrive C).Free;linuxDf=$after.df} | ConvertTo-Json
    } finally { if($lock){$lock.Dispose()} }
    exit 0
}
if ($Action -eq 'Status') {
    $probe=[pscustomobject]@{project=$Project;id='';distribution='Ubuntu';user='root';store=@{uid=0;driver='vfs';graphRoot='/var/lib/containers/storage'};skipWsl=$false}
    [pscustomobject]@{windows=(Get-PSDrive C | Select-Object Name,Used,Free);linux=(Invoke-CleanupLinux $probe 'inspect');tasks=@(Read-ProjectTasks | Where-Object {$_.project -eq $Project})} | ConvertTo-Json -Depth 20
    exit 0
}
$path=Get-ProjectTaskPath $TaskId
$task=Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
Assert-ProjectTaskToken $task $Token
if($task.project -ne $Project){throw 'Task belongs to a different project.'}
$context=[pscustomobject]@{Task=$task;Lease=(Open-CleanupMaintenanceLease);CacheLease=$null}
try {
    if($task.cacheRoot){$context.CacheLease=[IO.File]::Open((Join-Path $task.cacheRoot '.project-cache.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)}
    if ($Action -eq 'AdoptCache') {
        if (!$task.cacheRoot) { throw 'Task has no configured cache.' }
        Adopt-ProjectCache $context -Apply:$Apply | ConvertTo-Json
    } else {
        $result=Complete-ProjectTask $context -Apply:($Apply -and $Action -eq 'Complete')
        Show-ProjectCleanupReport $result
        $result | ConvertTo-Json -Depth 20
    }
} finally { if($context.CacheLease){$context.CacheLease.Dispose()};$context.Lease.Dispose() }
