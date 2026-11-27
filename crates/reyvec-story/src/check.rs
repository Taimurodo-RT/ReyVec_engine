//! Проверка сюжета: стрелки в никуда, неизвестные имена и переменные, ошибки условий.
//! Редактор показывает список, игра пишет его в консоль.

use std::collections::{BTreeSet, HashSet};

use crate::format::{Node, Value};
use crate::play::placeholders;
use crate::{Story, valid_id};

/// Найденная беда.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// Файл в `story/`.
    pub file: String,
    /// Где в файле: карточка, стадия, переменная.
    pub place: String,
    pub text: String,
    /// Только предупреждение: работать будет, но, похоже, задумано не так.
    pub warning: bool,
}

impl Problem {
    pub fn error(file: &str, place: &str, text: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            place: place.into(),
            text: text.into(),
            warning: false,
        }
    }

    pub fn warning(file: &str, place: &str, text: impl Into<String>) -> Self {
        Self {
            warning: true,
            ..Self::error(file, place, text)
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = if self.warning {
            "предупреждение"
        } else {
            "ошибка"
        };
        if self.place.is_empty() {
            write!(f, "{}: {kind}: {}", self.file, self.text)
        } else {
            write!(f, "{} [{}]: {kind}: {}", self.file, self.place, self.text)
        }
    }
}

/// Проверяет весь сюжет и заодно разбирает все условия.
pub fn check(story: &mut Story) -> Vec<Problem> {
    let mut out = Vec::new();
    story.conds.characters = story.characters.keys().cloned().collect();
    check_vars(story, &mut out);
    for c in story.characters.values() {
        for whom in c.relations.keys() {
            if !story.characters.contains_key(whom) {
                out.push(Problem::error(
                    story
                        .files
                        .get(&format!("c:{}", c.id))
                        .map_or("", String::as_str),
                    "relations",
                    format!("отношение к «{whom}», а такого персонажа нет"),
                ));
            }
        }
        if !valid_id(&c.id) {
            out.push(Problem::error(
                story
                    .files
                    .get(&format!("c:{}", c.id))
                    .map_or("", String::as_str),
                "",
                bad_id(&c.id),
            ));
        }
    }
    let ids: Vec<String> = story.dialogs.keys().cloned().collect();
    for id in ids {
        check_dialog(story, &id, &mut out);
    }
    check_quests(story, &mut out);
    out
}

fn bad_id(id: &str) -> String {
    format!("имя «{id}»: только латиница в нижнем регистре, цифры, _ и . (на имя ссылается код)")
}

fn check_vars(story: &Story, out: &mut Vec<Problem>) {
    for name in story.vars.keys() {
        if !valid_id(name) {
            out.push(Problem::error("vars.ron", name, bad_id(name)));
        }
        // `kuznec` и `kuznec.znakom` вместе нельзя: в условиях `kuznec` — таблица.
        let prefix = format!("{name}.");
        if story.vars.keys().any(|o| o.starts_with(&prefix)) {
            out.push(Problem::error(
                "vars.ron",
                name,
                format!("«{name}» — и переменная, и начало других имён ({name}.…)"),
            ));
        }
        for (taken, what) in [
            ("quest", "заданиями (quest.<задание>.state)"),
            ("memory", "памятью персонажей (memory.<персонаж>.<факт>)"),
            ("relation", "отношениями (relation.<персонаж>.<к кому>)"),
        ] {
            if name == taken || name.starts_with(&format!("{taken}.")) {
                out.push(Problem::error(
                    "vars.ron",
                    name,
                    format!("имя {taken} занято {what}"),
                ));
            }
        }
    }
}

fn check_dialog(story: &mut Story, id: &str, out: &mut Vec<Problem>) {
    let d = story.dialogs[id].clone();
    let file = story.dialog_file(id).to_string();
    if !valid_id(&d.id) {
        out.push(Problem::error(&file, "", bad_id(&d.id)));
    }
    if d.nodes.is_empty() {
        out.push(Problem::error(&file, "", "в диалоге нет карточек"));
        return;
    }
    let mut seen = HashSet::new();
    for n in &d.nodes {
        if !seen.insert(n.id()) {
            out.push(Problem::error(&file, n.id(), "две карточки с одним именем"));
        }
        if n.id().is_empty() {
            out.push(Problem::error(&file, "", "у карточки нет имени"));
        }
    }
    let start = d.start_node().unwrap_or("");
    if d.node(start).is_none() {
        out.push(Problem::error(
            &file,
            "start",
            format!("нет карточки «{start}»"),
        ));
    }
    for w in &d.with {
        if !story.characters.contains_key(w) {
            out.push(Problem::error(
                &file,
                "with",
                format!("нет персонажа «{w}»"),
            ));
        }
    }
    let state = story.new_state();
    let cond = |story: &mut Story, place: &str, expr: &str, out: &mut Vec<Problem>| {
        if expr.trim().is_empty() {
            return;
        }
        if let Err(e) = story.conds.compile(expr) {
            out.push(Problem::error(
                &file,
                place,
                format!("условие «{expr}»: {e}"),
            ));
        } else if let Err(e) = story.conds.eval(expr, &state) {
            out.push(Problem::error(&file, place, e));
        }
    };
    for n in &d.nodes {
        let place = n.id();
        for t in n.targets() {
            if t.is_empty() {
                out.push(Problem::error(&file, place, "стрелка никуда не ведёт"));
            } else if d.node(t).is_none() {
                out.push(Problem::error(
                    &file,
                    place,
                    format!("стрелка в несуществующую «{t}»"),
                ));
            }
        }
        match n {
            Node::Line {
                who, text, choices, ..
            } => {
                if !story.characters.contains_key(who) {
                    out.push(Problem::error(
                        &file,
                        place,
                        format!("нет персонажа «{who}»"),
                    ));
                }
                let mut texts = vec![text.as_str()];
                texts.extend(choices.iter().map(|c| c.text.as_str()));
                for t in texts {
                    for p in placeholders(t) {
                        if !story.vars.contains_key(p) {
                            out.push(Problem::error(
                                &file,
                                place,
                                format!("в тексте {{{p}}}, а такой переменной нет"),
                            ));
                        }
                    }
                }
                for c in choices {
                    cond(story, place, &c.when, out);
                }
            }
            Node::Generate {
                who,
                task,
                fallback,
                words,
                allowed,
                ..
            } => {
                if !story.characters.contains_key(who) {
                    out.push(Problem::error(
                        &file,
                        place,
                        format!("нет персонажа «{who}»"),
                    ));
                }
                if fallback.trim().is_empty() {
                    out.push(Problem::error(
                        &file,
                        place,
                        "нет запасного текста: он звучит, когда модели нет или она ошиблась",
                    ));
                }
                if task.trim().is_empty() {
                    out.push(Problem::warning(
                        &file,
                        place,
                        "нет задачи для модели: она не поймёт, что сказать",
                    ));
                }
                if *words == 0 {
                    out.push(Problem::error(&file, place, "предел слов 0"));
                }
                for a in allowed {
                    if !crate::writer::allowed_event(a) {
                        out.push(Problem::error(
                            &file,
                            place,
                            format!("модели нельзя разрешить «{a}»: только quest.*, memory.*, relation.*"),
                        ));
                    }
                }
                for p in placeholders(fallback) {
                    if !story.vars.contains_key(p) {
                        out.push(Problem::error(
                            &file,
                            place,
                            format!("в тексте {{{p}}}, а такой переменной нет"),
                        ));
                    }
                }
            }
            Node::Check { when, .. } => {
                if when.trim().is_empty() {
                    out.push(Problem::error(&file, place, "у развилки нет условия"));
                }
                cond(story, place, when, out);
            }
            Node::Set { changes, .. } => {
                for c in changes {
                    let mut probe = story.new_state();
                    if let Err(e) = crate::play::apply(&mut probe, c) {
                        out.push(Problem::error(&file, place, e));
                    }
                }
            }
            Node::Event { name, args, .. } => {
                if let Some(op) = name.strip_prefix("quest.") {
                    check_quest_event(story, op, args, &file, place, out);
                } else if let Some(need) = crate::memory::texts_needed(name) {
                    check_memory_event(story, name, need, args, &file, place, out);
                } else if name.starts_with("memory.") || name.starts_with("relation.") {
                    out.push(Problem::error(
                        &file,
                        place,
                        format!(
                            "нет события {name} (есть memory.add, memory.forget, relation.add, \
                             relation.set)"
                        ),
                    ));
                }
            }
            Node::Jump { dialog, node, .. } => match story.dialogs.get(dialog) {
                None => out.push(Problem::error(
                    &file,
                    place,
                    format!("нет диалога «{dialog}»"),
                )),
                Some(other) => {
                    if !node.is_empty() && other.node(node).is_none() {
                        out.push(Problem::error(
                            &file,
                            place,
                            format!("в диалоге «{dialog}» нет карточки «{node}»"),
                        ));
                    }
                }
            },
            Node::Chance { percent, .. } if !(0.0..=100.0).contains(percent) => {
                out.push(Problem::error(
                    &file,
                    place,
                    "процент должен быть от 0 до 100",
                ));
            }
            _ => {}
        }
    }
    // Карточки, до которых не дойти от начала (и от переходов из других диалогов).
    let mut reach = BTreeSet::new();
    let mut stack = vec![start.to_string()];
    for other in story.dialogs.values() {
        for n in &other.nodes {
            if let Node::Jump { dialog, node, .. } = n
                && dialog == id
                && !node.is_empty()
            {
                stack.push(node.clone());
            }
        }
    }
    while let Some(n) = stack.pop() {
        if !reach.insert(n.clone()) {
            continue;
        }
        if let Some(node) = d.node(&n) {
            stack.extend(node.targets().into_iter().map(String::from));
        }
    }
    for n in &d.nodes {
        if !reach.contains(n.id()) {
            out.push(Problem::warning(
                &file,
                n.id(),
                "до карточки не дойти от начала",
            ));
        }
    }
}

fn check_memory_event(
    story: &Story,
    name: &str,
    need: usize,
    args: &[Value],
    file: &str,
    place: &str,
    out: &mut Vec<Problem>,
) {
    let text = |i: usize| match args.get(i) {
        Some(Value::Text(t)) => Some(t.as_str()),
        _ => None,
    };
    if (0..need).any(|i| text(i).is_none()) {
        out.push(Problem::error(
            file,
            place,
            format!("{name}: первые {need} аргумента — текст (персонаж, …)"),
        ));
        return;
    }
    let npc = text(0).unwrap_or("");
    if !story.characters.contains_key(npc) {
        out.push(Problem::error(
            file,
            place,
            format!("нет персонажа «{npc}»"),
        ));
    }
    let second = text(1).unwrap_or("");
    if name.starts_with("relation.") && !story.characters.contains_key(second) {
        out.push(Problem::error(
            file,
            place,
            format!("нет персонажа «{second}»"),
        ));
    }
    if name.starts_with("memory.") && !valid_id(second) {
        out.push(Problem::error(file, place, bad_id(second)));
    }
}

fn check_quest_event(
    story: &Story,
    op: &str,
    args: &[Value],
    file: &str,
    place: &str,
    out: &mut Vec<Problem>,
) {
    let text = |i: usize| match args.get(i) {
        Some(Value::Text(t)) => Some(t.as_str()),
        _ => None,
    };
    let need = match op {
        "start" | "complete" | "fail" => 1,
        "add" | "stage" => 2,
        _ => {
            out.push(Problem::error(
                file,
                place,
                format!("нет события quest.{op}"),
            ));
            return;
        }
    };
    if (0..need).any(|i| text(i).is_none()) {
        out.push(Problem::error(
            file,
            place,
            format!("quest.{op}: нужно {need} текстовых аргумента (задание…)"),
        ));
        return;
    }
    let qid = text(0).unwrap_or("");
    let Some(q) = story.quests.get(qid) else {
        out.push(Problem::error(file, place, format!("нет задания «{qid}»")));
        return;
    };
    let second = text(1).unwrap_or("");
    match op {
        "add"
            if !q
                .stages
                .iter()
                .any(|s| s.goals.iter().any(|g| g.id == second)) =>
        {
            out.push(Problem::error(
                file,
                place,
                format!("у задания «{qid}» нет цели «{second}»"),
            ));
        }
        "stage" if q.stage(second).is_none() => {
            out.push(Problem::error(
                file,
                place,
                format!("у задания «{qid}» нет стадии «{second}»"),
            ));
        }
        _ => {}
    }
}

fn check_quests(story: &mut Story, out: &mut Vec<Problem>) {
    let state = story.new_state();
    let quests: Vec<crate::format::Quest> = story.quests.values().cloned().collect();
    for q in &quests {
        let file = story
            .files
            .get(&format!("q:{}", q.id))
            .cloned()
            .unwrap_or_default();
        let mut cond = |story: &mut Story, place: &str, expr: &str| {
            if expr.trim().is_empty() {
                return;
            }
            if let Err(e) = story.conds.compile(expr) {
                out.push(Problem::error(
                    &file,
                    place,
                    format!("условие «{expr}»: {e}"),
                ));
            } else if let Err(e) = story.conds.eval(expr, &state) {
                out.push(Problem::error(&file, place, e));
            }
        };
        cond(story, "fail_when", &q.fail_when);
        for s in &q.stages {
            cond(story, &s.id, &s.done_when);
        }
        for c in &q.rewards {
            let mut probe = story.new_state();
            if let Err(e) = crate::play::apply(&mut probe, c) {
                out.push(Problem::error(&file, "rewards", e));
            }
        }
    }
    for q in story.quests.values() {
        let file = story
            .files
            .get(&format!("q:{}", q.id))
            .map_or("", String::as_str);
        if !valid_id(&q.id) {
            out.push(Problem::error(file, "", bad_id(&q.id)));
        }
        if q.stages.is_empty() {
            out.push(Problem::error(file, "", "у задания нет стадий"));
        }
        let mut seen = HashSet::new();
        for s in &q.stages {
            if !seen.insert(s.id.as_str()) {
                out.push(Problem::error(file, &s.id, "две стадии с одним именем"));
            }
            if !s.next.is_empty() && q.stage(&s.next).is_none() {
                out.push(Problem::error(
                    file,
                    &s.id,
                    format!("нет стадии «{}»", s.next),
                ));
            }
            for g in &s.goals {
                if g.count == 0 {
                    out.push(Problem::error(
                        file,
                        &s.id,
                        format!("у цели «{}» count 0", g.id),
                    ));
                }
            }
        }
    }
}
