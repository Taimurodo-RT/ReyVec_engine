//! Ссылки по именам (Сю.8): где используется диалог, задание, персонаж или переменная,
//! безопасное переименование во всём сюжете и в скриптах Luau.
//!
//! Имена (`kuznec.privet`, `mech`, `hero.money`) — то, по чему части сюжета и скрипты
//! находят друг друга. Переименование меняет имя везде, где на него ссылаются: стрелки
//! «перейти» других диалогов, события и условия заданий, `{подстановки}` в репликах,
//! ключи переводов, вызовы `story.*`/`quest.*` в скриптах.

use crate::format::{Change, Node, Value};
use crate::{Story, valid_id};

/// Что переименовывается.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Dialog,
    Quest,
    Character,
    Var,
}

/// Что изменилось в сюжете: какие файлы переписать (и какой файл переехал).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Renamed {
    /// Диалоги, задания, персонажи (по новым именам), переводы — что переписать.
    pub dialogs: Vec<String>,
    pub quests: Vec<String>,
    pub characters: Vec<String>,
    pub langs: Vec<String>,
    pub vars: bool,
    /// Файл переименованного: (было, стало) относительно `story/`, если имя файла шло
    /// от имени (`dialogs/kuznec.privet.ron`).
    pub moved: Option<(String, String)>,
    /// Сколько мест поменялось.
    pub places: usize,
}

/// Функции Luau, первый аргумент которых — имя такого вида.
fn luau_funcs(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Dialog => &["story.talk", "story.seen"],
        Kind::Quest => &[
            "quest.start",
            "quest.add",
            "quest.stage",
            "quest.state",
            "quest.complete",
            "quest.fail",
        ],
        Kind::Var => &["story.get", "story.set"],
        Kind::Character => &[],
    }
}

/// Места в тексте скрипта, где имя `id` стоит первым аргументом функции сюжета:
/// диапазоны байт самого имени (без кавычек).
pub fn luau_uses(source: &str, kind: Kind, id: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    for f in luau_funcs(kind) {
        let mut from = 0;
        while let Some(i) = source[from..].find(f) {
            let at = from + i;
            from = at + f.len();
            // `mystory.talk` — не наше.
            if source[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
            {
                continue;
            }
            let rest = &source[from..];
            let skip = |s: &str| s.len() - s.trim_start().len();
            let mut p = from + skip(rest);
            if !source[p..].starts_with('(') {
                continue;
            }
            p += 1;
            p += skip(&source[p..]);
            let Some(q) = source[p..]
                .chars()
                .next()
                .filter(|c| *c == '"' || *c == '\'')
            else {
                continue;
            };
            let start = p + 1;
            if source[start..].starts_with(id) && source[start + id.len()..].starts_with(q) {
                out.push(start..start + id.len());
            }
        }
    }
    out.sort_by_key(|r| r.start);
    out
}

/// Номера строк (с 1) мест [`luau_uses`] и сами строки.
pub fn luau_lines(source: &str, kind: Kind, id: &str) -> Vec<(usize, String)> {
    luau_uses(source, kind, id)
        .into_iter()
        .map(|r| {
            let line = source[..r.start].matches('\n').count() + 1;
            let text = source
                .lines()
                .nth(line - 1)
                .unwrap_or("")
                .trim()
                .to_string();
            (line, text)
        })
        .collect()
}

/// Переименовать в тексте скрипта. Возвращает новый текст и сколько мест поменялось.
pub fn luau_rename(source: &str, kind: Kind, old: &str, new: &str) -> (String, usize) {
    let uses = luau_uses(source, kind, old);
    let mut out = String::with_capacity(source.len());
    let mut last = 0;
    for r in &uses {
        out.push_str(&source[last..r.start]);
        out.push_str(new);
        last = r.end;
    }
    out.push_str(&source[last..]);
    (out, uses.len())
}

/// Заменить имя `old` в выражении Luau (условии): только целое имя — `hero.money`, но не
/// `hero.money2` и не `superhero.money`. Возвращает, сколько замен.
pub fn rename_in_expr(expr: &mut String, old: &str, new: &str) -> usize {
    let is_name = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(expr.len());
    let mut n = 0;
    let mut last = 0;
    let mut from = 0;
    while let Some(i) = expr[from..].find(old) {
        let at = from + i;
        let end = at + old.len();
        from = end;
        let before = expr[..at].chars().next_back();
        let after = expr[end..].chars().next();
        if before.is_some_and(|c| is_name(c) || c == '.') || after.is_some_and(is_name) {
            continue;
        }
        // Внутри строки "..." имя не трогаем: это текст, а не переменная.
        if expr[..at].matches('"').count() % 2 == 1 {
            continue;
        }
        out.push_str(&expr[last..at]);
        out.push_str(new);
        last = end;
        n += 1;
    }
    out.push_str(&expr[last..]);
    if n > 0 {
        *expr = out;
    }
    n
}

/// Персонаж в условии: `memory.old.…`, `relation.old.…` и `relation.<кто>.old`.
fn rename_npc_in_expr(expr: &mut String, old: &str, new: &str) -> usize {
    let mut n = rename_in_expr(expr, &format!("memory.{old}"), &format!("memory.{new}"));
    n += rename_in_expr(expr, &format!("relation.{old}"), &format!("relation.{new}"));
    // «к кому»: `relation.kuznec.old` — имя после точки, целиком.
    let needle = format!(".{old}");
    let mut out = String::new();
    let mut last = 0;
    let mut from = 0;
    while let Some(i) = expr[from..].find(&needle) {
        let at = from + i;
        let end = at + needle.len();
        from = end;
        let start = expr[..at]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
            .map_or(0, |p| p + 1);
        let head = &expr[start..at];
        let after = expr[end..].chars().next();
        if head.starts_with("relation.")
            && head.matches('.').count() == 1
            && !after.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        {
            out.push_str(&expr[last..at + 1]);
            out.push_str(new);
            last = end;
            n += 1;
        }
    }
    if last > 0 {
        out.push_str(&expr[last..]);
        *expr = out;
    }
    n
}

fn rename_in_placeholders(text: &mut String, old: &str, new: &str) -> usize {
    let from = format!("{{{old}}}");
    let n = text.matches(&from).count();
    if n > 0 {
        *text = text.replace(&from, &format!("{{{new}}}"));
    }
    n
}

fn rename_changes(changes: &mut [Change], old: &str, new: &str) -> usize {
    let mut n = 0;
    for c in changes {
        if c.var == old {
            c.var = new.to_string();
            n += 1;
        }
    }
    n
}

impl Story {
    /// Есть ли имя такого вида.
    pub fn has_name(&self, kind: Kind, id: &str) -> bool {
        match kind {
            Kind::Dialog => self.dialogs.contains_key(id),
            Kind::Quest => self.quests.contains_key(id),
            Kind::Character => self.characters.contains_key(id),
            Kind::Var => self.vars.contains_key(id),
        }
    }

    /// Можно ли переименовать `old` в `new`: `Err` — почему нельзя.
    pub fn can_rename(&self, kind: Kind, old: &str, new: &str) -> Result<(), String> {
        if !self.has_name(kind, old) {
            return Err(format!("нет «{old}»"));
        }
        if old == new {
            return Err("имя то же".into());
        }
        if !valid_id(new) {
            return Err(
                "только латиница в нижнем регистре, цифры, _ и . (на имя ссылается код)".into(),
            );
        }
        if self.has_name(kind, new) {
            return Err(format!("«{new}» уже есть"));
        }
        if kind == Kind::Var && (new == "quest" || new.starts_with("quest.")) {
            return Err("имя quest занято заданиями".into());
        }
        Ok(())
    }

    /// Переименовать во всём сюжете. Файлы не пишутся: что переписать, — в ответе.
    pub fn rename(&mut self, kind: Kind, old: &str, new: &str) -> Result<Renamed, String> {
        self.can_rename(kind, old, new)?;
        let mut r = Renamed::default();
        let (key, dir) = match kind {
            Kind::Dialog => ("d", "dialogs"),
            Kind::Quest => ("q", "quests"),
            Kind::Character => ("c", "characters"),
            Kind::Var => ("", ""),
        };
        // Само имя и его файл.
        match kind {
            Kind::Dialog => {
                let mut d = self.dialogs.remove(old).expect("проверено");
                d.id = new.into();
                self.dialogs.insert(new.into(), d);
                r.dialogs.push(new.into());
            }
            Kind::Quest => {
                let mut q = self.quests.remove(old).expect("проверено");
                q.id = new.into();
                self.quests.insert(new.into(), q);
                r.quests.push(new.into());
            }
            Kind::Character => {
                let mut c = self.characters.remove(old).expect("проверено");
                c.id = new.into();
                self.characters.insert(new.into(), c);
                r.characters.push(new.into());
            }
            Kind::Var => {
                let v = self.vars.remove(old).expect("проверено");
                self.vars.insert(new.into(), v);
                r.vars = true;
            }
        }
        r.places += 1;
        if kind != Kind::Var
            && let Some(file) = self.files.remove(&format!("{key}:{old}"))
        {
            let named = format!("{dir}/{old}.ron");
            let file = if file == named {
                let to = format!("{dir}/{new}.ron");
                r.moved = Some((file, to.clone()));
                to
            } else {
                file
            };
            self.files.insert(format!("{key}:{new}"), file);
        }

        // Ссылки в диалогах.
        let quest_ref = (format!("quest.{old}"), format!("quest.{new}"));
        for d in self.dialogs.values_mut() {
            let mut n = 0;
            if kind == Kind::Character {
                for w in &mut d.with {
                    if w == old {
                        *w = new.into();
                        n += 1;
                    }
                }
            }
            for node in &mut d.nodes {
                n += match (kind, node) {
                    (Kind::Dialog, Node::Jump { dialog, .. }) if dialog == old => {
                        *dialog = new.into();
                        1
                    }
                    (Kind::Character, Node::Line { who, choices, .. }) => {
                        let mut k = 0;
                        if who == old {
                            *who = new.into();
                            k += 1;
                        }
                        for c in choices {
                            k += rename_npc_in_expr(&mut c.when, old, new);
                        }
                        k
                    }
                    (Kind::Character, Node::Generate { who, .. }) if who == old => {
                        *who = new.into();
                        1
                    }
                    (Kind::Var, Node::Generate { fallback, .. }) => {
                        rename_in_placeholders(fallback, old, new)
                    }
                    (Kind::Character, Node::Check { when, .. }) => {
                        rename_npc_in_expr(when, old, new)
                    }
                    // Память и отношения (Сю.9): персонаж — первый аргумент, у отношений
                    // и второй («к кому»).
                    (Kind::Character, Node::Event { name, args, .. })
                        if crate::memory::texts_needed(name).is_some() =>
                    {
                        let mut k = 0;
                        let whom = usize::from(name.starts_with("relation."));
                        for a in args.iter_mut().take(whom + 1) {
                            if matches!(a, Value::Text(t) if t == old) {
                                *a = Value::Text(new.into());
                                k += 1;
                            }
                        }
                        k
                    }
                    (Kind::Quest, Node::Event { name, args, .. })
                        if name.starts_with("quest.")
                            && matches!(args.first(), Some(Value::Text(t)) if t == old) =>
                    {
                        args[0] = Value::Text(new.into());
                        1
                    }
                    (Kind::Quest, Node::Check { when, .. }) => {
                        rename_in_expr(when, &quest_ref.0, &quest_ref.1)
                    }
                    (Kind::Quest, Node::Line { choices, .. }) => choices
                        .iter_mut()
                        .map(|c| rename_in_expr(&mut c.when, &quest_ref.0, &quest_ref.1))
                        .sum(),
                    (Kind::Var, Node::Check { when, .. }) => rename_in_expr(when, old, new),
                    (Kind::Var, Node::Set { changes, .. }) => rename_changes(changes, old, new),
                    (Kind::Var, Node::Line { text, choices, .. }) => {
                        let mut k = rename_in_placeholders(text, old, new);
                        for c in choices {
                            k += rename_in_expr(&mut c.when, old, new);
                            k += rename_in_placeholders(&mut c.text, old, new);
                        }
                        k
                    }
                    _ => 0,
                };
            }
            if n > 0 {
                r.places += n;
                if !r.dialogs.contains(&d.id) {
                    r.dialogs.push(d.id.clone());
                }
            }
        }

        // Отношения в начале игры у других персонажей.
        if kind == Kind::Character {
            for c in self.characters.values_mut() {
                if let Some(n) = c.relations.remove(old) {
                    c.relations.insert(new.into(), n);
                    r.places += 1;
                    if !r.characters.contains(&c.id) {
                        r.characters.push(c.id.clone());
                    }
                }
            }
        }
        // Условия и награды заданий.
        for q in self.quests.values_mut() {
            let mut n = 0;
            let fix = |e: &mut String| match kind {
                Kind::Var => rename_in_expr(e, old, new),
                Kind::Quest => rename_in_expr(e, &quest_ref.0, &quest_ref.1),
                Kind::Character => rename_npc_in_expr(e, old, new),
                Kind::Dialog => 0,
            };
            n += fix(&mut q.fail_when);
            for s in &mut q.stages {
                n += fix(&mut s.done_when);
            }
            if kind == Kind::Var {
                n += rename_changes(&mut q.rewards, old, new);
            }
            if n > 0 {
                r.places += n;
                if !r.quests.contains(&q.id) {
                    r.quests.push(q.id.clone());
                }
            }
        }

        // Ключи переводов: у реплик без своего ключа он `<диалог>.<карточка>`.
        if kind == Kind::Dialog {
            let prefix = format!("{old}.");
            for (code, words) in &mut self.lang {
                let keys: Vec<String> = words
                    .keys()
                    .filter(|k| k.starts_with(&prefix))
                    .cloned()
                    .collect();
                for k in &keys {
                    let v = words.remove(k).expect("ключ есть");
                    words.insert(format!("{new}.{}", &k[prefix.len()..]), v);
                }
                if !keys.is_empty() {
                    r.places += keys.len();
                    r.langs.push(code.clone());
                }
            }
        }
        // Условия разобраны заново при следующей проверке.
        self.conds = Default::default();
        Ok(r)
    }
}
