# Offline publisher routing fixtures. No credentials, network, signing or build.
$ErrorActionPreference='Stop'
$fixture=Join-Path ([IO.Path]::GetTempPath()) ('aetherdesk-publish-fixture-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture|Out-Null
$commit='a'*40
$remoteCommit=$commit
$branch='main'
$private=$false
$requests=[Collections.Generic.List[object]]::new()
function FixturePython { $global:LASTEXITCODE=0 }
function git {
    $global:LASTEXITCODE=0
    if($args -contains 'credential'){return 'password=offline-fixture-only'}
    if($args -contains 'rev-parse'){return $commit}
    if($args -contains 'status'){return}
    if($args -contains 'branch'){return $branch}
    throw 'Unexpected Git operation in offline fixture'
}
function Invoke-RestMethod {
    param($Uri,$Headers,$Method='Get',$ContentType,$Body,$InFile)
    if($Uri -eq 'https://api.github.com/repos/ashkanr65/AetherDesk'){
        return [pscustomobject]@{private=$private;default_branch='main'}
    }
    if($Uri.EndsWith('/git/ref/heads/main')){return [pscustomobject]@{object=@{sha=$remoteCommit}}}
    if($Method -eq 'Post' -and $Uri.EndsWith('/releases')){
        $requests.Add(($Body|ConvertFrom-Json))
        return [pscustomobject]@{id=123;html_url='https://example.test/draft'}
    }
    if($Method -eq 'Post' -and $Uri.StartsWith('https://uploads.github.com/repos/ashkanr65/AetherDesk/releases/123/assets?')){return}
    throw 'Unexpected API route in offline fixture'
}
try{
    $manifest=@{version='1.2.3';build=4;notes='offline fixture'}|ConvertTo-Json -Compress
    @{manifest=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($manifest))}|ConvertTo-Json|Set-Content (Join-Path $fixture 'update.json')
    foreach($scenario in @('private','unpushed','branch','valid')){
        $private=$scenario -eq 'private'
        $remoteCommit=if($scenario -eq 'unpushed'){'b'*40}else{$commit}
        $branch=if($scenario -eq 'branch'){'fixture-other'}else{'main'}
        $failed=$false
        try{& (Join-Path $PSScriptRoot 'Publish-Update.ps1') -Directory $fixture -Python FixturePython}catch{$failed=$true}
        if($scenario -eq 'valid'){
            if($failed -or $requests.Count -ne 1){throw 'Valid public main did not create one draft'}
            if(!$requests[0].draft -or $requests[0].target_commitish -ne $commit){throw 'Release did not target the exact main source commit as a draft'}
        }elseif(!$failed -or $requests.Count){throw "Unsafe publisher scenario accepted: $scenario"}
    }
    Write-Host 'PASS: public publisher rejects private, unpushed and non-main source; valid source creates a draft at the exact commit (network/Git/signing mocked).'
}finally{
    Remove-Item -LiteralPath $fixture -Recurse -Force
    $global:LASTEXITCODE=0
}
