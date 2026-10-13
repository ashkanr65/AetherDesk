[CmdletBinding()]
param(
    [string]$Repository,
    [string]$BuildRoot = $env:AETHERDESK_BUILD_ROOT,
    [string]$OutputRoot = $env:AETHERDESK_OUTPUT_ROOT,
    [string]$KitHome = $env:AETHERDESK_KIT_HOME
)
$ErrorActionPreference = 'Stop'
if (!$Repository) { $Repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..')) }
$source = [IO.Path]::GetFullPath($Repository).TrimEnd('\')
$workspace = Split-Path $source -Parent
if (!$workspace) { throw 'The source repository cannot be a filesystem root.' }
# Keep generated files in one root, separate from source and launchers.
$output = [IO.Path]::GetFullPath($(if ($OutputRoot) { $OutputRoot } else { Join-Path $workspace 'AetherDesk-Rust-output' })).TrimEnd('\')
$build = [IO.Path]::GetFullPath($(if ($BuildRoot) { $BuildRoot } else { Join-Path $output 'AetherDesk-localbuild' })).TrimEnd('\')
$kitLocation = [IO.Path]::GetFullPath($(if ($KitHome) { $KitHome } else { $output })).TrimEnd('\')
foreach ($location in @($build, $output, $kitLocation)) {
    if ($location.Equals([IO.Path]::GetPathRoot($location).TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Build, package and log output cannot be filesystem roots.'
    }
    if ($location.Equals($source, [StringComparison]::OrdinalIgnoreCase) -or
        $location.StartsWith($source + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Build, package and log output must be outside the source repository.'
    }
}
if ($source.StartsWith($build + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'The build checkout cannot contain the source repository.'
}
if ($build.Equals($output, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'The build checkout and package output folder must be separate.'
}
[pscustomobject]@{ source = $source; build = $build; output = $output; kit = $kitLocation }
