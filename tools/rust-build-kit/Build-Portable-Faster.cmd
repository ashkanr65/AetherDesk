@echo off
setlocal
set "AETHERDESK_RUST_KEEP_BUILD_CACHE=1"
call "%~dp0Build-Portable-Only.cmd"
exit /b %errorlevel%
