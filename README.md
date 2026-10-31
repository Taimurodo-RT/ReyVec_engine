# ReyVec

ReyVec — игровой движок на Rust и Vulkan, главные инструменты которого векторная графика и реймарчинг SDF.
Платформа: Windows (ПК).

*ReyVec is a Rust + Vulkan game engine built around vector graphics and SDF raymarching. Windows first.*

Лицензия: MIT или Apache-2.0 на выбор (см. `LICENSE-MIT`, `LICENSE-APACHE`).

## Быстрый старт (Windows)

1. Установи программы по инструкции [docs/setup-windows.md](docs/setup-windows.md).
2. `git clone https://github.com/Taimurodo-RT/ReyVec_engine.git $HOME\ReyVec`
3. Дважды щёлкни `update.bat`, потом `run.bat`.

`update.bat` скачивает свежий код, собирает песочницу и кладёт её в папку `dist`. Чтобы проверить ветку PR: `.\update.bat -Branch <ветка>`. Флаги песочницы (выбор видеокарты, валидация): `.\run.bat --help`.

## Структура

| Папка | Что внутри |
| --- | --- |
| `crates/reyvec-app` | Окно, цикл кадра, время, ввод, логи |
| `crates/reyvec-gpu` | Обёртка над Vulkan: выбор видеокарты, вывод в окно. Весь `unsafe` Vulkan только здесь |
| `apps/sandbox` | Песочница для проверки каждого этапа |
| `scripts/` | Скрипты обновления и запуска локальной версии |
| `docs/` | Дорожная карта, решения, инструкции |

Статус этапов: [docs/roadmap.md](docs/roadmap.md).
