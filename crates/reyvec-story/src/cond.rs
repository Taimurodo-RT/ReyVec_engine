//! Условия сюжета: выражения Luau только для чтения (решение 0023).
//!
//! Условие `kuznec.znakom and money >= 10` превращается в функцию `return (…)` один раз,
//! при загрузке. Перед проверкой ей дают окружение из переменных мира: имя с точками
//! становится вложенными таблицами (`kuznec.znakom` → `kuznec = { znakom = … }`), задания —
//! `quest.<имя>.state` (`"none"`, `"active"`, `"done"`, `"failed"`) и `quest.<имя>.stage`.
//! Математика и строки Luau доступны, запись в глобальные — нет (песочница).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use mlua::{Function, Lua, Table, VmState};

use crate::State;
use crate::format::Value;

/// Сколько раз Luau может прерваться внутри одного условия, прежде чем его оборвут
/// (вечный цикл в условии невозможен без функций, но строки бывают тяжёлыми).
const BUDGET: u32 = 10_000;

/// Набор разобранных условий.
pub struct Conditions {
    lua: Lua,
    compiled: HashMap<String, Result<Function, String>>,
    left: Arc<AtomicU32>,
}

impl Default for Conditions {
    fn default() -> Self {
        Self::new()
    }
}

impl Conditions {
    pub fn new() -> Self {
        let lua = Lua::new();
        if let Err(e) = lua.sandbox(true) {
            tracing::warn!("песочница Luau для условий: {e}");
        }
        let _ = lua.set_memory_limit(16 << 20);
        let left = Arc::new(AtomicU32::new(BUDGET));
        let l = Arc::clone(&left);
        lua.set_interrupt(move |_| {
            if l.fetch_sub(1, Ordering::Relaxed) <= 1 {
                return Err(mlua::Error::runtime("условие считается слишком долго"));
            }
            Ok(VmState::Continue)
        });
        Self {
            lua,
            compiled: HashMap::new(),
            left,
        }
    }

    /// Разбирает условие (один раз на текст). Ошибка разбора — текстом.
    pub fn compile(&mut self, expr: &str) -> Result<(), String> {
        if !self.compiled.contains_key(expr) {
            let f = self
                .lua
                .load(format!("return ({expr})"))
                .set_name("условие")
                .into_function()
                .map_err(|e| clean(&e));
            self.compiled.insert(expr.to_string(), f);
        }
        self.compiled[expr].clone().map(|_| ())
    }

    /// Истинно ли условие при этом состоянии мира. Пустое — истинно. Ответ не
    /// «да/нет» (например, `nil` от опечатки в имени) — ошибка.
    pub fn eval(&mut self, expr: &str, state: &State) -> Result<bool, String> {
        if expr.trim().is_empty() {
            return Ok(true);
        }
        self.compile(expr)?;
        let f = self.compiled[expr].clone()?;
        let env = self.env(state).map_err(|e| clean(&e))?;
        f.set_environment(env).map_err(|e| clean(&e))?;
        self.left.store(BUDGET, Ordering::Relaxed);
        let out: mlua::Value = f.call(()).map_err(|e| clean(&e))?;
        match out {
            mlua::Value::Boolean(b) => Ok(b),
            mlua::Value::Nil => Err(format!(
                "«{expr}» дало nil: нет такой переменной? (переменные — в story/vars.ron)"
            )),
            other => Err(format!(
                "«{expr}» дало {}, а нужно да/нет",
                other.type_name()
            )),
        }
    }

    fn env(&self, state: &State) -> mlua::Result<Table> {
        let env = self.lua.create_table()?;
        for (name, value) in &state.vars {
            let mut t = env.clone();
            let mut parts = name.split('.').peekable();
            while let Some(p) = parts.next() {
                if parts.peek().is_none() {
                    t.raw_set(p, self.value(value)?)?;
                } else {
                    let inner = match t.raw_get::<mlua::Value>(p)? {
                        mlua::Value::Table(inner) => inner,
                        _ => {
                            let inner = self.lua.create_table()?;
                            t.raw_set(p, &inner)?;
                            inner
                        }
                    };
                    t = inner;
                }
            }
        }
        let quests = self.lua.create_table()?;
        for (id, q) in &state.quests {
            let t = self.lua.create_table()?;
            t.raw_set("state", q.status.name())?;
            t.raw_set("stage", q.stage.as_str())?;
            quests.raw_set(id.as_str(), t)?;
        }
        env.raw_set("quest", quests)?;
        let meta = self.lua.create_table()?;
        meta.raw_set("__index", self.lua.globals())?;
        env.set_metatable(Some(meta))?;
        Ok(env)
    }

    fn value(&self, v: &Value) -> mlua::Result<mlua::Value> {
        Ok(match v {
            Value::Bool(b) => mlua::Value::Boolean(*b),
            Value::Int(i) => mlua::Value::Integer(*i),
            Value::Float(f) => mlua::Value::Number(*f),
            Value::Text(t) => mlua::Value::String(self.lua.create_string(t)?),
        })
    }
}

/// Ошибка Luau без служебного хвоста (стек вызовов).
fn clean(e: &mlua::Error) -> String {
    let s = e.to_string();
    s.lines().next().unwrap_or("").trim().to_string()
}
