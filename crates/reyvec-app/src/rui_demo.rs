//! Экран интерфейса игры из файла `ui/demo.rui` (шаги И.1–И.2а, решение 0022).
//!
//! F3 показывает и прячет его поверх любой сцены, F4 меняет тему, Shift+F3 пишет в лог и
//! консоль, откуда взялось каждое свойство. Экран читается вместе
//! с темами и стилями из его `head`; любой из этих файлов перечитывается, когда меняется
//! (проверка раз в четверть секунды), так что правка сразу видна в окне. Если в файле
//! ошибка, вместо экрана видна плашка с текстом ошибки и местом в файле, а прежний
//! разобранный экран ждёт: после исправления вернётся новый.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use reyvec_text::GlyphAtlas;
use reyvec_ui::{UiDraw, UiError, UiScreen};

/// Пример, если папки репозитория нет рядом (собранная программа на другом ПК).
const BUILT_IN: &str = include_str!("../../../ui/demo.rui");
const BUILT_IN_FILES: &[(&str, &str)] = &[
    ("themes/base", include_str!("../../../ui/themes/base.rui")),
    ("styles/demo", include_str!("../../../ui/styles/demo.rui")),
];

/// Как часто смотреть, не поменялся ли файл.
const POLL: Duration = Duration::from_millis(250);

pub(crate) struct RuiDemo {
    dir: PathBuf,
    shown: bool,
    screen: Option<UiScreen>,
    /// Плашка с ошибкой разбора файла (свой маленький экран).
    error: Option<UiScreen>,
    /// Меняется при каждом новом разборе: по нему (вместе с номером сборки экрана)
    /// видеокарта перезагружает атлас букв.
    key: u64,
    /// Файлы экрана и когда они менялись в последний раз.
    watched: Vec<(PathBuf, Option<SystemTime>)>,
    checked: Instant,
    /// Тема, выбранная F4: сохраняется, когда файлы перечитываются.
    theme: Option<String>,
}

impl RuiDemo {
    pub(crate) fn new() -> Self {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
        let mut demo = Self {
            dir,
            shown: false,
            screen: None,
            error: None,
            key: 0,
            watched: Vec::new(),
            checked: Instant::now(),
            theme: None,
        };
        demo.load();
        demo
    }

    /// F3: показать или спрятать.
    pub(crate) fn toggle(&mut self) {
        self.shown = !self.shown;
        if self.shown {
            // Файлы могли поменять, пока экран был спрятан.
            self.poll(true);
        }
    }

    /// F4: следующая тема из подключённых. Ничего не делает, пока экран спрятан.
    pub(crate) fn next_theme(&mut self) {
        let Some(screen) = self.screen.as_mut().filter(|_| self.shown) else {
            return;
        };
        let themes: Vec<String> = screen.themes().iter().map(|t| (*t).to_owned()).collect();
        if themes.is_empty() {
            return;
        }
        let now = screen
            .theme()
            .and_then(|t| themes.iter().position(|n| n == t));
        let next = themes[now.map_or(0, |i| (i + 1) % themes.len())].clone();
        match screen.set_theme(&next) {
            Ok(_) => {
                tracing::info!("интерфейс: тема {next}");
                self.theme = Some(next);
            }
            Err(e) => self.show_error(&e),
        }
    }

    /// Shift+F3: откуда взялось каждое свойство экрана (стиль с файлом и строкой или сам
    /// блок) — в лог и в консоль разработчика.
    pub(crate) fn log_origins(&self) {
        match &self.screen {
            Some(s) => {
                for line in s.explain().lines() {
                    tracing::info!("интерфейс: {line}");
                }
            }
            None => tracing::info!("интерфейс: экран ещё не прочитан"),
        }
    }

    /// Что рисовать в этом кадре: список, атлас и ключ атласа. `None` — экран спрятан.
    /// `viewport` — размер картинки, поверх которой рисуется интерфейс (пиксели).
    pub(crate) fn frame(&mut self, viewport: [f32; 2]) -> Option<(&UiDraw, &GlyphAtlas, u64)> {
        if !self.shown {
            return None;
        }
        self.poll(false);
        // Ключ плашки ошибки отличается от ключа экрана: у неё свой атлас.
        let (screen, key) = match (self.error.as_mut(), self.screen.as_mut()) {
            (Some(e), _) => (e, self.key | 1 << 63),
            (None, Some(s)) => {
                let key = (self.key << 20).wrapping_add(s.generation()) & !(1 << 63);
                (s, key)
            }
            (None, None) => return None,
        };
        let (draw, atlas) = screen.draw_with_atlas(viewport);
        Some((draw, atlas, key))
    }

    fn poll(&mut self, now: bool) {
        if !now && self.checked.elapsed() < POLL {
            return;
        }
        self.checked = Instant::now();
        let changed = self
            .watched
            .iter()
            .any(|(path, at)| modified(path).is_some_and(|m| Some(m) != *at));
        if changed {
            self.load();
        }
    }

    fn load(&mut self) {
        self.key = self.key.wrapping_add(1);
        let main = self.dir.join("demo.rui");
        let loaded = if main.exists() {
            UiScreen::load(&self.dir, "demo.rui")
        } else {
            // Папки нет: пример, вшитый в программу.
            UiScreen::parse_with(BUILT_IN, "demo.rui", &mut |path| {
                BUILT_IN_FILES
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, t)| (*t).to_owned())
                    .ok_or_else(|| "во встроенном примере такого файла нет".to_owned())
            })
        };
        match loaded {
            Ok(mut screen) => {
                if let Some(t) = &self.theme
                    && let Err(e) = screen.set_theme(t)
                {
                    self.watch(&screen);
                    return self.show_error(&e);
                }
                tracing::info!("интерфейс: прочитан ui/demo.rui");
                self.watch(&screen);
                self.screen = Some(screen);
                self.error = None;
            }
            Err(e) => {
                // Следим за прежними файлами и за самим экраном: исправят — перечитаем.
                if self.watched.is_empty() {
                    self.watched.push((main.clone(), modified(&main)));
                }
                for (path, at) in &mut self.watched {
                    *at = modified(path);
                }
                self.show_error(&e);
            }
        }
    }

    fn watch(&mut self, screen: &UiScreen) {
        self.watched = screen
            .files()
            .iter()
            .map(|p| (p.clone(), modified(p)))
            .collect();
    }

    fn show_error(&mut self, e: &UiError) {
        let message = match e {
            UiError::File { .. } => e.to_string(),
            _ => format!("demo.rui: {e}"),
        };
        tracing::warn!("интерфейс: ошибка в {message}");
        self.error = UiScreen::parse(&error_screen(&message)).ok();
    }
}

fn modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Плашка с текстом ошибки вверху окна.
fn error_screen(message: &str) -> String {
    let escape = |s: &str| {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    };
    format!(
        "screen Error {{\n\
         Column(spacing: 8) {{\n\
         Text(\"Ошибка в файлах интерфейса (папка ui)\").font(22).color(#FFB4B4)\n\
         Text(\"{}\").font(20)\n\
         }}\n\
         .padding(16, 24).background(#3A1418F0).radius(10).border(#E5484D, 2)\n\
         .anchor(top, margin: 24)\n\
         }}\n",
        escape(message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_demo_parses() {
        let mut s = UiScreen::parse_with(BUILT_IN, "demo.rui", &mut |path| {
            BUILT_IN_FILES
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| (*t).to_owned())
                .ok_or_else(|| path.to_owned())
        })
        .expect("пример разбирается");
        assert!(!s.draw([1920.0, 1080.0]).quads.is_empty());
        assert!(s.themes().len() >= 2);
    }

    #[test]
    fn error_screen_parses_any_message() {
        let src = error_screen("строка 3: \"плохо\" \\ и\nдальше");
        let mut s = UiScreen::parse(&src).expect("плашка ошибки разбирается");
        assert!(!s.draw([1280.0, 720.0]).quads.is_empty());
    }
}
