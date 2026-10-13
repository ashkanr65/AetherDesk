@echo off
setlocal
cd /d "%~dp0"
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0Run-Build-Step.ps1" -Rebuild
set "build_exit=%errorlevel%"
if errorlevel 1 echo BUILD STOPPED. Keep the newest log and retry after fixing the error.
echo.
pause
exit /b %build_exit%
