param([string]$Repository = ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))))
$data = Get-Content -LiteralPath (Join-Path $Repository 'release.json') -Raw | ConvertFrom-Json
if ($data.version -isnot [string] -or $data.version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*))?$' -or
    ($data.build -isnot [int] -and $data.build -isnot [long]) -or $data.build -lt 1 -or $data.build -gt 65535) { throw 'Invalid release.json. Use version 0.2 or 0.2.0 and a positive build number.' }
$parts = @($data.version.Split('.') | ForEach-Object { [int]$_ })
if (@($parts | Where-Object { $_ -gt 65535 }).Count) { throw 'Release version components must be at most 65535.' }
while ($parts.Count -lt 3) { $parts += 0 }
[pscustomobject]@{ version=$data.version; normalized=($parts -join '.'); build=$data.build }
