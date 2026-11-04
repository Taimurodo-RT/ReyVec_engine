//! Управление (6.10б, 6.12): какие клавиши и кнопки мыши дают какое действие, как
//! ведёт себя управление и какая схема получает нажатие.
//!
//! Три слоя (решение 0015):
//!
//! - **Действия** ([`Action`]) — что умеет игра: «влево», «прыжок», «удар». Игра
//!   спрашивает только действия, не клавиши.
//! - **Привязки** — источник ([`Input`]: клавиша, кнопка мыши, колесо) → действие. У
//!   действия до [`MAX_KEYS`] привязок, источник принадлежит одному действию.
//! - **Схемы** ([`Scheme`]) — наборы действий для «Консоли», «Меню», «Игры» и
//!   «Песочницы», лежат стопкой: нажатие получает верхняя, у которой оно что-то
//!   значит ([`Controls::route`]).
//!
//! Кроме привязок есть **поведение** ([`OPTIONS`]): выборы вроде «удар мышью — в
//! сторону курсора или куда смотрит герой». Всё описано данными, чтобы меню и будущий
//! визуальный редактор строились по этим таблицам. Файл `settings/controls.ron` рядом с
//! программой читается человеком:
//!
//! ```text
//! (
//!     клавиши: {"прыжок": ["Space", "KeyW"], "удар": ["KeyA", "МышьЛевая"]},
//!     поведение: {"удар мышью": "к курсору", "камера в игре": "средняя"},
//! )
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use reyvec_scene::game::Action;
use serde::{Deserialize, Serialize};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

/// Сколько привязок может быть у одного действия.
pub(crate) const MAX_KEYS: usize = 4;

/// Кнопка мыши, которую можно назначить.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MouseKey {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl MouseKey {
    pub(crate) const ALL: [Self; 5] = [
        Self::Left,
        Self::Right,
        Self::Middle,
        Self::Back,
        Self::Forward,
    ];

    pub(crate) fn from_winit(b: MouseButton) -> Option<Self> {
        Some(match b {
            MouseButton::Left => Self::Left,
            MouseButton::Right => Self::Right,
            MouseButton::Middle => Self::Middle,
            MouseButton::Back => Self::Back,
            MouseButton::Forward => Self::Forward,
            MouseButton::Other(_) => return None,
        })
    }

    fn file(self) -> &'static str {
        match self {
            Self::Left => "МышьЛевая",
            Self::Right => "МышьПравая",
            Self::Middle => "МышьСредняя",
            Self::Back => "МышьНазад",
            Self::Forward => "МышьВперёд",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Left => "Левая кнопка мыши",
            Self::Right => "Правая кнопка мыши",
            Self::Middle => "Средняя кнопка мыши",
            Self::Back => "Боковая кнопка «назад»",
            Self::Forward => "Боковая кнопка «вперёд»",
        }
    }
}

/// Щелчок колеса.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Wheel {
    Up,
    Down,
}

/// Источник ввода: клавиша (по месту на клавиатуре), кнопка мыши или щелчок колеса.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Input {
    Key(KeyCode),
    Mouse(MouseKey),
    Wheel(Wheel),
}

impl Input {
    /// Имя в меню (`None` — назначить нельзя: Esc, F1, F12, `~` служебные).
    pub(crate) fn label(self) -> Option<&'static str> {
        match self {
            Self::Key(k) => key_name(k),
            Self::Mouse(m) => Some(m.label()),
            Self::Wheel(Wheel::Up) => Some("Колесо вверх"),
            Self::Wheel(Wheel::Down) => Some("Колесо вниз"),
        }
    }

    /// Имя в файле настроек.
    pub(crate) fn file(self) -> String {
        match self {
            Self::Key(k) => format!("{k:?}"),
            Self::Mouse(m) => m.file().to_string(),
            Self::Wheel(Wheel::Up) => "КолесоВверх".into(),
            Self::Wheel(Wheel::Down) => "КолесоВниз".into(),
        }
    }

    /// Источник по имени из файла.
    pub(crate) fn parse(name: &str) -> Option<Self> {
        if let Some(m) = MouseKey::ALL.into_iter().find(|m| m.file() == name) {
            return Some(Self::Mouse(m));
        }
        match name {
            "КолесоВверх" => Some(Self::Wheel(Wheel::Up)),
            "КолесоВниз" => Some(Self::Wheel(Wheel::Down)),
            _ => parse_key(name).map(Self::Key),
        }
    }

    /// Мышь (кнопка или колесо): по ней удар может быть «к курсору».
    pub(crate) fn is_mouse(self) -> bool {
        !matches!(self, Self::Key(_))
    }
}

/// Один выбор поведения: имя в файле и в меню.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Choice {
    pub file: &'static str,
    pub label: &'static str,
}

/// Настройка поведения управления: имя, подсказка, варианты и какой по умолчанию.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OptionDesc {
    pub file: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub choices: &'static [Choice],
    pub default: usize,
}

/// Куда бьёт удар, нажатый мышью (номер в [`OPTIONS`]).
pub(crate) const ATTACK_AIM: usize = 0;
/// Какой кнопкой мыши тащить камеру в игре.
pub(crate) const CAMERA_DRAG: usize = 1;

/// Все настройки поведения. Порядок — как в меню; номера — константы выше.
pub(crate) const OPTIONS: &[OptionDesc] = &[
    OptionDesc {
        file: "удар мышью",
        label: "Удар мышью",
        hint: "Куда бьёт герой, когда удар нажат кнопкой мыши. Удар клавишей — всегда туда, куда он смотрит.",
        choices: &[
            Choice {
                file: "к курсору",
                label: "в сторону курсора",
            },
            Choice {
                file: "по взгляду",
                label: "куда смотрит герой",
            },
        ],
        default: 0,
    },
    OptionDesc {
        file: "камера в игре",
        label: "Камера в игре",
        hint: "Какой кнопкой мыши отодвигать камеру, пока идёт игра. Если кнопка назначена действию, действие главнее.",
        choices: &[
            Choice {
                file: "средняя",
                label: "средняя кнопка",
            },
            Choice {
                file: "правая",
                label: "правая кнопка",
            },
            Choice {
                file: "левая",
                label: "левая кнопка",
            },
            Choice {
                file: "нет",
                label: "не двигать",
            },
        ],
        default: 0,
    },
];

/// Схема управления: набор действий для одной ситуации.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scheme {
    /// Строка консоли разработчика: забирает всё.
    Console,
    /// Меню паузы: стрелки, выбор, назад; мышь — кнопкам интерфейса. Забирает всё.
    Menu,
    /// Игра: действия героя, пауза, консоль, камера мышью.
    Game,
    /// Песочница (всегда внизу): сцены, панель, снимок, камера.
    Sandbox,
}

impl Scheme {
    /// Название схемы для людей (консоль, будущий редактор).
    #[cfg(test)]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Console => "Консоль",
            Self::Menu => "Меню",
            Self::Game => "Игра",
            Self::Sandbox => "Песочница",
        }
    }
}

/// Стопка схем сейчас, верхняя первой.
pub(crate) fn schemes(in_game: bool, menu: bool, console: bool) -> Vec<Scheme> {
    let mut s = Vec::new();
    if in_game && console {
        s.push(Scheme::Console);
    }
    if in_game && menu {
        s.push(Scheme::Menu);
    }
    if in_game {
        s.push(Scheme::Game);
    }
    s.push(Scheme::Sandbox);
    s
}

/// Действие меню.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuAction {
    Up,
    Down,
    Select,
    Back,
}

/// Клавиши меню (пока не настраиваются: без них из меню не выйти).
pub(crate) const MENU_KEYS: &[(MenuAction, &[KeyCode])] = &[
    (MenuAction::Up, &[KeyCode::ArrowUp, KeyCode::KeyW]),
    (MenuAction::Down, &[KeyCode::ArrowDown, KeyCode::KeyS]),
    (
        MenuAction::Select,
        &[KeyCode::Enter, KeyCode::NumpadEnter, KeyCode::Space],
    ),
    (MenuAction::Back, &[KeyCode::Escape]),
];

/// Кому достаётся нажатие.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// Интерфейсу (egui): ввод текста, кнопки под мышью.
    Ui,
    OpenConsole,
    CloseConsole,
    OpenMenu,
    Menu(MenuAction),
    Game(Action),
    /// Тащить камеру (кнопка из настройки «камера в игре»).
    Camera,
    /// Клавишам песочницы.
    Sandbox,
}

/// Привязки и поведение.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Controls {
    binds: BTreeMap<ActionKey, Vec<Input>>,
    options: Vec<usize>,
}

/// Действие как ключ упорядоченной таблицы (в порядке [`Action::ALL`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ActionKey(usize);

impl ActionKey {
    fn of(a: Action) -> Self {
        Self(Action::ALL.iter().position(|x| *x == a).unwrap_or(0))
    }

    fn action(self) -> Action {
        Action::ALL[self.0]
    }
}

/// Файл настроек управления.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ControlsFile {
    #[serde(rename = "клавиши", default)]
    keys: BTreeMap<String, Vec<String>>,
    #[serde(rename = "поведение", default)]
    options: BTreeMap<String, String>,
}

impl Default for Controls {
    fn default() -> Self {
        use KeyCode as K;
        let mut c = Self {
            binds: BTreeMap::new(),
            options: OPTIONS.iter().map(|o| o.default).collect(),
        };
        let key = Input::Key;
        for (a, inputs) in [
            (Action::Left, vec![key(K::ArrowLeft)]),
            (Action::Right, vec![key(K::ArrowRight)]),
            (Action::Up, vec![key(K::ArrowUp)]),
            (Action::Down, vec![key(K::ArrowDown)]),
            (Action::Jump, vec![key(K::Space)]),
            (
                Action::Dash,
                vec![
                    key(K::ShiftLeft),
                    key(K::ShiftRight),
                    key(K::KeyX),
                    Input::Mouse(MouseKey::Right),
                ],
            ),
            (
                Action::Attack,
                vec![
                    key(K::ControlLeft),
                    key(K::ControlRight),
                    key(K::KeyA),
                    Input::Mouse(MouseKey::Left),
                ],
            ),
        ] {
            c.binds.insert(ActionKey::of(a), inputs);
        }
        c
    }
}

impl Controls {
    /// Действие источника `input` в игре.
    pub(crate) fn action_of(&self, input: Input) -> Option<Action> {
        self.binds
            .iter()
            .find(|(_, inputs)| inputs.contains(&input))
            .map(|(a, _)| a.action())
    }

    /// Действие клавиши `code`.
    #[cfg(test)]
    pub(crate) fn action(&self, code: KeyCode) -> Option<Action> {
        self.action_of(Input::Key(code))
    }

    /// Привязки действия.
    pub(crate) fn inputs(&self, a: Action) -> &[Input] {
        self.binds
            .get(&ActionKey::of(a))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Клавиши действия (без мыши).
    #[cfg(test)]
    pub(crate) fn keys(&self, a: Action) -> Vec<KeyCode> {
        self.inputs(a)
            .iter()
            .filter_map(|i| match i {
                Input::Key(k) => Some(*k),
                _ => None,
            })
            .collect()
    }

    /// Назначает источник действию. `replace` — вместо всех его привязок, иначе в
    /// добавку (самая старая уходит, если их уже [`MAX_KEYS`]). У другого действия этот
    /// источник пропадает. `false` — назначить нельзя (служебная клавиша).
    pub(crate) fn bind(&mut self, a: Action, input: Input, replace: bool) -> bool {
        if input.label().is_none() {
            return false;
        }
        for inputs in self.binds.values_mut() {
            inputs.retain(|i| *i != input);
        }
        let inputs = self.binds.entry(ActionKey::of(a)).or_default();
        if replace {
            inputs.clear();
        }
        inputs.push(input);
        if inputs.len() > MAX_KEYS {
            inputs.remove(0);
        }
        true
    }

    /// Действия без единой привязки (о них предупреждает меню).
    pub(crate) fn unbound(&self) -> Vec<Action> {
        Action::ALL
            .into_iter()
            .filter(|a| self.inputs(*a).is_empty())
            .collect()
    }

    /// Выбор в настройке поведения `i` (номер варианта в [`OPTIONS`]).
    pub(crate) fn option(&self, i: usize) -> usize {
        self.options.get(i).copied().unwrap_or(OPTIONS[i].default)
    }

    pub(crate) fn set_option(&mut self, i: usize, choice: usize) {
        if choice < OPTIONS[i].choices.len() {
            self.options[i] = choice;
        }
    }

    /// Удар мышью — в сторону курсора.
    pub(crate) fn attack_toward_cursor(&self) -> bool {
        self.option(ATTACK_AIM) == 0
    }

    /// Кнопка мыши, которой в игре тащат камеру.
    pub(crate) fn camera_button(&self) -> Option<MouseKey> {
        match self.option(CAMERA_DRAG) {
            0 => Some(MouseKey::Middle),
            1 => Some(MouseKey::Right),
            2 => Some(MouseKey::Left),
            _ => None,
        }
    }

    /// Кому достаётся нажатие `input` при стопке схем `stack` (верхняя первой).
    pub(crate) fn route(&self, stack: &[Scheme], input: Input) -> Route {
        for scheme in stack {
            match scheme {
                Scheme::Console => {
                    return match input {
                        Input::Key(KeyCode::Backquote | KeyCode::Escape) => Route::CloseConsole,
                        _ => Route::Ui,
                    };
                }
                Scheme::Menu => {
                    if let Input::Key(k) = input
                        && let Some((a, _)) = MENU_KEYS.iter().find(|(_, keys)| keys.contains(&k))
                    {
                        return Route::Menu(*a);
                    }
                    return Route::Ui;
                }
                Scheme::Game => {
                    match input {
                        Input::Key(KeyCode::Escape) => return Route::OpenMenu,
                        Input::Key(KeyCode::Backquote) => return Route::OpenConsole,
                        _ => {}
                    }
                    if let Some(a) = self.action_of(input) {
                        return Route::Game(a);
                    }
                    if let Input::Mouse(m) = input
                        && Some(m) == self.camera_button()
                    {
                        return Route::Camera;
                    }
                    // Мышь в игре песочнице не достаётся (иначе левая кнопка тащила бы
                    // камеру), а колесо — достаётся: приближение.
                    if matches!(input, Input::Mouse(_)) {
                        return Route::Ui;
                    }
                }
                Scheme::Sandbox => return Route::Sandbox,
            }
        }
        Route::Sandbox
    }

    /// Читает настройки из текста файла. Неизвестные имена пропускаются с
    /// предупреждением; чего нет в файле — по умолчанию.
    pub(crate) fn from_text(text: &str) -> Result<(Self, Vec<String>), String> {
        let file: ControlsFile = ron::from_str(text).map_err(|e| e.to_string())?;
        let mut out = Self::default();
        let mut warnings = Vec::new();
        for (name, inputs) in &file.keys {
            let Some(a) = Action::parse(name) else {
                warnings.push(format!("нет такого действия «{name}»"));
                continue;
            };
            out.binds.insert(ActionKey::of(a), Vec::new());
            for k in inputs {
                match Input::parse(k) {
                    Some(input) => {
                        out.bind(a, input, false);
                    }
                    None => warnings.push(format!("нет такой клавиши «{k}» у «{name}»")),
                }
            }
        }
        for (name, value) in &file.options {
            let Some(i) = OPTIONS.iter().position(|o| o.file == name) else {
                warnings.push(format!("нет такой настройки «{name}»"));
                continue;
            };
            match OPTIONS[i].choices.iter().position(|c| c.file == value) {
                Some(c) => out.options[i] = c,
                None => warnings.push(format!("у «{name}» нет варианта «{value}»")),
            }
        }
        Ok((out, warnings))
    }

    /// Текст файла настроек.
    pub(crate) fn to_text(&self) -> String {
        let file = ControlsFile {
            keys: Action::ALL
                .into_iter()
                .map(|a| {
                    let inputs = self.inputs(a).iter().map(|i| i.file()).collect();
                    (a.key().to_string(), inputs)
                })
                .collect(),
            options: OPTIONS
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    (
                        o.file.to_string(),
                        o.choices[self.option(i)].file.to_string(),
                    )
                })
                .collect(),
        };
        let pretty = ron::ser::PrettyConfig::new().struct_names(false);
        ron::ser::to_string_pretty(&file, pretty).unwrap_or_default()
    }
    /// Читает настройки из `path`. Нет файла — клавиши по умолчанию, файл с ошибкой —
    /// тоже, с записью в журнал.
    pub(crate) fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Self::default(),
        };
        match Self::from_text(&text) {
            Ok((c, warnings)) => {
                for w in warnings {
                    tracing::warn!("Управление ({}): {w}", path.display());
                }
                tracing::info!("Управление: клавиши из {}", path.display());
                c
            }
            Err(e) => {
                tracing::warn!(
                    "Управление: файл {} не прочитан ({e}), клавиши по умолчанию",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Пишет настройки в `path`.
    pub(crate) fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_text())
    }
}

/// Где лежит файл настроек: `settings/controls.ron` рядом с программой.
pub(crate) fn controls_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join("settings")
        .join("controls.ron")
}

/// Клавиши, которые можно назначить, и их имена в меню. Esc, F1 и F12 служебные.
const KEYS: &[(KeyCode, &str)] = {
    use KeyCode as K;
    &[
        (K::ArrowLeft, "←"),
        (K::ArrowRight, "→"),
        (K::ArrowUp, "↑"),
        (K::ArrowDown, "↓"),
        (K::Space, "Пробел"),
        (K::Enter, "Enter"),
        (K::Tab, "Tab"),
        (K::Backspace, "Backspace"),
        (K::ShiftLeft, "Левый Shift"),
        (K::ShiftRight, "Правый Shift"),
        (K::ControlLeft, "Левый Ctrl"),
        (K::ControlRight, "Правый Ctrl"),
        (K::AltLeft, "Левый Alt"),
        (K::AltRight, "Правый Alt"),
        (K::KeyA, "A"),
        (K::KeyB, "B"),
        (K::KeyC, "C"),
        (K::KeyD, "D"),
        (K::KeyE, "E"),
        (K::KeyF, "F"),
        (K::KeyG, "G"),
        (K::KeyH, "H"),
        (K::KeyI, "I"),
        (K::KeyJ, "J"),
        (K::KeyK, "K"),
        (K::KeyL, "L"),
        (K::KeyM, "M"),
        (K::KeyN, "N"),
        (K::KeyO, "O"),
        (K::KeyP, "P"),
        (K::KeyQ, "Q"),
        (K::KeyR, "R"),
        (K::KeyS, "S"),
        (K::KeyT, "T"),
        (K::KeyU, "U"),
        (K::KeyV, "V"),
        (K::KeyW, "W"),
        (K::KeyX, "X"),
        (K::KeyY, "Y"),
        (K::KeyZ, "Z"),
        (K::Digit0, "0"),
        (K::Digit1, "1"),
        (K::Digit2, "2"),
        (K::Digit3, "3"),
        (K::Digit4, "4"),
        (K::Digit5, "5"),
        (K::Digit6, "6"),
        (K::Digit7, "7"),
        (K::Digit8, "8"),
        (K::Digit9, "9"),
        (K::Numpad0, "Цифр. 0"),
        (K::Numpad1, "Цифр. 1"),
        (K::Numpad2, "Цифр. 2"),
        (K::Numpad3, "Цифр. 3"),
        (K::Numpad4, "Цифр. 4"),
        (K::Numpad5, "Цифр. 5"),
        (K::Numpad6, "Цифр. 6"),
        (K::Numpad7, "Цифр. 7"),
        (K::Numpad8, "Цифр. 8"),
        (K::Numpad9, "Цифр. 9"),
        (K::Comma, ","),
        (K::Period, "."),
        (K::Slash, "/"),
        (K::Semicolon, ";"),
        (K::Quote, "'"),
        (K::BracketLeft, "["),
        (K::BracketRight, "]"),
        (K::Minus, "-"),
        (K::Equal, "="),
    ]
};

/// Имя клавиши в меню (`None` — её назначить нельзя).
pub(crate) fn key_name(code: KeyCode) -> Option<&'static str> {
    KEYS.iter().find(|(k, _)| *k == code).map(|(_, n)| *n)
}

/// Клавиша по имени из файла (`KeyA`, `Space`).
fn parse_key(name: &str) -> Option<KeyCode> {
    KEYS.iter()
        .map(|(k, _)| *k)
        .find(|k| format!("{k:?}") == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_old_keys() {
        let c = Controls::default();
        assert_eq!(c.action(KeyCode::Space), Some(Action::Jump));
        assert_eq!(c.action(KeyCode::KeyX), Some(Action::Dash));
        assert_eq!(c.action(KeyCode::KeyA), Some(Action::Attack));
        assert_eq!(c.action(KeyCode::KeyQ), None);
        assert!(c.unbound().is_empty());
    }

    #[test]
    fn a_key_moves_from_its_old_action() {
        let mut c = Controls::default();
        assert!(c.bind(Action::Jump, Input::Key(KeyCode::KeyX), false));
        assert_eq!(c.action(KeyCode::KeyX), Some(Action::Jump));
        assert_eq!(
            c.keys(Action::Dash),
            &[KeyCode::ShiftLeft, KeyCode::ShiftRight]
        );
        assert_eq!(c.keys(Action::Jump), &[KeyCode::Space, KeyCode::KeyX]);
        // Лишняя клавиша вытесняет самую старую.
        c.bind(Action::Jump, Input::Key(KeyCode::KeyW), false);
        c.bind(Action::Jump, Input::Key(KeyCode::KeyE), false);
        c.bind(Action::Jump, Input::Key(KeyCode::KeyR), false);
        assert_eq!(
            c.keys(Action::Jump),
            &[KeyCode::KeyX, KeyCode::KeyW, KeyCode::KeyE, KeyCode::KeyR]
        );
        // Замена: только новая.
        c.bind(Action::Attack, Input::Key(KeyCode::KeyZ), true);
        assert_eq!(c.keys(Action::Attack), &[KeyCode::KeyZ]);
        // Служебные клавиши не назначаются.
        assert!(!c.bind(Action::Jump, Input::Key(KeyCode::Backquote), true));
        assert!(!c.bind(Action::Jump, Input::Key(KeyCode::Escape), true));
        assert!(!c.bind(Action::Jump, Input::Key(KeyCode::F1), true));
    }

    #[test]
    fn file_round_trips_and_reports_unknown_names() {
        let mut c = Controls::default();
        c.bind(Action::Jump, Input::Key(KeyCode::KeyW), true);
        let text = c.to_text();
        assert!(text.contains("клавиши") && text.contains("прыжок") && text.contains("KeyW"));
        let (back, warnings) = Controls::from_text(&text).unwrap();
        assert_eq!(back, c);
        assert!(warnings.is_empty());

        let (part, warnings) =
            Controls::from_text(r#"(клавиши: {"прыжок": ["KeyW", "Мяу"], "полёт": ["KeyF"]})"#)
                .unwrap();
        assert_eq!(part.keys(Action::Jump), &[KeyCode::KeyW]);
        // Чего нет в файле — по умолчанию.
        assert_eq!(part.keys(Action::Left), &[KeyCode::ArrowLeft]);
        assert_eq!(warnings.len(), 2);
        assert!(Controls::from_text("(клавиши: {").is_err());
    }

    #[test]
    fn mouse_buttons_and_wheel_bind_like_keys() {
        let mut c = Controls::default();
        // По умолчанию: левая — удар, правая — рывок.
        assert_eq!(
            c.action_of(Input::Mouse(MouseKey::Left)),
            Some(Action::Attack)
        );
        assert_eq!(
            c.action_of(Input::Mouse(MouseKey::Right)),
            Some(Action::Dash)
        );
        assert!(c.bind(Action::Jump, Input::Wheel(Wheel::Up), false));
        assert!(c.bind(Action::Jump, Input::Mouse(MouseKey::Right), false));
        assert_eq!(
            c.action_of(Input::Mouse(MouseKey::Right)),
            Some(Action::Jump)
        );
        assert!(
            !c.inputs(Action::Dash)
                .contains(&Input::Mouse(MouseKey::Right))
        );
        let (back, warnings) = Controls::from_text(&c.to_text()).unwrap();
        assert_eq!(back, c);
        assert!(warnings.is_empty());
        for m in MouseKey::ALL {
            assert_eq!(Input::parse(&Input::Mouse(m).file()), Some(Input::Mouse(m)));
        }
    }

    #[test]
    fn behaviour_options_are_saved_by_their_names() {
        let mut c = Controls::default();
        assert!(c.attack_toward_cursor());
        assert_eq!(c.camera_button(), Some(MouseKey::Middle));
        c.set_option(ATTACK_AIM, 1);
        c.set_option(CAMERA_DRAG, 3);
        c.set_option(CAMERA_DRAG, 99);
        assert!(!c.attack_toward_cursor());
        assert_eq!(c.camera_button(), None);
        let text = c.to_text();
        assert!(
            text.contains("поведение") && text.contains("по взгляду"),
            "{text}"
        );
        assert_eq!(Controls::from_text(&text).unwrap().0, c);
        // Старый файл без поведения — поведение по умолчанию; ошибки в именах — с
        // предупреждением.
        let (old, w) = Controls::from_text(r#"(клавиши: {"прыжок": ["KeyW"]})"#).unwrap();
        assert!(old.attack_toward_cursor() && w.is_empty());
        let (_, w) =
            Controls::from_text(r#"(поведение: {"удар мышью": "куда-то", "полёт": "да"})"#)
                .unwrap();
        assert_eq!(w.len(), 2);
        // Описание настроек цельное: у каждой есть варианты и верный вариант по
        // умолчанию (по этой таблице строятся меню и будущий редактор).
        for o in OPTIONS {
            assert!(
                o.default < o.choices.len() && !o.hint.is_empty(),
                "{}",
                o.file
            );
        }
    }

    #[test]
    fn schemes_route_presses_from_the_top() {
        let c = Controls::default();
        let key = Input::Key;
        let game = schemes(true, false, false);
        assert_eq!(game, [Scheme::Game, Scheme::Sandbox]);
        assert_eq!(
            c.route(&game, key(KeyCode::Space)),
            Route::Game(Action::Jump)
        );
        assert_eq!(
            c.route(&game, key(KeyCode::ArrowUp)),
            Route::Game(Action::Up)
        );
        assert_eq!(c.route(&game, key(KeyCode::Escape)), Route::OpenMenu);
        assert_eq!(c.route(&game, key(KeyCode::Backquote)), Route::OpenConsole);
        assert_eq!(c.route(&game, key(KeyCode::F1)), Route::Sandbox);
        assert_eq!(
            c.route(&game, Input::Mouse(MouseKey::Left)),
            Route::Game(Action::Attack)
        );
        assert_eq!(
            c.route(&game, Input::Mouse(MouseKey::Middle)),
            Route::Camera
        );
        assert_eq!(c.route(&game, Input::Mouse(MouseKey::Back)), Route::Ui);
        assert_eq!(c.route(&game, Input::Wheel(Wheel::Up)), Route::Sandbox);
        // Меню поверх игры: та же стрелка двигает выбор, а не героя.
        let menu = schemes(true, true, false);
        assert_eq!(menu[0], Scheme::Menu);
        assert_eq!(
            c.route(&menu, key(KeyCode::ArrowUp)),
            Route::Menu(MenuAction::Up)
        );
        assert_eq!(
            c.route(&menu, key(KeyCode::Enter)),
            Route::Menu(MenuAction::Select)
        );
        assert_eq!(
            c.route(&menu, key(KeyCode::Escape)),
            Route::Menu(MenuAction::Back)
        );
        assert_eq!(c.route(&menu, key(KeyCode::KeyA)), Route::Ui);
        assert_eq!(c.route(&menu, Input::Mouse(MouseKey::Left)), Route::Ui);
        // Консоль сверху забирает всё, кроме своих клавиш закрытия.
        let console = schemes(true, false, true);
        assert_eq!(c.route(&console, key(KeyCode::Space)), Route::Ui);
        assert_eq!(
            c.route(&console, key(KeyCode::Backquote)),
            Route::CloseConsole
        );
        assert_eq!(c.route(&console, key(KeyCode::Escape)), Route::CloseConsole);
        // Без игры — только песочница.
        let sandbox = schemes(false, true, true);
        assert_eq!(sandbox, [Scheme::Sandbox]);
        assert_eq!(c.route(&sandbox, key(KeyCode::Space)), Route::Sandbox);
        // Камера на левой кнопке, а левая — удар: действие главнее.
        let mut c2 = c.clone();
        c2.set_option(CAMERA_DRAG, 2);
        assert_eq!(
            c2.route(&game, Input::Mouse(MouseKey::Left)),
            Route::Game(Action::Attack)
        );
        for s in [Scheme::Console, Scheme::Menu, Scheme::Game, Scheme::Sandbox] {
            assert!(!s.label().is_empty());
        }
    }
}
