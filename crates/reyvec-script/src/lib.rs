//! Скрипты поведения на Luau (6.9): враги и предметы.
//!
//! Скрипт — файл `.luau` с функцией `update(me, hero, dt)`. Её зовут на каждом шаге
//! логики (60 раз в секунду) для каждого предмета с этим скриптом. `me` — таблица
//! предмета: в ней движок пишет, где предмет и что с ним (`x`, `y`, `vx`, `vy`,
//! `on_ground`, `wall`, `hp`, `alive`, `touching`, `time`), а скрипт пишет, что делать
//! (`move` от -1 до 1, `jump`, `hidden`, `collected`). Свои поля скрипт тоже может
//! хранить в `me`: таблица у каждого предмета своя и живёт, пока живёт мир. `hero` —
//! только для чтения: где герой и бьёт ли он.
//!
//! Правка на лету: [`Scripts::reload_changed`] смотрит время изменения файлов и
//! перечитывает изменённые. Состояние предметов (`me`) при этом остаётся. Ошибка в
//! скрипте не роняет игру: старая версия скрипта продолжает работать (или предмет
//! стоит, если старой нет), а текст ошибки со строкой виден на панели.
//!
//! Защита: скрипты в песочнице Luau (нет доступа к файлам и системе, общие таблицы
//! только для чтения), у каждого файла свои глобальные имена, на вызов — не больше
//! [`CALL_BUDGET_TICKS`] проверок Luau и не дольше [`CALL_BACKSTOP_MS`] по часам
//! (вечный цикл обрывается), на всё — не больше [`MEMORY_LIMIT`] байт.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime};

use mlua::{Function, Lua, RegistryKey, Table, VmState};

/// Сколько раз один вызов скрипта может пройти проверку Luau (она стоит на каждом обороте
/// цикла и вызове функции), прежде чем его оборвут. Счёт, а не часы: раньше предел был
/// 5 мс по часам, и вызов, который система на миг остановила (загруженный процессор,
/// рывок), обрывался, а скрипт выключался до правки файла — монетки замирали, и один и
/// тот же шаг логики давал разное. Миллион проверок — десятки миллисекунд счёта: обычный
/// скрипт тратит сотни, вечный цикл обрывается быстро и одинаково на любом компьютере.
pub const CALL_BUDGET_TICKS: u64 = 1_000_000;
/// Запасной предел вызова по часам. Проверка Luau не стоит внутри встроенных функций
/// (`string.rep`, `table.concat`…): цикл из таких вызовов тратит проверку на оборот, а
/// оборот — миллисекунды, и по счёту его не оборвать за разумное время. Секунда — далеко
/// за любой остановкой потока системой, поэтому обычные вызовы по часам не обрываются.
pub const CALL_BACKSTOP_MS: u64 = 1000;
/// Память всех скриптов вместе.
pub const MEMORY_LIMIT: usize = 64 << 20;

/// Номер загруженного скрипта.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScriptId(usize);

/// Что движок сообщает скрипту о предмете.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActorIn {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub on_ground: bool,
    /// Стена сбоку: -1 слева, 1 справа, 0 нет.
    pub wall: f64,
    pub hp: u32,
    pub alive: bool,
    /// Касается героя.
    pub touching: bool,
    /// Время сцены, секунды.
    pub time: f64,
}

/// Где герой (для скрипта).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeroIn {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub striking: bool,
}

/// Что скрипт решил.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActorOut {
    /// Бег: от -1 (влево) до 1 (вправо), доля наибольшей скорости.
    pub move_x: f64,
    pub jump: bool,
    pub hidden: bool,
    /// Подобран (предмет): движок один раз даёт событие «подобрал».
    pub collected: bool,
    /// Место после вызова (скрипт предмета может его поменять).
    pub x: Option<f64>,
    pub y: Option<f64>,
}

/// Предмет со скриптом: его таблица `me`.
pub struct Actor {
    script: ScriptId,
    me: RegistryKey,
}

impl Actor {
    pub fn script(&self) -> ScriptId {
        self.script
    }
}

struct ScriptFile {
    path: PathBuf,
    /// Имя для сообщений: имя файла.
    name: String,
    modified: Option<SystemTime>,
    update: Option<Function>,
    error: Option<String>,
}

/// Все скрипты мира.
pub struct Scripts {
    lua: Lua,
    files: Vec<ScriptFile>,
    /// Сколько проверок Luau осталось текущему вызову (0 — вызова нет, не обрывать).
    remaining: Arc<AtomicU64>,
    /// Запасной предел текущего вызова: наносекунды от `epoch`.
    deadline: Arc<AtomicU64>,
    epoch: Instant,
    /// Сколько раз перечитаны файлы (для панели и тестов).
    pub reloads: u32,
}

impl Default for Scripts {
    fn default() -> Self {
        Self::new()
    }
}

impl Scripts {
    pub fn new() -> Self {
        let lua = Lua::new();
        if let Err(e) = lua.sandbox(true) {
            tracing::warn!("песочница Luau: {e}");
        }
        let _ = lua.set_memory_limit(MEMORY_LIMIT);
        let remaining = Arc::new(AtomicU64::new(0));
        let deadline = Arc::new(AtomicU64::new(0));
        let epoch = Instant::now();
        let (r, d) = (Arc::clone(&remaining), Arc::clone(&deadline));
        lua.set_interrupt(move |_| {
            let left = r.load(Ordering::Relaxed);
            if left == 0 {
                return Ok(VmState::Continue);
            }
            if left == 1 || epoch.elapsed().as_nanos() as u64 > d.load(Ordering::Relaxed) {
                return Err(mlua::Error::runtime(
                    "скрипт думает слишком долго (вечный цикл?): вызов оборван",
                ));
            }
            r.store(left - 1, Ordering::Relaxed);
            Ok(VmState::Continue)
        });
        Self {
            lua,
            files: Vec::new(),
            remaining,
            deadline,
            epoch,
            reloads: 0,
        }
    }

    /// Загружает скрипт из файла (один и тот же файл — один раз). Ошибка не мешает:
    /// она в [`Self::errors`], а предметы с этим скриптом стоят.
    pub fn load(&mut self, path: &Path) -> ScriptId {
        if let Some(i) = self.files.iter().position(|f| f.path == path) {
            return ScriptId(i);
        }
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        self.files.push(ScriptFile {
            path: path.to_path_buf(),
            name,
            modified: None,
            update: None,
            error: None,
        });
        let i = self.files.len() - 1;
        self.compile(i);
        ScriptId(i)
    }

    /// Загружает скрипт из текста (для тестов и встроенных скриптов): без файла, без
    /// правки на лету.
    pub fn load_text(&mut self, name: &str, source: &str) -> ScriptId {
        self.files.push(ScriptFile {
            path: PathBuf::new(),
            name: name.into(),
            modified: None,
            update: None,
            error: None,
        });
        let i = self.files.len() - 1;
        self.compile_source(i, source);
        ScriptId(i)
    }

    fn compile(&mut self, i: usize) {
        let f = &mut self.files[i];
        f.modified = std::fs::metadata(&f.path).and_then(|m| m.modified()).ok();
        match std::fs::read_to_string(&f.path) {
            Ok(source) => self.compile_source(i, &source),
            Err(e) => {
                f.error = Some(format!("{}: не прочитать файл: {e}", f.path.display()));
            }
        }
    }

    fn compile_source(&mut self, i: usize, source: &str) {
        let name = self.files[i].name.clone();
        let result = (|| -> mlua::Result<Function> {
            let env = self.lua.create_table()?;
            let meta = self.lua.create_table()?;
            meta.raw_set("__index", self.lua.globals())?;
            env.set_metatable(Some(meta))?;
            self.arm();
            let run = self
                .lua
                .load(source)
                .set_name(format!("={name}"))
                .set_environment(env.clone())
                .exec();
            self.disarm();
            run?;
            env.get::<Option<Function>>("update")?
                .ok_or_else(|| mlua::Error::runtime("нет функции update(me, hero, dt)"))
        })();
        let f = &mut self.files[i];
        match result {
            Ok(update) => {
                f.update = Some(update);
                f.error = None;
            }
            Err(e) => {
                // Старая версия (если была) продолжает работать.
                let text = named(&name, &e);
                tracing::warn!("скрипт: {text}");
                f.error = Some(text);
            }
        }
    }

    fn arm(&self) {
        let limit = self.epoch.elapsed().as_nanos() as u64 + CALL_BACKSTOP_MS * 1_000_000;
        self.deadline.store(limit, Ordering::Relaxed);
        self.remaining.store(CALL_BUDGET_TICKS, Ordering::Relaxed);
    }

    fn disarm(&self) {
        self.remaining.store(0, Ordering::Relaxed);
    }

    /// Перечитывает изменённые файлы. Возвращает, сколько перечитано.
    pub fn reload_changed(&mut self) -> usize {
        let mut n = 0;
        for i in 0..self.files.len() {
            let f = &self.files[i];
            if f.path.as_os_str().is_empty() {
                continue;
            }
            let now = std::fs::metadata(&f.path).and_then(|m| m.modified()).ok();
            if now != f.modified {
                tracing::info!("скрипт {} изменён: перечитываю", f.name);
                self.compile(i);
                n += 1;
            }
        }
        self.reloads += n as u32;
        n
    }

    /// Новый предмет со скриптом `script`.
    pub fn actor(&self, script: ScriptId) -> Option<Actor> {
        let me = self.lua.create_table().ok()?;
        let me = self.lua.create_registry_value(me).ok()?;
        Some(Actor { script, me })
    }

    /// Шаг скрипта для предмета.
    pub fn run(&mut self, actor: &Actor, input: &ActorIn, hero: &HeroIn, dt: f64) -> ActorOut {
        let i = actor.script.0;
        let Some(update) = self.files[i].update.clone() else {
            return ActorOut::default();
        };
        let result = (|| -> mlua::Result<ActorOut> {
            let me: Table = self.lua.registry_value(&actor.me)?;
            me.raw_set("x", input.x)?;
            me.raw_set("y", input.y)?;
            me.raw_set("vx", input.vx)?;
            me.raw_set("vy", input.vy)?;
            me.raw_set("on_ground", input.on_ground)?;
            me.raw_set("wall", input.wall)?;
            me.raw_set("hp", input.hp)?;
            me.raw_set("alive", input.alive)?;
            me.raw_set("touching", input.touching)?;
            me.raw_set("time", input.time)?;
            me.raw_set("jump", false)?;
            let h = self.lua.create_table()?;
            h.raw_set("x", hero.x)?;
            h.raw_set("y", hero.y)?;
            h.raw_set("vx", hero.vx)?;
            h.raw_set("vy", hero.vy)?;
            h.raw_set("striking", hero.striking)?;
            self.arm();
            let called = update.call::<()>((me.clone(), h, dt));
            self.disarm();
            called?;
            let num = |k: &str| me.raw_get::<Option<f64>>(k).map(|v| v.unwrap_or(0.0));
            let flag = |k: &str| me.raw_get::<Option<bool>>(k).map(|v| v.unwrap_or(false));
            Ok(ActorOut {
                move_x: num("move")?.clamp(-1.0, 1.0),
                jump: flag("jump")?,
                hidden: flag("hidden")?,
                collected: flag("collected")?,
                x: me.raw_get("x")?,
                y: me.raw_get("y")?,
            })
        })();
        match result {
            Ok(out) => out,
            Err(e) => {
                let f = &mut self.files[i];
                let text = named(&f.name, &e);
                tracing::warn!("скрипт: {text}");
                // Сломанный скрипт выключается до правки файла: иначе ошибка 60 раз в
                // секунду.
                f.update = None;
                f.error = Some(text);
                ActorOut::default()
            }
        }
    }

    /// Ошибки скриптов (по одной на файл).
    pub fn errors(&self) -> Vec<String> {
        self.files.iter().filter_map(|f| f.error.clone()).collect()
    }

    /// Сколько скриптов загружено.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Память скриптов, байт.
    pub fn memory(&self) -> usize {
        self.lua.used_memory()
    }
}

/// Ошибка с именем файла впереди (Luau обычно уже пишет «файл:строка:»).
fn named(name: &str, e: &mlua::Error) -> String {
    let text = short(e);
    if text.starts_with(name) {
        text
    } else {
        format!("{name}: {text}")
    }
}

/// Ошибка Luau одной строкой: без стека вызовов.
fn short(e: &mlua::Error) -> String {
    let text = match e {
        mlua::Error::CallbackError { cause, .. } => return short(cause),
        other => other.to_string(),
    };
    text.lines()
        .next()
        .unwrap_or("")
        .trim_start_matches("runtime error: ")
        .trim_start_matches("syntax error: ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WALKER: &str = r#"
        -- Идёт к герою, у стены прыгает; считает свои шаги в me.
        function update(me, hero, dt)
            me.steps = (me.steps or 0) + 1
            if hero.x > me.x then me.move = 1 else me.move = -1 end
            if me.wall ~= 0 and me.on_ground then me.jump = true end
            if me.touching then me.collected = true; me.hidden = true end
        end
    "#;

    fn input() -> ActorIn {
        ActorIn {
            on_ground: true,
            alive: true,
            hp: 3,
            ..ActorIn::default()
        }
    }

    #[test]
    fn script_decides_and_keeps_its_own_state() {
        let mut s = Scripts::new();
        let id = s.load_text("walker.luau", WALKER);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        let a = s.actor(id).unwrap();
        let b = s.actor(id).unwrap();
        let hero = HeroIn {
            x: 1.0,
            ..HeroIn::default()
        };
        let out = s.run(&a, &input(), &hero, 1.0 / 60.0);
        assert_eq!(out.move_x, 1.0);
        assert!(!out.jump && !out.hidden);
        let walled = ActorIn {
            wall: 1.0,
            x: 2.0,
            touching: true,
            ..input()
        };
        let out = s.run(&a, &walled, &hero, 1.0 / 60.0);
        assert_eq!(out.move_x, -1.0);
        assert!(out.jump && out.collected && out.hidden);
        // jump сбрасывается каждый шаг, свои поля — у каждого предмета свои.
        let out = s.run(&a, &input(), &hero, 1.0 / 60.0);
        assert!(!out.jump);
        s.run(&b, &input(), &hero, 1.0 / 60.0);
        let steps = |x: &Actor| {
            s.lua
                .registry_value::<Table>(&x.me)
                .unwrap()
                .get::<u32>("steps")
                .unwrap()
        };
        assert_eq!((steps(&a), steps(&b)), (3, 1));
    }

    #[test]
    fn errors_name_the_file_and_line_and_do_not_stop_the_game() {
        let mut s = Scripts::new();
        let bad = s.load_text(
            "bad.luau",
            "function update(me, hero, dt)\n  me.move = = 1\nend",
        );
        let e = s.errors();
        assert!(e[0].starts_with("bad.luau:2"), "{e:?}");
        let a = s.actor(bad).unwrap();
        assert_eq!(
            s.run(&a, &input(), &HeroIn::default(), 0.016),
            ActorOut::default()
        );
        let mut s = Scripts::new();
        let runtime = s.load_text(
            "oops.luau",
            "function update(me, hero, dt)\n  local t = nil\n  me.move = t.x\nend",
        );
        let a = s.actor(runtime).unwrap();
        s.run(&a, &input(), &HeroIn::default(), 0.016);
        let e = s.errors();
        assert!(e[0].starts_with("oops.luau:3"), "{e:?}");
        let mut s = Scripts::new();
        s.load_text("none.luau", "local x = 1");
        assert!(
            s.errors()[0].contains("нет функции update"),
            "{:?}",
            s.errors()
        );
    }

    #[test]
    fn endless_loop_is_cut_and_sandbox_has_no_files() {
        let mut s = Scripts::new();
        let id = s.load_text(
            "loop.luau",
            "function update(me, hero, dt) while true do end end",
        );
        let a = s.actor(id).unwrap();
        let started = Instant::now();
        s.run(&a, &input(), &HeroIn::default(), 0.016);
        let took = started.elapsed();
        eprintln!("вечный цикл оборван через {took:?}");
        // Обрыв считается проверками, не часами: предел по времени — только от зависания.
        assert!(took.as_secs() < 10, "{took:?}");
        assert!(s.errors()[0].contains("оборван"), "{:?}", s.errors());
        // Цикл из тяжёлых встроенных вызовов (проверка Luau — раз на оборот) обрывает
        // запасной предел по часам.
        let mut s = Scripts::new();
        let id = s.load_text(
            "rep.luau",
            "function update(me) while true do local x = string.rep('x', 100000) end end",
        );
        let a = s.actor(id).unwrap();
        let started = Instant::now();
        s.run(&a, &input(), &HeroIn::default(), 0.016);
        let took = started.elapsed();
        eprintln!("цикл встроенных вызовов оборван через {took:?}");
        assert!(took.as_secs() < 10, "{took:?}");
        assert!(s.errors()[0].contains("оборван"), "{:?}", s.errors());
        let mut s = Scripts::new();
        s.load_text(
            "io.luau",
            "local f = io.open('x') function update(me, hero, dt) end",
        );
        assert!(!s.errors().is_empty(), "в песочнице нет io");
        // Глобальные имена у каждого файла свои.
        let mut s = Scripts::new();
        let a = s.load_text(
            "a.luau",
            "speed = 1 function update(me) me.move = speed end",
        );
        let b = s.load_text("b.luau", "function update(me) me.move = speed or 0.5 end");
        let (aa, bb) = (s.actor(a).unwrap(), s.actor(b).unwrap());
        assert_eq!(s.run(&aa, &input(), &HeroIn::default(), 0.0).move_x, 1.0);
        assert_eq!(s.run(&bb, &input(), &HeroIn::default(), 0.0).move_x, 0.5);
    }

    /// Предел вызова считается проверками Luau, а не часами (CI 2026-11-11: под нагрузкой
    /// вызов монетки обрывался по 5 мс, скрипт выключался, и кадры одной сцены
    /// расходились). Долгий, но конечный счёт не обрывается, сколько бы он ни шёл по
    /// часам; ровно столько же оборотов — всегда одинаково.
    #[test]
    fn call_budget_counts_work_not_time() {
        let mut s = Scripts::new();
        let id = s.load_text(
            "busy.luau",
            "function update(me, hero, dt) local n = 0 for i = 1, 200000 do n = n + i end me.move = 1 end",
        );
        let a = s.actor(id).unwrap();
        for _ in 0..3 {
            let out = s.run(&a, &input(), &HeroIn::default(), 0.016);
            assert_eq!(out.move_x, 1.0, "{:?}", s.errors());
        }
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    #[test]
    fn edited_file_is_picked_up_and_broken_edit_keeps_the_old_one() {
        let dir = std::env::temp_dir().join(format!("reyvec-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("slime.luau");
        std::fs::write(&path, "function update(me) me.move = 0.25 end").unwrap();
        let mut s = Scripts::new();
        let id = s.load(&path);
        let a = s.actor(id).unwrap();
        let run = |s: &mut Scripts| s.run(&a, &input(), &HeroIn::default(), 0.0).move_x;
        assert_eq!(run(&mut s), 0.25);
        assert_eq!(s.reload_changed(), 0);
        // Время файла меняется не чаще раза в секунду на некоторых системах: ставим сами.
        let touch = |text: &str, secs: u64| {
            std::fs::write(&path, text).unwrap();
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(SystemTime::now() + std::time::Duration::from_secs(secs))
                .unwrap();
        };
        touch("function update(me) me.move = -0.5 end", 5);
        assert_eq!(s.reload_changed(), 1);
        assert_eq!(run(&mut s), -0.5);
        touch("function update(me) me.move = end", 10);
        assert_eq!(s.reload_changed(), 1);
        assert_eq!(run(&mut s), -0.5, "сломанная правка — работает прежний");
        assert!(
            s.errors()[0].starts_with("slime.luau:1"),
            "{:?}",
            s.errors()
        );
        assert_eq!(s.reloads, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
