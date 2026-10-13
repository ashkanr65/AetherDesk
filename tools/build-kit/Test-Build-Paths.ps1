$ErrorActionPreference = 'Stop'
$resolver = Join-Path $PSScriptRoot 'Resolve-Build-Paths.ps1'
function Assert-Equal($Actual, $Expected) {
    if ($Actual -ne $Expected) { throw "Expected '$Expected', got '$Actual'." }
}
function Assert-Rejected([scriptblock]$Action) {
    try { & $Action | Out-Null } catch { return }
    throw 'Unsafe build/output overlap was accepted.'
}
$default = & $resolver -BuildRoot '' -OutputRoot '' -KitHome ''
Assert-Equal $default.source ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..')))
$source = 'C:\example\Moved Aether\AetherDesk'
$value = & $resolver -Repository $source -BuildRoot '' -OutputRoot '' -KitHome ''
Assert-Equal $value.source $source
Assert-Equal $value.build 'C:\example\Moved Aether\AetherDesk-Rust-output\AetherDesk-localbuild'
Assert-Equal $value.output 'C:\example\Moved Aether\AetherDesk-Rust-output'
Assert-Equal $value.kit $value.output
$grouped = & $resolver -Repository $source -OutputRoot 'C:\grouped-output' -BuildRoot '' -KitHome ''
Assert-Equal $grouped.output 'C:\grouped-output'
Assert-Equal $grouped.build 'C:\grouped-output\AetherDesk-localbuild'
Assert-Equal $grouped.kit $grouped.output
Assert-Rejected { & $resolver -Repository $source -KitHome ($source + '\logs') }
Assert-Rejected { & $resolver -Repository $source -KitHome 'C:\' }
$custom = & $resolver -Repository $source -BuildRoot 'C:\cache\AetherBuild' -OutputRoot 'C:\packages' -KitHome 'C:\kit'
Assert-Equal $custom.build 'C:\cache\AetherBuild'
Assert-Equal $custom.output 'C:\packages'
Assert-Equal $custom.kit 'C:\kit'
Assert-Rejected { & $resolver -Repository $source -BuildRoot ($source + '\build') -OutputRoot 'C:\packages' }
Assert-Rejected { & $resolver -Repository $source -BuildRoot 'C:\cache' -OutputRoot ($source + '\packages') }
Assert-Rejected { & $resolver -Repository $source -BuildRoot 'C:\same' -OutputRoot 'C:\same' }
Assert-Rejected { & $resolver -Repository $source -BuildRoot 'C:\example\Moved Aether' -OutputRoot 'C:\packages' }
Assert-Rejected { & $resolver -Repository $source -BuildRoot 'C:\' -OutputRoot 'C:\packages' }
Assert-Rejected { & $resolver -Repository $source -BuildRoot 'C:\cache' -OutputRoot 'C:\' }
Assert-Rejected { & $resolver -Repository $source -BuildRoot 'C:\EXAMPLE\Moved Aether\AetherDesk\..\AetherDesk' -OutputRoot 'C:\packages' }

# Exercise the production fingerprint with identical source content and only
# location changes. No build tools or version synchronization are invoked.
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('aetherdesk-path-test-' + [guid]::NewGuid().ToString('N'))
try {
    foreach ($name in @('first', 'moved')) {
        $folder = Join-Path $temporary $name
        New-Item -ItemType Directory -Path (Join-Path $folder 'core\rust') -Force | Out-Null
        [IO.File]::WriteAllText((Join-Path $folder 'core\rust\Cargo.lock'), '# lock fixture')
        [IO.File]::WriteAllText((Join-Path $folder 'README.md'), 'same source fixture')
    }
    $kit = Join-Path $temporary 'recipe'
    New-Item -ItemType Directory -Path $kit | Out-Null
    [IO.File]::WriteAllText((Join-Path $kit 'fixture.ps1'), '# same recipe fixture')
    $tokens = $null
    $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'Build-Common.ps1'), [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw ($errors | Out-String) }
    $function = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Get-BuildFingerprint'
    }, $true)
    if (!$function) { throw 'Production fingerprint function missing.' }
    . ([scriptblock]::Create($function.Extent.Text))
    function Get-SourceFiles { 'README.md' }
    $sourceRepo = Join-Path $temporary 'first'
    $repo = Join-Path $temporary 'build'
    $outputRoot = Join-Path $temporary 'packages'
    $kitOutput = Join-Path $temporary 'kit'
    $original = Get-BuildFingerprint
    foreach ($name in @('repo', 'outputRoot', 'kitOutput')) {
        $before = Get-Variable -Name $name -ValueOnly
        Set-Variable -Name $name -Value ($before + '-moved')
        if ((Get-BuildFingerprint) -eq $original) { throw "Moving $name reused the old completion fingerprint." }
        Set-Variable -Name $name -Value $before
    }
    $sourceRepo = Join-Path $temporary 'moved'
    if ((Get-BuildFingerprint) -eq $original) { throw 'Moving identical source reused the old completion fingerprint.' }
} finally {
    $full = [IO.Path]::GetFullPath($temporary)
    $prefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($full) -notlike 'aetherdesk-path-test-*') { throw 'Unsafe path-test cleanup.' }
    if (Test-Path -LiteralPath $full) { Remove-Item -LiteralPath $full -Recurse -Force }
}
Write-Host 'PASS: consolidated outputs, relocated defaults, overrides, source protection and path-sensitive resume records.'
