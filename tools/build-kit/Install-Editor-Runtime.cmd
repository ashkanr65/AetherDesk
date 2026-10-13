@echo off
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0Install-Editor-Runtime.ps1"
if errorlevel 1 (
  echo Editor runtime setup did not complete. See the message above.
  pause
  exit /b 1
)
pause
