@echo off
setlocal
cd /d "%~dp0"
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0Run-Build-Step.ps1" -All
set "build_exit=%errorlevel%"
if errorlevel 1 echo BUILD STOPPED. Keep this window's error message. Double-click this file to retry.
echo.
pause
exit /b %build_exit%
