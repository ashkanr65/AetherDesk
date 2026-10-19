@echo off
setlocal
set "AETHERDESK_RUST_KEEP_BUILD_CACHE=1"
call "%~dp0Build-Everything.cmd"
exit /b %errorlevel%
