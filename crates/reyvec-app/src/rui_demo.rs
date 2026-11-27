//! Экраны интерфейса игры из файлов `ui/` (шаги И.1–И.3а, решение 0022).
//!
//! - `ui/demo.rui` — пример меню. F3 показывает и прячет его поверх любой сцены, F4
//!   меняет тему; кнопки отвечают мыши, Tab, стрелкам и Enter (нажатие — в лог и
//!   консоль, «Язык» переключает русский и английский, «Выход» прячет экран); Shift+F3
//!   пишет в лог и консоль, откуда взялось каждое свойство.
//! - `ui/hud.rui` — полоска здоровья и монеты в сцене с героем (И.3а): экран читает
//!   `game.hero.hp`, `game.hero.hp_max`, `game.hero.coins`, `game.hero.hurt`. Пока он
//!   читается без ошибок, старые сердечки на egui не рисуются.
//!
//! Экран читается вместе с темами и стилями из его `head` и с языками из `ui/lang`;
//! любой из этих файлов перечитывается, когда меняется (проверка раз в четверть
//! секунды), так что правка сразу видна в окне. Если в файле ошибка, вместо экрана видна
//! плашка с текстом ошибки и местом в файле, а прежний разобранный экран ждёт: после
//! исправления вернётся новый.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use reyvec_text::GlyphAtlas;
use reyvec_ui::{Texts, UiDraw, UiError, UiEvent, UiKey, UiScreen};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{Key, NamedKey};

/// Экраны и подключённые файлы, если папки репозитория нет рядом (собранная программа
/// на другом ПК).
const BUILT_IN: &[(&str, &str)] = &[
    ("demo.rui", include_str!("../../../ui/demo.rui")),
    ("hud.rui", include_str!("../../../ui/hud.rui")),
];
const BUILT_IN_FILES: &[(&str, &str)] = &[
    ("themes/base", include_str!("../../../ui/themes/base.rui")),
    ("styles/demo", include_str!("../../../ui/styles/demo.rui")),
    ("styles/hud", include_str!("../../../ui/styles/hud.rui")),
];
const BUILT_IN_LANGS: &[(&str, &str)] = &[
    ("ru", include_str!("../../../ui/lang/ru.ron")),
    ("en", include_str!("../../../ui/lang/en.ron")),
];

/// Главный язык: его слова видны, если в другом языке ключа нет.
const MAIN_LANG: &str = "ru";

/// Как часто смотреть, не поменялся ли файл.
const POLL: Duration = Duration::from_millis(250);

/// Что знает о герое полоска здоровья (`game.hero.*`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeroData {
    pub hp: u32,
    pub hp_max: u32,
    pub coins: u32,
    /// Неуязвим после удара: полоска светлеет.
    pub hurt: bool,
}

/// Один экран из файла со слежкой за правкой.
struct Page {
    /// Файл экрана в папке `ui/`.
    file: &'static str,
    /// Номер экрана для ключа атласа: у каждого экрана свой атлас.
    slot: u64,
    screen: Option<UiScreen>,
    /// Плашка с ошибкой разбора файла (свой маленький экран).
    error: Option<UiScreen>,
    /// Меняется при каждом новом разборе: по нему (вместе с номером сборки экрана)
    /// видеокарта перезагружает атлас букв.
    key: u64,
    /// Файлы экрана и когда они менялись в последний раз.
    watched: Vec<(PathBuf, Option<SystemTime>)>,
    checked: Instant,
    /// Последняя ошибка пересчёта по данным игры, уже показанная в логе.
    problem: Option<String>,
}

impl Page {
    fn new(file: &'static str, slot: u64) -> Self {
        Self {
            file,
            slot,
            screen: None,
            error: None,
            key: 0,
            watched: Vec::new(),
            checked: Instant::now(),
            problem: None,
        }
    }

    fn poll(&mut self, dir: &Path, now: bool, choice: &Choice) {
        if !now && self.checked.elapsed() < POLL {
            return;
        }
        self.checked = Instant::now();
        let changed = self
            .watched
            .iter()
            .any(|(path, at)| modified(path).is_some_and(|m| Some(m) != *at));
        if changed {
            self.load(dir, choice);
        }
    }

    fn load(&mut self, dir: &Path, choice: &Choice) {
        self.key = self.key.wrapping_add(1);
        let main = dir.join(self.file);
        let loaded = if main.exists() {
            UiScreen::load(dir, self.file)
        } else {
            // Папки нет: пример, вшитый в программу.
            let src = BUILT_IN
                .iter()
                .find(|(f, _)| *f == self.file)
                .map_or("", |(_, t)| t);
            UiScreen::parse_with(src, self.file, &mut |path| {
                BUILT_IN_FILES
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, t)| (*t).to_owned())
                    .ok_or_else(|| "во встроенном примере такого файла нет".to_owned())
            })
        };
        match loaded {
            Ok(mut screen) => {
                let texts = texts(dir);
                let files: Vec<PathBuf> = match &texts {
                    Ok(t) => t.files().to_vec(),
                    Err(_) => lang_files(dir),
                };
                let applied = texts.and_then(|texts| {
                    for gap in texts.gaps() {
                        tracing::warn!("интерфейс: {gap}");
                    }
                    screen.set_texts(texts)?;
                    screen.set_lang(&choice.lang)?;
                    match &choice.theme {
                        Some(t) => screen.set_theme(t).map(|_| ()),
                        None => Ok(()),
                    }
                });
                self.watch(&screen, files);
                if let Err(e) = applied {
                    return self.show_error(&e);
                }
                tracing::info!("интерфейс: прочитан ui/{}", self.file);
                self.screen = Some(screen);
                self.error = None;
                self.problem = None;
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

    fn watch(&mut self, screen: &UiScreen, langs: Vec<PathBuf>) {
        self.watched = screen
            .files()
            .iter()
            .cloned()
            .chain(langs)
            .map(|p| {
                let at = modified(&p);
                (p, at)
            })
            .collect();
    }

    fn show_error(&mut self, e: &UiError) {
        let message = match e {
            UiError::File { .. } => e.to_string(),
            _ => format!("{}: {e}", self.file),
        };
        tracing::warn!("интерфейс: ошибка в {message}");
        self.error = UiScreen::parse(&error_screen(&message)).ok();
    }

    /// Экран прочитан без ошибок.
    fn ready(&self) -> bool {
        self.screen.is_some() && self.error.is_none()
    }

    /// Список рисования, атлас и ключ атласа. Ошибку пересчёта по данным игры
    /// показывает плашкой, пока её не исправят.
    fn frame(&mut self, dir: &Path, viewport: [f32; 2], choice: &Choice) -> Draw<'_> {
        self.poll(dir, false, choice);
        if let Some(s) = self.screen.as_mut() {
            s.draw(viewport);
            let now = s.problem().map(|e| format!("{}: {e}", self.file));
            if now != self.problem {
                match &now {
                    Some(m) => {
                        tracing::warn!("интерфейс: ошибка в {m}");
                        self.error = UiScreen::parse(&error_screen(m)).ok();
                    }
                    None => self.error = None,
                }
                self.problem = now;
            }
        }
        // Ключ плашки ошибки отличается от ключа экрана: у неё свой атлас.
        let slot = self.slot << 60;
        let (screen, key) = match (self.error.as_mut(), self.screen.as_mut()) {
            (Some(e), _) => (e, slot | 1 << 59 | self.key),
            (None, Some(s)) => {
                let key = slot | ((self.key << 40) ^ s.generation()) & ((1 << 59) - 1);
                (s, key)
            }
            (None, None) => return None,
        };
        let (draw, atlas) = screen.draw_with_atlas(viewport);
        Some((draw, atlas, key))
    }
}

type Draw<'a> = Option<(&'a UiDraw, &'a GlyphAtlas, u64)>;

/// Что игрок выбрал: сохраняется, когда файлы перечитываются.
struct Choice {
    /// Тема, выбранная F4.
    theme: Option<String>,
    /// Язык, выбранный кнопкой «Язык».
    lang: String,
}

pub(crate) struct RuiDemo {
    dir: PathBuf,
    shown: bool,
    demo: Page,
    hud: Page,
    choice: Choice,
}

impl RuiDemo {
    pub(crate) fn new() -> Self {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
        let mut demo = Self {
            dir,
            shown: false,
            demo: Page::new("demo.rui", 1),
            hud: Page::new("hud.rui", 2),
            choice: Choice {
                theme: None,
                lang: MAIN_LANG.to_owned(),
            },
        };
        demo.demo.load(&demo.dir, &demo.choice);
        demo.hud.load(&demo.dir, &demo.choice);
        demo
    }

    /// F3: показать или спрятать.
    pub(crate) fn toggle(&mut self) {
        self.shown = !self.shown;
        if self.shown {
            // Файлы могли поменять, пока экран был спрятан.
            self.demo.poll(&self.dir, true, &self.choice);
        }
    }

    /// Полоска здоровья из `ui/hud.rui` прочитана без ошибок: старую рисовать не нужно.
    pub(crate) fn hud_ready(&self) -> bool {
        self.hud.ready()
    }

    /// F4: следующая тема из подключённых. Ничего не делает, пока экран спрятан.
    pub(crate) fn next_theme(&mut self) {
        let Some(screen) = self.demo.screen.as_mut().filter(|_| self.shown) else {
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
                if let Some(hud) = self.hud.screen.as_mut()
                    && let Err(e) = hud.set_theme(&next)
                {
                    self.hud.show_error(&e);
                }
                self.choice.theme = Some(next);
            }
            Err(e) => self.demo.show_error(&e),
        }
    }

    /// Кнопка «Язык»: следующий язык из `ui/lang`, для всех экранов.
    fn next_lang(&mut self) {
        let Some(screen) = self.demo.screen.as_ref() else {
            return;
        };
        let langs: Vec<String> = screen.languages().map(|(c, _)| c.to_owned()).collect();
        let Some(now) = langs.iter().position(|c| *c == self.choice.lang) else {
            return;
        };
        let next = langs[(now + 1) % langs.len()].clone();
        for page in [&mut self.demo, &mut self.hud] {
            if let Some(s) = page.screen.as_mut()
                && let Err(e) = s.set_lang(&next)
            {
                page.show_error(&e);
            }
        }
        tracing::info!("интерфейс: язык {next}");
        self.choice.lang = next;
    }

    /// Мышь и клавиатура для экрана (И.2б). `offset` — где в окне левый верхний угол
    /// картинки, поверх которой нарисован интерфейс; `shift` — зажат ли Shift. `true` —
    /// событие забрал интерфейс (наведение мыши не забирает, оно нужно и камере).
    pub(crate) fn on_event(&mut self, event: &WindowEvent, offset: [f32; 2], shift: bool) -> bool {
        if !self.shown || self.demo.error.is_some() {
            return false;
        }
        let Some(screen) = self.demo.screen.as_mut() else {
            return false;
        };
        let event = match event {
            WindowEvent::CursorMoved { position, .. } => {
                screen.pointer_move([position.x as f32 - offset[0], position.y as f32 - offset[1]]);
                return false;
            }
            WindowEvent::CursorLeft { .. } => {
                screen.pointer_leave();
                return false;
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => return screen.pointer_down(),
                ElementState::Released => match screen.pointer_up() {
                    Some(e) => e,
                    None => return false,
                },
            },
            WindowEvent::KeyboardInput { event: key, .. } if key.state == ElementState::Pressed => {
                let k = match &key.logical_key {
                    Key::Named(NamedKey::Tab) if shift => UiKey::Prev,
                    Key::Named(NamedKey::Tab | NamedKey::ArrowDown | NamedKey::ArrowRight) => {
                        UiKey::Next
                    }
                    Key::Named(NamedKey::ArrowUp | NamedKey::ArrowLeft) => UiKey::Prev,
                    Key::Named(NamedKey::Enter | NamedKey::Space) => UiKey::Activate,
                    _ => return false,
                };
                match screen.key(k) {
                    Some(e) => e,
                    None => return true,
                }
            }
            _ => return false,
        };
        let UiEvent::Click(name) = event;
        tracing::info!("интерфейс: нажата кнопка «{name}»");
        match name.as_str() {
            "close" => self.shown = false,
            "language" => self.next_lang(),
            _ => {}
        }
        true
    }

    /// Shift+F3: откуда взялось каждое свойство экрана (стиль с файлом и строкой или сам
    /// блок) — в лог и в консоль разработчика.
    pub(crate) fn log_origins(&self) {
        match &self.demo.screen {
            Some(s) => {
                for line in s.explain().lines() {
                    tracing::info!("интерфейс: {line}");
                }
            }
            None => tracing::info!("интерфейс: экран ещё не прочитан"),
        }
    }

    /// Что рисовать в этом кадре: список, атлас и ключ атласа. `None` — рисовать нечего.
    /// `viewport` — размер картинки, поверх которой рисуется интерфейс (пиксели);
    /// `hero` — данные героя, если в сцене он есть. Меню F3 закрывает полоску здоровья:
    /// стопка из нескольких экранов — шаг И.4.
    pub(crate) fn frame(&mut self, viewport: [f32; 2], hero: Option<HeroData>) -> Draw<'_> {
        if self.shown {
            return self.demo.frame(&self.dir, viewport, &self.choice);
        }
        let hero = hero?;
        if let Some(s) = self.hud.screen.as_mut() {
            s.set_number("game.hero.hp", f64::from(hero.hp));
            s.set_number("game.hero.hp_max", f64::from(hero.hp_max));
            s.set_number("game.hero.coins", f64::from(hero.coins));
            s.set_flag("game.hero.hurt", hero.hurt);
        }
        self.hud.frame(&self.dir, viewport, &self.choice)
    }
}

/// Языки из `ui/lang`, а если папки нет — вшитые в программу.
fn texts(dir: &Path) -> Result<Texts, UiError> {
    let dir = dir.join("lang");
    if dir.is_dir() {
        return Texts::load(&dir, MAIN_LANG);
    }
    built_in_texts()
}

fn built_in_texts() -> Result<Texts, UiError> {
    let mut texts = Texts::default();
    for (code, src) in BUILT_IN_LANGS {
        texts.add(code, src, &format!("lang/{code}.ron"))?;
    }
    Ok(texts)
}

/// Файлы языков в папке, даже если какой-то не читается: исправят — перечитаем.
fn lang_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir.join("lang"))
        .map(|d| {
            d.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "ron"))
                .collect()
        })
        .unwrap_or_default()
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

    fn built_in(file: &str) -> UiScreen {
        let src = BUILT_IN.iter().find(|(f, _)| *f == file).unwrap().1;
        let mut s = UiScreen::parse_with(src, file, &mut |path| {
            BUILT_IN_FILES
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| (*t).to_owned())
                .ok_or_else(|| path.to_owned())
        })
        .expect("пример разбирается");
        let texts = built_in_texts().expect("языки читаются");
        assert!(texts.gaps().is_empty(), "{:?}", texts.gaps());
        s.set_texts(texts).expect("все ключи на месте");
        s
    }

    #[test]
    fn built_in_demo_parses() {
        let mut s = built_in("demo.rui");
        assert!(!s.draw([1920.0, 1080.0]).quads.is_empty());
        assert!(s.themes().len() >= 2);
        s.set_lang("en").expect("английский");
        assert!(!s.draw([1920.0, 1080.0]).quads.is_empty());
    }

    #[test]
    fn built_in_hud_follows_the_hero() {
        let mut s = built_in("hud.rui");
        s.set_number("game.hero.hp", 3.0);
        s.set_number("game.hero.hp_max", 5.0);
        s.set_number("game.hero.coins", 2.0);
        s.set_flag("game.hero.hurt", false);
        let blocks: Vec<[f32; 4]> = s
            .draw([1920.0, 1080.0])
            .quads
            .iter()
            .filter(|q| matches!(q.kind, reyvec_ui::QuadKind::Block { .. }))
            .map(|q| q.rect)
            .collect();
        // Плашка, пустая полоска, заполненная на 3/5, кружок монеты.
        assert_eq!(blocks.len(), 4, "{blocks:?}");
        let (bar, fill) = (blocks[1], blocks[2]);
        let part = (fill[2] - fill[0]) / (bar[2] - bar[0]);
        assert!((part - 0.6).abs() < 0.01, "{part}");
        assert!(bar[1] < 200.0, "наверху экрана: {bar:?}");
        assert!(
            ((bar[0] + bar[2]) / 2.0 - 960.0).abs() < 1.0,
            "посередине: {bar:?}"
        );
        assert!(s.problem().is_none(), "{:?}", s.problem());
        s.set_number("game.hero.hp", 1.0);
        s.set_flag("game.hero.hurt", true);
        assert!(!s.draw([1920.0, 1080.0]).quads.is_empty());
        assert!(s.problem().is_none(), "{:?}", s.problem());
        for t in s
            .themes()
            .iter()
            .map(|t| (*t).to_owned())
            .collect::<Vec<_>>()
        {
            s.set_theme(&t).expect("тема подходит полоске");
        }
    }

    #[test]
    fn error_screen_parses_any_message() {
        let src = error_screen("строка 3: \"плохо\" \\ и\nдальше");
        let mut s = UiScreen::parse(&src).expect("плашка ошибки разбирается");
        assert!(!s.draw([1280.0, 720.0]).quads.is_empty());
    }
}
