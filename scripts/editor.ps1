# Запускает собранный редактор ReyVec из папки dist.
# Все параметры передаются редактору как есть, например: editor.bat --window-frame

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$Root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $Root "dist\reyvec-editor.exe"

if (-not (Test-Path $exe)) {
    Write-Host "Редактор ещё не собран. Сначала запусти update.bat." -ForegroundColor Yellow
    exit 1
}

$versionFile = Join-Path $Root "dist\version.txt"
if (Test-Path $versionFile) {
    Get-Content $versionFile | ForEach-Object { Write-Host "  $_" -ForegroundColor DarkGray }
}

& $exe @args
$code = $LASTEXITCODE
if ($code -ne 0) {
    Write-Host "Редактор завершился с ошибкой (код $code). Пришли Claude файл dist\reyvec-editor.log." -ForegroundColor Yellow
}
exit $code
