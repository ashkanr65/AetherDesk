# Disable Smart App Control (requires admin)
Set-SmartAppControlState -State Disabled

# Add comprehensive exclusions
Add-MpPreference -ExclusionPath "C:\Users\rezae\AetherDesk" -Force
Add-MpPreference -ExclusionPath "C:\Users\rezae\.cargo" -Force
Add-MpPreference -ExclusionPath "C:\Users\rezae\.rustup" -Force
Add-MpPreference -ExclusionPath "C:\Users\rezae\AetherDesk\core\rust\target" -Force

# Unblock everything recursively
Get-ChildItem -Path "C:\Users\rezae\AetherDesk" -Recurse -Force | Unblock-File
Get-ChildItem -Path "C:\Users\rezae\.cargo" -Recurse -Force | Unblock-File
Get-ChildItem -Path "C:\Users\rezae\.rustup" -Recurse -Force | Unblock-File

# Clean target directory completely
Remove-Item -Path "target" -Recurse -Force -ErrorAction SilentlyContinue

# Set environment variable to use JIT compilation instead of build scripts
$env:CARGO_INCREMENTAL = "0"
$env:RUSTFLAGS = "-C linker=link.exe"

Write-Host "Starting clean build..." -ForegroundColor Green
cargo build --release --verbose
