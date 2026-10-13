# AetherDesk Windows Build Script
# Run this in PowerShell on Windows 11

Write-Host "=== AetherDesk Windows Build ===" -ForegroundColor Cyan

$rustPath = Join-Path $PSScriptRoot "core\rust"
$distPath = Join-Path $PSScriptRoot "dist"

# Check if Rust is installed
if (!(Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Rust not found. Installing..." -ForegroundColor Yellow
    winget install Rustlang.Rustup --silent
    $env:PATH = [System.Environment]::GetEnvironmentVariable("PATH","User")
}

# Build the project
Write-Host "Building AetherDesk..." -ForegroundColor Green
Set-Location $rustPath
cargo build --release

if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}

# Create dist folder
if (Test-Path $distPath) {
    Remove-Item $distPath -Recurse -Force
}
New-Item -ItemType Directory -Path $distPath | Out-Null

# Copy binaries
Copy-Item "$rustPath\target\release\aetherdesk.exe" $distPath\
Copy-Item "$rustPath\target\release\ad_app.dll" $distPath\

Write-Host "`n=== Build Complete! ===" -ForegroundColor Green
Write-Host "Your standalone Windows software is ready in: $distPath" -ForegroundColor Cyan
Write-Host "`nFiles created:" -ForegroundColor White
Get-ChildItem $distPath | ForEach-Object { Write-Host "  - $($_.Name)" }
Write-Host "`nTo run: cd $distPath && .\aetherdesk.exe" -ForegroundColor Yellow
