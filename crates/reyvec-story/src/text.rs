//! Текстовый вид диалога (Сю.13), похожий на сценарии Ren'Py: тот же диалог, но набранный
//! как пьеса. Каждая карточка — блок с заголовком `== имя`:
//!
//! ```text
//! == nachalo
//! если kuznec.znakom -> po_imeni
//! иначе -> chuzhak
//!
//! == chuzhak
//! kuznec: Чего надо, чужак? Я кузнец, если что.
//! -> vopros
//!
//! == vopros
//! kuznec: Что тебе нужно?
//! * Мне нужен меч. [если quest.mech.state == "none"] -> pro_mech
//! * Просто смотрю. -> proshchai
//!
//! == znakomstvo
//! kuznec.znakom = true
//! hero.ruda += 1
//! -> konec
//! ```
//!
//! Остальные карточки: `событие quest.start("mech")`, `выполнить open_door("vorota")`,
//! `пауза 1.5` (без числа — до нажатия), `наугад -> a, b`, `процент 30 -> a | b`,
//! `перейти kuznec.ruda` (или `kuznec.ruda#nachalo`), `конец`, живая реплика
//! `живая kuznec (20 слов; может: relation.add): задача` со строкой `запасной: текст`.
//! Перенос строки внутри реплики — `\n`. Строки `//` — пометки для себя, в диалог они не
//! попадают.
//!
//! Чего в тексте нет, берётся из старого диалога по имени карточки: место на схеме,
//! настройки реплики (вид, настроение, голос), ключи перевода. Так схема и текст — два
//! способа править один файл.

use crate::format::{Change, Choice, Dialog, LineSettings, Node, Value};
use crate::valid_id;

/// Шаг по X для новой карточки, у которой нет места на схеме.
const NEW_STEP: f32 = 390.0;

/// Диалог текстом.
pub fn to_text(d: &Dialog) -> String {
    let mut out = String::new();
    for n in &d.nodes {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("== {}\n", n.id()));
        let next = |out: &mut String, next: &str| {
            if !next.is_empty() {
                out.push_str(&format!("-> {next}\n"));
            }
        };
        match n {
            Node::Line {
                who,
                text,
                choices,
                next: nx,
                ..
            } => {
                out.push_str(&format!("{who}: {}\n", escape_line(text)));
                for c in choices {
                    out.push_str(&format!("* {}", escape_line(&c.text)));
                    if !c.when.trim().is_empty() {
                        out.push_str(&format!(" [если {}]", c.when.trim()));
                    }
                    if !c.next.is_empty() {
                        out.push_str(&format!(" -> {}", c.next));
                    }
                    out.push('\n');
                }
                next(&mut out, nx);
            }
            Node::Generate {
                who,
                task,
                fallback,
                words,
                allowed,
                next: nx,
                ..
            } => {
                let may = if allowed.is_empty() {
                    String::new()
                } else {
                    format!("; может: {}", allowed.join(", "))
                };
                out.push_str(&format!(
                    "живая {who} ({words} слов{may}): {}\n",
                    escape_line(task)
                ));
                out.push_str(&format!("запасной: {}\n", escape_line(fallback)));
                next(&mut out, nx);
            }
            Node::Check { when, yes, no, .. } => {
                out.push_str(&format!("если {} -> {yes}\n", when.trim()));
                if !no.is_empty() {
                    out.push_str(&format!("иначе -> {no}\n"));
                }
            }
            Node::Set {
                changes, next: nx, ..
            } => {
                for c in changes {
                    out.push_str(&change_text(c));
                    out.push('\n');
                }
                next(&mut out, nx);
            }
            Node::Event {
                name,
                args,
                next: nx,
                ..
            } => {
                out.push_str(&format!("событие {name}({})\n", args_text(args)));
                next(&mut out, nx);
            }
            Node::Run {
                func,
                args,
                next: nx,
                ..
            } => {
                out.push_str(&format!("выполнить {func}({})\n", args_text(args)));
                next(&mut out, nx);
            }
            Node::Wait {
                seconds, next: nx, ..
            } => {
                match seconds {
                    Some(s) => out.push_str(&format!("пауза {s}\n")),
                    None => out.push_str("пауза\n"),
                }
                next(&mut out, nx);
            }
            Node::Random { next: nx, .. } => {
                out.push_str(&format!("наугад -> {}\n", nx.join(", ")));
            }
            Node::Chance {
                percent, yes, no, ..
            } => {
                out.push_str(&format!("процент {percent} -> {yes} | {no}\n"));
            }
            Node::Jump { dialog, node, .. } => {
                if node.is_empty() {
                    out.push_str(&format!("перейти {dialog}\n"));
                } else {
                    out.push_str(&format!("перейти {dialog}#{node}\n"));
                }
            }
            Node::End { .. } => out.push_str("конец\n"),
        }
    }
    out
}

/// Ошибка в тексте: строка (с 1) и что не так.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextError {
    pub line: usize,
    pub text: String,
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "строка {}: {}", self.line, self.text)
    }
}

/// Диалог из текста. `old` — тот же диалог до правки: из него берутся заголовок,
/// участники, места карточек, настройки реплик и ключи перевода. Ошибки — все сразу.
pub fn from_text(text: &str, old: &Dialog) -> Result<Dialog, Vec<TextError>> {
    let mut errors = Vec::new();
    // Блоки: (строка заголовка, имя, строки тела с номерами).
    let mut blocks: Vec<(usize, String, Vec<Row>)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let t = raw.trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        if let Some(id) = t.strip_prefix("==") {
            blocks.push((line, id.trim().to_string(), Vec::new()));
        } else if let Some(b) = blocks.last_mut() {
            b.2.push((line, t));
        } else {
            errors.push(err(
                line,
                "строка до первой карточки: начните карточку строкой «== имя»",
            ));
        }
    }
    let mut nodes = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (line, id, body) in blocks {
        if !valid_id(&id) {
            errors.push(err(
                line,
                format!("имя карточки «{id}»: только латиница в нижнем регистре, цифры, _ и ."),
            ));
            continue;
        }
        if seen.contains(&id) {
            errors.push(err(line, format!("карточка «{id}» уже есть выше")));
            continue;
        }
        seen.push(id.clone());
        if body.is_empty() {
            errors.push(err(line, format!("карточка «{id}» пустая")));
            continue;
        }
        match parse_card(&id, &body) {
            Ok(n) => nodes.push(n),
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    // Что текст не хранит — из старого диалога.
    let mut last_at = (0.0, 0.0);
    for n in &mut nodes {
        let before = old.node(n.id());
        let at = before.map_or((last_at.0 + NEW_STEP, last_at.1), node_at);
        set_at(n, at);
        last_at = at;
        keep_extras(n, before);
    }
    if let Some(first) = nodes.first_mut()
        && old.nodes.is_empty()
    {
        set_at(first, (0.0, 0.0));
    }
    let mut d = old.clone();
    d.nodes = nodes;
    if !d.start.is_empty() && d.node(&d.start).is_none() {
        d.start.clear();
    }
    Ok(d)
}

/// Строка тела карточки: номер (с 1) и текст.
type Row<'a> = (usize, &'a str);

fn err(line: usize, text: impl Into<String>) -> TextError {
    TextError {
        line,
        text: text.into(),
    }
}

/// Одна карточка из строк тела.
fn parse_card(id: &str, body: &[(usize, &str)]) -> Result<Node, TextError> {
    let (first_line, first) = body[0];
    let id = id.to_string();
    let at = (0.0, 0.0);
    // Последняя строка «-> куда» — стрелка дальше (у карточек, где она есть).
    let (rest, next) = match body.last() {
        Some((_, t)) if body.len() > 1 && arrow_only(t).is_some() => (
            &body[..body.len() - 1],
            arrow_only(t).unwrap_or_default().to_string(),
        ),
        _ => (body, String::new()),
    };
    let only = |n: Node| -> Result<Node, TextError> {
        match rest.get(1) {
            Some((l, t)) => Err(err(*l, format!("лишняя строка в карточке «{id}»: {t}"))),
            None => Ok(n),
        }
    };
    let word = |w: &str| first == w || first.starts_with(&format!("{w} "));
    if word("если") {
        let (cond, yes) = split_arrow(&first["если".len()..])
            .ok_or_else(|| err(first_line, "после условия нужна стрелка: если … -> куда"))?;
        let mut no = String::new();
        for (l, t) in &body[1..] {
            match t.strip_prefix("иначе").map(str::trim).and_then(arrow_only) {
                Some(n) if no.is_empty() => no = n.to_string(),
                _ => return Err(err(*l, format!("ждали «иначе -> куда», а тут: {t}"))),
            }
        }
        return Ok(Node::Check {
            id,
            at,
            when: cond.trim().to_string(),
            yes: yes.to_string(),
            no,
        });
    }
    if word("событие") || word("выполнить") {
        let event = word("событие");
        let call = first
            .split_once(' ')
            .map_or("", |(_, r)| r)
            .trim()
            .to_string();
        let (name, args) = parse_call(&call).map_err(|e| err(first_line, e))?;
        return only(if event {
            Node::Event {
                id: id.clone(),
                at,
                name,
                args,
                next,
            }
        } else {
            Node::Run {
                id: id.clone(),
                at,
                func: name,
                args,
                next,
            }
        });
    }
    if word("пауза") {
        let n = first["пауза".len()..].trim().replace(',', ".");
        let seconds = if n.is_empty() {
            None
        } else {
            Some(
                n.parse::<f32>()
                    .map_err(|_| err(first_line, format!("пауза: «{n}» — не число секунд")))?,
            )
        };
        return only(Node::Wait {
            id: id.clone(),
            at,
            seconds,
            next,
        });
    }
    if word("наугад") {
        let to = arrow_only(first["наугад".len()..].trim())
            .ok_or_else(|| err(first_line, "наугад -> первая, вторая"))?;
        return only(Node::Random {
            id: id.clone(),
            at,
            next: to.split(',').map(|s| s.trim().to_string()).collect(),
        });
    }
    if word("процент") {
        let (p, to) = split_arrow(&first["процент".len()..])
            .ok_or_else(|| err(first_line, "процент 30 -> да | нет"))?;
        let percent = p
            .trim()
            .trim_end_matches('%')
            .replace(',', ".")
            .parse::<f32>()
            .map_err(|_| err(first_line, format!("процент: «{}» — не число", p.trim())))?;
        let (yes, no) = to.split_once('|').unwrap_or((to, ""));
        return only(Node::Chance {
            id: id.clone(),
            at,
            percent,
            yes: yes.trim().to_string(),
            no: no.trim().to_string(),
        });
    }
    if word("перейти") {
        let to = first["перейти".len()..].trim();
        let (dialog, node) = to.split_once('#').unwrap_or((to, ""));
        return only(Node::Jump {
            id: id.clone(),
            at,
            dialog: dialog.trim().to_string(),
            node: node.trim().to_string(),
        });
    }
    if first == "конец" {
        return only(Node::End { id: id.clone(), at });
    }
    if word("живая") {
        return parse_generate(id, rest, next);
    }
    if set_line(first).is_some() {
        let mut changes = Vec::new();
        for (l, t) in rest {
            let c = set_line(t)
                .ok_or_else(|| err(*l, format!("ждали «переменная = значение», а тут: {t}")))?
                .map_err(|e| err(*l, e.text))?;
            changes.push(c);
        }
        return Ok(Node::Set {
            id,
            at,
            changes,
            next,
        });
    }
    if let Some((who, text)) = first.split_once(':')
        && valid_id(who.trim())
    {
        let mut choices = Vec::new();
        for (l, t) in &rest[1..] {
            let Some(c) = t.strip_prefix('*') else {
                return Err(err(
                    *l,
                    format!("у реплики дальше только варианты «* текст -> куда»: {t}"),
                ));
            };
            choices.push(parse_choice(c.trim()));
        }
        if !choices.is_empty() && !next.is_empty() {
            return Err(err(
                body.last().map_or(first_line, |(l, _)| *l),
                "у реплики с вариантами нет «дальше»: стрелки — у вариантов",
            ));
        }
        return Ok(Node::Line {
            id,
            at,
            who: who.trim().to_string(),
            text: unescape_line(text.trim()),
            settings: LineSettings::default(),
            choices,
            next,
            key: String::new(),
        });
    }
    Err(err(
        first_line,
        format!(
            "непонятная строка: {first}. Реплика пишется «кто: текст», развилка — «если …», \
             переменная — «имя = значение»"
        ),
    ))
}

fn parse_generate(id: String, rest: &[(usize, &str)], next: String) -> Result<Node, TextError> {
    let (line, first) = rest[0];
    let head = first["живая".len()..].trim();
    let (head, task) = head
        .split_once("):")
        .map(|(h, t)| (format!("{h})"), t))
        .or_else(|| head.split_once(':').map(|(h, t)| (h.to_string(), t)))
        .ok_or_else(|| err(line, "живая кто (20 слов): задача"))?;
    let (who, opts) = head.split_once('(').unwrap_or((head.as_str(), ""));
    let mut words = 40;
    let mut allowed = Vec::new();
    for part in opts.trim_end_matches(')').split(';') {
        let p = part.trim();
        if let Some(a) = p.strip_prefix("может:") {
            allowed = a
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        } else if let Some(n) = p.strip_suffix("слов") {
            words = n
                .trim()
                .parse()
                .map_err(|_| err(line, format!("«{p}»: сколько слов — числом")))?;
        } else if !p.is_empty() {
            return Err(err(
                line,
                format!("непонятно: «{p}» (ждали «20 слов» или «может: …»)"),
            ));
        }
    }
    let mut fallback = None;
    for (l, t) in &rest[1..] {
        match t.strip_prefix("запасной:") {
            Some(f) if fallback.is_none() => fallback = Some(unescape_line(f.trim())),
            _ => return Err(err(*l, format!("ждали «запасной: текст», а тут: {t}"))),
        }
    }
    Ok(Node::Generate {
        id,
        at: (0.0, 0.0),
        who: who.trim().to_string(),
        task: unescape_line(task.trim()),
        fallback: fallback.unwrap_or_default(),
        words,
        allowed,
        settings: LineSettings::default(),
        next,
        key: String::new(),
    })
}

fn parse_choice(c: &str) -> Choice {
    let (rest, next) = split_arrow(c).unwrap_or((c, ""));
    let rest = rest.trim();
    let (text, when) = match rest.rfind("[если ") {
        Some(i) if rest.ends_with(']') => (&rest[..i], &rest[i + "[если ".len()..rest.len() - 1]),
        _ => (rest, ""),
    };
    Choice {
        text: unescape_line(text.trim()),
        when: when.trim().to_string(),
        next: next.trim().to_string(),
        key: String::new(),
    }
}

/// `имя = значение`, `имя += 2`, `имя -= 1`. `None` — строка не про переменную.
fn set_line(t: &str) -> Option<Result<Change, TextError>> {
    let (var, op, value) = ["+=", "-=", "="].iter().find_map(|op| {
        let (v, r) = t.split_once(op)?;
        Some((v.trim(), *op, r.trim()))
    })?;
    if !valid_id(var) || var.contains(':') {
        return None;
    }
    let change = match op {
        "=" => Change {
            var: var.into(),
            to: Some(parse_value(value)),
            add: None,
        },
        _ => {
            let n: f64 = match value.replace(',', ".").parse() {
                Ok(n) => n,
                Err(_) => {
                    return Some(Err(err(0, format!("{var} {op} {value}: нужно число"))));
                }
            };
            Change {
                var: var.into(),
                to: None,
                add: Some(if op == "-=" { -n } else { n }),
            }
        }
    };
    Some(Ok(change))
}

fn change_text(c: &Change) -> String {
    match (&c.to, c.add) {
        (_, Some(a)) if a < 0.0 => format!("{} -= {}", c.var, -a),
        (_, Some(a)) => format!("{} += {a}", c.var),
        (Some(v), None) => format!("{} = {}", c.var, value_text(v)),
        (None, None) => format!("{} = true", c.var),
    }
}

/// Значение так, как его пишут в тексте: строки в кавычках.
pub fn value_text(v: &Value) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) if f.fract() == 0.0 && f.is_finite() => format!("{f:.1}"),
        Value::Float(f) => f.to_string(),
        Value::Text(t) => format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\"")),
    }
}

fn args_text(args: &[Value]) -> String {
    args.iter().map(value_text).collect::<Vec<_>>().join(", ")
}

/// Значение из текста: `"текст"`, `true`/`false` (или да/нет), целое, дробное; иначе —
/// слово как текст.
pub fn parse_value(s: &str) -> Value {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        return Value::Text(unquote(inner));
    }
    match s {
        "true" | "да" => return Value::Bool(true),
        "false" | "нет" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(i) = s.parse::<i64>() {
        return Value::Int(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return Value::Float(f);
    }
    Value::Text(s.to_string())
}

fn unquote(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(n) = chars.next()
        {
            out.push(n);
        } else {
            out.push(c);
        }
    }
    out
}

/// `имя(аргументы)`.
fn parse_call(s: &str) -> Result<(String, Vec<Value>), String> {
    let (name, args) = match s.split_once('(') {
        Some((n, a)) => (
            n.trim(),
            a.trim()
                .strip_suffix(')')
                .ok_or_else(|| format!("«{s}»: нет закрывающей скобки"))?,
        ),
        None => (s.trim(), ""),
    };
    if name.is_empty() {
        return Err("нужно имя: событие quest.start(\"mech\")".into());
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in args.chars() {
        match c {
            _ if escaped => {
                cur.push(c);
                escaped = false;
            }
            '\\' if quoted => {
                cur.push(c);
                escaped = true;
            }
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => out.push(parse_value(&std::mem::take(&mut cur))),
            _ => cur.push(c),
        }
    }
    if quoted {
        return Err(format!("«{s}»: кавычка не закрыта"));
    }
    if !cur.trim().is_empty() || !out.is_empty() {
        out.push(parse_value(&cur));
    }
    Ok((name.to_string(), out))
}

/// `что-то -> куда` → (что-то, куда). Стрелка — последняя `->` или `→`.
fn split_arrow(s: &str) -> Option<(&str, &str)> {
    let (i, len) = [
        s.rfind("->").map(|i| (i, 2)),
        s.rfind('→').map(|i| (i, '→'.len_utf8())),
    ]
    .into_iter()
    .flatten()
    .max_by_key(|(i, _)| *i)?;
    Some((&s[..i], s[i + len..].trim()))
}

/// Строка только со стрелкой: `-> куда`.
fn arrow_only(s: &str) -> Option<&str> {
    let (before, to) = split_arrow(s)?;
    before.trim().is_empty().then_some(to)
}

fn escape_line(s: &str) -> String {
    s.replace('\n', "\\n")
}

fn unescape_line(s: &str) -> String {
    s.replace("\\n", "\n")
}

fn node_at(n: &Node) -> (f32, f32) {
    match n {
        Node::Line { at, .. }
        | Node::Generate { at, .. }
        | Node::Check { at, .. }
        | Node::Set { at, .. }
        | Node::Event { at, .. }
        | Node::Run { at, .. }
        | Node::Jump { at, .. }
        | Node::Random { at, .. }
        | Node::Chance { at, .. }
        | Node::Wait { at, .. }
        | Node::End { at, .. } => *at,
    }
}

fn set_at(n: &mut Node, to: (f32, f32)) {
    match n {
        Node::Line { at, .. }
        | Node::Generate { at, .. }
        | Node::Check { at, .. }
        | Node::Set { at, .. }
        | Node::Event { at, .. }
        | Node::Run { at, .. }
        | Node::Jump { at, .. }
        | Node::Random { at, .. }
        | Node::Chance { at, .. }
        | Node::Wait { at, .. }
        | Node::End { at, .. } => *at = to,
    }
}

/// Настройки реплик и ключи перевода — из той же карточки до правки.
fn keep_extras(n: &mut Node, before: Option<&Node>) {
    match (n, before) {
        (
            Node::Line {
                settings,
                key,
                choices,
                ..
            },
            Some(Node::Line {
                settings: s0,
                key: k0,
                choices: c0,
                ..
            }),
        ) => {
            *settings = s0.clone();
            key.clone_from(k0);
            for (c, old) in choices.iter_mut().zip(c0) {
                c.key.clone_from(&old.key);
            }
        }
        (
            Node::Generate { settings, key, .. },
            Some(Node::Generate {
                settings: s0,
                key: k0,
                ..
            }),
        ) => {
            *settings = s0.clone();
            key.clone_from(k0);
        }
        _ => {}
    }
}
