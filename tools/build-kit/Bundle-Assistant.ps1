[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Destination,
    [string]$CacheDirectory = (Join-Path $env:LOCALAPPDATA 'AetherDesk\build-cache\assistant')
)
$ErrorActionPreference = 'Stop'
$python = & (Join-Path $PSScriptRoot 'Resolve-Python.ps1')
& $python -X utf8 (Join-Path $PSScriptRoot 'assistant_bundle.py') --destination $Destination --cache-directory $CacheDirectory
if ($LASTEXITCODE -ne 0) { throw "Assistant bundling failed with exit code $LASTEXITCODE" }
