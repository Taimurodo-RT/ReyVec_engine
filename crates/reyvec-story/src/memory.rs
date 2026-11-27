//! Память и отношения персонажей (Сю.9).
//!
//! У каждого персонажа — список фактов («герой принёс три куска руды») и отношения к
//! другим (`hero`: +3). Факты пишут диалоги (события `memory.add`, `memory.forget`),
//! задания и скрипты (`npc.remember`, `npc.forget`); отношения — события `relation.add`,
//! `relation.set` и `npc.relation` в скриптах. Условия читают их так:
//! `memory.kuznec.ruda_prinesena` (да/нет), `relation.kuznec.hero >= 2` (число).
//!
//! Важность факта — от 1 до 10. Неважное со временем забывается: факт важности `n` живёт
//! `n × FORGET_STEP` секунд игры, факт важности 10 — всегда. У персонажа не больше
//! [`MAX_FACTS`] фактов: лишние вытесняются самые слабые (мало важности, давно было).
//! Так устроены «генеративные агенты» (Стэнфорд, 2023): память с важностью и временем;
//! позже (Сю.10) модель будет брать из памяти самое важное и свежее.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::format::Value;
use crate::{State, Story};

/// Сколько секунд игры живёт одна единица важности (важность 1 — 10 минут).
pub const FORGET_STEP: f64 = 600.0;
/// Важность, при которой факт не забывается.
pub const NEVER_FORGET: u8 = 10;
/// Больше фактов у одного персонажа не держим.
pub const MAX_FACTS: usize = 64;

/// Факт в памяти персонажа.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    /// Имя для условий и кода (`ruda_prinesena`).
    pub key: String,
    /// Что помнит — словами (для автора, журнала и модели).
    pub text: String,
    /// 1–10.
    pub importance: u8,
    /// Когда запомнил: секунды игры ([`State::time`]).
    pub at: f64,
}

impl Fact {
    /// Сила факта сейчас: важность минус прошедшее время в единицах забывания.
    pub fn strength(&self, now: f64) -> f64 {
        if self.importance >= NEVER_FORGET {
            return f64::INFINITY;
        }
        f64::from(self.importance) - (now - self.at).max(0.0) / FORGET_STEP
    }
}

/// Память одного персонажа.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    pub facts: Vec<Fact>,
    /// К кому как относится: имя персонажа (`hero`) → число (0 — никак, больше — лучше).
    pub relations: BTreeMap<String, i64>,
}

impl Memory {
    pub fn knows(&self, key: &str) -> bool {
        self.facts.iter().any(|f| f.key == key)
    }

    pub fn relation(&self, whom: &str) -> i64 {
        self.relations.get(whom).copied().unwrap_or(0)
    }
}

impl Story {
    /// Память персонажа (пустая, если он ничего не помнит).
    pub fn memory<'a>(&self, state: &'a State, npc: &str) -> Option<&'a Memory> {
        state.memory.get(npc)
    }

    fn npc<'a>(&self, state: &'a mut State, npc: &str) -> Result<&'a mut Memory, String> {
        if !self.characters.contains_key(npc) {
            return Err(format!("нет персонажа «{npc}»"));
        }
        Ok(state.memory.entry(npc.to_string()).or_default())
    }

    /// Запомнить факт. Тот же ключ — факт обновляется (текст, важность, время).
    pub fn remember(
        &self,
        state: &mut State,
        npc: &str,
        key: &str,
        text: &str,
        importance: u8,
    ) -> Result<(), String> {
        if !crate::valid_id(key) {
            return Err(format!(
                "имя факта «{key}»: только латиница в нижнем регистре, цифры, _ и ."
            ));
        }
        let now = state.time;
        let m = self.npc(state, npc)?;
        let fact = Fact {
            key: key.into(),
            text: text.into(),
            importance: importance.clamp(1, NEVER_FORGET),
            at: now,
        };
        match m.facts.iter_mut().find(|f| f.key == key) {
            Some(f) => *f = fact,
            None => m.facts.push(fact),
        }
        if m.facts.len() > MAX_FACTS {
            // Вытесняется самый слабый.
            if let Some(weak) = (0..m.facts.len()).min_by(|&a, &b| {
                m.facts[a]
                    .strength(now)
                    .total_cmp(&m.facts[b].strength(now))
            }) {
                m.facts.remove(weak);
            }
        }
        Ok(())
    }

    /// Забыть факт. Не помнил — ничего.
    pub fn forget(&self, state: &mut State, npc: &str, key: &str) -> Result<(), String> {
        self.npc(state, npc)?.facts.retain(|f| f.key != key);
        Ok(())
    }

    /// Изменить отношение `npc` к `whom`: прибавить (`add`) или поставить.
    pub fn relate(
        &self,
        state: &mut State,
        npc: &str,
        whom: &str,
        n: i64,
        add: bool,
    ) -> Result<i64, String> {
        if !self.characters.contains_key(whom) {
            return Err(format!("нет персонажа «{whom}»"));
        }
        let m = self.npc(state, npc)?;
        let r = m.relations.entry(whom.to_string()).or_insert(0);
        *r = if add { *r + n } else { n };
        Ok(*r)
    }

    /// Время игры прошло: `dt` секунд. Слабые факты забываются.
    pub fn memory_tick(&self, state: &mut State, dt: f64) {
        state.time += dt.max(0.0);
        let now = state.time;
        for m in state.memory.values_mut() {
            m.facts.retain(|f| f.strength(now) > 0.0);
        }
    }

    /// События `memory.*` и `relation.*` из карточки «Событие».
    pub(crate) fn memory_event(
        &self,
        state: &mut State,
        name: &str,
        args: &[Value],
    ) -> Result<(), String> {
        let text = |i: usize| match args.get(i) {
            Some(Value::Text(t)) => Ok(t.as_str()),
            _ => Err(format!("{name}: аргумент {} должен быть текстом", i + 1)),
        };
        let num = |i: usize, default: f64| args.get(i).and_then(Value::number).unwrap_or(default);
        match name {
            "memory.add" => self.remember(
                state,
                text(0)?,
                text(1)?,
                args.get(2).map_or("", |v| match v {
                    Value::Text(t) => t.as_str(),
                    _ => "",
                }),
                num(3, 5.0).clamp(1.0, 10.0) as u8,
            ),
            "memory.forget" => self.forget(state, text(0)?, text(1)?),
            "relation.add" => self
                .relate(state, text(0)?, text(1)?, num(2, 1.0) as i64, true)
                .map(|_| ()),
            "relation.set" => self
                .relate(state, text(0)?, text(1)?, num(2, 0.0) as i64, false)
                .map(|_| ()),
            _ => Err(format!("нет события {name}")),
        }
    }
}

/// События памяти и отношений, которые выполняет сам сюжет: имя, аргументы, что делает.
pub const EVENTS: &[(&str, &str, &str)] = &[
    (
        "memory.add",
        "персонаж, факт, текст, важность 1–10",
        "персонаж запомнил",
    ),
    ("memory.forget", "персонаж, факт", "персонаж забыл"),
    (
        "relation.add",
        "персонаж, к кому, сколько",
        "отношение лучше (или хуже, если меньше нуля)",
    ),
    (
        "relation.set",
        "персонаж, к кому, число",
        "отношение — ровно",
    ),
];

/// Сколько текстовых аргументов нужно событию в начале (для проверки сюжета).
pub fn texts_needed(name: &str) -> Option<usize> {
    Some(match name {
        "memory.add" | "memory.forget" | "relation.add" | "relation.set" => 2,
        _ => return None,
    })
}
