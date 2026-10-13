$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
$Host.UI.RawUI.WindowTitle = 'AetherDesk local build - follow START-HERE.html'
Write-Host ''
Write-Host 'AetherDesk build terminal is ready.' -ForegroundColor Green
Write-Host 'Keep the guide open next to this window.'
Write-Host 'Recommended: double-click Build-Everything.cmd to build or resume automatically.'
Write-Host 'Or paste one numbered step command here and wait for STEP ... COMPLETE.'
Write-Host 'The commands configure the installed tools and open Ubuntu automatically.'
Write-Host ''
Write-Host 'Build or resume:  .\Run-Build-Step.ps1 -All' -ForegroundColor Cyan
Write-Host 'Rerun all steps:  .\Run-Build-Step.ps1 -All -Rebuild' -ForegroundColor Cyan
