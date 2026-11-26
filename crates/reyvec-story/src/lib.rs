//! `reyvec-story`: сюжет ReyVec без окна и видеокарты (решение 0023, `docs/story.md`).
//!
//! Сюжет — данные проекта в папке `story/`:
//!
//! ```text
//! story/
//!   vars.ron               переменные мира и их значения в начале игры
//!   characters/*.ron       персонажи: имя, характер, настройки реплик по умолчанию
//!   dialogs/*.ron          диалоги: карточки (реплика, развилка, событие, …) и стрелки
//!   quests/*.ron           задания: стадии, цели, записи журнала
//!   lang/<язык>.ron        переводы: ключ реплики → текст
//! ```
//!
//! [`Story::load_dir`] читает папку и проверяет её ([`check`]). [`Story::talk`] начинает
//! разговор, [`Story::step`] ведёт его до следующей реплики или события, [`Talk::choose`]
//! передаёт выбор игрока. Всё, что меняется по ходу игры (переменные, задания, выборы),
//! лежит в [`State`] — это часть сохранения.

pub mod check;
pub mod cond;
pub mod format;
pub mod play;
pub mod quest;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

pub use check::Problem;
pub use format::{Character, Dialog, LineSettings, Node, Quest, Value, VarsFile, View};
pub use play::{Shown, ShownChoice, Step, Talk};
pub use quest::{QuestState, Status};

/// Пример сюжета «кузнец и руда» (папка `story/` в корне репозитория), для тестов.
pub const EXAMPLE: &[(&str, &str)] = &[
    ("vars.ron", include_str!("../../../story/vars.ron")),
    (
        "characters/hero.ron",
        include_str!("../../../story/characters/hero.ron"),
    ),
    (
        "characters/kuznec.ron",
        include_str!("../../../story/characters/kuznec.ron"),
    ),
    (
        "dialogs/kuznec.privet.ron",
        include_str!("../../../story/dialogs/kuznec.privet.ron"),
    ),
    (
        "dialogs/kuznec.ruda.ron",
        include_str!("../../../story/dialogs/kuznec.ruda.ron"),
    ),
    (
        "quests/mech.ron",
        include_str!("../../../story/quests/mech.ron"),
    ),
    ("lang/en.ron", include_str!("../../../story/lang/en.ron")),
];

/// Сюжет проекта: всё прочитанное из `story/` и разобранные условия.
#[derive(Default)]
pub struct Story {
    pub vars: BTreeMap<String, Value>,
    pub characters: BTreeMap<String, Character>,
    pub dialogs: BTreeMap<String, Dialog>,
    pub quests: BTreeMap<String, Quest>,
    /// Язык → ключ → текст.
    pub lang: BTreeMap<String, HashMap<String, String>>,
    /// Из какого файла пришёл персонаж, диалог или задание (для сообщений).
    pub files: HashMap<String, String>,
    pub(crate) conds: cond::Conditions,
}

/// Состояние сюжета в игре — часть сохранения.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub vars: BTreeMap<String, Value>,
    pub quests: BTreeMap<String, QuestState>,
    /// Какие варианты ответа игрок уже выбирал (ключи вариантов).
    pub chosen: BTreeSet<String>,
    /// Язык текстов; пусто — основной (как написано в диалогах).
    pub lang: String,
    /// Зерно случайных карточек: одинаковое зерно — одинаковый путь.
    pub seed: u64,
}

impl Story {
    /// Читает сюжет из текстов «путь в `story/` → содержимое». Ошибки чтения и проверки —
    /// в списке; сюжет собирается из того, что прочиталось.
    pub fn from_texts(files: &[(&str, &str)]) -> (Story, Vec<Problem>) {
        let mut story = Story::default();
        let mut problems = Vec::new();
        for (path, text) in files {
            let path = path.replace('\\', "/");
            if let Err(e) = story.add(&path, text) {
                problems.push(Problem::error(&path, "", e));
            }
        }
        problems.extend(check::check(&mut story));
        (story, problems)
    }

    /// Читает папку `story/`. Нет папки — пустой сюжет без ошибок.
    pub fn load_dir(dir: &Path) -> (Story, Vec<Problem>) {
        let mut texts = Vec::new();
        let mut problems = Vec::new();
        let vars = dir.join("vars.ron");
        if vars.exists() {
            match std::fs::read_to_string(&vars) {
                Ok(t) => texts.push(("vars.ron".to_string(), t)),
                Err(e) => problems.push(Problem::error("vars.ron", "", e.to_string())),
            }
        }
        for sub in ["characters", "dialogs", "quests", "lang"] {
            let Ok(rd) = std::fs::read_dir(dir.join(sub)) else {
                continue;
            };
            let mut paths: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            paths.sort();
            for p in paths {
                if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ron")) {
                    continue;
                }
                let name = format!(
                    "{sub}/{}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                );
                match std::fs::read_to_string(&p) {
                    Ok(t) => texts.push((name, t)),
                    Err(e) => problems.push(Problem::error(&name, "", e.to_string())),
                }
            }
        }
        let refs: Vec<(&str, &str)> = texts
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let (story, more) = Story::from_texts(&refs);
        problems.extend(more);
        (story, problems)
    }

    fn add(&mut self, path: &str, text: &str) -> Result<(), String> {
        let kind = path.split('/').next().unwrap_or("");
        match kind {
            "vars.ron" => {
                let f: VarsFile = format::parse(text)?;
                expect_format(&f.format, "reyvec-story-vars")?;
                self.vars = f.vars;
            }
            "characters" => {
                let c: Character = format::parse(text)?;
                expect_format(&c.format, "reyvec-character")?;
                self.files.insert(format!("c:{}", c.id), path.into());
                if self.characters.insert(c.id.clone(), c).is_some() {
                    return Err("персонаж с таким именем уже есть".into());
                }
            }
            "dialogs" => {
                let d: Dialog = format::parse(text)?;
                expect_format(&d.format, "reyvec-dialog")?;
                self.files.insert(format!("d:{}", d.id), path.into());
                if self.dialogs.insert(d.id.clone(), d).is_some() {
                    return Err("диалог с таким именем уже есть".into());
                }
            }
            "quests" => {
                let q: Quest = format::parse(text)?;
                expect_format(&q.format, "reyvec-quest")?;
                self.files.insert(format!("q:{}", q.id), path.into());
                if self.quests.insert(q.id.clone(), q).is_some() {
                    return Err("задание с таким именем уже есть".into());
                }
            }
            "lang" => {
                let words: HashMap<String, String> = format::parse(text)?;
                let code = path
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(".ron")
                    .to_string();
                self.lang.insert(code, words);
            }
            _ => return Err("неизвестная папка сюжета".into()),
        }
        Ok(())
    }

    /// Состояние мира в начале игры.
    pub fn new_state(&self) -> State {
        State {
            vars: self.vars.clone(),
            // Все задания сразу, «не взятыми»: условие `quest.mech.state == "none"`
            // работает и до начала задания.
            quests: self
                .quests
                .keys()
                .map(|id| (id.clone(), QuestState::default()))
                .collect(),
            ..State::default()
        }
    }

    /// Истинно ли условие Luau при этом состоянии.
    pub fn eval(&mut self, expr: &str, state: &State) -> Result<bool, String> {
        self.conds.eval(expr, state)
    }

    /// Текст по ключу на языке состояния; нет перевода — основной текст.
    pub fn text<'a>(&'a self, key: &str, main: &'a str, state: &State) -> &'a str {
        if state.lang.is_empty() {
            return main;
        }
        self.lang
            .get(&state.lang)
            .and_then(|w| w.get(key))
            .map_or(main, String::as_str)
    }

    /// Файл, из которого пришёл диалог.
    pub fn dialog_file(&self, id: &str) -> &str {
        self.files
            .get(&format!("d:{id}"))
            .map_or("", String::as_str)
    }
}

fn expect_format(got: &str, want: &str) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("format: \"{got}\", а ждали \"{want}\""))
    }
}

/// Годится ли имя: латиница в нижнем регистре, цифры, `_` и `.` (не в начале и конце).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
        && !id.starts_with(|c: char| c.is_ascii_digit())
}
