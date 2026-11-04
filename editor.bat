@echo off
rem Run the built ReyVec editor. All options are passed to the editor.
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\editor.ps1" %*
echo.
pause
