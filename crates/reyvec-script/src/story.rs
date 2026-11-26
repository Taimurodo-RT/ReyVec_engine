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
use reyvec_story::{State, Story, Talk, Value};

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
    /// Начатый разговор (его ведёт окно диалога игры).
    pub talk: Option<Talk>,
    /// Какие диалоги уже начинались (для `story.seen`).
    pub talked: std::collections::BTreeSet<String>,
}

impl StoryHost {
    pub fn new(story: Story) -> Self {
        let state = story.new_state();
        Self {
            story,
            state,
            talk: None,
            talked: Default::default(),
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
        self.talked.insert(dialog.to_string());
        self.talk = Some(t);
        Ok(true)
    }
}

/// Общий сюжет: скрипты и игра держат одну и ту же копию.
pub type SharedStory = Arc<Mutex<StoryHost>>;

pub(crate) fn lock(s: &SharedStory) -> MutexGuard<'_, StoryHost> {
    // Скрипт, упавший посреди вызова, не должен навсегда закрыть сюжет.
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn to_lua(lua: &Lua, v: &Value) -> mlua::Result<Lv> {
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
            Ok(())
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
            Ok(h.talked.contains(&dialog))
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
}
