//! Живые реплики (Сю.10): карточка «Сгенерировать» просит текст у модели.
//!
//! Модель — что-то, что умеет [`Writer`]: облачная по ключу, своя на ПК или заглушка для
//! тестов. Сюжет даёт ей [`Request`]: кто говорит, его характер, что он помнит, как
//! относится к собеседнику, задачу автора и рамки. Ответ ([`Reply`]) проверяется
//! ([`accept`]): пустой, слишком длинный или с действиями вне списка разрешённого — и
//! звучит запасной текст автора. Модель не может нарушить сюжет: деньги, задания и
//! переменные меняет только движок, а модель лишь выбирает из того, что автор разрешил.
//!
//! Пока модель не подключена (решение за Романом), всегда звучит запасной текст.

use crate::format::{Node, Value};
use crate::{State, Story};

/// Сколько фактов памяти дать модели: самые сильные (важные и свежие).
pub const MEMORY_IN_PROMPT: usize = 8;

/// Что модель должна написать.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    /// Кто говорит (имя в коде и на экране).
    pub who: String,
    pub name: String,
    /// Характер из файла персонажа.
    pub about: String,
    /// Что помнит: самые сильные факты, сильнее — раньше.
    pub memory: Vec<String>,
    /// Как относится к другим: (имя на экране, число).
    pub relations: Vec<(String, i64)>,
    /// Задача автора.
    pub task: String,
    /// Предел слов.
    pub words: u32,
    /// Разрешённые события.
    pub allowed: Vec<String>,
    /// Язык ответа: пусто — основной (русский).
    pub lang: String,
    /// Что было в разговоре перед этим (Сю.11): реплики по порядку.
    pub before: Vec<String>,
    /// Чего не повторять: уже предложенные варианты (Сю.11).
    pub avoid: Vec<String>,
}

/// Ответ модели.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    /// Действия: событие и аргументы (только из [`Request::allowed`]).
    pub events: Vec<(String, Vec<Value>)>,
}

/// Модель, которая пишет реплики.
pub trait Writer: Send {
    /// Имя для журнала и редактора («облако», «на ПК», «заглушка»).
    fn name(&self) -> &str;
    fn write(&mut self, req: &Request) -> Result<Reply, String>;
}

/// Текст задания для модели — из него облачная или своя модель получает всё нужное.
pub fn prompt(req: &Request) -> String {
    let mut p = String::new();
    p.push_str(&format!(
        "Ты — {} в игре. Скажи одну реплику от первого лица.\n",
        req.name
    ));
    if !req.about.trim().is_empty() {
        p.push_str(&format!("Характер: {}\n", req.about.trim()));
    }
    if !req.memory.is_empty() {
        p.push_str("Ты помнишь:\n");
        for m in &req.memory {
            p.push_str(&format!("- {m}\n"));
        }
    }
    if !req.relations.is_empty() {
        p.push_str("Отношения (0 — никак, больше — лучше):\n");
        for (whom, n) in &req.relations {
            p.push_str(&format!("- {whom}: {n}\n"));
        }
    }
    if !req.before.is_empty() {
        p.push_str("Перед этим в разговоре:\n");
        for b in &req.before {
            p.push_str(&format!("- {b}\n"));
        }
    }
    p.push_str(&format!("Задача: {}\n", req.task.trim()));
    if !req.avoid.is_empty() {
        p.push_str("Не повторяй уже предложенное:\n");
        for a in &req.avoid {
            p.push_str(&format!("- {a}\n"));
        }
    }
    p.push_str(&format!(
        "Правила: не длиннее {} слов, без пояснений и кавычек, только сама реплика{}.\n",
        req.words,
        if req.lang.is_empty() {
            ", по-русски".to_string()
        } else {
            format!(", на языке «{}»", req.lang)
        }
    ));
    if !req.allowed.is_empty() {
        p.push_str(&format!(
            "Можно сделать (по желанию, строкой «действие: имя(аргументы)»): {}. Другого \
             делать нельзя.\n",
            req.allowed.join(", ")
        ));
    }
    p
}

/// Проверить ответ модели. `Err` — почему он не годится (тогда звучит запасной текст).
/// Действия вне списка разрешённого просто отбрасываются.
pub fn accept(req: &Request, mut reply: Reply) -> Result<Reply, String> {
    let text = reply.text.trim().trim_matches(['"', '«', '»']).trim();
    if text.is_empty() {
        return Err("модель ответила пустым текстом".into());
    }
    let words = text.split_whitespace().count();
    // Немного запаса: модели считают слова неточно.
    let limit = (req.words as usize * 3).div_ceil(2).max(3);
    if words > limit {
        return Err(format!(
            "модель написала {words} слов при пределе {}",
            req.words
        ));
    }
    reply.text = text.to_string();
    reply.events.retain(|(name, _)| req.allowed.contains(name));
    Ok(reply)
}

/// События, которые карточка «Сгенерировать» может разрешить модели.
pub fn allowed_event(name: &str) -> bool {
    crate::memory::texts_needed(name).is_some()
        || matches!(
            name,
            "quest.start" | "quest.add" | "quest.stage" | "quest.complete" | "quest.fail"
        )
}

impl Story {
    /// Что дать модели для реплики `who`.
    pub fn request(
        &self,
        state: &State,
        who: &str,
        task: &str,
        words: u32,
        allowed: &[String],
    ) -> Request {
        let ch = self.characters.get(who);
        let name_of = |id: &str| {
            self.characters
                .get(id)
                .map_or_else(|| id.to_string(), |c| c.name.clone())
        };
        let (memory, relations) = match state.memory.get(who) {
            Some(m) => {
                let mut facts: Vec<_> = m.facts.iter().collect();
                facts.sort_by(|a, b| b.strength(state.time).total_cmp(&a.strength(state.time)));
                (
                    facts
                        .into_iter()
                        .take(MEMORY_IN_PROMPT)
                        .map(|f| {
                            if f.text.is_empty() {
                                f.key.clone()
                            } else {
                                f.text.clone()
                            }
                        })
                        .collect(),
                    m.relations.iter().map(|(w, n)| (name_of(w), *n)).collect(),
                )
            }
            None => (Vec::new(), Vec::new()),
        };
        Request {
            who: who.into(),
            name: name_of(who),
            about: ch.map(|c| c.about.clone()).unwrap_or_default(),
            memory,
            relations,
            task: task.into(),
            words,
            allowed: allowed.to_vec(),
            lang: state.lang.clone(),
            before: Vec::new(),
            avoid: Vec::new(),
        }
    }

    /// Предложить автору `n` вариантов (Сю.11): переписать реплику карточки `node` или
    /// придумать ответы игрока на неё. Модель видит характер, задачу и до
    /// [`BEFORE_IN_PROMPT`] реплик перед карточкой. Нет модели или все ответы не годятся —
    /// `Err` с причиной.
    pub fn suggest(
        &mut self,
        dialog: &str,
        node: &str,
        what: Suggest,
        n: usize,
    ) -> Result<Vec<String>, String> {
        let d = self
            .dialogs
            .get(dialog)
            .ok_or_else(|| format!("нет диалога «{dialog}»"))?;
        let Some(Node::Line { who, text, .. }) = d.node(node) else {
            return Err(format!("«{node}» — не реплика"));
        };
        let (who, text) = (who.clone(), text.trim().to_string());
        let before = lines_before(d, node);
        let state = self.new_state();
        let mut req = match what {
            Suggest::Line => {
                let task = if text.is_empty() {
                    "Придумай реплику, к месту в этом разговоре.".to_string()
                } else {
                    format!("Скажи то же самое по-своему, смысл сохрани: «{text}».")
                };
                self.request(&state, &who, &task, LINE_WORDS, &[])
            }
            Suggest::Answers => {
                let mut r = self.request(
                    &state,
                    "",
                    &format!(
                        "Ты — игрок. Короткий ответ игрока на реплику {}: «{text}».",
                        self.characters.get(&who).map_or(who.as_str(), |c| &c.name)
                    ),
                    ANSWER_WORDS,
                    &[],
                );
                r.name = "игрок".into();
                r
            }
        };
        req.before = before;
        let Some(w) = self.writer.as_mut() else {
            return Err("модель не подключена".into());
        };
        let mut out: Vec<String> = Vec::new();
        let mut first_error = None;
        for _ in 0..n {
            req.avoid = out.clone();
            match w.write(&req).and_then(|r| accept(&req, r)) {
                Ok(r) => {
                    if !out.contains(&r.text) && r.text != text {
                        out.push(r.text);
                    }
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        match (out.is_empty(), first_error) {
            (true, Some(e)) => Err(e),
            (true, None) => Err("модель повторила то, что уже есть".into()),
            _ => Ok(out),
        }
    }
}

/// Что предложить автору (Сю.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suggest {
    /// Та же реплика другими словами (или новая, если пусто).
    Line,
    /// Ответы игрока на реплику.
    Answers,
}

/// Предел слов для предложенной реплики и ответа игрока.
pub const LINE_WORDS: u32 = 30;
pub const ANSWER_WORDS: u32 = 10;
/// Сколько реплик перед карточкой показать модели.
pub const BEFORE_IN_PROMPT: usize = 4;

/// Реплики перед карточкой `node`: идём назад по стрелкам, по первой найденной.
fn lines_before(d: &crate::Dialog, node: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = node.to_string();
    let mut seen = vec![at.clone()];
    while out.len() < BEFORE_IN_PROMPT {
        let Some(prev) = d
            .nodes
            .iter()
            .find(|n| n.targets().contains(&at.as_str()) && !seen.contains(&n.id().to_string()))
        else {
            break;
        };
        match prev {
            Node::Line { text, choices, .. } => {
                // Если сюда ведёт вариант игрока — его тоже.
                if let Some(c) = choices.iter().find(|c| c.next == at) {
                    out.push(format!("игрок: {}", c.text));
                }
                out.push(text.clone());
            }
            Node::Generate { fallback, .. } => out.push(fallback.clone()),
            _ => {}
        }
        at = prev.id().to_string();
        seen.push(at.clone());
    }
    out.truncate(BEFORE_IN_PROMPT);
    out.reverse();
    out
}

/// Заглушка для тестов и проверки в редакторе: отвечает заранее заданным.
pub struct Canned {
    pub reply: Result<Reply, String>,
    /// Что у неё спросили последним.
    pub asked: std::sync::Arc<std::sync::Mutex<Option<Request>>>,
}

impl Writer for Canned {
    fn name(&self) -> &str {
        "заглушка"
    }

    fn write(&mut self, req: &Request) -> Result<Reply, String> {
        *self.asked.lock().unwrap_or_else(|e| e.into_inner()) = Some(req.clone());
        self.reply.clone()
    }
}
