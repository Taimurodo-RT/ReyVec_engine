@echo off
rem Update the local ReyVec build. Options: -Branch <name>, -NoPull
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\update.ps1" %*
echo.
pause
