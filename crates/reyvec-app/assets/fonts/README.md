# Шрифты панели отладки

Встроены в программу (`include_bytes!` в `crates/reyvec-app/src/panel.rs`).

| Файл | Шрифт | Лицензия |
| --- | --- | --- |
| `Inter-Regular.otf` | [Inter](https://github.com/rsms/inter) 4.0, © The Inter Project Authors | SIL OFL 1.1, текст в `OFL-Inter.txt` |
| `JetBrainsMono-Regular.ttf` | [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono) 2.304, © JetBrains s.r.o. | Apache-2.0 (так помечена эта сборка в Debian; новые выпуски выходят под SIL OFL 1.1) |

Шрифты egui по умолчанию не используются: среди них Ubuntu-Light с лицензией Ubuntu Font Licence, которой нет в `deny.toml`.
