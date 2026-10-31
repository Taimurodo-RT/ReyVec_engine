@echo off
rem Run the built ReyVec sandbox. All options are passed to the sandbox.
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\run.ps1" %*
echo.
pause
