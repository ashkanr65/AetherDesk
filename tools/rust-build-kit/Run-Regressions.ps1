[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$Snapshot,[Parameter(Mandatory=$true)][string]$Report)
$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$false
. (Join-Path $PSScriptRoot 'Check-Regressions.ps1')
function Invoke-RegressionTool([string[]]$Arguments){
    Write-Host ('Running compiler regressions: cargo.exe '+($Arguments -join ' '))
    $lines=@(& cargo.exe @Arguments | ForEach-Object{if($Arguments -notcontains '--list'){Write-Host $_};$_})
    if($LASTEXITCODE -ne 0){throw "Native regression command failed with exit code $LASTEXITCODE"}
    return $lines -join [Environment]::NewLine
}
function Invoke-NativeRegressions([string]$Root,[string]$Destination){
    if(Test-Path -LiteralPath $Destination -PathType Leaf){Remove-Item -LiteralPath $Destination -Force}
    $catalog=Get-RegressionCatalog $Root
    $workspaces=@(
        @{path='ui/rust/Cargo.toml';packages=@('ad-desktop-model','ad-desktop')},
        @{path='core/rust/Cargo.toml';packages=@('ad-ssh','ad-vpn','ad-stream-protocol','ad-stream-client','ad-display-decoder','ad-remote-session','ad-qos-controller')}
    )
    $registry='';$summaries=@()
    foreach($workspace in $workspaces){
        $arguments=@('test','--locked','--manifest-path',(Join-Path $Root $workspace.path),'--target','x86_64-pc-windows-msvc')
        foreach($package in $workspace.packages){$arguments+=@('-p',$package)}
        $registry+= (Invoke-RegressionTool ($arguments+@('--','--list'))) + [Environment]::NewLine
    }
    $registrations=Assert-RegisteredRegressions $catalog 'windows' $registry
    foreach($workspace in $workspaces){
        $arguments=@('test','--locked','--manifest-path',(Join-Path $Root $workspace.path),'--target','x86_64-pc-windows-msvc')
        foreach($package in $workspace.packages){$arguments+=@('-p',$package)}
        $text=Invoke-RegressionTool ($arguments+@('--','--test-threads=1'))
        $passed=@([regex]::Matches($text,'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;')|ForEach-Object{
            [pscustomobject]@{passed=[int]$_.Groups[1].Value;failed=[int]$_.Groups[2].Value;ignored=[int]$_.Groups[3].Value}
        })
        if(!($passed|Where-Object{$_.passed -gt 0}) -or @($passed|Where-Object{$_.failed -gt 0}).Count){throw 'Native tests did not produce a nonempty passing test result.'}
        $summaries+=@{workspace=$workspace.path;results=$passed}
    }
    $totalPassed=0
    foreach($summary in $summaries){foreach($result in $summary.results){$totalPassed+=$result.passed}}
    if($totalPassed -lt $registrations.required){throw 'Fewer behavioral tests passed than the required registered baseline.'}
    $record=[ordered]@{schemaVersion=1;compilerTestsPassed=$true;liveAcceptancePassed=$false;replacementReady=$false;
        catalogSha256=(Get-FileHash -LiteralPath (Join-Path $Root 'rust-regressions.json')).Hash;registered=$registrations.registered;required=$registrations.required;
        workspaces=$summaries;checked=(Get-Date -Format o)}
    New-Item -ItemType Directory -Force -Path (Split-Path $Destination -Parent)|Out-Null
    $record|ConvertTo-Json -Depth 7|Set-Content -LiteralPath $Destination -Encoding UTF8
}
if($MyInvocation.InvocationName -ne '.') {Invoke-NativeRegressions $Snapshot $Report}
