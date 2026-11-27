//! Проигрыватель диалога: идёт по карточкам до реплики, события или конца.

use crate::format::{LineSettings, Node, Value};
use crate::{State, Story};

/// Сколько «тихих» карточек (развилки, переменные, переходы) подряд можно пройти, прежде
/// чем решить, что диалог зациклился.
const SILENT_LIMIT: usize = 1000;

/// Идущий разговор.
#[derive(Clone, Debug, PartialEq)]
pub struct Talk {
    pub dialog: String,
    /// Карточка, с которой продолжится разговор.
    pub node: String,
    pending: Pending,
    /// Последняя показанная реплика с вариантами (пока игрок не выбрал).
    shown: Option<Shown>,
    /// Почему разговор оборвался (ошибка в данных), для консоли и редактора.
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Pending {
    /// Начать с `node`.
    Start,
    /// Идти в карточку (пусто — конец).
    Then(String),
    /// Ждём выбор: куда ведёт каждый показанный вариант и ключ варианта.
    Choices(Vec<(String, String)>),
    Ended,
}

/// Что показать или сделать.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Реплика. Если у неё есть варианты — ждём [`Talk::choose`].
    Line(Shown),
    /// Событие для игры (не `quest.*`: их выполняет сюжет).
    Event { name: String, args: Vec<Value> },
    /// Позвать функцию скрипта.
    Run { func: String, args: Vec<Value> },
    /// Пауза: секунды или до нажатия.
    Wait { seconds: Option<f32> },
    /// Разговор окончен.
    End,
}

/// Реплика на экране.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    /// Имя персонажа (`kuznec`).
    pub who: String,
    /// Как его зовут на экране.
    pub name: String,
    pub color: String,
    pub text: String,
    /// Ключ перевода.
    pub key: String,
    /// Настройки: реплика поверх диалога поверх персонажа.
    pub settings: LineSettings,
    pub choices: Vec<ShownChoice>,
    /// Откуда текст у карточки «Сгенерировать» (Сю.10): `None` — обычная реплика.
    pub generated: Option<Generated>,
}

/// Чей текст у карточки «Сгенерировать».
#[derive(Clone, Debug, PartialEq)]
pub enum Generated {
    /// Написала модель (её имя).
    Model(String),
    /// Запасной текст: модели нет или почему её ответ не подошёл.
    Fallback(Option<String>),
}

/// Вариант ответа на экране.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownChoice {
    pub text: String,
    pub key: String,
    /// Игрок уже выбирал этот вариант (раньше, в этом или другом разговоре).
    pub seen: bool,
}

impl Talk {
    /// Ждёт ли разговор выбора игрока.
    pub fn waiting(&self) -> bool {
        matches!(self.pending, Pending::Choices(_))
    }

    /// Закончен ли разговор.
    pub fn ended(&self) -> bool {
        self.pending == Pending::Ended
    }

    /// Выбор игрока: номер среди показанных вариантов (с 0). Неверный номер — ничего.
    pub fn choose(&mut self, index: usize, state: &mut State) -> bool {
        let Pending::Choices(options) = &self.pending else {
            return false;
        };
        let Some((next, key)) = options.get(index).cloned() else {
            return false;
        };
        state.chosen.insert(key);
        self.pending = Pending::Then(next);
        self.shown = None;
        true
    }
}

impl Story {
    /// Начать разговор: диалог `dialog` с его первой карточки.
    pub fn talk(&self, dialog: &str) -> Result<Talk, String> {
        let d = self
            .dialogs
            .get(dialog)
            .ok_or(format!("нет диалога «{dialog}»"))?;
        let start = d
            .start_node()
            .ok_or(format!("диалог «{dialog}» пустой"))?
            .to_string();
        Ok(Talk {
            dialog: dialog.into(),
            node: start,
            pending: Pending::Start,
            shown: None,
            error: None,
        })
    }

    /// Ведёт разговор до следующей реплики, события, паузы или конца. После шага
    /// проверяет условия заданий ([`Story::quest_tick`]): разговор мог поменять мир.
    pub fn step(&mut self, talk: &mut Talk, state: &mut State) -> Step {
        let step = self.step_talk(talk, state);
        if let Err(e) = self.quest_tick(state) {
            tracing::warn!("условия заданий: {e}");
        }
        step
    }

    fn step_talk(&mut self, talk: &mut Talk, state: &mut State) -> Step {
        match std::mem::replace(&mut talk.pending, Pending::Ended) {
            Pending::Ended => return Step::End,
            Pending::Choices(c) => {
                talk.pending = Pending::Choices(c);
                return talk.shown.clone().map_or(Step::End, Step::Line);
            }
            Pending::Start => {}
            Pending::Then(next) => {
                if next.is_empty() {
                    return Step::End;
                }
                talk.node = next;
            }
        }
        for _ in 0..SILENT_LIMIT {
            match self.visit(talk, state) {
                Ok(Some(step)) => {
                    if step == Step::End {
                        talk.pending = Pending::Ended;
                    }
                    return step;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("диалог «{}», карточка «{}»: {e}", talk.dialog, talk.node);
                    talk.error = Some(format!("карточка «{}»: {e}", talk.node));
                    talk.pending = Pending::Ended;
                    return Step::End;
                }
            }
        }
        talk.error = Some("диалог ходит по кругу без реплик".into());
        talk.pending = Pending::Ended;
        Step::End
    }

    /// Одна карточка. `None` — карточка «тихая», идём дальше (`talk.node` уже сдвинут).
    fn visit(&mut self, talk: &mut Talk, state: &mut State) -> Result<Option<Step>, String> {
        let dialog = self
            .dialogs
            .get(&talk.dialog)
            .ok_or(format!("нет диалога «{}»", talk.dialog))?;
        let node = dialog
            .node(&talk.node)
            .ok_or(format!("нет карточки «{}»", talk.node))?
            .clone();
        let goto = |talk: &mut Talk, next: &str| {
            if next.is_empty() {
                talk.pending = Pending::Then(String::new());
                Some(Step::End)
            } else {
                talk.node = next.to_string();
                None
            }
        };
        Ok(match node {
            Node::Line {
                id,
                who,
                text,
                settings,
                choices,
                next,
                key,
                ..
            } => {
                let line_key = if key.is_empty() {
                    format!("{}.{id}", talk.dialog)
                } else {
                    key
                };
                let ch = self.characters.get(&who);
                let mut shown_choices = Vec::new();
                let mut options = Vec::new();
                for (i, c) in choices.iter().enumerate() {
                    if !self.conds.eval(&c.when, state)? {
                        continue;
                    }
                    let k = if c.key.is_empty() {
                        format!("{line_key}.{}", i + 1)
                    } else {
                        c.key.clone()
                    };
                    shown_choices.push(ShownChoice {
                        text: fill(self.text(&k, &c.text, state), state),
                        seen: state.chosen.contains(&k),
                        key: k.clone(),
                    });
                    options.push((c.next.clone(), k));
                }
                let shown = Shown {
                    name: ch.map_or_else(|| who.clone(), |c| c.name.clone()),
                    color: ch.map(|c| c.color.clone()).unwrap_or_default(),
                    text: fill(self.text(&line_key, &text, state), state),
                    key: line_key,
                    settings: settings
                        .over(&dialog.lines)
                        .over(&ch.map(|c| c.lines.clone()).unwrap_or_default()),
                    choices: shown_choices,
                    who,
                    generated: None,
                };
                if options.is_empty() {
                    talk.pending = Pending::Then(next);
                } else {
                    talk.pending = Pending::Choices(options);
                    talk.shown = Some(shown.clone());
                }
                Some(Step::Line(shown))
            }
            Node::Generate {
                id,
                who,
                task,
                fallback,
                words,
                allowed,
                settings,
                next,
                key,
                ..
            } => {
                let line_key = if key.is_empty() {
                    format!("{}.{id}", talk.dialog)
                } else {
                    key
                };
                let lines = dialog.lines.clone();
                let req = self.request(state, &who, &task, words, &allowed);
                let answer = self.writer.as_mut().map(|w| {
                    let name = w.name().to_string();
                    (
                        name,
                        w.write(&req).and_then(|r| crate::writer::accept(&req, r)),
                    )
                });
                let (text, generated) = match answer {
                    Some((name, Ok(reply))) => {
                        for (event, args) in &reply.events {
                            let done = match event.strip_prefix("quest.") {
                                Some(op) => self.quest_event(state, op, args),
                                None => self.memory_event(state, event, args),
                            };
                            if let Err(e) = done {
                                tracing::warn!("действие модели {event}: {e}");
                            }
                        }
                        (reply.text, Generated::Model(name))
                    }
                    Some((_, Err(e))) => {
                        tracing::warn!("модель: {e}; звучит запасной текст");
                        (
                            fill(self.text(&line_key, &fallback, state), state),
                            Generated::Fallback(Some(e)),
                        )
                    }
                    None => (
                        fill(self.text(&line_key, &fallback, state), state),
                        Generated::Fallback(None),
                    ),
                };
                let ch = self.characters.get(&who);
                let shown = Shown {
                    name: ch.map_or_else(|| who.clone(), |c| c.name.clone()),
                    color: ch.map(|c| c.color.clone()).unwrap_or_default(),
                    text,
                    key: line_key,
                    settings: settings
                        .over(&lines)
                        .over(&ch.map(|c| c.lines.clone()).unwrap_or_default()),
                    choices: Vec::new(),
                    who,
                    generated: Some(generated),
                };
                talk.pending = Pending::Then(next);
                Some(Step::Line(shown))
            }
            Node::Check { when, yes, no, .. } => {
                let ok = self.conds.eval(&when, state)?;
                goto(talk, if ok { &yes } else { &no })
            }
            Node::Set { changes, next, .. } => {
                for c in &changes {
                    apply(state, c)?;
                }
                goto(talk, &next)
            }
            Node::Event {
                name, args, next, ..
            } => {
                if let Some(op) = name.strip_prefix("quest.") {
                    self.quest_event(state, op, &args)?;
                    goto(talk, &next)
                } else if crate::memory::texts_needed(&name).is_some() {
                    self.memory_event(state, &name, &args)?;
                    goto(talk, &next)
                } else {
                    talk.pending = Pending::Then(next);
                    Some(Step::Event { name, args })
                }
            }
            Node::Run {
                func, args, next, ..
            } => {
                talk.pending = Pending::Then(next);
                Some(Step::Run { func, args })
            }
            Node::Jump {
                dialog: to, node, ..
            } => {
                let d = self.dialogs.get(&to).ok_or(format!("нет диалога «{to}»"))?;
                let start = if node.is_empty() {
                    d.start_node().unwrap_or("").to_string()
                } else {
                    node
                };
                talk.dialog = to;
                talk.node = start;
                None
            }
            Node::Random { next, .. } => {
                if next.is_empty() {
                    return Err("у «наугад» нет стрелок".into());
                }
                let i = (roll(state) * next.len() as f64) as usize;
                goto(talk, &next[i.min(next.len() - 1)])
            }
            Node::Chance {
                percent, yes, no, ..
            } => {
                let hit = roll(state) * 100.0 < f64::from(percent);
                goto(talk, if hit { &yes } else { &no })
            }
            Node::Wait { seconds, next, .. } => {
                talk.pending = Pending::Then(next);
                Some(Step::Wait { seconds })
            }
            Node::End { .. } => Some(Step::End),
        })
    }

    fn quest_event(&self, state: &mut State, op: &str, args: &[Value]) -> Result<(), String> {
        let text = |i: usize| match args.get(i) {
            Some(Value::Text(t)) => Ok(t.as_str()),
            _ => Err(format!(
                "quest.{op}: аргумент {} должен быть текстом",
                i + 1
            )),
        };
        match op {
            "start" => self.quest_start(state, text(0)?),
            "add" => {
                let n = args.get(2).and_then(Value::number).unwrap_or(1.0).max(0.0) as u32;
                self.quest_add(state, text(0)?, text(1)?, n)
            }
            "stage" => self.quest_stage(state, text(0)?, text(1)?),
            "complete" => self.quest_complete(state, text(0)?),
            "fail" => self.quest_fail(state, text(0)?),
            _ => Err(format!("нет события quest.{op}")),
        }
    }
}

/// Изменить переменную.
pub(crate) fn apply(state: &mut State, c: &crate::format::Change) -> Result<(), String> {
    let old = state
        .vars
        .get(&c.var)
        .ok_or(format!("нет переменной «{}»", c.var))?
        .clone();
    let new = if let Some(to) = &c.to {
        to.clone()
    } else if let Some(add) = c.add {
        match old {
            Value::Int(i) => Value::Int(i + add.round() as i64),
            Value::Float(f) => Value::Float(f + add),
            _ => return Err(format!("к «{}» нельзя прибавить: не число", c.var)),
        }
    } else {
        return Err(format!("у «{}» нет ни to, ни add", c.var));
    };
    if !old.same_kind(&new) {
        return Err(format!(
            "«{}» — {}, а записывают {}",
            c.var,
            old.kind(),
            new.kind()
        ));
    }
    let new = match (old, new) {
        (Value::Int(_), Value::Float(f)) => Value::Int(f.round() as i64),
        (Value::Float(_), Value::Int(i)) => Value::Float(i as f64),
        (_, n) => n,
    };
    state.vars.insert(c.var.clone(), new);
    Ok(())
}

/// Подставляет `{имя}` значениями переменных. Неизвестное имя остаётся как есть.
pub(crate) fn fill(text: &str, state: &State) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match state.vars.get(name) {
                    Some(v) => out.push_str(&v.show()),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Имена `{…}` в тексте.
pub(crate) fn placeholders(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            break;
        };
        out.push(&after[..close]);
        rest = &after[close + 1..];
    }
    out
}

/// Случайное число [0, 1) из зерна состояния (splitmix64); зерно сдвигается.
fn roll(state: &mut State) -> f64 {
    state.seed = state.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state.seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}
