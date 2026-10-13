$ErrorActionPreference = 'Stop'
$candidate = if ($env:AETHERDESK_NODE) {
    Get-Command $env:AETHERDESK_NODE -CommandType Application -ErrorAction Stop | Select-Object -First 1
} else { Get-Command node.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1 }
if (!$candidate) {
    foreach ($location in @((Join-Path $env:ProgramFiles 'nodejs\node.exe'), (Join-Path $env:LOCALAPPDATA 'Programs\nodejs\node.exe'))) {
        if (Test-Path -LiteralPath $location -PathType Leaf) { $candidate = Get-Command $location; break }
    }
}
if (!$candidate) { throw 'Node.js 22 or newer is required to package the editor. Add node.exe to PATH or set AETHERDESK_NODE.' }
$nodeDirectory = Split-Path -Parent $candidate.Source
$npm = Join-Path $nodeDirectory 'npm.cmd'
if (!(Test-Path -LiteralPath $npm -PathType Leaf)) { throw "npm.cmd is missing beside $($candidate.Source). Install the complete Node.js distribution." }
[pscustomobject]@{ node = $candidate.Source; npm = $npm; directory = $nodeDirectory }
