//! `reyvec-ui`: интерфейс игры ReyVec без видеокарты (решение 0022, `docs/game-ui.md`).
//!
//! Экран пишется в файле `.rui` в виде SwiftUI:
//!
//! ```text
//! screen Demo {
//!     Column(spacing: 16, align: center) {
//!         Text("ReyVec").font(48)
//!         Rect().size(240, 12).background(#7FD36B).radius(6)
//!     }
//!     .padding(32)
//!     .background(#171A21E0)
//!     .radius(16)
//! }
//! ```
//!
//! Путь от файла до рисования:
//!
//! 1. [`syntax`] разбирает текст в дерево блоков с местами в файле;
//! 2. [`model`] проверяет имена и значения и строит элементы с раскладкой и оформлением;
//! 3. [`UiScreen::draw`] раскладывает элементы библиотекой `taffy` (flexbox и сетка, как
//!    в вебе) под размер окна и отдаёт список прямоугольников и букв ([`UiDraw`]);
//! 4. видеокарта (`reyvec-gpu`, проход «интерфейс») рисует список одной командой.
//!
//! Процессор раскладывает экран заново, только когда меняется размер окна или файл:
//! неподвижный экран каждый кадр ничего не пересчитывает.
//!
//! Единицы: пиксели экрана 1920×1080. На другом окне всё умножается на высоту / 1080,
//! поэтому экран выглядит одинаково на любом размере.

pub mod data;
pub mod lang;
pub mod model;
pub mod style;
pub mod syntax;

use std::path::{Path, PathBuf};

use reyvec_text::{GlyphAtlas, TextBlock, TextLibrary};
use reyvec_vector::{Color, TextId};
use taffy::prelude::*;

pub use data::UiData;
pub use lang::Texts;
use model::{Edge, Element, Justify, Kind, Len};
pub use syntax::Pos;

/// Высота экрана, под которую пишутся размеры в `.rui`.
pub const DESIGN_HEIGHT: f32 = 1080.0;

/// Ошибка файла `.rui`: место и что не так, по-русски.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum UiError {
    /// Текст не складывается в блоки (скобка, кавычка, знак).
    #[error("{pos}: {message}")]
    Syntax { pos: Pos, message: String },
    /// Блоки сложились, но имя или значение непонятно.
    #[error("{pos}: {message}")]
    Meaning { pos: Pos, message: String },
    /// В файле нет ни одного экрана.
    #[error("в файле нет ни одного экрана: начните с «screen Имя {{ ... }}»")]
    Empty,
    /// Ошибка в подключённом файле темы или стилей.
    #[error("{file}: {source}")]
    File {
        file: String,
        #[source]
        source: Box<UiError>,
    },
}

impl UiError {
    fn pos(&self) -> Option<Pos> {
        match self {
            UiError::Syntax { pos, .. } | UiError::Meaning { pos, .. } => Some(*pos),
            _ => None,
        }
    }
}

/// Что рисовать: прямоугольник со скруглением и обводкой или букву из атласа.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QuadKind {
    Block {
        /// Скругление углов в пикселях окна.
        radius: f32,
        /// Толщина обводки в пикселях окна (0 — без обводки).
        border: f32,
        /// Цвет обводки: линейный, прозрачность не умножена.
        border_color: [f32; 4],
    },
    Glyph {
        /// Прямоугольник буквы в пикселях атласа: левый, верхний, правый, нижний.
        uv: [f32; 4],
    },
}

/// Один прямоугольник списка рисования.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiQuad {
    /// В пикселях окна, (0, 0) — левый верхний угол: левый, верхний, правый, нижний.
    pub rect: [f32; 4],
    /// Цвет заливки или буквы: линейный, прозрачность не умножена.
    pub color: [f32; 4],
    pub kind: QuadKind,
}

/// Нажимаемый блок на экране: где он и как называется.
#[derive(Clone, Debug, PartialEq)]
pub struct UiTarget {
    /// Путь в дереве: номера детей от корня экрана.
    pub path: Vec<usize>,
    /// В пикселях окна: левый, верхний, правый, нижний.
    pub rect: [f32; 4],
    pub disabled: bool,
    /// `.id(…)` блока или, если его нет, первая надпись внутри.
    pub name: String,
}

/// Что случилось с интерфейсом от мыши или клавиатуры.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEvent {
    /// Нажата кнопка: её `.id` (или надпись, если `.id` нет).
    Click(String),
}

/// Клавиши, которые понимает интерфейс.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiKey {
    /// Tab, стрелки вниз и вправо: следующая кнопка.
    Next,
    /// Shift+Tab, стрелки вверх и влево: прежняя.
    Prev,
    /// Enter или пробел: нажать выбранную.
    Activate,
}

/// Список рисования экрана: в порядке «снизу вверх».
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiDraw {
    pub quads: Vec<UiQuad>,
    /// Нажимаемые блоки в порядке файла (по нему ходит Tab).
    pub targets: Vec<UiTarget>,
    /// Размер окна, под который разложен экран.
    pub viewport: [f32; 2],
}

/// Экран из файла `.rui`, готовый к рисованию.
pub struct UiScreen {
    decls: Vec<syntax::ScreenDecl>,
    styles: style::StyleSet,
    /// Имена файлов по номерам [`Pos::file`]: 0 — сам экран.
    names: Vec<String>,
    /// Пути прочитанных файлов (для слежки за правкой); пусто, если файл дан текстом.
    files: Vec<PathBuf>,
    theme: Option<usize>,
    /// Слова для `tr("ключ")`; язык — в `env.lang`.
    texts: Texts,
    screens: Vec<model::Screen>,
    current: usize,
    text: TextLibrary,
    /// Блоки текста элементов `Text` в порядке обхода дерева.
    text_ids: Vec<TextId>,
    draw: UiDraw,
    /// Разложен ли экран под `draw.viewport` после последней правки.
    fresh: bool,
    /// Растёт при каждой сборке экранов: тогда меняется и атлас букв.
    generation: u64,
    /// Над какой кнопкой мышь, какая нажата и какая выбрана клавиатурой (пути в дереве).
    hover: Option<Vec<usize>>,
    pressed: Option<Vec<usize>>,
    focus: Option<Vec<usize>>,
    /// Состояния поменялись: вид блоков пересчитать перед рисованием.
    restyle: bool,
    /// Окно, язык, способ ввода и данные игры для `.when(…)` и живых значений.
    env: model::Env,
    /// Ошибка последнего пересчёта по данным игры (нет такого пути, формула не
    /// считается). Экран при этом показывает прежний вид.
    problem: Option<UiError>,
}

impl UiScreen {
    /// Разбирает файл. Ошибка — с местом в файле; прежний экран при этом не трогается
    /// (его хранит тот, кто вызывает). Файлы из `head` здесь подключить нельзя — для них
    /// [`Self::load`].
    pub fn parse(src: &str) -> Result<Self, UiError> {
        Self::parse_with(src, "экран", &mut |path| {
            Err(format!(
                "файл «{path}» не подключить: экран дан текстом, а не из папки ui/"
            ))
        })
    }

    /// Читает экран `file` из папки `dir` (обычно `ui/`) вместе с темами и стилями из его
    /// `head`. Пути в `head` — от `dir`, `.rui` можно не писать.
    pub fn load(dir: &Path, file: &str) -> Result<Self, UiError> {
        let main = dir.join(file);
        let src = std::fs::read_to_string(&main).map_err(|e| UiError::File {
            file: file.to_owned(),
            source: Box::new(UiError::Meaning {
                pos: Pos::default(),
                message: format!("файл не прочитать: {e}"),
            }),
        })?;
        let mut read = Vec::new();
        let mut screen = Self::parse_with(&src, file, &mut |path| {
            let p = rui_path(dir, path);
            let text =
                std::fs::read_to_string(&p).map_err(|e| format!("файл не прочитать: {e}"))?;
            read.push(p);
            Ok(text)
        })?;
        screen.files.push(main);
        screen.files.extend(read);
        Ok(screen)
    }

    /// Разбирает экран `src` (его имя для ошибок — `name`); файлы из `head` даёт `read`
    /// по пути, как он написан.
    pub fn parse_with(
        src: &str,
        name: &str,
        read: &mut dyn FnMut(&str) -> Result<String, String>,
    ) -> Result<Self, UiError> {
        let mut names = vec![name.to_owned()];
        let result = Self::assemble(src, &mut names, read);
        result.map_err(|e| match e.pos() {
            Some(pos) if pos.file > 0 => UiError::File {
                file: names
                    .get(usize::from(pos.file))
                    .cloned()
                    .unwrap_or_default(),
                source: Box::new(e),
            },
            _ => e,
        })
    }

    fn assemble(
        src: &str,
        names: &mut Vec<String>,
        read: &mut dyn FnMut(&str) -> Result<String, String>,
    ) -> Result<Self, UiError> {
        let doc = syntax::parse_file(src, 0)?;
        if doc.screens.is_empty() {
            return Err(UiError::Empty);
        }
        let mut styles = style::StyleSet::default();
        for inc in &doc.head {
            let file = with_rui(&inc.path);
            if names.contains(&file) {
                return Err(UiError::Meaning {
                    pos: inc.pos,
                    message: format!("«{}» уже подключён выше", inc.path),
                });
            }
            let text = read(&inc.path).map_err(|message| UiError::Meaning {
                pos: inc.pos,
                message: format!("{}: {message}", file),
            })?;
            let id = u16::try_from(names.len()).unwrap_or(u16::MAX);
            names.push(file.clone());
            let part = syntax::parse_file(&text, id)?;
            let bad = |pos: Pos, what: &str| UiError::Meaning {
                pos,
                message: format!("{what} пишется в файле экрана, а это подключённый файл"),
            };
            if let Some(s) = part.screens.first() {
                return Err(bad(s.pos, "screen"));
            }
            if let Some(h) = part.head.first() {
                return Err(bad(h.pos, "head"));
            }
            match inc.kind {
                syntax::IncludeKind::Theme => {
                    if let Some(r) = part.styles.first() {
                        return Err(UiError::Meaning {
                            pos: r.pos,
                            message: "в файле темы только theme { }; стили — в файле, подключённом через styles(...)".into(),
                        });
                    }
                }
                syntax::IncludeKind::Styles => {
                    if let Some(t) = part.themes.first() {
                        return Err(UiError::Meaning {
                            pos: t.pos,
                            message: "в файле стилей только style { }; темы — в файле, подключённом через theme(...)".into(),
                        });
                    }
                }
            }
            for t in part.themes {
                if styles.themes.iter().any(|o| o.name == t.name) {
                    return Err(UiError::Meaning {
                        pos: t.pos,
                        message: format!("тема «{}» уже есть в другом подключённом файле", t.name),
                    });
                }
                styles.themes.push(t);
            }
            styles.add_rules(&part.styles, &file);
        }
        // Темы и стили самого экрана — последними: при равенстве они сильнее.
        for t in &doc.themes {
            if styles.themes.iter().any(|o| o.name == t.name) {
                return Err(UiError::Meaning {
                    pos: t.pos,
                    message: format!("тема «{}» уже есть в подключённом файле", t.name),
                });
            }
            styles.themes.push(t.clone());
        }
        styles.add_rules(&doc.styles, &names[0]);
        let mut ui = Self {
            decls: doc.screens,
            styles,
            names: names.clone(),
            files: Vec::new(),
            theme: None,
            texts: Texts::default(),
            screens: Vec::new(),
            current: 0,
            text: TextLibrary::new(),
            text_ids: Vec::new(),
            draw: UiDraw::default(),
            fresh: false,
            generation: 0,
            hover: None,
            pressed: None,
            focus: None,
            restyle: false,
            env: model::Env::default(),
            problem: None,
        };
        let first = (!ui.styles.themes.is_empty()).then_some(0);
        ui.rebuild(first)?;
        Ok(ui)
    }

    /// Строит экраны с темой `theme`. При ошибке прежние экраны остаются.
    fn rebuild(&mut self, theme: Option<usize>) -> Result<(), UiError> {
        let vars = self.styles.theme_vars(theme)?;
        let screens = self
            .decls
            .iter()
            .map(|d| model::screen(d, &self.styles, &vars, &self.texts, &self.env.lang))
            .collect::<Result<Vec<_>, _>>()?;
        let mut text = TextLibrary::new();
        let mut text_ids = Vec::new();
        for s in &screens {
            collect_text(&s.root, &mut text, &mut text_ids);
        }
        self.screens = screens;
        self.text = text;
        self.text_ids = text_ids;
        self.theme = theme;
        self.fresh = false;
        self.generation += 1;
        // Дерево то же (другие тема или язык), поэтому наведение и фокус остаются:
        // язык можно сменить кнопкой, не теряя её.
        // Условия `.when` при сборке считались для окна 1920×1080: пересчитать под своё.
        self.restyle = true;
        Ok(())
    }

    /// Язык игры: слова для `tr("ключ")` и `.when(lang: en)`. Экраны собираются заново
    /// (у текста другая длина). При ошибке остаются прежние язык и экраны.
    pub fn set_lang(&mut self, lang: &str) -> Result<(), UiError> {
        if self.env.lang == lang {
            return Ok(());
        }
        let old = std::mem::replace(&mut self.env.lang, lang.to_owned());
        if let Err(e) = self.rebuild_keep_theme() {
            self.env.lang = old;
            return Err(e);
        }
        Ok(())
    }

    /// Язык игры (`ru` по умолчанию).
    pub fn lang(&self) -> &str {
        &self.env.lang
    }

    /// Слова на языках для `tr("ключ")`. Без них `tr` показывает сам ключ. При ошибке
    /// (ключа нет в главном языке) остаются прежние слова.
    pub fn set_texts(&mut self, texts: Texts) -> Result<(), UiError> {
        let old = std::mem::replace(&mut self.texts, texts);
        if let Err(e) = self.rebuild_keep_theme() {
            self.texts = old;
            return Err(e);
        }
        Ok(())
    }

    /// Языки из [`Self::set_texts`]: код и имя для игрока.
    pub fn languages(&self) -> impl Iterator<Item = (&str, &str)> {
        self.texts.languages()
    }

    fn rebuild_keep_theme(&mut self) -> Result<(), UiError> {
        let names = self.names.clone();
        self.rebuild(self.theme).map_err(|e| match e.pos() {
            Some(pos) if pos.file > 0 => UiError::File {
                file: names[usize::from(pos.file)].clone(),
                source: Box::new(e),
            },
            _ => e,
        })
    }

    /// Чем игрок пользуется сейчас, для `.when(input: keyboard)`: `mouse`, `keyboard`,
    /// `gamepad`. Мышь и клавиши интерфейса ставят его сами.
    pub fn set_input(&mut self, input: &str) {
        if self.env.input != input {
            input.clone_into(&mut self.env.input);
            self.restyle = true;
        }
    }

    /// Мышь в точке окна `p` (пиксели, как у [`Self::draw`]). `true` — она над кнопкой.
    pub fn pointer_move(&mut self, p: [f32; 2]) -> bool {
        self.set_input("mouse");
        let hit = self
            .draw
            .targets
            .iter()
            .rev()
            .find(|t| {
                !t.disabled
                    && p[0] >= t.rect[0]
                    && p[0] < t.rect[2]
                    && p[1] >= t.rect[1]
                    && p[1] < t.rect[3]
            })
            .map(|t| t.path.clone());
        if hit != self.hover {
            // Как в меню игр: наведённая мышью кнопка становится и выбранной.
            if hit.is_some() {
                self.focus.clone_from(&hit);
            }
            self.hover = hit;
            self.restyle = true;
        }
        self.hover.is_some()
    }

    /// Мышь ушла из окна.
    pub fn pointer_leave(&mut self) {
        if self.hover.take().is_some() || self.pressed.take().is_some() {
            self.restyle = true;
        }
    }

    /// Кнопка мыши нажата. `true` — над кнопкой интерфейса: нажатие его, не игры.
    pub fn pointer_down(&mut self) -> bool {
        if self.hover.is_none() {
            return false;
        }
        self.pressed.clone_from(&self.hover);
        self.restyle = true;
        true
    }

    /// Кнопка мыши отпущена: если над той же кнопкой, что была нажата, — это нажатие.
    pub fn pointer_up(&mut self) -> Option<UiEvent> {
        let pressed = self.pressed.take()?;
        self.restyle = true;
        (self.hover.as_ref() == Some(&pressed))
            .then(|| self.target_name(&pressed))
            .flatten()
            .map(UiEvent::Click)
    }

    /// Клавиша. `None` — ничего не нажато (фокус мог сдвинуться).
    pub fn key(&mut self, key: UiKey) -> Option<UiEvent> {
        self.set_input("keyboard");
        let order: Vec<&UiTarget> = self.draw.targets.iter().filter(|t| !t.disabled).collect();
        if order.is_empty() {
            return None;
        }
        let now = self
            .focus
            .as_ref()
            .and_then(|f| order.iter().position(|t| t.path == *f));
        let step = |forward: bool| match now {
            None => {
                if forward {
                    0
                } else {
                    order.len() - 1
                }
            }
            Some(i) if forward => (i + 1) % order.len(),
            Some(i) => (i + order.len() - 1) % order.len(),
        };
        match key {
            UiKey::Next | UiKey::Prev => {
                let i = step(key == UiKey::Next);
                self.focus = Some(order[i].path.clone());
                // Подсветка одна: выбранная клавиатурой, а не та, где осталась мышь.
                self.hover = None;
                self.restyle = true;
                None
            }
            UiKey::Activate => {
                let i = now?;
                Some(UiEvent::Click(order[i].name.clone()))
            }
        }
    }

    /// Какая кнопка сейчас выбрана (её имя, как в [`UiEvent::Click`]).
    pub fn focused(&self) -> Option<String> {
        self.focus.as_ref().and_then(|f| self.target_name(f))
    }

    fn target_name(&self, path: &[usize]) -> Option<String> {
        self.draw
            .targets
            .iter()
            .find(|t| t.path == path)
            .map(|t| t.name.clone())
    }

    /// Ставит блокам их состояния и пересчитывает вид.
    fn apply_states(&mut self) {
        let Some(screen) = self.screens.get_mut(self.current) else {
            return;
        };
        clear_dynamic(&mut screen.root);
        for (path, bit) in [
            (&self.hover, model::HOVER),
            (&self.pressed, model::PRESSED),
            (&self.focus, model::FOCUS),
        ] {
            if let Some(path) = path {
                let mut el = &mut screen.root;
                let mut ok = true;
                for i in path {
                    match el.children.get_mut(*i) {
                        Some(c) => el = c,
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    el.dynamic |= bit;
                }
            }
        }
        // Модификаторы проверены при сборке; ошибка бывает только от данных игры.
        let done = model::restyle(
            &mut screen.root,
            model::DEFAULT_FONT,
            Color::WHITE,
            0,
            &self.env,
        );
        self.problem = done.err();
        // Живые тексты: раскладываются заново только поменявшиеся.
        let first: usize = self.screens[..self.current]
            .iter()
            .map(|s| count_text(&s.root))
            .sum();
        let mut next = first;
        sync_text(
            &self.screens[self.current].root,
            &mut self.text,
            &self.text_ids,
            &mut next,
        );
        self.fresh = false;
    }

    /// Файлы, из которых собран экран: сам экран и подключённые. Правка любого —
    /// повод перечитать.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Имена тем из подключённых файлов по порядку.
    pub fn themes(&self) -> Vec<&str> {
        self.styles.theme_names().collect()
    }

    /// Активная тема (по умолчанию — первая подключённая).
    pub fn theme(&self) -> Option<&str> {
        self.theme.map(|i| self.styles.themes[i].name.as_str())
    }

    /// Включает тему по имени. `Ok(false)` — такой темы нет; ошибка — в теме что-то не так
    /// (тогда остаётся прежняя).
    pub fn set_theme(&mut self, name: &str) -> Result<bool, UiError> {
        let Some(i) = self.styles.themes.iter().position(|t| t.name == name) else {
            return Ok(false);
        };
        if Some(i) != self.theme {
            let names = self.names.clone();
            self.rebuild(Some(i)).map_err(|e| match e.pos() {
                Some(pos) if pos.file > 0 => UiError::File {
                    file: names[usize::from(pos.file)].clone(),
                    source: Box::new(e),
                },
                _ => e,
            })?;
        }
        Ok(true)
    }

    /// Номер сборки: меняется, когда экраны собраны заново (другая тема) — тогда
    /// видеокарте нужен новый атлас букв.
    pub fn generation(&self) -> u64 {
        // Новые буквы живых текстов тоже попадают в атлас.
        (self.generation << 32) ^ self.text.revision()
    }

    /// Число из игры по пути `game.…`: `Bar(game.hero.hp / game.hero.hp_max)`.
    pub fn set_number(&mut self, path: &str, v: f64) {
        if self.env.data.set_number(path, v) {
            self.restyle = true;
        }
    }

    /// Строка из игры по пути.
    pub fn set_text(&mut self, path: &str, v: &str) {
        if self.env.data.set_text(path, v) {
            self.restyle = true;
        }
    }

    /// Да или нет из игры по пути: `.when(game.hero.hurt) { … }`.
    pub fn set_flag(&mut self, path: &str, v: bool) {
        if self.env.data.set_flag(path, v) {
            self.restyle = true;
        }
    }

    /// Данные игры, которые видит экран.
    pub fn data(&self) -> &UiData {
        &self.env.data
    }

    /// Ошибка последнего пересчёта по данным игры: путь, которого игра не даёт, или
    /// формула, которая не считается. `None` — всё хорошо.
    pub fn problem(&self) -> Option<&UiError> {
        self.problem.as_ref()
    }

    /// «Откуда взялось»: дерево текущего экрана, у каждого блока — его свойства и
    /// источник каждого (стиль с файлом и строкой или сам блок).
    pub fn explain(&self) -> String {
        let mut out = String::new();
        if let Some(s) = self.screens.get(self.current) {
            explain(&s.root, 0, &mut out);
        }
        out
    }

    /// Имена экранов файла по порядку.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.screens.iter().map(|s| s.name.as_str())
    }

    /// Какой экран рисовать. `false`, если такого нет.
    pub fn show(&mut self, name: &str) -> bool {
        match self.screens.iter().position(|s| s.name == name) {
            Some(i) => {
                if i != self.current {
                    self.current = i;
                    self.fresh = false;
                }
                true
            }
            None => false,
        }
    }

    /// Атлас букв всех текстов файла: его загружает видеокарта. Меняется только при
    /// разборе файла.
    pub fn atlas(&self) -> &GlyphAtlas {
        self.text.atlas()
    }

    /// Список рисования под окно `viewport` (пиксели). Раскладка считается заново,
    /// только если размер окна поменялся.
    pub fn draw(&mut self, viewport: [f32; 2]) -> &UiDraw {
        if self.env.width != viewport[0] || self.env.height != viewport[1] {
            self.env.width = viewport[0];
            self.env.height = viewport[1];
            self.restyle = true;
        }
        if self.restyle {
            self.restyle = false;
            self.apply_states();
        }
        if !self.fresh || self.draw.viewport != viewport {
            self.draw = self.lay_out(viewport);
            self.fresh = true;
        }
        &self.draw
    }

    /// То же, что [`Self::draw`], вместе с атласом букв: для того, кто отдаёт оба
    /// видеокарте сразу.
    pub fn draw_with_atlas(&mut self, viewport: [f32; 2]) -> (&UiDraw, &GlyphAtlas) {
        self.draw(viewport);
        (&self.draw, self.text.atlas())
    }

    fn lay_out(&self, viewport: [f32; 2]) -> UiDraw {
        let mut out = UiDraw {
            quads: Vec::new(),
            targets: Vec::new(),
            viewport,
        };
        if viewport[0] <= 0.0 || viewport[1] <= 0.0 {
            return out;
        }
        let scale = viewport[1] / DESIGN_HEIGHT;
        // Тексты экранов до текущего: номера их блоков идут раньше.
        let mut first_text = 0;
        for s in &self.screens[..self.current] {
            first_text += count_text(&s.root);
        }
        let root = &self.screens[self.current].root;
        let mut tree: TaffyTree<usize> = TaffyTree::new();
        let mut next_text = first_text;
        let Ok(node) = build(&mut tree, root, None, &mut next_text) else {
            return out;
        };
        let sizes: Vec<(f32, f32)> = self
            .text_ids
            .iter()
            .map(|id| self.text.get(*id).map_or((0.0, 0.0), |t| t.size()))
            .collect();
        let available = Size {
            width: AvailableSpace::Definite(viewport[0] / scale),
            height: AvailableSpace::Definite(DESIGN_HEIGHT),
        };
        let measured =
            tree.compute_layout_with_measure(node, available, |inputs, _, context, style| {
                taffy::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, _| {
                        let (w, h) = context.map_or((0.0, 0.0), |i| sizes[*i]);
                        Size {
                            width: known.width.unwrap_or(w),
                            height: known.height.unwrap_or(h),
                        }
                    },
                )
            });
        if measured.is_err() {
            return out;
        }
        let mut next_text = first_text;
        self.paint(
            &tree,
            node,
            root,
            [0.0, 0.0],
            1.0,
            scale,
            &mut next_text,
            &mut Vec::new(),
            &mut out,
        );
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &self,
        tree: &TaffyTree<usize>,
        node: NodeId,
        el: &Element,
        origin: [f32; 2],
        opacity: f32,
        scale: f32,
        next_text: &mut usize,
        path: &mut Vec<usize>,
        out: &mut UiDraw,
    ) {
        if !el.placement.visible {
            *next_text += count_text(el);
            return;
        }
        let Ok(layout) = tree.layout(node) else {
            return;
        };
        let x = origin[0] + layout.location.x;
        let y = origin[1] + layout.location.y;
        let (w, h) = (layout.size.width, layout.size.height);
        let opacity = opacity * el.look.opacity;
        let rect = [x * scale, y * scale, (x + w) * scale, (y + h) * scale];
        if el.interactive {
            out.targets.push(UiTarget {
                path: path.clone(),
                rect,
                disabled: el.disabled,
                name: el.id.clone().or_else(|| first_text(el)).unwrap_or_default(),
            });
        }
        let border = el.look.border.map_or((0.0, [0.0; 4]), |(c, width)| {
            (width * scale, faded(c, opacity))
        });
        if (el.look.background.is_some() || border.0 > 0.0) && w > 0.0 && h > 0.0 {
            out.quads.push(UiQuad {
                rect,
                color: el.look.background.map_or([0.0; 4], |c| faded(c, opacity)),
                kind: QuadKind::Block {
                    radius: el.look.radius * scale,
                    border: border.0,
                    border_color: border.1,
                },
            });
        }
        if let (Kind::Bar, Some(fill)) = (&el.kind, el.look.fill)
            && el.look.value > 0.0
            && w > 0.0
            && h > 0.0
        {
            let r = [x, y, x + w * el.look.value, y + h];
            out.quads.push(UiQuad {
                rect: r.map(|v| v * scale),
                color: faded(fill, opacity),
                kind: QuadKind::Block {
                    radius: el.look.radius.min(w * el.look.value * 0.5) * scale,
                    border: 0.0,
                    border_color: [0.0; 4],
                },
            });
        }
        if let Kind::Text(spec) = &el.kind {
            let id = self.text_ids[*next_text];
            *next_text += 1;
            if let Some(laid) = self.text.get(id) {
                let color = faded(spec.color, opacity);
                let left = x + layout.padding.left;
                let top = y + layout.padding.top;
                for g in &laid.glyphs {
                    let [l, t, r, b] = g.rect;
                    // Буквы разложены с y вверх от верха блока: на экране y вниз.
                    out.quads.push(UiQuad {
                        rect: [
                            (left + l) * scale,
                            (top - t) * scale,
                            (left + r) * scale,
                            (top - b) * scale,
                        ],
                        color,
                        kind: QuadKind::Glyph { uv: g.uv },
                    });
                }
            }
        }
        let Ok(children) = tree.children(node) else {
            return;
        };
        for (i, (child, el)) in children.into_iter().zip(&el.children).enumerate() {
            path.push(i);
            self.paint(
                tree,
                child,
                el,
                [x, y],
                opacity,
                scale,
                next_text,
                path,
                out,
            );
            path.pop();
        }
    }
}

fn clear_dynamic(el: &mut Element) {
    el.dynamic = 0;
    for c in &mut el.children {
        clear_dynamic(c);
    }
}

fn first_text(el: &Element) -> Option<String> {
    if let Kind::Text(t) = &el.kind {
        return Some(t.text.clone());
    }
    el.children.iter().find_map(first_text)
}

fn rui_path(dir: &Path, path: &str) -> PathBuf {
    dir.join(with_rui(path))
}

fn with_rui(path: &str) -> String {
    if path.ends_with(".rui") {
        path.to_owned()
    } else {
        format!("{path}.rui")
    }
}

fn explain(el: &Element, depth: usize, out: &mut String) {
    use std::fmt::Write;
    let pad = "  ".repeat(depth);
    let classes: String = el.classes.iter().map(|c| format!(".{c}")).collect();
    let _ = writeln!(
        out,
        "{pad}{}{classes} (строка {})",
        el.type_name, el.pos.line
    );
    for (name, from) in &el.origins {
        let _ = writeln!(out, "{pad}  .{name} ← {from}");
    }
    for c in &el.children {
        explain(c, depth + 1, out);
    }
}

fn faded(c: Color, opacity: f32) -> [f32; 4] {
    [c.r, c.g, c.b, c.a * opacity]
}

// Цвет букв берётся из элемента при рисовании, поэтому в блоке текста его нет: смена
// цвета при наведении не раскладывает текст заново.
fn collect_text(el: &Element, text: &mut TextLibrary, ids: &mut Vec<TextId>) {
    if let Kind::Text(spec) = &el.kind {
        ids.push(text.add(TextBlock::new(spec.text.clone(), spec.size)));
    }
    for c in &el.children {
        collect_text(c, text, ids);
    }
}

/// Отдаёт библиотеке поменявшиеся тексты (`TextLibrary::set` сам пропускает прежние).
fn sync_text(el: &Element, text: &mut TextLibrary, ids: &[TextId], next: &mut usize) {
    if let Kind::Text(spec) = &el.kind {
        if let Some(id) = ids.get(*next) {
            text.set(*id, TextBlock::new(spec.text.clone(), spec.size));
        }
        *next += 1;
    }
    for c in &el.children {
        sync_text(c, text, ids, next);
    }
}

fn count_text(el: &Element) -> usize {
    usize::from(matches!(el.kind, Kind::Text(_)))
        + el.children.iter().map(count_text).sum::<usize>()
}

fn dimension(len: Len) -> Dimension {
    match len {
        Len::Auto => Dimension::auto(),
        Len::Px(px) => Dimension::length(px),
        Len::Percent(p) => Dimension::percent(p / 100.0),
    }
}

fn self_align(edge: Edge) -> AlignSelf {
    match edge {
        Edge::Start => AlignSelf::START,
        Edge::Center => AlignSelf::CENTER,
        Edge::End => AlignSelf::END,
        Edge::Stretch => AlignSelf::STRETCH,
    }
}

/// Какой родитель у элемента: от этого зависит, что значит `.align`.
#[derive(Clone, Copy)]
enum Parent {
    Column,
    Row,
    Stack,
}

fn build(
    tree: &mut TaffyTree<usize>,
    el: &Element,
    parent: Option<Parent>,
    next_text: &mut usize,
) -> taffy::TaffyResult<NodeId> {
    let p = &el.placement;
    let [pt, pr, pb, pl] = p.padding;
    let [mt, mr, mb, ml] = p.margin;
    let mut style = Style {
        size: Size {
            width: dimension(p.width),
            height: dimension(p.height),
        },
        padding: Rect {
            left: LengthPercentage::length(pl),
            right: LengthPercentage::length(pr),
            top: LengthPercentage::length(pt),
            bottom: LengthPercentage::length(pb),
        },
        margin: Rect {
            left: LengthPercentageAuto::length(ml),
            right: LengthPercentageAuto::length(mr),
            top: LengthPercentageAuto::length(mt),
            bottom: LengthPercentageAuto::length(mb),
        },
        flex_grow: p.grow,
        ..Style::default()
    };
    let (h, v) = p.align;
    match parent {
        Some(Parent::Column) => style.align_self = h.map(self_align),
        Some(Parent::Row) => style.align_self = v.map(self_align),
        Some(Parent::Stack) => {
            // Как ZStack у SwiftUI: без `.align` — в середину.
            style.justify_self = Some(self_align(h.unwrap_or(Edge::Center)));
            style.align_self = Some(self_align(v.unwrap_or(Edge::Center)));
            style.grid_row = Line {
                start: line(1),
                end: span(1),
            };
            style.grid_column = Line {
                start: line(1),
                end: span(1),
            };
        }
        None => {}
    }
    let me = match &el.kind {
        Kind::Column | Kind::Row => {
            style.display = Display::Flex;
            style.flex_direction = if matches!(el.kind, Kind::Column) {
                FlexDirection::Column
            } else {
                FlexDirection::Row
            };
            style.gap = Size {
                width: LengthPercentage::length(p.spacing),
                height: LengthPercentage::length(p.spacing),
            };
            style.align_items = match p.cross {
                Edge::Start => AlignItems::START,
                Edge::Center => AlignItems::CENTER,
                Edge::End => AlignItems::END,
                Edge::Stretch => AlignItems::STRETCH,
            };
            style.justify_content = match p.justify {
                Justify::Start => JustifyContent::START,
                Justify::Center => JustifyContent::CENTER,
                Justify::End => JustifyContent::END,
                Justify::Between => JustifyContent::SPACE_BETWEEN,
                Justify::Around => JustifyContent::SPACE_AROUND,
            };
            Some(if matches!(el.kind, Kind::Column) {
                Parent::Column
            } else {
                Parent::Row
            })
        }
        Kind::Stack => {
            style.display = Display::Grid;
            style.grid_template_columns = vec![fr(1.0)];
            style.grid_template_rows = vec![fr(1.0)];
            Some(Parent::Stack)
        }
        Kind::Text(_) => {
            let index = *next_text;
            *next_text += 1;
            if !p.visible {
                style.display = Display::None;
            }
            return tree.new_leaf_with_context(style, index);
        }
        Kind::Spacer | Kind::Bar => None,
    };
    if !p.visible {
        style.display = Display::None;
    }
    let mut children = Vec::with_capacity(el.children.len());
    for c in &el.children {
        children.push(build(tree, c, me, next_text)?);
    }
    tree.new_with_children(style, &children)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(draw: &UiDraw) -> Vec<&UiQuad> {
        draw.quads
            .iter()
            .filter(|q| matches!(q.kind, QuadKind::Block { .. }))
            .collect()
    }

    #[test]
    fn centered_panel_scales_with_window() {
        let mut ui =
            UiScreen::parse("screen A { Rect().size(400, 200).background(#FFFFFF).radius(10) }")
                .unwrap();
        let full = *blocks(ui.draw([1920.0, 1080.0]))[0];
        assert_eq!(full.rect, [760.0, 440.0, 1160.0, 640.0]);
        let QuadKind::Block { radius, .. } = full.kind else {
            unreachable!()
        };
        assert_eq!(radius, 10.0);
        // Вдвое меньшее окно — вдвое меньше и панель, всё так же в середине.
        let half = *blocks(ui.draw([960.0, 540.0]))[0];
        assert_eq!(half.rect, [380.0, 220.0, 580.0, 320.0]);
    }

    #[test]
    fn column_stacks_children_with_spacing_and_padding() {
        let mut ui = UiScreen::parse(
            r##"screen A {
                Column(spacing: 10) {
                    Rect().size(100, 20).background(#FF0000)
                    Rect().size(50, 30).background(#00FF00)
                }
                .padding(5)
                .background(#000000)
                .align(top_left)
            }"##,
        )
        .unwrap();
        let draw = ui.draw([1920.0, 1080.0]).clone();
        let b = blocks(&draw);
        assert_eq!(b.len(), 3);
        // Фон столбца: 5 + 100 + 5 по ширине, 5 + 20 + 10 + 30 + 5 по высоте.
        assert_eq!(b[0].rect, [0.0, 0.0, 110.0, 70.0]);
        assert_eq!(b[1].rect, [5.0, 5.0, 105.0, 25.0]);
        assert_eq!(b[2].rect, [5.0, 35.0, 55.0, 65.0]);
    }

    #[test]
    fn row_with_spacer_pushes_to_the_edges() {
        let mut ui = UiScreen::parse(
            r##"screen A {
                Row {
                    Rect().size(10, 10).background(#FF0000)
                    Spacer()
                    Rect().size(10, 10).background(#00FF00)
                }
                .width(fill)
                .align(top)
            }"##,
        )
        .unwrap();
        let draw = ui.draw([1920.0, 1080.0]).clone();
        let b = blocks(&draw);
        assert_eq!(b[0].rect[0], 0.0);
        assert_eq!(b[1].rect[2], 1920.0);
    }

    #[test]
    fn anchored_hud_bar_sits_in_the_corner() {
        let mut ui = UiScreen::parse(
            "layer Hud { Rect().size(260, 10).background(#7FD36B).anchor(bottom_left, margin: 24) }",
        )
        .unwrap();
        let draw = ui.draw([1920.0, 1080.0]).clone();
        assert_eq!(blocks(&draw)[0].rect, [24.0, 1046.0, 284.0, 1056.0]);
    }

    #[test]
    fn text_is_measured_and_its_letters_sit_inside() {
        let mut ui = UiScreen::parse(
            r##"screen A {
                Column {
                    Text("Играть").font(40).color(#F5A54A)
                }
                .padding(20)
                .background(#000000)
                .align(top_left)
                .opacity(0.5)
            }"##,
        )
        .unwrap();
        let draw = ui.draw([1920.0, 1080.0]).clone();
        let panel = blocks(&draw)[0].rect;
        let glyphs: Vec<_> = draw
            .quads
            .iter()
            .filter(|q| matches!(q.kind, QuadKind::Glyph { .. }))
            .collect();
        assert_eq!(glyphs.len(), 6);
        // Панель обнимает текст: ширина текста плюс два отступа, высота строки 40 × 1.25.
        assert!(panel[2] > 100.0 && panel[2] < 300.0, "{panel:?}");
        assert!((panel[3] - (40.0 + 50.0)).abs() < 1.0, "{panel:?}");
        for g in &glyphs {
            // С запасом поля вокруг буквы (несколько пикселей).
            assert!(g.rect[0] > 15.0 && g.rect[2] < panel[2], "{:?}", g.rect);
            assert!(g.rect[1] > 10.0 && g.rect[3] < panel[3], "{:?}", g.rect);
            assert!((g.color[3] - 0.5).abs() < 1e-6, "прозрачность родителя");
        }
        assert!(!ui.atlas().is_empty());
    }

    #[test]
    fn layout_is_cached_until_the_window_changes() {
        let mut ui =
            UiScreen::parse("screen A { Rect().size(10, 10).background(#FFFFFF) }").unwrap();
        let a = ui.draw([800.0, 600.0]) as *const UiDraw;
        let b = ui.draw([800.0, 600.0]) as *const UiDraw;
        assert_eq!(a, b);
        assert!(ui.fresh);
    }

    #[test]
    fn second_screen_keeps_its_own_texts() {
        let mut ui = UiScreen::parse(
            r#"screen A { Text("один") }
               screen B { Text("два слова") }"#,
        )
        .unwrap();
        assert_eq!(ui.names().collect::<Vec<_>>(), ["A", "B"]);
        let a = ui.draw([1920.0, 1080.0]).quads.len();
        assert!(ui.show("B"));
        let b = ui.draw([1920.0, 1080.0]).quads.len();
        assert_eq!((a, b), (4, 8));
        assert!(!ui.show("C"));
    }

    #[test]
    fn empty_file_is_an_error() {
        assert_eq!(UiScreen::parse("// пусто").err(), Some(UiError::Empty));
    }

    /// Экран с подключёнными файлами из памяти: путь → текст.
    fn with_files(src: &str, files: &[(&str, &str)]) -> Result<UiScreen, UiError> {
        UiScreen::parse_with(src, "экран.rui", &mut |path| {
            files
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| (*t).to_owned())
                .ok_or_else(|| "нет такого файла".to_owned())
        })
    }

    fn el<'a>(ui: &'a UiScreen, path: &[usize]) -> &'a Element {
        let mut e = &ui.screens[ui.current].root;
        for i in path {
            e = &e.children[*i];
        }
        e
    }

    fn bg(e: &Element) -> [u8; 3] {
        let c = e.look.background.expect("есть фон");
        let to = |v: f32| {
            let s = if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0).round() as u8
        };
        [to(c.r), to(c.g), to(c.b)]
    }

    const THEMES: &str = r##"
        theme Forge {
            panel = #111111
            accent = #F5A54A
            gap = 12
            gap_big = var(gap) * 2
        }
        theme Ice {
            panel = #0A1A2A
            accent = #6BB8F5
            gap = 8
            gap_big = var(gap) * 2
        }
    "##;

    #[test]
    fn ladder_kind_class_nested_own() {
        let styles = r##"
            style Rect { .background(#010101).radius(4) }
            style big { .background(#020202) }
            style Menu {
                Rect { .background(#030303) }
            }
            style Rect { .radius(9) }
        "##;
        let ui = with_files(
            r##"
            head { styles("styles/a") }
            screen Menu {
                Column {
                    Rect().size(10)
                    Rect().class(big).size(10)
                    Rect().class(big).background(#040404)
                }
            }
            "##,
            &[("styles/a", styles)],
        )
        .unwrap();
        // Вложенный «Menu Rect» сильнее и класса, и вида.
        assert_eq!(bg(el(&ui, &[0, 0])), [3, 3, 3]);
        assert_eq!(bg(el(&ui, &[0, 1])), [3, 3, 3]);
        // Модификатор у самого блока сильнее всех.
        assert_eq!(bg(el(&ui, &[0, 2])), [4, 4, 4]);
        // На одной ступени побеждает записанное позже.
        assert_eq!(el(&ui, &[0, 0]).look.radius, 9.0);
        // «Откуда взялось».
        let why = ui.explain();
        assert!(
            why.contains(".background ← style Menu Rect (styles/a.rui, строка 5)"),
            "{why}"
        );
        assert!(why.contains(".background ← у блока (строка 7)"), "{why}");
    }

    #[test]
    fn class_beats_kind_and_later_file_wins() {
        let a = "style Rect { .background(#010101) } style big { .background(#020202) }";
        let b = "style Rect { .background(#050505) }";
        let ui = with_files(
            r#"head { styles("a") styles("b") }
            screen S { Rect().size(4) Rect().class(big).size(4) }"#,
            &[("a", a), ("b", b)],
        )
        .unwrap();
        assert_eq!(bg(el(&ui, &[0])), [5, 5, 5]);
        assert_eq!(bg(el(&ui, &[1])), [2, 2, 2]);
    }

    #[test]
    fn nested_styles_reach_any_depth_but_only_inside() {
        let styles = r##"
            style Menu { Button { .background(#0A0A0A) } }
            style Menu.wide { Rect { .width(500) } }
        "##;
        // Button — пока просто имя вида блока: в стилях это любое слово с большой буквы.
        let ui = with_files(
            r#"head { styles("s") }
            screen S {
                Column { Rect().size(4) }
                Stack { Column { Rect().size(4) } }.class(menu)
            }"#,
            &[("s", styles)],
        )
        .unwrap();
        // Экран не Menu — вложенные стили не действуют.
        assert!(el(&ui, &[0, 0]).look.background.is_none());
        let ui = with_files(
            r#"head { styles("s") }
            screen Menu { Stack { Column { Rect().size(4) } } }"#,
            &[("s", "style Menu { Rect { .background(#0B0B0B) } }")],
        )
        .unwrap();
        assert_eq!(bg(el(&ui, &[0, 0, 0])), [11, 11, 11]);
    }

    #[test]
    fn variables_come_from_theme_and_can_be_overridden_inside() {
        let styles = r##"
            style Rect { .background(var(accent)).radius(var(gap_big)) }
            style ice { .var(accent: #6BB8F5) }
        "##;
        let mut ui = with_files(
            r#"head { theme("themes/base") styles("styles/s") }
            screen S {
                Column(spacing: var(gap)) { Rect().size(4) }
                Column { Stack { Rect().size(4) } }.class(ice)
            }"#,
            &[("themes/base", THEMES), ("styles/s", styles)],
        )
        .unwrap();
        assert_eq!(ui.themes(), ["Forge", "Ice"]);
        assert_eq!(ui.theme(), Some("Forge"));
        assert_eq!(bg(el(&ui, &[0, 0])), [0xF5, 0xA5, 0x4A]);
        assert_eq!(el(&ui, &[0, 0]).look.radius, 24.0);
        assert_eq!(el(&ui, &[0]).placement.spacing, 12.0);
        // Переопределение действует на всё внутри, на любой глубине.
        assert_eq!(bg(el(&ui, &[1, 0, 0])), [0x6B, 0xB8, 0xF5]);
        // Смена темы — другие значения, новая сборка (и атлас букв).
        let g = ui.generation();
        assert!(ui.set_theme("Ice").unwrap());
        assert!(ui.generation() > g);
        assert_eq!(bg(el(&ui, &[0, 0])), [0x6B, 0xB8, 0xF5]);
        assert_eq!(el(&ui, &[0]).placement.spacing, 8.0);
        assert!(!ui.set_theme("Нет").unwrap());
    }

    #[test]
    fn text_inherits_font_and_color() {
        let ui = UiScreen::parse(
            r##"screen S {
                Column { Text("a") Text("b").color(#FF0000) Stack { Text("c") } }
                    .font(40).color(#00FF00)
            }"##,
        )
        .unwrap();
        let spec = |e: &Element| match &e.kind {
            Kind::Text(t) => (t.size, t.color.g > 0.9, t.color.r > 0.9),
            _ => panic!("ждали текст"),
        };
        assert_eq!(spec(el(&ui, &[0, 0])), (40.0, true, false));
        assert_eq!(spec(el(&ui, &[0, 1])), (40.0, false, true));
        assert_eq!(spec(el(&ui, &[0, 2, 0])), (40.0, true, false));
    }

    #[test]
    fn style_errors_name_the_file_and_line() {
        let e = with_files(
            r#"head { styles("s") }
            screen S { Rect() }"#,
            &[("s", "style Rect {\n  .backgrond(#000000)\n}")],
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.starts_with("s.rui: строка 2"), "{e}");
        assert!(e.contains("Может, .background?"), "{e}");
        let e = with_files(r#"head { styles("нет") } screen S { }"#, &[])
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("нет.rui: нет такого файла"), "{e}");
        let e = with_files(
            r#"head { theme("t") } screen S { Rect().background(var(acent)) }"#,
            &[("t", THEMES)],
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("Может, var(accent)?"), "{e}");
        let e = with_files(
            r#"head { theme("t") } screen S { }"#,
            &[("t", "theme A { a = var(b) b = var(a) }")],
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("по кругу: a → b → a"), "{e}");
        let e = with_files(
            r#"head { styles("s") } screen S { }"#,
            &[("s", "screen X { }")],
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("пишется в файле экрана"), "{e}");
        let e = with_files(
            r#"head { styles("s") } screen S { Rect() }"#,
            &[("s", "style Rect { .class(big) }")],
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains(".class пишется у блока"), "{e}");
        let e = UiScreen::parse(r#"head { styles("s") } screen S { }"#)
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("экран дан текстом"), "{e}");
    }

    #[test]
    fn load_reads_files_from_the_folder() {
        let dir = std::env::temp_dir().join(format!("reyvec-ui-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("styles")).unwrap();
        std::fs::write(
            dir.join("a.rui"),
            r#"head { styles("styles/s") } screen A { Rect().size(4) }"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("styles/s.rui"),
            "style Rect { .background(#070707) }",
        )
        .unwrap();
        let ui = UiScreen::load(&dir, "a.rui").unwrap();
        assert_eq!(bg(el(&ui, &[0])), [7, 7, 7]);
        assert_eq!(ui.files().len(), 2);
        assert!(ui.files()[1].ends_with("styles/s.rui"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_demo_file_works() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
        let mut ui = UiScreen::load(&dir, "demo.rui").unwrap();
        assert!(ui.themes().len() >= 2, "{:?}", ui.themes());
        assert!(!ui.draw([1920.0, 1080.0]).quads.is_empty());
        let other = ui.themes()[1].to_owned();
        assert!(ui.set_theme(&other).unwrap());
        assert!(!ui.draw([1920.0, 1080.0]).quads.is_empty());
    }

    const MENU: &str = r##"
        style Button {
            .background(#101010).padding(10)
            .hover { .background(#202020).color(#FF0000) }
            .pressed { .background(#303030) }
            .focus { .border(#FFFFFF, 2) }
            .disabled { .opacity(0.5) }
        }
        screen M {
            Column {
                Button("Играть").id(play).size(200, 40)
                Button("Настройки").size(200, 40)
                Button("Выход").size(200, 40).disabled()
                Button("Мой цвет").size(200, 40).background(#050505)
            }.align(top_left)
        }
    "##;

    fn center(ui: &UiScreen, i: usize) -> [f32; 2] {
        let r = ui.draw.targets[i].rect;
        [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]
    }

    fn text_color(e: &Element) -> [bool; 3] {
        match &e.kind {
            Kind::Text(t) => [t.color.r > 0.9, t.color.g > 0.9, t.color.b > 0.9],
            _ => panic!("ждали текст"),
        }
    }

    #[test]
    fn hover_and_press_restyle_the_button_and_its_text() {
        let mut ui = UiScreen::parse(MENU).unwrap();
        ui.draw([1920.0, 1080.0]);
        assert_eq!(ui.draw.targets.len(), 4);
        assert_eq!(ui.draw.targets[0].name, "play");
        assert_eq!(ui.draw.targets[1].name, "Настройки");
        assert_eq!(bg(el(&ui, &[0, 0])), [16, 16, 16]);
        // Мышь над первой кнопкой: её фон и цвет надписи внутри меняются.
        assert!(ui.pointer_move(center(&ui, 0)));
        ui.draw([1920.0, 1080.0]);
        assert_eq!(bg(el(&ui, &[0, 0])), [32, 32, 32]);
        assert_eq!(text_color(el(&ui, &[0, 0, 0])), [true, false, false]);
        // Наведённая кнопка ещё и выбрана: рамка из .focus.
        assert!(el(&ui, &[0, 0]).look.border.is_some());
        assert_eq!(bg(el(&ui, &[0, 1])), [16, 16, 16]);
        // Нажатие: свой вид, а отпускание над той же кнопкой — событие.
        assert!(ui.pointer_down());
        ui.draw([1920.0, 1080.0]);
        assert_eq!(bg(el(&ui, &[0, 0])), [48, 48, 48]);
        assert_eq!(ui.pointer_up(), Some(UiEvent::Click("play".into())));
        ui.draw([1920.0, 1080.0]);
        assert_eq!(bg(el(&ui, &[0, 0])), [32, 32, 32]);
        // Нажал и увёл мышь — не нажатие.
        assert!(ui.pointer_down());
        ui.pointer_move(center(&ui, 1));
        assert_eq!(ui.pointer_up(), None);
        // Мимо кнопок: нажатие не интерфейса.
        ui.pointer_move([1900.0, 1000.0]);
        assert!(!ui.pointer_down());
        // Свой фон у блока сильнее наведения из стиля (ступень важнее состояния).
        ui.pointer_move(center(&ui, 3));
        ui.draw([1920.0, 1080.0]);
        assert_eq!(bg(el(&ui, &[0, 3])), [5, 5, 5]);
    }

    #[test]
    fn disabled_buttons_are_skipped() {
        let mut ui = UiScreen::parse(MENU).unwrap();
        ui.draw([1920.0, 1080.0]);
        assert_eq!(el(&ui, &[0, 2]).look.opacity, 0.5);
        assert!(!ui.pointer_move(center(&ui, 2)));
        assert_eq!(ui.key(UiKey::Next), None);
        assert_eq!(ui.focused().as_deref(), Some("play"));
        ui.key(UiKey::Next);
        ui.key(UiKey::Next);
        assert_eq!(ui.focused().as_deref(), Some("Мой цвет"));
        ui.key(UiKey::Next);
        assert_eq!(ui.focused().as_deref(), Some("play"));
        ui.key(UiKey::Prev);
        assert_eq!(
            ui.key(UiKey::Activate),
            Some(UiEvent::Click("Мой цвет".into()))
        );
        ui.draw([1920.0, 1080.0]);
        assert!(el(&ui, &[0, 3]).look.border.is_some());
        assert!(el(&ui, &[0, 0]).look.border.is_none());
    }

    #[test]
    fn place_among_siblings() {
        let ui = UiScreen::parse(
            r##"screen S { Column {
                Rect().size(4).first { .background(#010101) }
                Rect().size(4).even { .background(#020202) }
                Rect().size(4).nth(3) { .background(#030303) }
                Rect().size(4).last { .background(#040404) }
            } }"##,
        )
        .unwrap();
        assert_eq!(bg(el(&ui, &[0, 0])), [1, 1, 1]);
        assert_eq!(bg(el(&ui, &[0, 1])), [2, 2, 2]);
        assert_eq!(bg(el(&ui, &[0, 2])), [3, 3, 3]);
        assert_eq!(bg(el(&ui, &[0, 3])), [4, 4, 4]);
        // Нечётные тексты: первый и третий.
        let ui = UiScreen::parse(
            r#"style Text { .odd { .color(#00FF00) } }
            screen S { Column { Text("a") Text("b") Text("c") } }"#,
        )
        .unwrap();
        assert_eq!(text_color(el(&ui, &[0, 0])), [false, true, false]);
        assert_eq!(text_color(el(&ui, &[0, 1])), [true, true, true]);
    }

    #[test]
    fn state_mistakes_are_found_when_the_file_is_saved() {
        let e = |src: &str| UiScreen::parse(src).err().unwrap().to_string();
        let m = e("screen S { Button(\"a\").hover { .font(30) } }");
        assert!(m.contains(".font в состоянии пока нельзя"), "{m}");
        let m = e("screen S { Button(\"a\").hover { .backgrund(#000000) } }");
        assert!(m.contains("Может, .background?"), "{m}");
        let m = e("screen S { Button(\"a\").hover }");
        assert!(m.contains("в фигурных скобках"), "{m}");
        let m = e("screen S { Button(\"a\").padding(4) { .opacity(1) } }");
        assert!(m.contains("не состояние"), "{m}");
        let m = e("screen S { Button(12) }");
        assert!(m.contains("надпись в кавычках"), "{m}");
    }

    #[test]
    fn when_follows_the_window_language_and_input() {
        let mut ui = UiScreen::parse(
            r##"
            style Rect {
                .background(#010101)
                when(width < 1280) { .background(#020202) }
            }
            screen S {
                Rect().size(10)
                Rect().size(10).when(lang: en) { .background(#030303) }
                Rect().size(10).when(input: keyboard, height >= 700) { .background(#040404) }
                Button("a").size(10)
            }
            "##,
        )
        .unwrap();
        ui.draw([1920.0, 1080.0]);
        assert_eq!(bg(el(&ui, &[0])), [1, 1, 1]);
        ui.draw([1000.0, 600.0]);
        assert_eq!(bg(el(&ui, &[0])), [2, 2, 2]);
        ui.draw([1920.0, 1080.0]);
        assert_eq!(bg(el(&ui, &[0])), [1, 1, 1]);
        assert_eq!(bg(el(&ui, &[1])), [1, 1, 1]);
        ui.set_lang("en").unwrap();
        ui.draw([1920.0, 1080.0]);
        assert_eq!(bg(el(&ui, &[1])), [3, 3, 3]);
        // Клавиша интерфейса — значит, игрок на клавиатуре.
        assert_eq!(bg(el(&ui, &[2])), [1, 1, 1]);
        ui.key(UiKey::Next);
        ui.draw([1920.0, 1080.0]);
        assert_eq!(bg(el(&ui, &[2])), [4, 4, 4]);
        ui.draw([1920.0, 600.0]);
        assert_eq!(bg(el(&ui, &[2])), [1, 1, 1]);
        let e = UiScreen::parse("screen S { Rect().when(depth < 3) { .opacity(1) } }")
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains(".when: width < 1280"), "{e}");
    }

    fn texts() -> Texts {
        let mut t = Texts::default();
        t.add(
            "ru",
            r#"(name: "Русский", texts: { "play": "Играть", "coins": "Монеты: {n}", "quit": "Выход" })"#,
            "lang/ru.ron",
        )
        .unwrap();
        t.add(
            "en",
            r#"(name: "English", texts: { "play": "Play", "coins": "Coins: {n}" })"#,
            "lang/en.ron",
        )
        .unwrap();
        t
    }

    fn label(ui: &UiScreen, path: &[usize]) -> String {
        first_text(el(ui, path)).unwrap()
    }

    #[test]
    fn tr_takes_words_from_the_language_and_keeps_focus() {
        let mut ui = UiScreen::parse(
            r#"screen S {
                Column {
                    Button(tr("play")).size(200, 40)
                    Button(tr("quit")).size(200, 40)
                    Text(tr("coins", n: 12))
                }
            }"#,
        )
        .unwrap();
        // Без файлов языков виден сам ключ.
        assert_eq!(label(&ui, &[0, 0]), "play");
        ui.set_texts(texts()).unwrap();
        assert_eq!(label(&ui, &[0, 0]), "Играть");
        assert_eq!(label(&ui, &[0, 2]), "Монеты: 12");
        ui.draw([1920.0, 1080.0]);
        ui.key(UiKey::Next);
        assert_eq!(ui.focused().as_deref(), Some("Играть"));
        let before = ui.draw([1920.0, 1080.0]).targets[0].rect;
        ui.set_lang("en").unwrap();
        assert_eq!(ui.lang(), "en");
        assert_eq!(label(&ui, &[0, 0]), "Play");
        assert_eq!(label(&ui, &[0, 2]), "Coins: 12");
        // В английском нет «quit»: слово главного языка.
        assert_eq!(label(&ui, &[0, 1]), "Выход");
        assert_eq!(ui.draw([1920.0, 1080.0]).targets[0].rect, before);
        // Фокус остался на той же кнопке, у которой теперь другое имя.
        assert_eq!(ui.focused().as_deref(), Some("Play"));
        assert_eq!(
            ui.languages().collect::<Vec<_>>(),
            [("ru", "Русский"), ("en", "English")]
        );
    }

    #[test]
    fn missing_key_is_an_error_and_keeps_the_old_words() {
        let mut ui = UiScreen::parse("screen S {\n Text(tr(\"pley\"))\n}").unwrap();
        let e = ui.set_texts(texts()).unwrap_err().to_string();
        assert!(
            e.starts_with("строка 2") && e.contains("tr(\"play\")"),
            "{e}"
        );
        assert_eq!(label(&ui, &[0]), "pley");
        let e = UiScreen::parse("screen S { Text(tr(\"a\", 5)) }")
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("с именем"), "{e}");
    }

    #[test]
    fn bar_text_and_conditions_follow_game_data() {
        let mut ui = UiScreen::parse(
            r##"screen Hud {
                Column(spacing: 4) {
                    Bar(game.hero.hp / game.hero.hp_max)
                        .size(200, 10).background(#101010).fill(#E04040)
                        .when(game.hero.hp < 2) { .fill(#FF0000) }
                    Text("Монеты: \(game.hero.coins)")
                    Text(tr("coins", n: game.hero.coins * 2))
                    Rect().size(10).background(#202020).visible(game.hero.hp > 0)
                }.align(top_left)
            }"##,
        )
        .unwrap();
        ui.set_texts(texts()).unwrap();
        ui.set_number("game.hero.hp", 3.0);
        ui.set_number("game.hero.hp_max", 4.0);
        ui.set_number("game.hero.coins", 7.0);
        let fill = |ui: &mut UiScreen| {
            let d = ui.draw([1920.0, 1080.0]);
            let b = blocks(d);
            (b[1].rect[2] - b[1].rect[0], b[1].color, b.len())
        };
        ui.draw([1920.0, 1080.0]);
        assert!(ui.problem().is_none(), "{:?}", ui.problem());
        let (w, c, n) = fill(&mut ui);
        assert!((w - 150.0).abs() < 0.5, "{w}");
        assert!(c[1] > 0.01, "обычный цвет: {c:?}");
        assert_eq!(n, 3, "фон полоски, заполнение и квадрат");
        assert_eq!(label(&ui, &[0, 1]), "Монеты: 7");
        assert_eq!(label(&ui, &[0, 2]), "Монеты: 14");
        let before = ui.generation();
        ui.set_number("game.hero.hp", 1.0);
        ui.set_number("game.hero.coins", 12.0);
        let (w, c, _) = fill(&mut ui);
        assert!((w - 50.0).abs() < 0.5, "{w}");
        assert!(c[1] < 0.001, "мало здоровья — красная: {c:?}");
        assert_eq!(label(&ui, &[0, 1]), "Монеты: 12");
        assert_ne!(ui.generation(), before, "новые буквы — новый атлас");
        ui.set_number("game.hero.hp", 0.0);
        let n = blocks(ui.draw([1920.0, 1080.0])).len();
        assert_eq!(n, 1, "пустая полоска без заполнения, квадрат спрятан");
        assert!(ui.problem().is_none());
        // Опечатка в пути видна, когда игра дала данные.
        let mut bad = UiScreen::parse("screen S { Bar(game.hero.hpp) }").unwrap();
        bad.set_number("game.hero.hp", 1.0);
        bad.draw([1920.0, 1080.0]);
        let e = bad.problem().unwrap().to_string();
        assert!(e.contains("Может, game.hero.hp?"), "{e}");
    }
}
