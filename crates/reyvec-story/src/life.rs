//! Жизнь персонажей (Сю.12): распорядок дня и разговоры между собой.
//!
//! У персонажа есть распорядок (`day`): с какого часа где он и чем занят («с 7 — в кузне,
//! работает»). Часы идут от времени игры [`State::time`]: в часе игры [`HOUR`] секунд,
//! игра начинается в [`START_HOUR`]. Есть и разговоры (`chats`): с кем, какой диалог, при
//! каком условии и как часто. Если оба оказались в одном месте, диалог проигрывается сам,
//! без игрока (первый видимый вариант ответа), — его события меняют мир и память, а
//! реплики слышит герой, если кто-то из говорящих рядом.
//!
//! Чтобы сотня персонажей не тратила время зря, они считаются тем реже, чем дальше от
//! героя ([`Story::life_tick`]): рядом — каждый шаг, поодаль — раз в секунду, далеко — раз
//! в десять секунд. Далёкие разговоры идут молча: мир меняется, реплик не слышно.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{State, Step, Story};

/// Секунд игры в часе игры: день — 24 минуты.
pub const HOUR: f64 = 60.0;
/// С какого часа начинается игра.
pub const START_HOUR: f64 = 8.0;
/// Ближе стольких метров персонаж считается каждый шаг.
pub const NEAR: f64 = 15.0;
/// Ближе стольких метров — раз в [`MIDDLE_STEP`], дальше — раз в [`FAR_STEP`].
pub const MIDDLE: f64 = 60.0;
pub const MIDDLE_STEP: f64 = 1.0;
pub const FAR_STEP: f64 = 10.0;
/// Сколько секунд звучит одна подслушанная реплика.
pub const LINE_SECONDS: f64 = 3.0;
/// Длиннее разговор не проигрываем (защита от петли в диалоге).
const MAX_CHAT_STEPS: usize = 200;

/// Строка распорядка: с какого часа где и что.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Routine {
    /// Час игры, 0–24 (можно с долями: 7.5 — полвосьмого).
    pub from: f64,
    /// Место — имя для скриптов и сцены (`kuznya`, `dom`, `ploshad`).
    pub place: String,
    /// Чем занят (`rabotaet`, `spit`), для скриптов и модели.
    #[serde(default)]
    pub doing: String,
}

/// Разговор с другим персонажем, когда оба в одном месте.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chat {
    pub with: String,
    pub dialog: String,
    /// Условие Luau. Пусто — всегда.
    #[serde(default)]
    pub when: String,
    /// Не чаще, чем раз в столько часов игры.
    #[serde(default = "every")]
    pub every: f64,
}

fn every() -> f64 {
    1.0
}

/// Что сейчас с персонажем (часть сохранения).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Life {
    pub place: String,
    pub doing: String,
    /// Сколько секунд накопилось с прошлого расчёта (дальних считаем реже).
    pub wait: f64,
    /// Когда (время игры) последний раз говорил с кем: имя → секунды.
    pub chatted: BTreeMap<String, f64>,
    /// Реплики разговора, которые ещё прозвучат: кто, текст.
    pub saying: VecDeque<(String, String)>,
    /// Через сколько секунд следующая реплика.
    pub next_line: f64,
}

/// Что случилось в жизни персонажей за шаг.
#[derive(Clone, Debug, PartialEq)]
pub enum LifeEvent {
    /// Перешёл в другое место или занялся другим.
    Moved {
        who: String,
        place: String,
        doing: String,
    },
    /// Сказал реплику в разговоре, который слышит герой.
    Said {
        who: String,
        name: String,
        color: String,
        text: String,
    },
}

/// Час игры (0–24) в это время игры.
pub fn hour(time: f64) -> f64 {
    (START_HOUR + time / HOUR).rem_euclid(24.0)
}

/// Строка распорядка, которая действует в час `h`. Распорядок идёт по кругу: до первой
/// строки дня действует последняя (кузнец спит с 22 до 7).
pub fn routine_at(day: &[Routine], h: f64) -> Option<&Routine> {
    day.iter()
        .filter(|r| r.from <= h)
        .max_by(|a, b| a.from.total_cmp(&b.from))
        .or_else(|| day.iter().max_by(|a, b| a.from.total_cmp(&b.from)))
}

impl Story {
    /// Где персонаж и чем занят сейчас (по распорядку).
    pub fn now_of(&self, state: &State, who: &str) -> Option<&Routine> {
        routine_at(&self.characters.get(who)?.day, hour(state.time))
    }

    /// Шаг жизни персонажей: `dt` секунд прошло (время игры двигает
    /// [`Story::memory_tick`]), `distance` — как далеко персонаж от героя, метры
    /// (`f64::INFINITY` — не знаем: персонажа нет в сцене).
    pub fn life_tick(
        &mut self,
        state: &mut State,
        dt: f64,
        distance: &dyn Fn(&str) -> f64,
    ) -> Vec<LifeEvent> {
        let mut out = Vec::new();
        let h = hour(state.time);
        let living: Vec<String> = self
            .characters
            .values()
            .filter(|c| !c.day.is_empty() || !c.chats.is_empty())
            .map(|c| c.id.clone())
            .collect();
        for who in living {
            let d = distance(&who);
            let step = if d < NEAR {
                0.0
            } else if d < MIDDLE {
                MIDDLE_STEP
            } else {
                FAR_STEP
            };
            let life = state.life.entry(who.clone()).or_default();
            life.wait += dt.max(0.0);
            if life.wait < step {
                continue;
            }
            let passed = std::mem::take(&mut life.wait);
            // Подслушанные реплики: по одной в LINE_SECONDS; далеко — не слышно.
            if !life.saying.is_empty() {
                life.next_line -= passed;
                if d >= MIDDLE {
                    life.saying.clear();
                } else if life.next_line <= 0.0
                    && let Some((by, text)) = life.saying.pop_front()
                {
                    life.next_line = LINE_SECONDS;
                    let c = self.characters.get(&by);
                    out.push(LifeEvent::Said {
                        name: c.map_or_else(|| by.clone(), |c| c.name.clone()),
                        color: c.map(|c| c.color.clone()).unwrap_or_default(),
                        who: by,
                        text,
                    });
                }
            }
            // Распорядок.
            if let Some(r) = self.now_of(state, &who).cloned() {
                let life = state.life.entry(who.clone()).or_default();
                if life.place != r.place || life.doing != r.doing {
                    life.place = r.place.clone();
                    life.doing = r.doing.clone();
                    out.push(LifeEvent::Moved {
                        who: who.clone(),
                        place: r.place,
                        doing: r.doing,
                    });
                }
            }
            self.start_chats(state, &who, h, d, distance);
        }
        out
    }

    /// Начать разговоры `who`, для которых пришло время.
    fn start_chats(
        &mut self,
        state: &mut State,
        who: &str,
        h: f64,
        d: f64,
        distance: &dyn Fn(&str) -> f64,
    ) {
        let Some(c) = self.characters.get(who) else {
            return;
        };
        let here = routine_at(&c.day, h).map(|r| r.place.clone());
        let busy = |s: &State, id: &str| s.life.get(id).is_some_and(|l| !l.saying.is_empty());
        for chat in c.chats.clone() {
            if busy(state, who) {
                return;
            }
            if busy(state, &chat.with) {
                continue;
            }
            let there = self
                .characters
                .get(&chat.with)
                .and_then(|o| routine_at(&o.day, h))
                .map(|r| r.place.clone());
            if here.is_none() || here != there {
                continue;
            }
            let last = state
                .life
                .get(who)
                .and_then(|l| l.chatted.get(&chat.with))
                .copied();
            if last.is_some_and(|t| state.time - t < chat.every * HOUR) {
                continue;
            }
            if !chat.when.trim().is_empty() && !self.eval(&chat.when, state).unwrap_or(false) {
                continue;
            }
            let lines = self.overhear(state, &chat.dialog);
            let now = state.time;
            for id in [who, chat.with.as_str()] {
                state
                    .life
                    .entry(id.to_string())
                    .or_default()
                    .chatted
                    .insert(if id == who { &chat.with } else { who }.to_string(), now);
            }
            // Слышно, если рядом хоть один из двоих.
            if d.min(distance(&chat.with)) < MIDDLE {
                let life = state.life.entry(who.to_string()).or_default();
                life.saying = lines.into();
                life.next_line = 0.0;
            }
        }
    }

    /// Проиграть диалог без игрока: реплики по порядку (кто, текст). Вариант ответа —
    /// первый видимый; события и переменные меняют мир, как в обычном разговоре.
    pub fn overhear(&mut self, state: &mut State, dialog: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let Ok(mut talk) = self.talk(dialog) else {
            return out;
        };
        for _ in 0..MAX_CHAT_STEPS {
            match self.step(&mut talk, state) {
                Step::Line(l) => {
                    out.push((l.who, l.text));
                    if !l.choices.is_empty() {
                        talk.choose(0, state);
                    }
                }
                Step::End => break,
                _ => {}
            }
        }
        out
    }
}
