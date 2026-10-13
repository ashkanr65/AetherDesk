#!/usr/bin/env pwsh

param(
    [string]$Configuration = "Release",
    [switch]$Clean,
    [switch]$Test,
    [switch]$Run
)

$ErrorActionPreference = "Stop"

Write-Host "=== AetherDesk Build Script ===" -ForegroundColor Cyan
Write-Host "Configuration: $Configuration" -ForegroundColor Green

# Navigate to Rust directory
Push-Location "$PSScriptRoot\core\rust"

try {
    # Clean if requested
    if ($Clean) {
        Write-Host "Cleaning build artifacts..." -ForegroundColor Yellow
        cargo clean
    }

    # Build
    Write-Host "Building Rust core..." -ForegroundColor Yellow
    if ($Configuration -eq "Release") {
        cargo build --release
    } else {
        cargo build
    }

    if ($LASTEXITCODE -ne 0) {
        throw "Rust build failed"
    }

    # Test if requested
    if ($Test) {
        Write-Host "Running tests..." -ForegroundColor Yellow
        cargo test
        if ($LASTEXITCODE -ne 0) {
            throw "Tests failed"
        }
    }

    # Create distribution folder
    Write-Host "Creating distribution package..." -ForegroundColor Yellow
    $distPath = "..\..\dist"
    if (Test-Path $distPath) {
        Remove-Item -Recurse -Force $distPath
    }
    New-Item -ItemType Directory -Path $distPath | Out-Null

    # Copy binaries
    $binPath = if ($Configuration -eq "Release") { "target\release" } else { "target\debug" }
    Copy-Item "$binPath\aetherdesk.exe" "$distPath\"
    Copy-Item "$binPath\ad_app.dll" "$distPath\"

    Write-Host "Build complete!" -ForegroundColor Green
    Write-Host "Distribution files in: $distPath" -ForegroundColor Green
    Write-Host ""
    
    if ($Run) {
        Write-Host "Running AetherDesk..." -ForegroundColor Cyan
        & "..\\..\\$distPath\\aetherdesk.exe"
    } else {
        Write-Host "To run: ..\\..\\$distPath\\aetherdesk.exe" -ForegroundColor Cyan
    }

} finally {
    Pop-Location
}
