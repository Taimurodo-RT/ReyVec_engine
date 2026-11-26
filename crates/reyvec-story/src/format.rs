//! Файлы сюжета (RON): персонаж, диалог, задание, переменные мира, перевод.
//!
//! Имена (`id`) — латиница в нижнем регистре, цифры, `_` и `.`: на них ссылаются код Luau
//! и условия. Заголовки и тексты — на любом языке.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Версия форматов сюжета. Новые поля добавляются с значением по умолчанию, поэтому
/// старые файлы читаются без правки.
pub const VERSION: u32 = 1;

/// Значение переменной мира: да/нет, целое, дробное или текст.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl Value {
    /// Название типа для сообщений об ошибках.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Bool(_) => "да/нет",
            Value::Int(_) => "целое",
            Value::Float(_) => "дробное",
            Value::Text(_) => "текст",
        }
    }

    /// Число, если значение числовое.
    pub fn number(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Можно ли записать `other` в переменную с таким значением (целое и дробное
    /// взаимозаменяемы).
    pub fn same_kind(&self, other: &Value) -> bool {
        self.number().is_some() && other.number().is_some()
            || std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// Текст для подстановки в реплику.
    pub fn show(&self) -> String {
        match self {
            Value::Bool(b) => if *b { "да" } else { "нет" }.into(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Text(t) => t.clone(),
        }
    }
}

/// Переменные мира: `story/vars.ron`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VarsFile {
    pub format: String,
    #[serde(default = "version")]
    pub version: u32,
    /// Имя переменной (`kuznec.znakom`) → значение в начале игры. Тип берётся из него.
    pub vars: BTreeMap<String, Value>,
}

/// Как показывается реплика.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum View {
    /// Окно диалога внизу экрана.
    #[default]
    Window,
    /// Фраза над головой говорящего, игра не останавливается.
    Bubble,
    /// Голос за кадром.
    Voiceover,
}

/// Настройки реплики. Пустое поле берётся из диалога, затем из персонажа (ответ Романа:
/// настройки у каждой реплики).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LineSettings {
    pub view: Option<View>,
    /// Портрет: настроение (`"zloy"`), картинка выбирается из папки персонажа.
    pub mood: Option<String>,
    /// Звук голоса (путь к файлу в проекте).
    pub voice: Option<String>,
    /// Камера плавно едет к говорящему.
    pub camera: Option<bool>,
    /// Пауза после реплики, секунды.
    pub pause: Option<f32>,
}

impl LineSettings {
    /// Заполняет пустые поля из `under`.
    pub fn over(&self, under: &LineSettings) -> LineSettings {
        LineSettings {
            view: self.view.or(under.view),
            mood: self.mood.clone().or_else(|| under.mood.clone()),
            voice: self.voice.clone().or_else(|| under.voice.clone()),
            camera: self.camera.or(under.camera),
            pause: self.pause.or(under.pause),
        }
    }
}

/// Персонаж: `story/characters/<id>.ron`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Character {
    pub format: String,
    #[serde(default = "version")]
    pub version: u32,
    pub id: String,
    /// Имя в окне диалога.
    pub name: String,
    /// Цвет имени, `#RRGGBB`.
    #[serde(default)]
    pub color: String,
    /// Характер, манера речи, цели — для автора и позже для модели (Сю.10).
    #[serde(default)]
    pub about: String,
    /// Настройки реплик этого персонажа по умолчанию.
    #[serde(default)]
    pub lines: LineSettings,
}

/// Вариант ответа игрока у реплики.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub text: String,
    /// Условие Luau: вариант виден, только если оно истинно. Пусто — виден всегда.
    #[serde(default)]
    pub when: String,
    pub next: String,
    /// Ключ перевода. Пусто — `<диалог>.<карточка>.<номер>`.
    #[serde(default)]
    pub key: String,
}

/// Изменение переменной: `to` — записать, `add` — прибавить к числу.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub var: String,
    #[serde(default)]
    pub to: Option<Value>,
    #[serde(default)]
    pub add: Option<f64>,
}

/// Место карточки на схеме редактора.
pub type At = (f32, f32);

/// Карточка диалога.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Node {
    /// Реплика. С вариантами — ждёт выбора игрока, без них идёт в `next`.
    Line {
        id: String,
        #[serde(default)]
        at: At,
        who: String,
        /// Текст с подстановками `{hero.name}`.
        text: String,
        #[serde(default)]
        settings: LineSettings,
        #[serde(default)]
        choices: Vec<Choice>,
        #[serde(default)]
        next: String,
        /// Ключ перевода. Пусто — `<диалог>.<карточка>`.
        #[serde(default)]
        key: String,
    },
    /// Развилка по условию Luau.
    Check {
        id: String,
        #[serde(default)]
        at: At,
        when: String,
        yes: String,
        no: String,
    },
    /// Изменить переменные мира.
    Set {
        id: String,
        #[serde(default)]
        at: At,
        changes: Vec<Change>,
        next: String,
    },
    /// Событие. `quest.start`, `quest.add`, `quest.stage`, `quest.complete`, `quest.fail`
    /// выполняет сам сюжет, остальные получает игра.
    Event {
        id: String,
        #[serde(default)]
        at: At,
        name: String,
        #[serde(default)]
        args: Vec<Value>,
        next: String,
    },
    /// Позвать функцию скрипта Luau.
    Run {
        id: String,
        #[serde(default)]
        at: At,
        func: String,
        #[serde(default)]
        args: Vec<Value>,
        next: String,
    },
    /// Перейти в другой диалог (с начала или с карточки `node`).
    Jump {
        id: String,
        #[serde(default)]
        at: At,
        dialog: String,
        #[serde(default)]
        node: String,
    },
    /// Наугад одна из стрелок.
    Random {
        id: String,
        #[serde(default)]
        at: At,
        next: Vec<String>,
    },
    /// С вероятностью `percent` — `yes`, иначе `no`.
    Chance {
        id: String,
        #[serde(default)]
        at: At,
        percent: f32,
        yes: String,
        no: String,
    },
    /// Пауза: секунды или до нажатия (`seconds: None`).
    Wait {
        id: String,
        #[serde(default)]
        at: At,
        #[serde(default)]
        seconds: Option<f32>,
        next: String,
    },
    /// Конец разговора.
    End {
        id: String,
        #[serde(default)]
        at: At,
    },
}

impl Node {
    pub fn id(&self) -> &str {
        match self {
            Node::Line { id, .. }
            | Node::Check { id, .. }
            | Node::Set { id, .. }
            | Node::Event { id, .. }
            | Node::Run { id, .. }
            | Node::Jump { id, .. }
            | Node::Random { id, .. }
            | Node::Chance { id, .. }
            | Node::Wait { id, .. }
            | Node::End { id, .. } => id,
        }
    }

    /// Куда ведут стрелки из карточки (внутри диалога).
    pub fn targets(&self) -> Vec<&str> {
        match self {
            Node::Line { choices, next, .. } => {
                let mut v: Vec<&str> = choices.iter().map(|c| c.next.as_str()).collect();
                if !next.is_empty() {
                    v.push(next);
                }
                v
            }
            Node::Check { yes, no, .. } | Node::Chance { yes, no, .. } => vec![yes, no],
            Node::Set { next, .. }
            | Node::Event { next, .. }
            | Node::Run { next, .. }
            | Node::Wait { next, .. } => vec![next],
            Node::Random { next, .. } => next.iter().map(String::as_str).collect(),
            Node::Jump { .. } | Node::End { .. } => Vec::new(),
        }
    }
}

/// Диалог: `story/dialogs/<id>.ron`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dialog {
    pub format: String,
    #[serde(default = "version")]
    pub version: u32,
    pub id: String,
    /// Заголовок в дереве редактора.
    #[serde(default)]
    pub title: String,
    /// Кто участвует (для поиска «все диалоги кузнеца»).
    #[serde(default)]
    pub with: Vec<String>,
    /// Первая карточка. Пусто — первая в списке.
    #[serde(default)]
    pub start: String,
    /// Настройки реплик этого диалога по умолчанию.
    #[serde(default)]
    pub lines: LineSettings,
    pub nodes: Vec<Node>,
}

impl Dialog {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id() == id)
    }

    pub fn start_node(&self) -> Option<&str> {
        if self.start.is_empty() {
            self.nodes.first().map(Node::id)
        } else {
            Some(&self.start)
        }
    }
}

/// Цель стадии задания: «руда, 0 из 3».
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    pub id: String,
    pub text: String,
    #[serde(default = "one")]
    pub count: u32,
}

/// Стадия задания.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub id: String,
    /// Запись в журнале игрока.
    pub journal: String,
    /// Цели. Все выполнены — задание идёт в `next`. Без целей стадия ждёт
    /// `quest.stage` или `quest.complete`.
    #[serde(default)]
    pub goals: Vec<Goal>,
    /// Следующая стадия. Пусто — задание выполнено.
    #[serde(default)]
    pub next: String,
    /// Условие Luau, без которого стадия не закончится (вместе с целями, если они есть):
    /// `hero.ruda >= 3`. Пусто — хватает целей.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub done_when: String,
}

/// Задание: `story/quests/<id>.ron`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quest {
    pub format: String,
    #[serde(default = "version")]
    pub version: u32,
    pub id: String,
    pub title: String,
    /// Описание для журнала: о чём задание в целом.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub stages: Vec<Stage>,
    /// Награда: что меняется в мире, когда задание выполнено.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rewards: Vec<Change>,
    /// Условие Luau провала: стало верным, пока задание идёт, — задание провалено.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fail_when: String,
}

impl Quest {
    pub fn stage(&self, id: &str) -> Option<&Stage> {
        self.stages.iter().find(|s| s.id == id)
    }
}

fn version() -> u32 {
    VERSION
}

fn one() -> u32 {
    1
}

/// Читает RON с расширением «Some можно не писать».
pub(crate) fn parse<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, String> {
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
        .from_str(text)
        .map_err(|e| e.to_string())
}

/// Пишет RON в читаемом виде.
pub fn write<T: Serialize>(value: &T) -> String {
    let cfg = ron::ser::PrettyConfig::new()
        .struct_names(false)
        .indentor("    ".to_string());
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
        .to_string_pretty(value, cfg)
        .unwrap_or_default()
}
