[CmdletBinding()]
param([string]$Repository = ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))),[switch]$CheckReferences)
$ErrorActionPreference='Stop'
# Explicit platform-only attributes must agree with the selected build phase.
# Compiler registrations remain authoritative for other conditional expressions.
function Assert-TestPlatformPhase([string]$Phase,[string]$Attributes,[string]$Name) {
    foreach($match in [regex]::Matches($Attributes,'#\[cfg\((?<condition>[^\r\n]*)\)\]')) {
        $condition=$match.Groups['condition'].Value -replace '\s+',''
        $allowed=@()
        if($condition -in @('target_os="linux"','unix','not(windows)','not(target_os="windows")','all(test,unix)','all(test,target_os="linux")')) {$allowed=@('linux')}
        if($condition -in @('target_os="windows"','windows','not(unix)','not(target_os="linux")','all(test,windows)','all(test,target_os="windows")')) {$allowed=@('windows','package')}
        if($allowed.Count -and $Phase -notin $allowed){throw "Test platform does not match regression phase ${Phase}: $Name"}
    }
}
function Get-RegressionCatalog([string]$Root,[bool]$References=$false) {
    $catalog=Get-Content -LiteralPath (Join-Path $Root 'rust-regressions.json') -Raw | ConvertFrom-Json
    $inventory=Get-Content -LiteralPath (Join-Path $Root 'rust-feature-inventory.json') -Raw | ConvertFrom-Json
    if($catalog.schemaVersion -ne 1 -or $catalog.replacementReady -ne $false -or !$catalog.features -or !$catalog.originalSuites){throw 'Invalid native regression catalog.'}
    $ids=@{}; $checks=@{}
    foreach($feature in $catalog.features){
        if(!$feature.id -or $ids.ContainsKey($feature.id) -or !$feature.liveAcceptance -or !$feature.checks){throw 'Every behavior requires unique regression and live acceptance coverage.'}
        $ids[$feature.id]=$true
        foreach($check in $feature.checks){
            foreach($relative in @($check.source,$check.script)|Where-Object{$_}){
                if([IO.Path]::IsPathRooted($relative) -or $relative -match '(^|[\\/])\.\.([\\/]|$)' -or !(Test-Path -LiteralPath (Join-Path $Root $relative) -PathType Leaf)){throw "Missing regression source: $relative"}
            }
            if($check.phase -notin @('windows','linux','package','offline')){throw 'Unknown regression phase.'}
            if($check.phase -eq 'offline') {if(!$check.script){throw 'Offline check has no script.'};continue}
            if(!$check.package -or !$check.test -or !$check.source){throw 'Native checks need package, test and source identities.'}
            $source=[IO.File]::ReadAllText((Join-Path $Root $check.source))
            $pattern='(?<attributes>(?:\s*#\[[^\]]*\])+)\s*(?:async\s+)?fn\s+'+[regex]::Escape($check.test)+'\s*\('
            $match=[regex]::Match($source,$pattern)
            if(!$match.Success -or $match.Groups['attributes'].Value -notmatch '#\[(?:tokio::)?test(?:\([^\]]*\))?\]'){throw "Missing behavioral test: $($check.test)"}
            Assert-TestPlatformPhase $check.phase $match.Groups['attributes'].Value $check.test
            if($check.phase -ne 'package' -and $match.Groups['attributes'].Value -match '#\[ignore'){throw "Ignored test cannot count as automated coverage: $($check.test)"}
            $key=$check.phase+'/'+$check.package+'/'+$check.test
            if($checks.ContainsKey($key) -and $checks[$key] -ne $check.source){throw "Ambiguous regression identity: $key"}
            $checks[$key]=$check.source
        }
    }
    $expected=@($inventory.features|ForEach-Object{$_.id})
    if($expected.Count -ne $ids.Count -or @($expected|Where-Object{!$ids.ContainsKey($_)}).Count){throw 'Regression catalog does not cover every inventoried behavior.'}
    $suites=@{}
    foreach($suite in $catalog.originalSuites){
        if(!$suite.reference -or $suites.ContainsKey($suite.reference) -or !$suite.features){throw 'Original test suite mapping is missing or duplicated.'}
        $suites[$suite.reference]=$true
        foreach($id in $suite.features){if(!$ids.ContainsKey($id)){throw "Unmapped original behavior: $id"}}
        if($References -and !(Test-Path -LiteralPath (Join-Path $Root $suite.reference) -PathType Leaf)){throw "Original test reference is missing: $($suite.reference)"}
    }
    if($References){
        foreach($file in Get-ChildItem -LiteralPath (Join-Path $Root 'ui/flutter/ad_remote_desktop/test') -Filter '*_test.dart'){
            $relative='ui/flutter/ad_remote_desktop/test/'+$file.Name
            if(!$suites.ContainsKey($relative)){throw "Original suite has no native equivalent: $relative"}
        }
    }
    return $catalog
}
function Assert-RegisteredRegressions($Catalog,[string]$Phase,[string]$Text) {
    $registered=@([regex]::Matches($Text,'(?m)^([A-Za-z0-9_:]+): test\s*\r?$')|ForEach-Object{$_.Groups[1].Value})
    if(!$registered.Count){throw "The $Phase compiler test registry is empty."}
    $required=@($Catalog.features|ForEach-Object{$_.checks}|Where-Object{$_.phase -eq $Phase}|Select-Object -ExpandProperty test -Unique)
    foreach($name in $required){
        if(!@($registered|Where-Object{$_ -eq $name -or $_.EndsWith('::'+$name,[StringComparison]::Ordinal)}).Count){throw "Compiler did not register required test: $name"}
    }
    return [pscustomobject]@{registered=$registered.Count;required=$required.Count;names=$registered}
}
if($MyInvocation.InvocationName -ne '.'){
    $catalog=Get-RegressionCatalog $Repository ([bool]$CheckReferences)
    Write-Host "PASS: $($catalog.features.Count) behavior groups and $($catalog.originalSuites.Count) original suites mapped to native regressions. Compiler/live results are separate."
}
