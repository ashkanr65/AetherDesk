# Return a real Python executable before the kit sanitizes the inherited PATH.
$ErrorActionPreference = 'Stop'
if ($env:AETHERDESK_PYTHON) {
    $configured = Get-Command $env:AETHERDESK_PYTHON -ErrorAction Stop | Select-Object -First 1
    if ($configured.CommandType -ne 'Application') { throw 'AETHERDESK_PYTHON must name a Python executable.' }
    return $configured.Source
}
foreach ($name in @('python.exe', 'python3.exe')) {
    $candidate = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue | Where-Object { $_.Source -notlike '*\Microsoft\WindowsApps\*' } | Select-Object -First 1
    if ($candidate) { return $candidate.Source }
}
$launcher = Get-Command py.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
if ($launcher) {
    $candidate = & $launcher.Source -3 -c 'import sys; print(sys.executable)'
    if ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $candidate -PathType Leaf)) { return $candidate }
}
foreach ($key in @('HKCU:\Software\Python\PythonCore', 'HKLM:\Software\Python\PythonCore')) {
    if (!(Test-Path -LiteralPath $key)) { continue }
    foreach ($entry in Get-ChildItem -LiteralPath $key | Sort-Object PSChildName -Descending) {
        $install = Get-Item -LiteralPath ($entry.PSPath + '\InstallPath') -ErrorAction SilentlyContinue
        if (!$install) { continue }
        $candidate = Join-Path $install.GetValue('') 'python.exe'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
    }
}
# Standard per-user installs may not register with the current registry view.
$userInstalls = Join-Path $env:LOCALAPPDATA 'Programs\Python'
if (Test-Path -LiteralPath $userInstalls) {
    foreach ($directory in Get-ChildItem -LiteralPath $userInstalls -Directory | Sort-Object Name -Descending) {
        $candidate = Join-Path $directory.FullName 'python.exe'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
    }
}
throw 'Python 3 is required. Add it to PATH or set AETHERDESK_PYTHON to its python.exe.'
