//! Экран интерфейса игры из файла `ui/demo.rui` (шаг И.1, решение 0022).
//!
//! F3 показывает и прячет его поверх любой сцены. Файл читается при запуске и
//! перечитывается, когда меняется (проверка раз в четверть секунды), так что правка
//! сразу видна в окне. Если в файле ошибка, вместо экрана видна плашка с текстом ошибки
//! и местом в файле, а прежний разобранный экран ждёт: после исправления вернётся новый.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use reyvec_text::GlyphAtlas;
use reyvec_ui::{UiDraw, UiScreen};

/// Файл примера, если папки репозитория нет рядом (собранная программа на другом ПК).
const BUILT_IN: &str = include_str!("../../../ui/demo.rui");

/// Как часто смотреть, не поменялся ли файл.
const POLL: Duration = Duration::from_millis(250);

pub(crate) struct RuiDemo {
    path: PathBuf,
    shown: bool,
    screen: Option<UiScreen>,
    /// Плашка с ошибкой разбора файла (свой маленький экран).
    error: Option<UiScreen>,
    /// Меняется при каждом новом разборе: по нему видеокарта перезагружает атлас букв.
    key: u64,
    modified: Option<SystemTime>,
    checked: Instant,
}

impl RuiDemo {
    pub(crate) fn new() -> Self {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/demo.rui");
        let mut demo = Self {
            path,
            shown: false,
            screen: None,
            error: None,
            key: 0,
            modified: None,
            checked: Instant::now(),
        };
        demo.load();
        demo
    }

    /// F3: показать или спрятать.
    pub(crate) fn toggle(&mut self) {
        self.shown = !self.shown;
        if self.shown {
            // Файл могли поменять, пока экран был спрятан.
            self.poll(true);
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
            (None, Some(s)) => (s, self.key),
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
        let modified = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        if modified.is_some() && modified != self.modified {
            self.load();
        }
    }

    fn load(&mut self) {
        let (src, label) = match std::fs::read_to_string(&self.path) {
            Ok(src) => {
                self.modified = std::fs::metadata(&self.path)
                    .and_then(|m| m.modified())
                    .ok();
                (src, "ui/demo.rui")
            }
            // Файла нет: пример, вшитый в программу.
            Err(_) => (BUILT_IN.to_owned(), "ui/demo.rui (встроенный)"),
        };
        self.key = self.key.wrapping_add(2) & !(1 << 63);
        match UiScreen::parse(&src) {
            Ok(screen) => {
                tracing::info!("интерфейс: прочитан {label}");
                self.screen = Some(screen);
                self.error = None;
            }
            Err(e) => {
                tracing::warn!("интерфейс: ошибка в {label}: {e}");
                self.error = UiScreen::parse(&error_screen(label, &e.to_string())).ok();
            }
        }
    }
}

/// Плашка с текстом ошибки вверху окна.
fn error_screen(label: &str, message: &str) -> String {
    let escape = |s: &str| {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    };
    format!(
        "screen Error {{\n\
         Column(spacing: 8) {{\n\
         Text(\"Ошибка в {}\").font(22).color(#FFB4B4)\n\
         Text(\"{}\").font(20)\n\
         }}\n\
         .padding(16, 24).background(#3A1418F0).radius(10).border(#E5484D, 2)\n\
         .anchor(top, margin: 24)\n\
         }}\n",
        escape(label),
        escape(message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_demo_parses() {
        let mut s = UiScreen::parse(BUILT_IN).expect("пример разбирается");
        assert!(!s.draw([1920.0, 1080.0]).quads.is_empty());
    }

    #[test]
    fn error_screen_parses_any_message() {
        let src = error_screen("ui/demo.rui", "строка 3: \"плохо\" \\ и\nдальше");
        let mut s = UiScreen::parse(&src).expect("плашка ошибки разбирается");
        assert!(!s.draw([1280.0, 720.0]).quads.is_empty());
    }
}
