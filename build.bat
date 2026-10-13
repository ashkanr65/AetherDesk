@echo off
REM AetherDesk Build Script for Windows

echo === AetherDesk Build Script ===
echo.

cd /d "%~dp0core\rust"

echo Building Rust core in release mode...
cargo build --release

if %ERRORLEVEL% NEQ 0 (
    echo Build failed!
    exit /b 1
)

echo.
echo Creating distribution package...
cd ..\..
if exist dist rmdir /s /q dist
mkdir dist

copy core\rust\target\release\aetherdesk.exe dist\
copy core\rust\target\release\ad_app.dll dist\

echo.
echo Build complete!
echo Distribution files in: dist\
echo.
echo To run: dist\aetherdesk.exe
pause
