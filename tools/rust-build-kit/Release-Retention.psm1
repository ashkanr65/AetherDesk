Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'Project-Cleanup.psm1') -DisableNameChecking
$script:ReleaseStem='AetherDesk-\d+\.\d+\.\d+(?:-[a-zA-Z0-9.-]+)?-build\d+_\d{8}_\d{6}_\d{3}'

function Test-ReleasePathOverlap([string]$Left,[string]$Right) {
    $a=[IO.Path]::GetFullPath($Left).TrimEnd('\');$b=[IO.Path]::GetFullPath($Right).TrimEnd('\')
    return $a.Equals($b,[StringComparison]::OrdinalIgnoreCase) -or $a.StartsWith($b+'\',[StringComparison]::OrdinalIgnoreCase) -or $b.StartsWith($a+'\',[StringComparison]::OrdinalIgnoreCase)
}
function Get-ReleaseSnapshot([string]$Path,[string]$Root) {
    $full=Assert-CleanupTree $Path $Root -Recurse -ForDeletion
    if ((Split-Path $full -Parent) -ne $Root) { throw 'Release artifacts must be direct children of the packages directory.' }
    $item=Get-Item -LiteralPath $full -Force
    if ($item.PSIsContainer) {
        $files=@(Get-ChildItem -LiteralPath $full -Recurse -File -Force | Sort-Object FullName | ForEach-Object {
            [pscustomobject]@{relative=$_.FullName.Substring($full.Length+1);length=$_.Length;sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash}
        })
        return [pscustomobject]@{path=$full;kind='directory';directories=@(Get-ChildItem -LiteralPath $full -Recurse -Directory -Force | ForEach-Object {$_.FullName.Substring($full.Length+1)} | Sort-Object);files=$files}
    }
    return [pscustomobject]@{path=$full;kind='file';length=$item.Length;sha256=(Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash}
}
function Get-ReleaseIdentity([string]$Package) {
    $stem=Split-Path $Package -Leaf
    if ($stem -notmatch ('^'+$script:ReleaseStem+'$')) { throw 'Unrecognized release package name.' }
    $info=Get-Content -LiteralPath (Join-Path $Package 'build-info.json') -Raw | ConvertFrom-Json
    $version=Get-Content -LiteralPath (Join-Path $Package 'release.json') -Raw | ConvertFrom-Json
    if ($info.product -ne 'AetherDesk' -or $info.runtime -ne 'native-rust' -or $info.schemaVersion -ne 1 -or
        $info.fingerprint -notmatch '^[a-f0-9]{64}$' -or $version.version -ne $info.version -or $version.build -ne $info.build -or
        !$stem.StartsWith(('AetherDesk-'+$info.version+'-build'+$info.build+'_'),[StringComparison]::Ordinal)) { throw 'Release identity does not match its package.' }
    return $info
}
function New-ReleaseRecord($Context,[string]$Root,[string[]]$Outputs) {
    $paths=@($Outputs|ForEach-Object {[IO.Path]::GetFullPath($_).TrimEnd('\')}|Sort-Object -Unique)
    $folders=@($paths|Where-Object {Test-Path -LiteralPath $_ -PathType Container})
    if ($folders.Count -ne 1 -or $paths.Count -notin @(2,3)) { throw 'A completed release requires one verified package, ZIP and optional installer.' }
    $package=$folders[0];$stem=Split-Path $package -Leaf
    $info=Get-ReleaseIdentity $package
    foreach($path in $paths) {
        $name=Split-Path $path -Leaf
        if ($path -ne $package -and $name -ne ($stem+'-Setup.exe') -and $name -notmatch ('^'+[regex]::Escape($stem)+'_\d{8}_\d{6}_\d{3}\.zip$')) { throw 'Output does not belong to this release.' }
    }
    if (@($paths|Where-Object {$_ -match '\.zip$'}).Count -ne 1) { throw 'Completed release ZIP missing or ambiguous.' }
    return [pscustomobject]@{schema=1;project='aetherdesk';projectRoot=$Context.Task.projectRoot;packagesRoot=$Root;stem=$stem;version=$info.version;build=$info.build;fingerprint=$info.fingerprint;completedUtc=[DateTime]::UtcNow.ToString('o');task=$Context.Task.id;artifacts=@($paths|ForEach-Object {Get-ReleaseSnapshot $_ $Root})}
}
function Test-ReleaseZip($Folder,[string]$Archive) {
    # Legacy adoption requires the ZIP to contain exactly the package's bytes,
    # including notices, with no added profile/user files. Never extract it.
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip=[IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $files=@{};$dirs=@{};$root=(Split-Path $Folder.path -Leaf)+'/'
        $expectedFiles=@{};foreach($file in $Folder.files){$expectedFiles[$file.relative]=$file}
        foreach($entry in $zip.Entries) {
            $name=$entry.FullName.Replace('\','/')
            if (!$name.StartsWith($root,[StringComparison]::Ordinal) -or $name -match '(^|/)\.\.(/|$)|:') { return $false }
            $relative=$name.Substring($root.Length).TrimEnd('/').Replace('/','\')
            if (!$relative) { continue }
            $parent=Split-Path $relative -Parent
            while($parent){$dirs[$parent]=$true;$parent=Split-Path $parent -Parent}
            if ($name.EndsWith('/')) {$dirs[$relative]=$true;continue}
            if ($files.ContainsKey($relative)) { return $false }
            if (!$expectedFiles.ContainsKey($relative)) {return $false}
            $expected=$expectedFiles[$relative]
            if ($expected.relative -cne $relative -or $entry.Length -ne $expected.length) { return $false }
            $stream=$entry.Open();$sha=[Security.Cryptography.SHA256]::Create()
            try {$hash=[BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-','')} finally {$stream.Dispose();$sha.Dispose()}
            if ($hash -ne $expected.sha256) { return $false };$files[$relative]=$true
        }
        return $files.Count -eq @($Folder.files).Count -and $dirs.Count -eq @($Folder.directories).Count -and @($Folder.directories|Where-Object {!$dirs.ContainsKey($_)}).Count -eq 0
    } finally {$zip.Dispose()}
}
function Get-LegacyReleaseRecord($Context,[string]$Root,[string]$Package) {
    $null=Assert-CleanupTree $Package $Root -Recurse -ForDeletion
    $info=Get-ReleaseIdentity $Package;$stem=Split-Path $Package -Leaf
    $zips=@(Get-ChildItem -LiteralPath $Root -File|Where-Object {$_.Name -match ('^'+[regex]::Escape($stem)+'_\d{8}_\d{6}_\d{3}\.zip$')})
    if ($zips.Count -ne 1) { throw 'Legacy release has no unique verifiable completed ZIP.' }
    $folder=Get-ReleaseSnapshot $Package $Root
    if (!(Test-ReleaseZip $folder $zips[0].FullName)) { throw 'Legacy release differs from its completed ZIP; preserving it.' }
    $outputs=@($Package,$zips[0].FullName);$setup=Join-Path $Root ($stem+'-Setup.exe')
    if (Test-Path -LiteralPath $setup) {
        $null=Assert-CleanupTree $setup $Root
        $v=(Get-Item -LiteralPath $setup).VersionInfo
        if ($v.ProductName.Trim() -ne 'AetherDesk' -or $v.FileDescription.Trim() -ne 'AetherDesk Setup' -or $v.FileVersion.Trim() -ne ($info.version+'.'+$info.build)) { throw 'Legacy installer identity is unverified.' }
        $outputs+=@($setup)
    }
    $record=New-ReleaseRecord $Context $Root $outputs
    $record.completedUtc=([DateTimeOffset]$info.built).UtcDateTime.ToString('o')
    return $record
}
function Get-ReleaseProcessPaths {
    # Failure to establish process/install state defers deletion, never kills a
    # process or guesses that an inaccessible application is inactive.
    $paths=@()
    foreach($process in Get-CimInstance -ClassName Win32_Process -ErrorAction Stop) {
        if ($process.ExecutablePath) {$paths+=@($process.ExecutablePath)}
        elseif($process.Name -match '^(AetherDesk.*|aetherdesk-connector|vcxsrv|XWin)\.exe$') { throw 'A running application path is inaccessible; release cleanup deferred.' }
    }
    return $paths
}
function Get-ReleaseInstallPaths {
    $paths=@()
    foreach($root in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall','HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall','HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall','HKCU:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall')) {
        if (!(Test-Path -LiteralPath $root)) { continue }
        foreach($key in Get-ChildItem -LiteralPath $root -ErrorAction Stop) {
            $app=Get-ItemProperty -LiteralPath $key.PSPath -ErrorAction Stop
            if (!$app.PSObject.Properties['DisplayName'] -or $app.DisplayName -notlike 'AetherDesk*') {continue}
            foreach($property in @('InstallLocation','Inno Setup: App Path')) {
                if($app.PSObject.Properties[$property] -and $app.$property) {$paths+=@($app.$property.Trim('"'))}
            }
        }
    }
    return $paths
}
function Get-ReleaseProtection($Context) {
    $paths=@(Get-ReleaseProcessPaths)+@(Get-ReleaseInstallPaths)
    foreach($other in Read-ProjectTasks|Where-Object {$_.id -ne $Context.Task.id -and (Test-CleanupTaskActive $_)}) {
        $paths+=@($other.projectRoot,$other.work)
        if($other.cacheRoot){$paths+=@($other.cacheRoot)}
        foreach($resource in $other.resources|Where-Object {$_.kind -eq 'windows-path'}) {$paths+=@($resource.path)}
    }
    return $paths
}
function Assert-ReleaseRecord($Record,$Context,[string]$Root) {
    if ($Record.schema -ne 1 -or $Record.project -ne 'aetherdesk' -or $Record.projectRoot -ne $Context.Task.projectRoot -or
        $Record.packagesRoot -ne $Root -or $Record.stem -notmatch ('^'+$script:ReleaseStem+'$')) { throw 'Release ownership record does not match this project/output root.' }
    $paths=@($Record.artifacts|ForEach-Object {$_.path})
    foreach($artifact in $Record.artifacts) {
        $null=Assert-CleanupTree $artifact.path $Root
        $name=Split-Path $artifact.path -Leaf
        if ((Split-Path $artifact.path -Parent) -ne $Root -or ($name -ne $Record.stem -and $name -ne ($Record.stem+'-Setup.exe') -and
            $name -notmatch ('^'+[regex]::Escape($Record.stem)+'_\d{8}_\d{6}_\d{3}\.zip$'))) {throw 'Recorded artifact is outside its release.'}
        if (Test-Path -LiteralPath $artifact.path) {
            $actual=Get-ReleaseSnapshot $artifact.path $Root
            if (($actual|ConvertTo-Json -Depth 12 -Compress) -cne ($artifact|ConvertTo-Json -Depth 12 -Compress)) { throw 'Release was modified after completion; preserving user/runtime files.' }
        }
    }
    if (@($paths|Sort-Object -Unique).Count -ne $paths.Count -or $paths.Count -notin @(2,3) -or (Join-Path $Root $Record.stem) -notin $paths) {throw 'Incomplete or duplicated release ownership record.'}
}
function Write-ReleaseJson([string]$Path,$Value,[string]$Work) {
    $temporary=Join-Path $Work ('release-record-'+[guid]::NewGuid().ToString('N')+'.tmp')
    $null=Assert-CleanupTree $temporary $Work
    try {
        [IO.File]::WriteAllText($temporary,($Value|ConvertTo-Json -Depth 16),[Text.UTF8Encoding]::new($false))
        Move-Item -LiteralPath $temporary -Destination $Path -Force
    } finally {if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}
}
function Complete-ProjectRelease {
    param($Context,[string]$PackagesRoot,[string[]]$Outputs,[switch]$BuildSucceeded,[switch]$KeepOld,[switch]$Apply)
    $result=[pscustomobject]@{dryRun=(!$Apply);keepOld=[bool]$KeepOld;newest='';queued=@();preserved=@()}
    if (!$BuildSucceeded) {$result.preserved+=@('Build did not succeed; all releases retained.');return $result}
    if ($Context.Task.project -ne 'aetherdesk') {throw 'Release retention is scoped to AetherDesk.'}
    $root=Assert-CleanupTree $PackagesRoot $PackagesRoot
    if ((Split-Path $root -Leaf) -ne 'packages' -or (Test-ReleasePathOverlap $root $Context.Task.projectRoot) -or (Test-ReleasePathOverlap $root $Context.Task.work)) {throw 'Release retention requires a separate packages output directory.'}
    $current=New-ReleaseRecord $Context $root $Outputs;$result.newest=$current.stem
    $recordRoot=Join-Path (Split-Path $root -Parent) 'reports\completed-releases'
    $null=Assert-CleanupTree $recordRoot $recordRoot -Recurse
    $records=@();$seen=@{};$protected=@()
    if (!$KeepOld) {
        $protected=@(Get-ReleaseProtection $Context)
        if(Test-Path -LiteralPath $recordRoot) {
            foreach($file in Get-ChildItem -LiteralPath $recordRoot -Filter '*.json' -File) {
                try {
                    $record=Get-Content -LiteralPath $file.FullName -Raw|ConvertFrom-Json
                    Assert-ReleaseRecord $record $Context $root
                    $seen[$record.stem]=$true
                    if($record.stem -ne $current.stem){$records+=@($record)}
                } catch {$result.preserved+=@($file.Name+': '+$_.Exception.Message);$seen[$file.BaseName]=$true}
            }
        }
        foreach($folder in Get-ChildItem -LiteralPath $root -Directory|Where-Object {$_.Name -ne $current.stem -and !$seen.ContainsKey($_.Name)}) {
            try {$records+=@(Get-LegacyReleaseRecord $Context $root $folder.FullName)}
            catch {$result.preserved+=@($folder.Name+': '+$_.Exception.Message)}
        }
    }
    if ($Apply) {
        New-Item -ItemType Directory -Path $recordRoot -Force|Out-Null
        Add-ProjectArtifact $Context (Join-Path $recordRoot ($current.stem+'.json')) -Role deliverable
        Write-ReleaseJson (Join-Path $recordRoot ($current.stem+'.json')) $current $Context.Task.work
    }
    foreach($record in $records) {
        try {
            if (!@($record.artifacts|Where-Object {Test-Path -LiteralPath $_.path}).Count) {continue}
            if ([DateTimeOffset]$record.completedUtc -gt [DateTimeOffset]$current.completedUtc) {throw 'A newer release record exists; preserving it.'}
            Assert-ReleaseRecord $record $Context $root
            foreach($artifact in $record.artifacts) {foreach($path in $protected){if(Test-ReleasePathOverlap $artifact.path $path){throw 'Release is installed, running or claimed by another active task.'}}}
            if ($Apply) {
                # Recheck immediately before moving. Windows file locks remain
                # authoritative; failures defer the affected artifacts safely.
                $live=@(Get-ReleaseProtection $Context)
                foreach($artifact in $record.artifacts){foreach($path in $live){if(Test-ReleasePathOverlap $artifact.path $path){throw 'Release became active; preserving it.'}}}
                # Persist legacy ownership before the first move, so an
                # interruption can safely finish a partially retired set.
                $recordPath=Join-Path $recordRoot ($record.stem+'.json')
                Add-ProjectArtifact $Context $recordPath -Role deliverable
                if (!(Test-Path -LiteralPath $recordPath)) {Write-ReleaseJson $recordPath $record $Context.Task.work}
                $quarantine=Join-Path $Context.Task.work ('obsolete-releases\'+[guid]::NewGuid().ToString('N'))
                $null=Assert-CleanupTree $quarantine $Context.Task.work
                New-Item -ItemType Directory -Path $quarantine -Force|Out-Null
                foreach($artifact in $record.artifacts) {
                    if(!(Test-Path -LiteralPath $artifact.path)){continue}
                    $source=Assert-CleanupTree $artifact.path $root -Recurse -ForDeletion
                    $destination=Join-Path $quarantine (Split-Path $source -Leaf)
                    $null=Assert-CleanupTree $destination $Context.Task.work
                    Add-ProjectArtifact $Context $destination -Role temporary
                    Move-Item -LiteralPath $source -Destination $destination -ErrorAction Stop
                    $result.queued+=@($source)
                }
            } else {$result.queued+=@($record.artifacts|Where-Object {Test-Path -LiteralPath $_.path}|ForEach-Object {$_.path})}
        } catch {$result.preserved+=@($record.stem+': '+$_.Exception.Message)}
    }
    if ($Apply) {
        $report=Join-Path (Split-Path $recordRoot -Parent) 'release-retention.json'
        Add-ProjectArtifact $Context $report -Role deliverable
        Write-ReleaseJson $report $result $Context.Task.work
    }
    return $result
}
Export-ModuleMember -Function Complete-ProjectRelease
