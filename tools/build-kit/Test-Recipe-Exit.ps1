# Exercise the same process exit wrapper that GitHub Actions uses for PowerShell.
# The recipe replaces compiler/build commands with mocks; no application is built.
[CmdletBinding()]
param([string]$Repository)
$ErrorActionPreference = 'Stop'
if (!$Repository) { $Repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..')) }
$recipe = Join-Path $Repository 'tools\build-kit\Test-Recipe.ps1'
$shell = if ($PSVersionTable.PSEdition -eq 'Core') {
    Join-Path $PSHOME 'pwsh.exe'
} else {
    $command = Get-Command 'pwsh.exe' -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($command) { $command.Source } else { Join-Path $PSHOME 'powershell.exe' }
}

function Invoke-RecipeProcess([string]$Path) {
    $literal = $Path.Replace("'", "''")
    $wrapper = @'
$ErrorActionPreference = 'Stop'
& '__RECIPE_PATH__'
if (Test-Path -LiteralPath variable:LASTEXITCODE) { exit $LASTEXITCODE }
'@.Replace('__RECIPE_PATH__', $literal)
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($wrapper))
    $savedPreference = $ErrorActionPreference
    try {
        # A failed child must return its exit code for the negative assertion below.
        $ErrorActionPreference = 'Continue'
        $output = @(& $shell -NoLogo -NoProfile -NonInteractive -EncodedCommand $encoded 2>&1 | ForEach-Object { $_.ToString() })
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $savedPreference }
    [pscustomobject]@{ code = $code; output = ($output -join "`n") }
}

$successful = Invoke-RecipeProcess $recipe
if ($successful.code -ne 0 -or $successful.output -notmatch 'PASS: recipe syntax, shared Windows commands') {
    throw "Successful recipe returned exit code $($successful.code): $($successful.output)"
}

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('aetherdesk-recipe-exit-' + [guid]::NewGuid().ToString('N'))
try {
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    $failingRecipe = Join-Path $testRoot 'Test-Recipe.ps1'
    Copy-Item -LiteralPath $recipe -Destination $failingRecipe
    # This passes parsing but intentionally violates the recipe's source assertion.
    [IO.File]::WriteAllText((Join-Path $testRoot 'invalid.ps1'), '# invalid source: ' + [char]1)
    $failing = Invoke-RecipeProcess $failingRecipe
    if ($failing.code -eq 0 -or $failing.output -notmatch 'Control character in build script: invalid.ps1') {
        throw "A failing recipe assertion must return a nonzero exit code: $($failing.output)"
    }
} finally {
    $resolved = [IO.Path]::GetFullPath($testRoot)
    $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$resolved.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolved) -notlike 'aetherdesk-recipe-exit-*') { throw 'Invalid recipe exit test cleanup path.' }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
Write-Host 'PASS: GitHub PowerShell recipe exits zero on success and nonzero for a real failed assertion.'
$global:LASTEXITCODE = 0
