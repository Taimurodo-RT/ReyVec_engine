//! Настройка клавиш (6.10б): какие клавиши дают какое действие игрока.
//!
//! У каждого действия до [`MAX_KEYS`] клавиш. Клавиша принадлежит только одному
//! действию: назначенная заново, она уходит от старого. Настройки хранятся в файле
//! `settings/controls.ron` рядом с программой, имена действий и клавиш в нём читаются
//! человеком:
//!
//! ```text
//! (клавиши: {"влево": ["ArrowLeft"], "прыжок": ["Space", "KeyW"]})
//! ```
//!
//! Это основа будущего управления: тот же список действий потом получит геймпад,
//! а меню — настройку кнопок геймпада.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use reyvec_scene::game::Action;
use serde::{Deserialize, Serialize};
use winit::keyboard::KeyCode;

/// Сколько клавиш может быть у одного действия.
pub(crate) const MAX_KEYS: usize = 3;

/// Клавиши всех действий.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Controls {
    keys: BTreeMap<ActionKey, Vec<KeyCode>>,
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
}

impl Default for Controls {
    fn default() -> Self {
        use KeyCode as K;
        let mut c = Self {
            keys: BTreeMap::new(),
        };
        for (a, keys) in [
            (Action::Left, vec![K::ArrowLeft]),
            (Action::Right, vec![K::ArrowRight]),
            (Action::Up, vec![K::ArrowUp]),
            (Action::Down, vec![K::ArrowDown]),
            (Action::Jump, vec![K::Space]),
            (Action::Dash, vec![K::ShiftLeft, K::ShiftRight, K::KeyX]),
            (
                Action::Attack,
                vec![K::ControlLeft, K::ControlRight, K::KeyA],
            ),
        ] {
            c.keys.insert(ActionKey::of(a), keys);
        }
        c
    }
}

impl Controls {
    /// Действие клавиши `code`.
    pub(crate) fn action(&self, code: KeyCode) -> Option<Action> {
        self.keys
            .iter()
            .find(|(_, keys)| keys.contains(&code))
            .map(|(a, _)| a.action())
    }

    /// Клавиши действия.
    pub(crate) fn keys(&self, a: Action) -> &[KeyCode] {
        self.keys
            .get(&ActionKey::of(a))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Назначает клавишу действию. `replace` — вместо всех его клавиш, иначе в добавку
    /// (самая старая уходит, если их уже [`MAX_KEYS`]). У другого действия эта клавиша
    /// пропадает. `false` — клавишу назначить нельзя (служебная).
    pub(crate) fn bind(&mut self, a: Action, code: KeyCode, replace: bool) -> bool {
        if key_name(code).is_none() {
            return false;
        }
        for keys in self.keys.values_mut() {
            keys.retain(|k| *k != code);
        }
        let keys = self.keys.entry(ActionKey::of(a)).or_default();
        if replace {
            keys.clear();
        }
        keys.push(code);
        if keys.len() > MAX_KEYS {
            keys.remove(0);
        }
        true
    }

    /// Действия без единой клавиши (о них предупреждает меню).
    pub(crate) fn unbound(&self) -> Vec<Action> {
        Action::ALL
            .into_iter()
            .filter(|a| self.keys(*a).is_empty())
            .collect()
    }

    /// Читает настройки из текста файла. Неизвестные имена пропускаются с
    /// предупреждением; действия, которых нет в файле, получают клавиши по умолчанию.
    pub(crate) fn from_text(text: &str) -> Result<(Self, Vec<String>), String> {
        let file: ControlsFile = ron::from_str(text).map_err(|e| e.to_string())?;
        let mut out = Self::default();
        let mut warnings = Vec::new();
        for (name, keys) in &file.keys {
            let Some(a) = Action::parse(name) else {
                warnings.push(format!("нет такого действия «{name}»"));
                continue;
            };
            out.keys.insert(ActionKey::of(a), Vec::new());
            for k in keys {
                match parse_key(k) {
                    Some(code) => {
                        out.bind(a, code, false);
                    }
                    None => warnings.push(format!("нет такой клавиши «{k}» у «{name}»")),
                }
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
                    let keys = self.keys(a).iter().map(|k| format!("{k:?}")).collect();
                    (a.key().to_string(), keys)
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
        assert!(c.bind(Action::Jump, KeyCode::KeyX, false));
        assert_eq!(c.action(KeyCode::KeyX), Some(Action::Jump));
        assert_eq!(
            c.keys(Action::Dash),
            &[KeyCode::ShiftLeft, KeyCode::ShiftRight]
        );
        assert_eq!(c.keys(Action::Jump), &[KeyCode::Space, KeyCode::KeyX]);
        // Лишняя клавиша вытесняет самую старую.
        c.bind(Action::Jump, KeyCode::KeyW, false);
        c.bind(Action::Jump, KeyCode::KeyE, false);
        assert_eq!(
            c.keys(Action::Jump),
            &[KeyCode::KeyX, KeyCode::KeyW, KeyCode::KeyE]
        );
        // Замена: только новая.
        c.bind(Action::Attack, KeyCode::KeyZ, true);
        assert_eq!(c.keys(Action::Attack), &[KeyCode::KeyZ]);
        // Служебные клавиши не назначаются.
        assert!(!c.bind(Action::Jump, KeyCode::Escape, true));
        assert!(!c.bind(Action::Jump, KeyCode::F1, true));
    }

    #[test]
    fn file_round_trips_and_reports_unknown_names() {
        let mut c = Controls::default();
        c.bind(Action::Jump, KeyCode::KeyW, true);
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
}
