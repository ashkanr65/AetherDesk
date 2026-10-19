[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$Directory,[string]$Python='python')
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$config=Get-Content -LiteralPath (Join-Path $root 'update-config.json') -Raw | ConvertFrom-Json
if($config.repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$'){throw 'Invalid release repository.'}
& $Python (Join-Path $PSScriptRoot 'update_release.py') verify --directory $Directory
if($LASTEXITCODE -ne 0){throw 'Signed update verification failed.'}
$envelope=Get-Content -LiteralPath (Join-Path $Directory 'update.json') -Raw|ConvertFrom-Json
$manifest=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($envelope.manifest))|ConvertFrom-Json
$tag='v'+$manifest.version+'-build'+$manifest.build
# Use the existing Git Credential Manager session, never a token in a script/file.
$env:GIT_TERMINAL_PROMPT='0'; $env:GCM_INTERACTIVE='never'
$credentials="protocol=https`nhost=github.com`n`n"|git credential fill 2>$null
$line=$credentials|Where-Object{$_.StartsWith('password=')}|Select-Object -First 1
if(!$line){throw 'Sign in through Git Credential Manager before publishing.'}
$headers=@{Authorization=('Bearer '+$line.Substring(9));Accept='application/vnd.github+json';'X-GitHub-Api-Version'='2022-11-28'}
$release=$null
try{
    $api='https://api.github.com/repos/'+$config.repository
    $repository=Invoke-RestMethod -Uri $api -Headers $headers
    if($repository.private -ne $false -or $config.repository -ne 'ashkanr65/AetherDesk'){throw 'Only the approved public binary-download repository can receive releases. Visibility will never be changed by this script.'}
    $source=Invoke-RestMethod -Uri 'https://api.github.com/repos/ashkanr65/AetherDesk-Rust' -Headers $headers
    if(!$source.private){throw 'The source repository must remain private.'}
    $files=@((Join-Path $Directory 'update.json'))
    foreach($asset in @($manifest.portable,$manifest.installer)|Where-Object{$_}){
        if($asset.name -match '[\/]' -or $asset.name -in @('.','..')){throw 'Invalid asset name.'}
        $path=Join-Path $Directory $asset.name
        if((Get-Item -LiteralPath $path).Length -ne $asset.bytes -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $asset.sha256){throw 'Update package hash does not match its metadata.'}
        $files+=$path
    }
    $commit=(& git -C $root rev-parse HEAD|Out-String).Trim()
    $dirty=@(& git -C $root status --porcelain)
    if($LASTEXITCODE -ne 0 -or $dirty.Count){throw 'Commit and push the tested release source before publishing.'}
    $null=Invoke-RestMethod -Uri ($source.url+'/commits/'+$commit) -Headers $headers
    $body=@{tag_name=$tag;target_commitish=$repository.default_branch;name=('AetherDesk '+$manifest.version);body=$manifest.notes;draft=$true;prerelease=$false}|ConvertTo-Json
    $release=Invoke-RestMethod -Method Post -Uri ($api+'/releases') -Headers $headers -ContentType 'application/json' -Body $body
    foreach($file in $files){
        $name=[Uri]::EscapeDataString([IO.Path]::GetFileName($file))
        $null=Invoke-RestMethod -Method Post -Uri ('https://uploads.github.com/repos/'+$config.repository+'/releases/'+$release.id+'/assets?name='+$name) -Headers $headers -ContentType 'application/octet-stream' -InFile $file
    }
    Write-Host ('Draft download release created: '+$release.html_url)
    Write-Host 'Review the draft and publish it only after updater and live acceptance pass.'
}catch{
    if($release){Write-Warning 'A draft may contain partial uploads. Inspect it before retrying.'}
    throw 'Release publication did not complete. Check GitHub access, package hashes, tag uniqueness and source push status.'
}finally{
    $headers.Clear(); $credentials=$null; $line=$null
}
