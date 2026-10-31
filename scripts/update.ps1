# Обновляет локальную версию ReyVec: скачивает свежий код с GitHub,
# собирает песочницу и кладёт готовую программу в папку dist.
#
# Обычно запускается двойным щелчком по update.bat в корне репозитория.
# Параметры:
#   -Branch <имя>  взять другую ветку, например ветку PR для проверки
#                  (update.bat -Branch stage-1/window). По умолчанию main.
#   -NoPull        ничего не скачивать, только собрать то, что уже есть.

param(
    [string]$Branch = "main",
    [switch]$NoPull
)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Write-Step([string]$Text) { Write-Host "`n==> $Text" -ForegroundColor Cyan }
function Write-Ok([string]$Text) { Write-Host "    $Text" -ForegroundColor Green }
function Write-Warn([string]$Text) { Write-Host "    $Text" -ForegroundColor Yellow }
function Stop-WithError([string]$Text) {
    Write-Host "`nОШИБКА: $Text" -ForegroundColor Red
    exit 1
}

# Запускает внешнюю программу и останавливает скрипт, если она завершилась с ошибкой.
# git и cargo пишут обычный ход работы в stderr, поэтому на время их запуска
# отключаем остановку по ошибкам PowerShell и смотрим только на код выхода.
function Invoke-Checked([string]$Exe, [string[]]$Arguments, [string]$What) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $Exe @Arguments
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $saved
    }
    if ($code -ne 0) { Stop-WithError "$What (код $code)." }
}

Write-Host "ReyVec: обновление локальной версии" -ForegroundColor White
Write-Host "Папка: $Root"

Write-Step "Проверяю установленные программы"
foreach ($tool in @("git", "cargo")) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Stop-WithError "не найдена программа '$tool'. Установи её по инструкции docs\setup-windows.md и открой новое окно."
    }
}
Write-Ok "git и cargo найдены"
if ($env:VULKAN_SDK) {
    Write-Ok "Vulkan SDK: $env:VULKAN_SDK"
} else {
    Write-Warn "Vulkan SDK не найден. Сейчас он не обязателен, но понадобится с этапа 1 (docs\setup-windows.md)."
}

if (-not $NoPull) {
    Write-Step "Скачиваю свежий код (ветка $Branch)"
    $changes = git status --porcelain
    if ($changes) {
        Write-Host $changes
        Stop-WithError "в папке есть изменённые файлы, и обновление могло бы их затереть. Сохрани их в другом месте или верни как было (git checkout -- .), потом запусти снова."
    }
    Invoke-Checked "git" @("fetch", "origin", $Branch) "не удалось скачать ветку $Branch"
    Invoke-Checked "git" @("checkout", "-B", $Branch, "origin/$Branch") "не удалось переключиться на ветку $Branch"
    Write-Ok "Код обновлён"
} else {
    Write-Step "Пропускаю скачивание (-NoPull)"
}

$commit = (git rev-parse --short=9 HEAD).Trim()
$branchNow = (git rev-parse --abbrev-ref HEAD).Trim()
Write-Ok "Ветка $branchNow, коммит $commit"

Write-Step "Собираю песочницу (первый раз это может занять несколько минут)"
Invoke-Checked "cargo" @("build", "--release", "-p", "reyvec-sandbox") "сборка не удалась. Пришли этот текст разработчику"

Write-Step "Копирую программу в папку dist"
$dist = Join-Path $Root "dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item -Force (Join-Path $Root "target\release\reyvec-sandbox.exe") (Join-Path $dist "reyvec-sandbox.exe")
$stamp = Get-Date -Format "yyyy-MM-dd HH:mm"
@(
    "ReyVec sandbox",
    "branch: $branchNow",
    "commit: $commit",
    "built:  $stamp"
) | Set-Content -Encoding UTF8 (Join-Path $dist "version.txt")
Write-Ok "Готово: dist\reyvec-sandbox.exe"

Write-Host "`nОбновление завершено. Запусти run.bat, чтобы открыть песочницу." -ForegroundColor Green
exit 0
