@echo off
setlocal
cd /d "%~dp0"
echo AetherDesk auto-update publisher
echo Default: verify, sign, push main, publish for all users, verify downloads.
echo Use --check for local checks only, or --draft to leave an unpublished draft.
echo.
set "publisher_python=%AETHERDESK_PUBLISHER_PYTHON%"
if not defined publisher_python if exist "%USERPROFILE%\.aetherdesk\publisher\Scripts\python.exe" set "publisher_python=%USERPROFILE%\.aetherdesk\publisher\Scripts\python.exe"
if not defined publisher_python if exist "%LOCALAPPDATA%\AetherDesk-Publisher\Scripts\python.exe" set "publisher_python=%LOCALAPPDATA%\AetherDesk-Publisher\Scripts\python.exe"
if defined publisher_python goto run_python
where py >nul 2>nul
if not errorlevel 1 goto run_py
where python >nul 2>nul
if not errorlevel 1 (
  set "publisher_python=python"
  goto run_python
)
rem Use an existing bundled Python if no standalone interpreter is configured.
if exist "%USERPROFILE%\.cache\codex-runtimes\codex-primary-runtime\dependencies\python\python.exe" (
  set "publisher_python=%USERPROFILE%\.cache\codex-runtimes\codex-primary-runtime\dependencies\python\python.exe"
  goto run_python
)
echo Python 3.9 or later is required. Install Python, then run this launcher again.
set "publisher_exit=1"
goto finish
:run_py
py -3 -u "%~dp0Prepare-Auto-Update.py" %*
goto capture_exit
:run_python
"%publisher_python%" -u "%~dp0Prepare-Auto-Update.py" %*
:capture_exit
set "publisher_exit=%errorlevel%"
:finish
echo.
if not "%publisher_exit%"=="0" echo PUBLISHING STOPPED. Keep the message above; fix the problem and retry.
pause
exit /b %publisher_exit%
