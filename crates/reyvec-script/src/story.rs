//! Сюжет для скриптов (Сю.5): таблицы `story` и `quest` в Luau.
//!
//! ```lua
//! function update(me, hero, dt)
//!     if me.touching and not story.busy() then
//!         story.talk("kuznec.privet")
//!     end
//! end
//! ```
//!
//! Сюжет и его состояние лежат в [`StoryHost`]: скрипты меняют его своими вызовами, игра
//! (окно диалога, Сю.7) ведёт начатый разговор и сохраняет состояние. Неизвестное имя
//! (диалог, задание, переменная) — понятная ошибка Luau со строкой скрипта, а не падение.

use std::sync::{Arc, Mutex, MutexGuard};

use mlua::{Lua, Table, Value as Lv};
use reyvec_story::{Shown, State, Step, Story, Talk, Value};

/// Функция сюжета для скриптов: имя, аргументы, что делает. Редактор кода показывает эти
/// списки в подсказках; тест `story_functions_match_engine` сверяет их с тем, что движок
/// кладёт в Luau.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Func {
    pub name: &'static str,
    pub args: &'static str,
    pub about: &'static str,
}

const fn func(name: &'static str, args: &'static str, about: &'static str) -> Func {
    Func { name, args, about }
}

/// `story.*`.
pub const STORY_FUNCS: &[Func] = &[
    func(
        "talk",
        "(диалог)",
        "начать разговор; false — уже идёт другой",
    ),
    func("busy", "()", "идёт ли разговор"),
    func("get", "(переменная)", "значение переменной мира"),
    func("set", "(переменная, значение)", "записать переменную мира"),
    func("seen", "(диалог)", "был ли уже этот разговор"),
];

/// `quest.*`.
pub const QUEST_FUNCS: &[Func] = &[
    func("start", "(задание)", "взять задание: первая стадия"),
    func(
        "add",
        "(задание, цель, сколько)",
        "продвинуть цель текущей стадии (сколько — по умолчанию 1)",
    ),
    func(
        "stage",
        "(задание [, стадия])",
        "текущая стадия; со вторым аргументом — перейти на стадию",
    ),
    func(
        "state",
        "(задание)",
        "\"none\", \"active\", \"done\" или \"failed\"",
    ),
    func("complete", "(задание)", "задание выполнено"),
    func("fail", "(задание)", "задание провалено"),
];

/// Сюжет игры, его состояние и идущий разговор.
#[derive(Default)]
pub struct StoryHost {
    pub story: Story,
    pub state: State,
    /// Начатый разговор (его ведёт [`StoryHost::advance`] в шаге логики).
    pub talk: Option<Talk>,
    /// Что сейчас на экране: реплика или пауза до нажатия (рисует окно диалога).
    pub screen: Option<Screen>,
    /// Ответ игрока на то, что на экране; забирает следующий шаг логики.
    pub answer: Option<Answer>,
    /// Пауза карточки «Подождать» с секундами: сколько ещё ждать.
    wait: f32,
    /// События сюжета для игры (`Event` не `quest.*`), последние [`KEEP`]: «имя(аргументы)».
    pub events: Vec<String>,
    /// Ошибки разговоров и функций сюжета (последние [`KEEP`]) — для консоли и окна диалога.
    pub problems: Vec<String>,
}

/// Сколько последних событий и ошибок сюжета хранить.
pub const KEEP: usize = 20;

/// Что окно диалога показывает игроку.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // одна штука на игру, копируется раз за реплику
pub enum Screen {
    /// Реплика; если у неё есть варианты — ждём выбор.
    Line(Shown),
    /// Карточка «Подождать» без секунд: до нажатия.
    Pause,
}

/// Ответ игрока окну диалога.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Дальше (реплика без вариантов или пауза).
    Next,
    /// Вариант ответа (с 0).
    Choose(usize),
}

fn keep(list: &mut Vec<String>, text: String) {
    list.push(text);
    if list.len() > KEEP {
        list.remove(0);
    }
}

impl StoryHost {
    pub fn new(story: Story) -> Self {
        let state = story.new_state();
        Self {
            story,
            state,
            ..Self::default()
        }
    }

    /// Идёт ли разговор.
    pub fn busy(&self) -> bool {
        self.talk.as_ref().is_some_and(|t| !t.ended())
    }

    /// Начать разговор. `Ok(false)` — уже идёт другой.
    pub fn talk(&mut self, dialog: &str) -> Result<bool, String> {
        if self.busy() {
            return Ok(false);
        }
        let t = self.story.talk(dialog)?;
        self.state.talked.insert(dialog.to_string());
        self.talk = Some(t);
        self.screen = None;
        self.answer = None;
        self.wait = 0.0;
        Ok(true)
    }

    /// Сохранение сюжета (RON). Идущий разговор не сохраняется: после загрузки его
    /// можно начать заново.
    pub fn save(&self) -> Result<String, String> {
        self.story.save(&self.state)
    }

    /// Загрузить сохранение: разговор обрывается, состояние — из текста.
    pub fn load(&mut self, text: &str) -> Result<(), String> {
        self.state = self.story.restore(text)?;
        self.stop();
        Ok(())
    }

    /// Оборвать разговор (загрузка, смена сцены).
    pub fn stop(&mut self) {
        self.talk = None;
        self.screen = None;
        self.answer = None;
        self.wait = 0.0;
    }

    /// Ведёт разговор на шаг логики `dt` секунд: берёт ответ игрока, идёт по карточкам до
    /// реплики, паузы или конца. Возвращает функцию скрипта, которую позвала карточка
    /// «Скрипт»: её надо позвать без замка на сюжете и снова вызвать `advance(0)`.
    pub fn advance(&mut self, dt: f32) -> Option<(String, Vec<Value>)> {
        let Self {
            story,
            state,
            talk,
            screen,
            answer,
            wait,
            events,
            problems,
        } = self;
        let Some(t) = talk.as_mut() else {
            *screen = None;
            return None;
        };
        if screen.is_some() {
            let a = answer.take()?;
            if t.waiting() {
                // Реплика с вариантами: «дальше» не годится, нужен выбор.
                let Answer::Choose(i) = a else {
                    return None;
                };
                if !t.choose(i, state) {
                    return None;
                }
            }
            *screen = None;
        }
        *answer = None;
        if *wait > 0.0 {
            *wait -= dt;
            if *wait > 0.0 {
                return None;
            }
            *wait = 0.0;
        }
        // Предел на случай сюжета из одних событий по кругу (сам проигрыватель
        // ловит только круги из тихих карточек).
        for _ in 0..64 {
            match story.step(t, state) {
                Step::Line(s) => {
                    *screen = Some(Screen::Line(s));
                    return None;
                }
                Step::Event { name, args } => {
                    let args: Vec<String> = args.iter().map(Value::show).collect();
                    let text = format!("{name}({})", args.join(", "));
                    tracing::info!("событие сюжета: {text}");
                    keep(events, text);
                }
                Step::Run { func, args } => return Some((func, args)),
                Step::Wait { seconds: Some(s) } => {
                    *wait = s;
                    if s > 0.0 {
                        return None;
                    }
                }
                Step::Wait { seconds: None } => {
                    *screen = Some(Screen::Pause);
                    return None;
                }
                Step::End => {
                    if let Some(e) = &t.error {
                        keep(problems, format!("диалог «{}»: {e}", t.dialog));
                    }
                    *talk = None;
                    *screen = None;
                    return None;
                }
            }
        }
        keep(
            problems,
            format!("диалог «{}»: события идут по кругу без реплик", t.dialog),
        );
        *talk = None;
        *screen = None;
        None
    }

    /// Ошибка функции сюжета (карточка «Скрипт»).
    pub fn problem(&mut self, text: String) {
        tracing::warn!("сюжет: {text}");
        keep(&mut self.problems, text);
    }
}

/// Идёт ли разговор (для игры: герой стоит, клавиши — окну диалога).
pub fn talking(s: &SharedStory) -> bool {
    lock(s).busy()
}

/// Общий сюжет: скрипты и игра держат одну и ту же копию.
pub type SharedStory = Arc<Mutex<StoryHost>>;

pub fn lock(s: &SharedStory) -> MutexGuard<'_, StoryHost> {
    // Скрипт, упавший посреди вызова, не должен навсегда закрыть сюжет.
    s.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn to_lua(lua: &Lua, v: &Value) -> mlua::Result<Lv> {
    Ok(match v {
        Value::Bool(b) => Lv::Boolean(*b),
        Value::Int(i) => Lv::Integer(*i),
        Value::Float(f) => Lv::Number(*f),
        Value::Text(t) => Lv::String(lua.create_string(t)?),
    })
}

/// Значение из Luau того же вида, что переменная в `story/vars.ron`.
fn from_lua(name: &str, was: &Value, v: &Lv) -> Result<Value, String> {
    let bad = |want: &str| format!("story.set: «{name}» — {want}, а пришло {}", v.type_name());
    Ok(match (was, v) {
        (Value::Bool(_), Lv::Boolean(b)) => Value::Bool(*b),
        (Value::Bool(_), _) => return Err(bad("да/нет")),
        (Value::Int(_), Lv::Integer(i)) => Value::Int(*i),
        (Value::Int(_), Lv::Number(n)) if n.fract() == 0.0 => Value::Int(*n as i64),
        (Value::Int(_), _) => return Err(bad("целое число")),
        (Value::Float(_), Lv::Integer(i)) => Value::Float(*i as f64),
        (Value::Float(_), Lv::Number(n)) => Value::Float(*n),
        (Value::Float(_), _) => return Err(bad("число")),
        (Value::Text(_), Lv::String(s)) => Value::Text(s.to_string_lossy()),
        (Value::Text(_), _) => return Err(bad("текст")),
    })
}

/// Ошибка со строкой скрипта, откуда позвали: «kuznec.luau:4: нет диалога …».
fn err(lua: &Lua, e: String) -> mlua::Error {
    let at = (1..6).find_map(|level| {
        lua.inspect_stack(level, |d| {
            let line = d.current_line()?;
            let src = d.source().short_src?.into_owned();
            Some(format!("{src}:{line}: "))
        })
        .flatten()
    });
    mlua::Error::runtime(format!("{}{e}", at.unwrap_or_default()))
}

/// Кладёт таблицы `story` и `quest` в глобальные имена Luau.
pub(crate) fn install(lua: &Lua, host: &SharedStory) -> mlua::Result<()> {
    let story = lua.create_table()?;
    let quest = lua.create_table()?;
    let h = Arc::clone(host);
    story.set(
        "talk",
        lua.create_function(move |lua, (dialog, _who): (String, Lv)| {
            lock(&h).talk(&dialog).map_err(|e| err(lua, e))
        })?,
    )?;
    let h = Arc::clone(host);
    story.set(
        "busy",
        lua.create_function(move |_, ()| Ok(lock(&h).busy()))?,
    )?;
    let h = Arc::clone(host);
    story.set(
        "get",
        lua.create_function(move |lua, name: String| {
            let h = lock(&h);
            let v = h.state.vars.get(&name).ok_or_else(|| {
                err(
                    lua,
                    format!("story.get: нет переменной «{name}» в story/vars.ron"),
                )
            })?;
            to_lua(lua, v)
        })?,
    )?;
    let h = Arc::clone(host);
    story.set(
        "set",
        lua.create_function(move |lua, (name, v): (String, Lv)| {
            let mut h = lock(&h);
            let was = h.state.vars.get(&name).ok_or_else(|| {
                err(
                    lua,
                    format!("story.set: нет переменной «{name}» в story/vars.ron"),
                )
            })?;
            let new = from_lua(&name, was, &v).map_err(|e| err(lua, e))?;
            h.state.vars.insert(name, new);
            // Переменная могла закончить стадию или провалить задание.
            let StoryHost { story, state, .. } = &mut *h;
            story.quest_tick(state).map_err(|e| err(lua, e))
        })?,
    )?;
    let h = Arc::clone(host);
    story.set(
        "seen",
        lua.create_function(move |lua, dialog: String| {
            let h = lock(&h);
            if !h.story.dialogs.contains_key(&dialog) {
                return Err(err(lua, format!("story.seen: нет диалога «{dialog}»")));
            }
            Ok(h.state.talked.contains(&dialog))
        })?,
    )?;

    // quest.*: одно и то же «нет задания» для всех.
    fn known(lua: &Lua, h: &StoryHost, f: &str, id: &str) -> mlua::Result<()> {
        if h.story.quests.contains_key(id) {
            Ok(())
        } else {
            Err(err(lua, format!("quest.{f}: нет задания «{id}»")))
        }
    }
    let h = Arc::clone(host);
    quest.set(
        "start",
        lua.create_function(move |lua, id: String| {
            let mut g = lock(&h);
            known(lua, &g, "start", &id)?;
            let StoryHost { story, state, .. } = &mut *g;
            story.quest_start(state, &id).map_err(|e| err(lua, e))
        })?,
    )?;
    let h = Arc::clone(host);
    quest.set(
        "add",
        lua.create_function(move |lua, (id, goal, n): (String, String, Option<u32>)| {
            let mut g = lock(&h);
            known(lua, &g, "add", &id)?;
            let StoryHost { story, state, .. } = &mut *g;
            story
                .quest_add(state, &id, &goal, n.unwrap_or(1))
                .and_then(|()| story.quest_tick(state))
                .map_err(|e| err(lua, e))
        })?,
    )?;
    let h = Arc::clone(host);
    quest.set(
        "stage",
        lua.create_function(move |lua, (id, stage): (String, Option<String>)| {
            let mut g = lock(&h);
            known(lua, &g, "stage", &id)?;
            let StoryHost { story, state, .. } = &mut *g;
            if let Some(stage) = stage {
                story
                    .quest_stage(state, &id, &stage)
                    .map_err(|e| err(lua, e))?;
            }
            Ok(story.quest_state(state, &id).stage)
        })?,
    )?;
    let h = Arc::clone(host);
    quest.set(
        "state",
        lua.create_function(move |lua, id: String| {
            let g = lock(&h);
            known(lua, &g, "state", &id)?;
            Ok(g.story.quest_state(&g.state, &id).status.name())
        })?,
    )?;
    let h = Arc::clone(host);
    quest.set(
        "complete",
        lua.create_function(move |lua, id: String| {
            let mut g = lock(&h);
            known(lua, &g, "complete", &id)?;
            let StoryHost { story, state, .. } = &mut *g;
            story.quest_complete(state, &id).map_err(|e| err(lua, e))
        })?,
    )?;
    let h = Arc::clone(host);
    quest.set(
        "fail",
        lua.create_function(move |lua, id: String| {
            let mut g = lock(&h);
            known(lua, &g, "fail", &id)?;
            let StoryHost { story, state, .. } = &mut *g;
            story.quest_fail(state, &id).map_err(|e| err(lua, e))
        })?,
    )?;
    story.set_readonly(true);
    quest.set_readonly(true);
    let g: Table = lua.globals();
    g.raw_set("story", story)?;
    g.raw_set("quest", quest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{ActorIn, HeroIn, Scripts};

    use super::*;

    fn scripts() -> (Scripts, SharedStory) {
        let mut s = Scripts::new();
        let (story, problems) = Story::from_texts(reyvec_story::EXAMPLE);
        assert!(problems.is_empty(), "{problems:?}");
        let host = s.set_story(story);
        (s, host)
    }

    /// Запускает `update` скрипта один раз (предмет касается героя).
    fn run(s: &mut Scripts, name: &str, source: &str) {
        let id = s.load_text(name, source);
        let actor = s.actor(id).unwrap();
        let input = ActorIn {
            touching: true,
            alive: true,
            ..ActorIn::default()
        };
        s.run(&actor, &input, &HeroIn::default(), 0.016);
    }

    /// Всё, что лежит в `story` и `quest`, описано в STORY_FUNCS и QUEST_FUNCS (по ним
    /// подсказывает редактор кода), и наоборот.
    #[test]
    fn story_functions_match_engine() {
        let (s, _) = scripts();
        for (table, funcs) in [("story", STORY_FUNCS), ("quest", QUEST_FUNCS)] {
            let t: Table = s.lua.globals().get(table).unwrap();
            let mut got: Vec<String> = t.pairs::<String, Lv>().map(|p| p.unwrap().0).collect();
            got.sort();
            let mut described: Vec<String> = funcs.iter().map(|f| f.name.to_owned()).collect();
            described.sort();
            assert_eq!(got, described, "{table}");
        }
    }

    #[test]
    fn smith_script_starts_a_talk_once() {
        let (mut s, host) = scripts();
        let smith = r#"
            function update(me, hero, dt)
                if me.touching and not story.busy() then
                    me.started = story.talk("kuznec.privet")
                end
            end
        "#;
        run(&mut s, "kuznec.luau", smith);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        {
            let h = lock(&host);
            assert!(h.busy());
            assert_eq!(h.talk.as_ref().unwrap().dialog, "kuznec.privet");
        }
        // Второй скрипт ждёт: разговор уже идёт.
        run(
            &mut s,
            "drugoi.luau",
            r#"function update(me) assert(story.talk("kuznec.ruda") == false) end"#,
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        // Игра довела разговор до конца — можно новый.
        lock(&host).talk = None;
        run(
            &mut s,
            "tretii.luau",
            r#"function update(me)
                assert(story.seen("kuznec.privet") and not story.seen("kuznec.ruda"))
                assert(story.talk("kuznec.ruda"))
            end"#,
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    #[test]
    fn quests_and_variables_from_a_script() {
        let (mut s, host) = scripts();
        run(
            &mut s,
            "ruda.luau",
            r#"function update(me)
                assert(quest.state("mech") == "none")
                quest.start("mech")
                assert(quest.stage("mech") == "ruda")
                quest.add("mech", "ruda", 2)
                quest.add("mech", "ruda")
                assert(quest.stage("mech") == "otnesti", quest.stage("mech"))
                story.set("hero.money", story.get("hero.money") + 2)
                story.set("kuznec.znakom", true)
                quest.stage("mech", "zhdat")
                quest.complete("mech")
            end"#,
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        let h = lock(&host);
        assert_eq!(h.state.vars["hero.money"], Value::Int(7));
        assert_eq!(h.state.vars["kuznec.znakom"], Value::Bool(true));
        assert_eq!(
            h.story.quest_state(&h.state, "mech").status,
            reyvec_story::Status::Done
        );
    }

    /// Опечатка в имени — ошибка со строкой скрипта, игра идёт дальше.
    #[test]
    fn unknown_names_are_clear_errors() {
        for (code, want) in [
            (
                r#"story.talk("kuznec.privt")"#,
                "нет диалога «kuznec.privt»",
            ),
            (r#"quest.start("mesh")"#, "quest.start: нет задания «mesh»"),
            (r#"story.get("hero.gold")"#, "нет переменной «hero.gold»"),
            (
                r#"story.set("hero.money", "много")"#,
                "целое число, а пришло string",
            ),
            (r#"story.talk = nil"#, "readonly"),
        ] {
            let (mut s, _) = scripts();
            run(
                &mut s,
                "oshibka.luau",
                &format!("function update(me)\n  {code}\nend"),
            );
            let e = s.errors().join("");
            assert!(
                e.contains("oshibka.luau:2") && e.contains(want),
                "{code}: {e}"
            );
        }
    }

    /// Без сюжета (`set_story` не звали) таблиц нет: обычная ошибка «nil», не падение.
    #[test]
    fn without_story_tables_are_missing() {
        let mut s = Scripts::new();
        run(&mut s, "bez.luau", "function update(me) story.busy() end");
        assert!(s.errors()[0].contains("bez.luau:1"), "{:?}", s.errors());
    }

    /// Пример `scenes/scripts/kuznec.luau`: руда собрана — кузнец начинает «руду».
    #[test]
    fn example_smith_script_picks_the_dialog_by_quest_stage() {
        let src = include_str!("../../../scenes/scripts/kuznec.luau");
        let (mut s, host) = scripts();
        {
            let mut h = lock(&host);
            let StoryHost { story, state, .. } = &mut *h;
            story.quest_start(state, "mech").unwrap();
            story.quest_add(state, "mech", "ruda", 3).unwrap();
        }
        run(&mut s, "kuznec.luau", src);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        assert_eq!(lock(&host).talk.as_ref().unwrap().dialog, "kuznec.ruda");
    }

    fn line(h: &StoryHost) -> (String, usize) {
        match &h.screen {
            Some(Screen::Line(s)) => (s.text.clone(), s.choices.len()),
            other => panic!("на экране {other:?}"),
        }
    }

    /// Окно диалога (Сю.7): реплика ждёт «дальше», варианты — выбор, событие
    /// `quest.start` берёт задание, конец закрывает окно. Сохранение помнит всё это.
    #[test]
    fn talk_runs_to_the_end_by_answers_and_is_saved() {
        let (_s, host) = scripts();
        let mut h = lock(&host);
        assert!(h.talk("kuznec.privet").unwrap());
        assert!(h.advance(0.016).is_none());
        assert_eq!(line(&h).0, "Чего надо, чужак? Я кузнец, если что.");
        // Без ответа ничего не меняется.
        h.advance(0.016);
        assert_eq!(line(&h).1, 0);
        h.answer = Some(Answer::Next);
        h.advance(0.016);
        let (text, choices) = line(&h);
        assert_eq!((text.as_str(), choices), ("Что тебе нужно?", 2));
        // «Дальше» на вариантах не годится.
        h.answer = Some(Answer::Next);
        h.advance(0.016);
        assert_eq!(line(&h).0, "Что тебе нужно?");
        h.answer = Some(Answer::Choose(0));
        h.advance(0.016);
        assert!(line(&h).0.starts_with("Меч?"));
        h.answer = Some(Answer::Next);
        h.advance(0.016);
        assert!(!h.busy() && h.screen.is_none(), "{:?}", h.screen);
        assert_eq!(
            h.story.quest_state(&h.state, "mech").status,
            reyvec_story::Status::Active
        );
        assert!(h.problems.is_empty(), "{:?}", h.problems);

        let saved = h.save().unwrap();
        let (story, _) = Story::from_texts(reyvec_story::EXAMPLE);
        let mut other = StoryHost::new(story);
        other.load(&saved).unwrap();
        assert_eq!(other.state, h.state);
        assert!(other.state.talked.contains("kuznec.privet"));
        assert!(other.load("(vars: 5)").is_err());
    }

    /// Карточка «Скрипт» зовёт функцию из файла скрипта, «Подождать» держит паузу.
    #[test]
    fn run_card_calls_a_script_function_and_wait_pauses() {
        let mut texts: Vec<(&str, &str)> = reyvec_story::EXAMPLE.to_vec();
        let dialog = r#"(
            format: "reyvec-dialog", version: 1, id: "proba", with: ["kuznec"],
            nodes: [
                Run(id: "a", func: "nagrada", args: [3], next: "b"),
                Wait(id: "b", seconds: Some(0.5), next: "c"),
                Event(id: "c", name: "zvuk", args: ["molot"], next: "d"),
                Wait(id: "d", next: "e"),
                Run(id: "e", func: "netu", next: "f"),
                End(id: "f"),
            ],
        )"#;
        texts.push(("dialogs/proba.ron", dialog));
        let (story, problems) = Story::from_texts(&texts);
        assert!(problems.iter().all(|p| p.warning), "{problems:?}");
        let mut s = Scripts::new();
        let host = s.set_story(story);
        s.load_text(
            "nagrada.luau",
            r#"function update(me) end
               function nagrada(n) story.set("hero.money", story.get("hero.money") + n) end"#,
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        lock(&host).talk("proba").unwrap();
        s.story_step(0.1);
        assert_eq!(lock(&host).state.vars["hero.money"], Value::Int(8));
        // Пауза полсекунды: событие ещё не случилось.
        s.story_step(0.3);
        assert!(lock(&host).events.is_empty());
        s.story_step(0.3);
        assert_eq!(lock(&host).events, ["zvuk(molot)"]);
        assert_eq!(lock(&host).screen, Some(Screen::Pause));
        lock(&host).answer = Some(Answer::Next);
        s.story_step(0.1);
        let h = lock(&host);
        assert!(!h.busy());
        assert!(
            h.problems[0].contains("нет функции netu()"),
            "{:?}",
            h.problems
        );
    }
}
