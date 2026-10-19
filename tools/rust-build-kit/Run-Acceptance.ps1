[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Configuration,
    [string]$Package,
    [ValidateSet('SSH','X11','Desktop','Provider','UI')][string[]]$Checks=@('SSH','X11','Desktop','UI')
)
. (Join-Path $PSScriptRoot 'Build-Common.ps1')
if (!$Package) { $Package=(Get-Content -LiteralPath (Join-Path $outputRoot 'LATEST-FOLDER.txt') -Raw).Trim() }
$Package=(Resolve-Path -LiteralPath $Package).Path
Verify-RustPackage $Package
$configurationValue=Get-Content -LiteralPath $Configuration -Raw | ConvertFrom-Json
$evidence=Join-Path $outputRoot ('acceptance\r12-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $evidence -Force | Out-Null
$configurationValue.reportDirectory=$evidence
$configPath=Join-Path $evidence 'config.json'
[IO.File]::WriteAllText($configPath,($configurationValue|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
$nativeExecutable=$null
if (@($Checks|Where-Object{$_ -ne 'UI'}).Count) {
    Initialize-Toolchain
    $snapshot=Write-Snapshot (Get-SourceFingerprint)
    Initialize-WorkspaceLocks $snapshot *> (Join-Path $evidence 'locks.log')
    $cargoJson=Join-Path $evidence 'compiler.jsonl'
    $arguments=@('test','--locked','--manifest-path',(Join-Path $snapshot 'ui\rust\Cargo.toml'),'--target','x86_64-pc-windows-msvc','-p','ad-desktop','--no-run','--message-format=json')
    & cargo.exe @arguments | Set-Content -LiteralPath $cargoJson -Encoding UTF8
    if ($LASTEXITCODE -ne 0) { throw 'Acceptance harness compilation failed.' }
    $artifacts=@(Get-Content -LiteralPath $cargoJson|ForEach-Object{$_|ConvertFrom-Json}|Where-Object{$_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable})
    if ($artifacts.Count -ne 1) { throw 'Expected exactly one native acceptance executable.' }
    $nativeExecutable=Join-Path $Package ('r12-check-' + [guid]::NewGuid().ToString('N') + '.exe')
    Copy-Item -LiteralPath $artifacts[0].executable -Destination $nativeExecutable
}
function Invoke-IsolatedProcess([string]$Executable,[string]$Arguments,[string]$Name,[string]$Profile) {
    $start=[Diagnostics.ProcessStartInfo]::new($Executable,$Arguments)
    $start.UseShellExecute=$false; $start.CreateNoWindow=$true
    $start.EnvironmentVariables['PATH']="$env:SystemRoot\System32;$env:SystemRoot"
    foreach($variableName in @('GST_PLUGIN_PATH','GST_PLUGIN_SYSTEM_PATH','GST_PLUGIN_PATH_1_0','GST_PLUGIN_SYSTEM_PATH_1_0','GST_PLUGIN_SCANNER','GSTREAMER_ROOT')){$start.EnvironmentVariables.Remove($variableName)}
    $start.EnvironmentVariables['GST_REGISTRY_1_0']=Join-Path $evidence ($Name+'-gst-registry.bin')
    $start.EnvironmentVariables['AETHERDESK_R12_CONFIG']=$configPath
    if ($Profile) {
        New-Item -ItemType Directory -Force -Path (Join-Path $Profile 'profile'),(Join-Path $Profile 'local') | Out-Null
        $start.EnvironmentVariables['APPDATA']=Join-Path $Profile 'profile'
        $start.EnvironmentVariables['LOCALAPPDATA']=Join-Path $Profile 'local'
        $start.EnvironmentVariables['AETHERDESK_R12_UI_ROOT']=$Profile
    }
    $process=[Diagnostics.Process]::Start($start)
    try {
        if (!$process.WaitForExit(180000)) { $process.Kill(); $process.WaitForExit(); throw "$Name timed out." }
        if ($process.ExitCode -ne 0) { throw "$Name failed with exit code $($process.ExitCode). See $evidence" }
    } finally { $process.Dispose() }
}
try {
    foreach($check in $Checks) {
        if ($check -eq 'UI') {
            $profile=Join-Path $evidence 'ui'
            Invoke-IsolatedProcess (Join-Path $Package 'AetherDesk.exe') '--r12-ui-check' 'UI' $profile
            $ui=Get-Content -LiteralPath (Join-Path $profile 'ui-smoke.json') -Raw|ConvertFrom-Json
            if (!$ui.passed) {throw "UI smoke check failed: $($ui.error)"}
        } else {
            $name=switch($check){'SSH'{'live_ssh_sftp_editor_tools_and_cleanup'}'X11'{'live_x11_forwarding_and_cleanup'}'Desktop'{'live_desktop_resize_detach_reconnect_and_end'}'Provider'{'live_saved_provider_stream_and_file_tool'}}
            Invoke-IsolatedProcess $nativeExecutable "r12_acceptance::$name --ignored --exact --nocapture --test-threads=1" $check ''
        }
        Write-Host "$check acceptance passed." -ForegroundColor Green
    }
    [ordered]@{passedChecks=$Checks;sourceFingerprint=(Get-SourceFingerprint);packageFingerprint=(Get-Content -LiteralPath (Join-Path $Package 'build-info.json') -Raw|ConvertFrom-Json).fingerprint;cleanMachineVerified=$false;replacementReady=$false}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $evidence 'summary.json') -Encoding UTF8
} finally { if ($nativeExecutable -and (Test-Path -LiteralPath $nativeExecutable)) {Remove-Item -LiteralPath $nativeExecutable -Force} }
Write-Host "R12 evidence: $evidence"
