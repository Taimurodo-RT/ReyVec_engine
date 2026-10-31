# Запускает собранную песочницу ReyVec из папки dist.
# Все параметры передаются песочнице как есть, например: run.bat --version

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$Root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $Root "dist\reyvec-sandbox.exe"

if (-not (Test-Path $exe)) {
    Write-Host "Песочница ещё не собрана. Сначала запусти update.bat." -ForegroundColor Yellow
    exit 1
}

$versionFile = Join-Path $Root "dist\version.txt"
if (Test-Path $versionFile) {
    Get-Content $versionFile | ForEach-Object { Write-Host "  $_" -ForegroundColor DarkGray }
}

& $exe @args
$code = $LASTEXITCODE
if ($code -ne 0) {
    Write-Host "Песочница завершилась с ошибкой (код $code). Пришли Claude файл dist\reyvec.log." -ForegroundColor Yellow
}
exit $code
