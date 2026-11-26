//! Текст скрипта Luau для редактора кода (С.1): разбор на слова для подсветки, проверка
//! без запуска и подсказки полей `me` и `hero`.
//!
//! Разбор нарочно простой: он различает слова, числа, строки и комментарии, этого
//! хватает для цвета и подсказок. Правильность кода проверяет сам Luau (`check`):
//! текст переводится в байт-код, но не выполняется, так что проверка ничего не ломает
//! и не может зависнуть.

use std::ops::Range;

/// Вид куска текста.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tok {
    Keyword,
    /// `true`, `false`, `nil`.
    Literal,
    Number,
    Str,
    Comment,
    /// Имя, которое даёт движок или Luau: `me`, `hero`, `dt`, `math`, `print`…
    Known,
    /// Имя после точки: поле таблицы.
    Field,
    /// Имя функции сразу после `function`.
    FnName,
    Ident,
    Punct,
    Space,
}

const KEYWORDS: [&str; 19] = [
    "and", "break", "continue", "do", "else", "elseif", "end", "for", "function", "if", "in",
    "local", "not", "or", "repeat", "return", "then", "until", "while",
];

/// Имена, которые есть у скрипта и без объявления.
const KNOWN: [&str; 19] = [
    "me", "hero", "dt", "update", "math", "string", "table", "print", "pairs", "ipairs",
    "tostring", "tonumber", "type", "select", "error", "pcall", "bit32", "story", "quest",
];

/// Разбить текст на куски (байтовые границы); куски идут подряд и покрывают весь текст.
pub(crate) fn tokens(text: &str) -> Vec<(Range<usize>, Tok)> {
    let b = text.as_bytes();
    let mut out: Vec<(Range<usize>, Tok)> = Vec::new();
    let mut i = 0;
    let mut after_function = false;
    while i < b.len() {
        let start = i;
        let c = b[i];
        let tok = if c == b'-' && b.get(i + 1) == Some(&b'-') {
            i += 2;
            if let Some(level) = long_bracket(b, i) {
                i = close_long(b, i + level + 2, level);
            } else {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            Tok::Comment
        } else if c == b'"' || c == b'\'' || c == b'`' {
            i += 1;
            while i < b.len() && b[i] != c && b[i] != b'\n' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            Tok::Str
        } else if c == b'[' && long_bracket(b, i).is_some() {
            let level = long_bracket(b, i).unwrap_or(0);
            i = close_long(b, i + level + 2, level);
            Tok::Str
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            i += 1;
            while i < b.len()
                && (b[i].is_ascii_alphanumeric()
                    || b[i] == b'.'
                    || b[i] == b'_'
                    || ((b[i] == b'-' || b[i] == b'+') && matches!(b[i - 1], b'e' | b'E')))
            {
                i += 1;
            }
            Tok::Number
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            let after_dot = out
                .iter()
                .rev()
                .find(|(_, t)| *t != Tok::Space)
                .is_some_and(|(r, t)| *t == Tok::Punct && matches!(&text[r.clone()], "." | ":"));
            if after_dot {
                Tok::Field
            } else if KEYWORDS.contains(&word) {
                Tok::Keyword
            } else if matches!(word, "true" | "false" | "nil") {
                Tok::Literal
            } else if after_function {
                Tok::FnName
            } else if KNOWN.contains(&word) {
                Tok::Known
            } else {
                Tok::Ident
            }
        } else if c.is_ascii_whitespace() {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            Tok::Space
        } else {
            // Знак: один символ (русские буквы вне строк — тоже по одному символу).
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            Tok::Punct
        };
        if tok != Tok::Space {
            after_function = tok == Tok::Keyword && &text[start..i] == "function";
        }
        out.push((start..i, tok));
    }
    out
}

/// `[[`, `[=[` …: сколько `=` между скобками, если здесь начало длинной строки.
fn long_bracket(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'[') {
        return None;
    }
    let mut n = 0;
    while b.get(i + 1 + n) == Some(&b'=') {
        n += 1;
    }
    (b.get(i + 1 + n) == Some(&b'[')).then_some(n)
}

/// Конец длинной строки или комментария с `level` знаками `=`.
fn close_long(b: &[u8], mut i: usize, level: usize) -> usize {
    while i < b.len() {
        if b[i] == b']'
            && b[i + 1..].iter().take(level).all(|&c| c == b'=')
            && b.get(i + 1 + level) == Some(&b']')
        {
            return i + level + 2;
        }
        i += 1;
    }
    b.len()
}

/// Ошибка в коде: строка (с 1; 0 — весь файл) и что не так.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    pub line: usize,
    pub text: String,
}

/// Проверить скрипт: Luau переводит его в байт-код (без запуска); ещё нужна функция
/// `update` — её зовёт движок.
pub(crate) fn check(lua: &mlua::Lua, name: &str, text: &str) -> Option<Problem> {
    if let Err(e) = lua.load(text).set_name(format!("={name}")).into_function() {
        return Some(problem_of(name, &e.to_string()));
    }
    if !has_update(text) {
        return Some(Problem {
            line: 0,
            text: "Нет функции update(me, hero, dt): движок зовёт её каждый шаг игры".into(),
        });
    }
    None
}

/// Из сообщения Luau «имя:12: что не так» достать строку и текст.
fn problem_of(name: &str, message: &str) -> Problem {
    let tail = message
        .find(&format!("{name}:"))
        .map(|at| &message[at + name.len() + 1..]);
    if let Some(tail) = tail {
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(line) = digits.parse::<usize>() {
            let rest = tail[digits.len()..].trim_start_matches(':').trim();
            return Problem {
                line,
                text: rest.lines().next().unwrap_or(rest).to_owned(),
            };
        }
    }
    Problem {
        line: 0,
        text: message.lines().next().unwrap_or(message).to_owned(),
    }
}

/// В тексте объявлена `update`: `function update(` или `update = function`.
fn has_update(text: &str) -> bool {
    let words: Vec<&str> = tokens(text)
        .into_iter()
        .filter(|(_, t)| !matches!(t, Tok::Space | Tok::Comment))
        .map(|(r, _)| &text[r])
        .collect();
    words.windows(2).any(|w| w == ["function", "update"])
        || words.windows(3).any(|w| w == ["update", "=", "function"])
}

/// Поля `me` и `hero` описаны в движке (`reyvec-script`) рядом с кодом, который их
/// заполняет: новое поле движка редактор подсказывает сам.
pub(crate) use reyvec_script::{Field as Hint, HERO_FIELDS as HERO, ME_FIELDS as ME};
/// Функции сюжета (Сю.5) тоже описаны в движке.
pub(crate) use reyvec_script::{Func, QUEST_FUNCS, STORY_FUNCS};

/// Функции таблицы `story` или `quest`.
pub(crate) fn funcs_of(table: &str) -> Option<&'static [Func]> {
    match table {
        "story" => Some(STORY_FUNCS),
        "quest" => Some(QUEST_FUNCS),
        _ => None,
    }
}

/// Имена из сюжета проекта: на них ссылаются `story.talk("…")`, `quest.add("…")`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StoryNames {
    pub dialogs: Vec<String>,
    pub quests: Vec<String>,
    pub vars: Vec<String>,
}

impl StoryNames {
    /// Из папки `story/` проекта (нет папки — пусто, и имена не проверяются).
    pub(crate) fn of_project(project: &std::path::Path) -> Self {
        let (s, _) = reyvec_story::Story::load_dir(&project.join("story"));
        Self {
            dialogs: s.dialogs.into_keys().collect(),
            quests: s.quests.into_keys().collect(),
            vars: s.vars.into_keys().collect(),
        }
    }

    fn is_empty(&self) -> bool {
        self.dialogs.is_empty() && self.quests.is_empty() && self.vars.is_empty()
    }
}

/// Что за имя стоит в кавычках первым аргументом функции сюжета.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoryArg {
    Dialog,
    Quest,
    Var,
}

impl StoryArg {
    fn list(self, n: &StoryNames) -> &[String] {
        match self {
            StoryArg::Dialog => &n.dialogs,
            StoryArg::Quest => &n.quests,
            StoryArg::Var => &n.vars,
        }
    }

    fn what(self) -> &'static str {
        match self {
            StoryArg::Dialog => "диалога",
            StoryArg::Quest => "задания",
            StoryArg::Var => "переменной",
        }
    }
}

/// Строки-имена в вызовах сюжета: `story.talk("kuznec.privet")` → (место строки, вид,
/// имя без кавычек).
pub(crate) fn story_args(text: &str) -> Vec<(Range<usize>, StoryArg, String)> {
    let w = words(text);
    let mut out = Vec::new();
    for i in 0..w.len().saturating_sub(4) {
        let (table, dot, f, open, arg) = (&w[i], &w[i + 1], &w[i + 2], &w[i + 3], &w[i + 4]);
        if dot.2 != "." || open.2 != "(" || arg.1 != Tok::Str {
            continue;
        }
        let kind = match (table.2, f.2) {
            ("story", "talk" | "seen") => StoryArg::Dialog,
            ("story", "get" | "set") => StoryArg::Var,
            ("quest", _) => StoryArg::Quest,
            _ => continue,
        };
        let inner = arg.2.trim_matches(|c| c == '"' || c == '\'');
        out.push((arg.0.clone(), kind, inner.to_owned()));
    }
    out
}

/// Подсказка в кавычках: имена диалогов, заданий или переменных из сюжета проекта.
pub(crate) fn story_complete(
    text: &str,
    cursor: usize,
    names: &StoryNames,
) -> Option<(usize, String, Vec<String>)> {
    let (r, kind, _) = story_args(text)
        .into_iter()
        .find(|(r, _, _)| r.start < cursor && cursor <= r.end)?;
    let start = r.start + 1;
    let typed = text.get(start..cursor)?;
    if typed.contains(['"', '\'']) {
        return None;
    }
    let found: Vec<String> = kind
        .list(names)
        .iter()
        .filter(|n| n.starts_with(typed) && n.as_str() != typed)
        .cloned()
        .collect();
    (!found.is_empty()).then(|| (start, typed.to_owned(), found))
}

/// Опечатки в именах сюжета: нет такого диалога, задания или переменной.
pub(crate) fn story_warnings(text: &str, names: &StoryNames) -> Vec<Warning> {
    if names.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (r, kind, name) in story_args(text) {
        let list = kind.list(names);
        if list.contains(&name) {
            continue;
        }
        let near = list
            .iter()
            .map(|n| (distance(n, &name), n))
            .filter(|(d, _)| *d <= 3)
            .min_by_key(|(d, _)| *d)
            .map(|(_, n)| format!(" Может, {n}?"))
            .unwrap_or_default();
        out.push(Warning {
            line: text[..r.start].matches('\n').count() + 1,
            range: r,
            text: format!("В сюжете нет {} «{name}».{near}", kind.what()),
        });
    }
    out
}

/// Самое нужное из `math`.
const MATH: [&str; 16] = [
    "abs", "ceil", "clamp", "cos", "floor", "huge", "max", "min", "noise", "pi", "random", "round",
    "sign", "sin", "sqrt", "atan2",
];

/// Значимые куски (без пробелов и комментариев) с их текстом.
fn words(text: &str) -> Vec<(Range<usize>, Tok, &str)> {
    tokens(text)
        .into_iter()
        .filter(|(_, t)| !matches!(t, Tok::Space | Tok::Comment))
        .map(|(r, t)| {
            let w = &text[r.clone()];
            (r, t, w)
        })
        .collect()
}

/// Курсор внутри комментария или строки.
fn in_text_or_comment(text: &str, cursor: usize) -> bool {
    tokens(text)
        .into_iter()
        .any(|(r, t)| matches!(t, Tok::Comment | Tok::Str) && r.start < cursor && cursor < r.end)
}

/// Свои поля `me`, которые скрипт где-то записывает: `me.home = …`.
fn own_fields(text: &str) -> Vec<String> {
    let w = words(text);
    let mut out: Vec<String> = Vec::new();
    for i in 2..w.len().saturating_sub(1) {
        if w[i - 2].2 == "me" && w[i - 1].2 == "." && w[i].1 == Tok::Field && w[i + 1].2 == "=" {
            let name = w[i].2.to_owned();
            let next_is_eq = w.get(i + 2).is_some_and(|n| n.2 == "=");
            if !next_is_eq && !out.contains(&name) && !ME.iter().any(|f| f.name == name) {
                out.push(name);
            }
        }
    }
    out
}

/// Имена, объявленные в скрипте: `local`, функции, их параметры, переменные `for`.
pub(crate) fn locals(text: &str) -> Vec<String> {
    let w = words(text);
    let mut out: Vec<String> = Vec::new();
    let mut push = |name: &str| {
        if !out.iter().any(|n| n == name) {
            out.push(name.to_owned());
        }
    };
    let mut i = 0;
    while i < w.len() {
        match (w[i].1, w[i].2) {
            (Tok::Keyword, "local") | (Tok::Keyword, "for") => {
                // Имена через запятую до «=», «in» или конца.
                let mut j = i + 1;
                if w.get(j).is_some_and(|x| x.2 == "function") {
                    j += 1;
                }
                while let Some(x) = w.get(j) {
                    if matches!(x.1, Tok::Ident | Tok::Known | Tok::FnName) {
                        push(x.2);
                    } else if x.2 != "," {
                        break;
                    }
                    j += 1;
                }
                i = j;
            }
            (Tok::FnName, name) => {
                push(name);
                // Параметры в скобках.
                let mut j = i + 1;
                while let Some(x) = w.get(j) {
                    if x.2 == ")" {
                        break;
                    }
                    if matches!(x.1, Tok::Ident | Tok::Known) {
                        push(x.2);
                    }
                    j += 1;
                }
                i = j;
            }
            _ => i += 1,
        }
    }
    out
}

/// Что подсказать на месте курсора: начало слова (байт), что уже набрано и варианты.
/// После `me.`, `hero.`, `math.` — поля (у `me` и свои, которые скрипт записывает);
/// без точки, с двух букв — свои имена, имена движка и слова языка.
pub(crate) fn complete(text: &str, cursor: usize) -> Option<(usize, String, Vec<String>)> {
    let before = text.get(..cursor)?;
    let word_start = before
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    let typed = &before[word_start..];
    if in_text_or_comment(text, cursor) {
        return None;
    }
    let dot = word_start
        .checked_sub(1)
        .filter(|&b| before.as_bytes().get(b) == Some(&b'.'));
    let names: Vec<String> = match dot {
        Some(base_end) => {
            let base_start = before[..base_end]
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(0, |i| i + 1);
            match &before[base_start..base_end] {
                "me" => ME
                    .iter()
                    .map(|h| h.name.to_owned())
                    .chain(own_fields(text))
                    .collect(),
                "hero" => HERO.iter().map(|h| h.name.to_owned()).collect(),
                "math" => MATH.iter().map(|n| (*n).to_owned()).collect(),
                t if funcs_of(t).is_some() => funcs_of(t)
                    .unwrap_or_default()
                    .iter()
                    .map(|f| f.name.to_owned())
                    .collect(),
                _ => return None,
            }
        }
        None => {
            // Цифра в начале — число, не имя.
            if typed.chars().count() < 2 || typed.starts_with(|c: char| c.is_ascii_digit()) {
                return None;
            }
            let mut names = locals(text);
            names.extend(KNOWN.iter().map(|n| (*n).to_owned()));
            names.extend(KEYWORDS.iter().map(|n| (*n).to_owned()));
            names
        }
    };
    let mut found: Vec<String> = Vec::new();
    for n in names {
        if n.starts_with(typed) && n != typed && !found.contains(&n) {
            found.push(n);
        }
    }
    (!found.is_empty()).then(|| (word_start, typed.to_owned(), found))
}

/// Имя под курсором (байты): курсор внутри слова или сразу за ним.
pub(crate) fn word_at(text: &str, at: usize) -> Option<(Range<usize>, Tok)> {
    tokens(text).into_iter().find(|(r, t)| {
        matches!(t, Tok::Ident | Tok::Known | Tok::Field | Tok::FnName)
            && r.start <= at
            && at <= r.end
    })
}

/// Все места того же имени: для поля — то же поле той же таблицы, для имени — то же имя.
pub(crate) fn same_words(text: &str, word: &Range<usize>) -> Vec<Range<usize>> {
    let w = words(text);
    let Some(i) = w.iter().position(|x| x.0 == *word) else {
        return Vec::new();
    };
    let base = |i: usize| (i >= 2 && w[i - 1].2 == ".").then(|| w[i - 2].2);
    let (name, field, b) = (w[i].2, w[i].1 == Tok::Field, base(i));
    w.iter()
        .enumerate()
        .filter(|(j, x)| {
            x.2 == name
                && (x.1 == Tok::Field) == field
                && (!field || base(*j) == b)
                && matches!(x.1, Tok::Ident | Tok::Known | Tok::Field | Tok::FnName)
        })
        .map(|(_, x)| x.0.clone())
        .collect()
}

/// Пары: блок (`function`/`if`/`for`/`while`/`do` … `end`, `repeat` … `until`) и
/// скобки. Второе — открытые, которым нет пары.
/// Пара: где открыто и где закрыто (байты).
pub(crate) type Pair = (Range<usize>, Range<usize>);

pub(crate) fn pairs(text: &str) -> (Vec<Pair>, Vec<Range<usize>>) {
    #[derive(PartialEq)]
    enum Open {
        Block,
        /// `for`/`while` до своего `do`.
        Loop,
        Repeat,
        Bracket(&'static str),
    }
    let mut stack: Vec<(Range<usize>, Open)> = Vec::new();
    let mut out = Vec::new();
    for (r, t, w) in words(text) {
        match (t, w) {
            (Tok::Keyword, "function" | "if") => stack.push((r, Open::Block)),
            (Tok::Keyword, "for" | "while") => stack.push((r, Open::Loop)),
            (Tok::Keyword, "do") => {
                if let Some(top) = stack.last_mut()
                    && top.1 == Open::Loop
                {
                    top.1 = Open::Block;
                } else {
                    stack.push((r, Open::Block));
                }
            }
            (Tok::Keyword, "repeat") => stack.push((r, Open::Repeat)),
            (Tok::Keyword, "end") => {
                if stack.last().is_some_and(|s| s.1 == Open::Block) {
                    let (o, _) = stack.pop().unwrap_or_else(|| unreachable!());
                    out.push((o, r));
                }
            }
            (Tok::Keyword, "until") => {
                if stack.last().is_some_and(|s| s.1 == Open::Repeat) {
                    let (o, _) = stack.pop().unwrap_or_else(|| unreachable!());
                    out.push((o, r));
                }
            }
            (Tok::Punct, "(") => stack.push((r, Open::Bracket(")"))),
            (Tok::Punct, "[") => stack.push((r, Open::Bracket("]"))),
            (Tok::Punct, "{") => stack.push((r, Open::Bracket("}"))),
            (Tok::Punct, ")" | "]" | "}") => {
                let want = match w {
                    ")" => ")",
                    "]" => "]",
                    _ => "}",
                };
                if stack.last().is_some_and(|s| s.1 == Open::Bracket(want)) {
                    let (o, _) = stack.pop().unwrap_or_else(|| unreachable!());
                    out.push((o, r));
                }
            }
            _ => {}
        }
    }
    let open = stack
        .into_iter()
        .filter(|(_, o)| !matches!(o, Open::Bracket(_)))
        .map(|(r, _)| r)
        .collect();
    (out, open)
}

/// Пара для слова или скобки под курсором (курсор внутри или сразу за ним).
pub(crate) fn pair_at(text: &str, at: usize) -> Option<Pair> {
    let touches = |r: &Range<usize>| r.start <= at && at <= r.end;
    pairs(text)
        .0
        .into_iter()
        .find(|(a, b)| touches(a) || touches(b))
}

/// Блоки без `end`: после Enter за `then`/`do`/`function(…)` редактор ставит `end` сам.
pub(crate) fn needs_end(text: &str) -> bool {
    !pairs(text).1.is_empty()
}

/// Подозрительное место: не ошибка Luau, но, скорее всего, опечатка.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Warning {
    pub range: Range<usize>,
    pub line: usize,
    pub text: String,
}

/// Поля `me`: какие свои поля скрипт где-то читает и где-то пишет.
fn me_uses(text: &str) -> (Vec<String>, Vec<String>) {
    let w = words(text);
    let (mut reads, mut writes) = (Vec::new(), Vec::new());
    for i in 2..w.len() {
        if w[i].1 == Tok::Field && w[i - 1].2 == "." && w[i - 2].2 == "me" {
            let is_write = w.get(i + 1).is_some_and(|n| n.2 == "=")
                && !w.get(i + 2).is_some_and(|n| n.2 == "=");
            let list = if is_write { &mut writes } else { &mut reads };
            if !list.iter().any(|n: &String| n == w[i].2) {
                list.push(w[i].2.to_owned());
            }
        }
    }
    (reads, writes)
}

/// Подозрительные поля: у `me` — не из движка и только пишется или только читается
/// (своё поле и пишут, и читают), у `hero` — которых нет.
pub(crate) fn warnings(text: &str) -> Vec<Warning> {
    let w = words(text);
    let (reads, writes) = me_uses(text);
    let mut out = Vec::new();
    for i in 2..w.len() {
        if w[i].1 != Tok::Field || w[i - 1].2 != "." {
            continue;
        }
        let name = w[i].2;
        let line = text[..w[i].0.start].matches('\n').count() + 1;
        let near = |list: Vec<String>| {
            list.into_iter()
                .filter(|n| n != name)
                .map(|n| (distance(&n, name), n))
                .filter(|(d, _)| *d <= 2)
                .min_by_key(|(d, _)| *d)
                .map(|(_, n)| format!(" Может, {n}?"))
                .unwrap_or_default()
        };
        let read = reads.iter().any(|n| n == name);
        let written = writes.iter().any(|n| n == name);
        match w[i - 2].2 {
            "me" if !ME.iter().any(|f| f.name == name) && read != written => {
                let known: Vec<String> = ME
                    .iter()
                    .map(|f| f.name.to_owned())
                    .chain(reads.iter().filter(|n| writes.contains(n)).cloned())
                    .collect();
                let what = if written {
                    "записывается, но нигде не читается, и движок его не читает"
                } else {
                    "читается, но нигде не записывается, и движок его не даёт"
                };
                out.push(Warning {
                    range: w[i].0.clone(),
                    line,
                    text: format!("me.{name} {what}.{}", near(known)),
                });
            }
            t if funcs_of(t).is_some_and(|fs| !fs.iter().any(|f| f.name == name)) => {
                let known = funcs_of(t)
                    .unwrap_or_default()
                    .iter()
                    .map(|f| f.name.to_owned())
                    .collect();
                out.push(Warning {
                    range: w[i].0.clone(),
                    line,
                    text: format!("У {t} нет функции {name}.{}", near(known)),
                });
            }
            "hero" if !HERO.iter().any(|f| f.name == name) => {
                let known = HERO.iter().map(|f| f.name.to_owned()).collect();
                out.push(Warning {
                    range: w[i].0.clone(),
                    line,
                    text: format!("У героя нет поля {name}.{}", near(known)),
                });
            }
            _ => {}
        }
    }
    out
}

/// Сколько правок (буква вставить, убрать, заменить) отделяет одно слово от другого.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Подсказка при наведении на слово.
pub(crate) fn hover(text: &str, at: usize) -> Option<String> {
    let w = words(text);
    let i = w.iter().position(|x| x.0.start <= at && at < x.0.end)?;
    let (_, t, name) = &w[i];
    let base = (i >= 2 && w[i - 1].2 == ".").then(|| w[i - 2].2);
    let field = |list: &[Hint], who: &str| {
        list.iter().find(|f| f.name == *name).map(|f| {
            let by = if f.by_script {
                "пишет скрипт"
            } else {
                "даёт движок"
            };
            format!("{who}.{}: {} ({by})", f.name, f.about)
        })
    };
    match (t, base) {
        (Tok::Field, Some("me")) => field(ME, "me").or_else(|| {
            own_fields(text)
                .contains(&name.to_string())
                .then(|| format!("me.{name}: своё поле скрипта, живёт, пока жив мир"))
        }),
        (Tok::Field, Some("hero")) => field(HERO, "hero"),
        (Tok::Field, Some(t)) if funcs_of(t).is_some() => funcs_of(t)
            .unwrap_or_default()
            .iter()
            .find(|f| f.name == *name)
            .map(|f| format!("{t}.{}{}: {}", f.name, f.args, f.about)),
        (Tok::Known, _) => Some(
            match *name {
                "me" => "me: этот предмет. Движок пишет в него, где предмет и что с ним; скрипт — что делать",
                "hero" => "hero: герой (только читать): где он и бьёт ли",
                "dt" => "dt: шаг игры, секунды (около 1/60)",
                "update" => "update(me, hero, dt): движок зовёт её 60 раз в секунду",
                "math" => "math: числа (abs, min, max, sin, cos, floor, random…)",
                "string" => "string: работа с текстом",
                "table" => "table: работа со списками (insert, remove, sort…)",
                "print" => "print(…): строка в консоль редактора",
                "story" => "story: сюжет — разговоры и переменные мира (story.talk, story.get…)",
                "quest" => "quest: задания (quest.start, quest.add, quest.state…)",
                _ => return None,
            }
            .to_owned(),
        ),
        (Tok::Keyword, _) => Some(
            match *name {
                "local" => "local: новое имя, видно только в этом блоке",
                "function" => "function: функция, закрывается end",
                "if" => "if условие then … elseif … else … end",
                "for" => "for i = 1, 10 do … end — повторить; for k, v in pairs(t) do … end — по таблице",
                "while" => "while условие do … end — повторять, пока верно",
                "repeat" => "repeat … until условие — повторять, пока не станет верно",
                "return" => "return: выйти из функции (и вернуть значение)",
                "and" | "or" | "not" => "and, or, not: «и», «или», «не»",
                "end" => "end: конец блока",
                _ => return None,
            }
            .to_owned(),
        ),
        _ => None,
    }
}

/// Частые сообщения Luau по-русски; незнакомые остаются как есть.
pub(crate) fn russian(message: &str) -> String {
    let m = message.trim();
    let got = |m: &str| {
        m.rsplit_once(", got ")
            .map(|(_, g)| g.trim().to_owned())
            .unwrap_or_default()
    };
    let what = |g: &str| match g {
        "<eof>" => "конец файла".to_owned(),
        g => g.to_owned(),
    };
    let line_of = |m: &str| {
        m.split("at line ")
            .nth(1)
            .map(|t| {
                t.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    if m.starts_with("Expected 'then' when parsing if statement") {
        return format!("После условия if нужно then, а стоит {}", what(&got(m)));
    }
    if m.starts_with("Expected 'do' when parsing") {
        return format!("После for или while нужно do, а стоит {}", what(&got(m)));
    }
    if m.starts_with("Expected 'end' (to close") {
        let what_open = m.split('\'').nth(3).unwrap_or("блок").to_owned();
        return format!(
            "Нет end, который закрывает {what_open} из строки {}; вместо него {}",
            line_of(m),
            what(&got(m))
        );
    }
    if let Some(rest) = m.strip_prefix("Expected '")
        && let Some(close) = rest.split('\'').next()
        && m.contains("(to close '")
    {
        return format!(
            "Нет закрывающей {close} для скобки из строки {}; вместо неё {}",
            line_of(m),
            what(&got(m))
        );
    }
    if m.starts_with("Expected identifier") {
        return format!("Здесь должно быть имя, а стоит {}", what(&got(m)));
    }
    if m.starts_with("Incomplete statement") {
        return "Незаконченная строка: нужно присваивание (=) или вызов функции".into();
    }
    if m.starts_with("Malformed number") {
        return "Число записано неправильно".into();
    }
    if m.starts_with("Unfinished string") || m.starts_with("Malformed string") {
        return "Строка не закрыта кавычкой".into();
    }
    if m.starts_with("Unfinished long") {
        return "Длинная строка или комментарий не закрыты (]])".into();
    }
    if m.starts_with("Expected '=' when parsing assignment") {
        return "Нужно = (присваивание)".into();
    }
    if m.starts_with("Unexpected '=' ") || m.starts_with("Unexpected '='") {
        return "Лишнее =: для сравнения пишут ==".into();
    }
    if m.starts_with("Expected <eof>, got") {
        return format!("Лишнее {}: блок уже закрыт", what(&got(m)));
    }
    message.to_owned()
}

/// Закомментировать строки с `from` по `to` (байты) или раскомментировать, если они уже
/// все с `--`. Возвращает новый текст и где теперь конец последней строки.
pub(crate) fn toggle_comment(text: &str, from: usize, to: usize) -> (String, usize) {
    let start = text[..from].rfind('\n').map_or(0, |i| i + 1);
    let end = text[to..].find('\n').map_or(text.len(), |i| to + i);
    let block = &text[start..end];
    let lines: Vec<&str> = block.split('\n').collect();
    let all = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .all(|l| l.trim_start().starts_with("--"));
    let changed: Vec<String> = lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                return (*l).to_owned();
            }
            let indent = l.len() - l.trim_start().len();
            let (head, rest) = l.split_at(indent);
            if all {
                let rest = rest
                    .strip_prefix("-- ")
                    .or_else(|| rest.strip_prefix("--"))
                    .unwrap_or(rest);
                format!("{head}{rest}")
            } else {
                format!("{head}-- {rest}")
            }
        })
        .collect();
    let joined = changed.join("\n");
    let out = format!("{}{}{}", &text[..start], joined, &text[end..]);
    (out, start + joined.len())
}

/// Отступ для новой строки после строки `line`: тот же, а после `then`, `do`, `else`,
/// `repeat` и начала функции — на четыре пробела больше.
pub(crate) fn next_indent(line: &str) -> String {
    let base: String = line
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let code = line.split("--").next().unwrap_or("").trim_end();
    let opens = ["then", "do", "else", "repeat", "{", "("]
        .iter()
        .any(|w| code.ends_with(w))
        || (code.contains("function") && code.ends_with(')'));
    if opens { format!("{base}    ") } else { base }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(String, Tok)> {
        tokens(text)
            .into_iter()
            .filter(|(_, t)| *t != Tok::Space)
            .map(|(r, t)| (text[r].to_owned(), t))
            .collect()
    }

    #[test]
    fn tokens_cover_text_and_know_words() {
        let text = "local A = 0.25 -- скорость\nfunction update(me, hero, dt)\n  me.y = \"а\" .. [[б]]\nend";
        let all = tokens(text);
        assert_eq!(all.first().unwrap().0.start, 0);
        assert_eq!(all.last().unwrap().0.end, text.len());
        assert!(all.windows(2).all(|w| w[0].0.end == w[1].0.start));
        let k = kinds(text);
        let find = |w: &str| k.iter().find(|(s, _)| s == w).unwrap().1;
        assert_eq!(find("local"), Tok::Keyword);
        assert_eq!(find("0.25"), Tok::Number);
        assert_eq!(find("-- скорость"), Tok::Comment);
        assert_eq!(find("update"), Tok::FnName);
        assert_eq!(find("me"), Tok::Known);
        assert_eq!(find("y"), Tok::Field);
        assert_eq!(find("\"а\""), Tok::Str);
        assert_eq!(find("[[б]]"), Tok::Str);
        assert_eq!(find("A"), Tok::Ident);
        // Длинный комментарий тянется через строки.
        let k = kinds("--[[ раз\nдва ]] x");
        assert_eq!(k[0], ("--[[ раз\nдва ]]".to_owned(), Tok::Comment));
    }

    #[test]
    fn check_finds_line_and_missing_update() {
        let lua = mlua::Lua::new();
        let good = include_str!("../../../../scenes/scripts/beetle.luau");
        assert_eq!(check(&lua, "beetle.luau", good), None);
        assert_eq!(warnings(good), [], "у жука нет подозрительных полей");
        let coin = include_str!("../../../../scenes/scripts/coin.luau");
        assert_eq!(check(&lua, "coin.luau", coin), None);
        assert_eq!(warnings(coin), []);
        let bad =
            "function update(me, hero, dt)\n    if me.x > 1\n        me.move = 1\n    end\nend\n";
        let p = check(&lua, "b.luau", bad).unwrap();
        assert_eq!(p.line, 3, "{p:?}");
        assert!(!p.text.is_empty());
        let none = check(&lua, "c.luau", "local x = 1\n").unwrap();
        assert_eq!(none.line, 0);
        assert!(none.text.contains("update"));
        // Проверка не запускает код: вечный цикл наверху файла не вешает редактор.
        assert_eq!(
            check(&lua, "d.luau", "while true do end\nfunction update() end"),
            None
        );
    }

    #[test]
    fn completes_fields_after_dot() {
        let text = "function update(me, hero, dt)\n    me.to";
        let (at, typed, found) = complete(text, text.len()).unwrap();
        assert_eq!(&text[at..], "to");
        assert_eq!(typed, "to");
        assert_eq!(found, ["touching"]);
        let text = "hero.";
        assert_eq!(complete(text, 5).unwrap().2.len(), HERO.len());
        assert!(
            complete("-- me.x", 7).is_none(),
            "в комментарии не подсказывать"
        );
        assert!(complete("enemy.x", 7).is_none());
        assert!(complete("me.touching", 11).is_none(), "уже набрано целиком");
    }

    #[test]
    fn own_names_words_pairs_and_warnings() {
        let text = "local SPEED = 1\nfunction update(me, hero, dt)\n    me.home = me.home or me.x\n    for i, v in ipairs({}) do\n        me.mvoe = SPEED\n    end\n    if hero.xx then me.tim = me.hom end\nend";
        let names = locals(text);
        for n in ["SPEED", "update", "me", "hero", "dt", "i", "v"] {
            assert!(names.contains(&n.to_owned()), "{n}: {names:?}");
        }
        let (_, _, found) = complete("local SPEED = 1\nSP", 18).unwrap();
        assert_eq!(found, ["SPEED"]);
        let (_, _, found) = complete(text, text.find("me.hom ").unwrap() + 6).unwrap();
        assert_eq!(found, ["home"], "своё поле подсказывается");
        assert!(complete("x = 1", 1).is_none(), "одна буква — рано");
        // Все места me.home, но не me.x и не hero.x.
        let at = text.find("me.home").unwrap() + 4;
        let (word, _) = word_at(text, at).unwrap();
        assert_eq!(&text[word.clone()], "home");
        assert_eq!(same_words(text, &word).len(), 2);
        // Пары: function…end, for…end, if…end и скобки.
        let (pairs, open) = pairs(text);
        assert!(open.is_empty());
        let f = text.find("function").unwrap();
        let last_end = text.rfind("end").unwrap();
        assert!(pairs.contains(&(f..f + 8, last_end..last_end + 3)));
        let for_at = text.find("for").unwrap();
        let (a, b) = pair_at(text, for_at + 1).unwrap();
        assert_eq!((&text[a], &text[b]), ("for", "end"));
        assert!(needs_end(
            "function update(me, hero, dt)\n    if me.alive then\n    \nend"
        ));
        assert!(!needs_end(text));
        // Опечатки: me.mvoe, me.tim, hero.xx; me.hom — тоже (его никто не пишет).
        let w: Vec<String> = warnings(text).into_iter().map(|w| w.text).collect();
        assert_eq!(w.len(), 4, "{w:?}");
        assert!(w[0].contains("mvoe") && w[0].contains("move?"), "{w:?}");
        assert!(
            w.iter()
                .any(|t| t.contains("героя нет поля xx") && t.contains("x?"))
        );
        assert!(
            w.iter()
                .any(|t| t.contains("me.hom ") && t.contains("home?"))
        );
    }

    #[test]
    fn hover_and_russian_messages() {
        let text = "function update(me, hero, dt)\n    me.move = hero.x\nend";
        let h = hover(text, text.find("move").unwrap() + 1).unwrap();
        assert!(h.contains("бег") && h.contains("пишет скрипт"), "{h}");
        assert!(
            hover(text, text.find("hero.x").unwrap() + 5)
                .unwrap()
                .contains("даёт движок")
        );
        assert!(hover(text, 1).unwrap().contains("function"));
        let lua = mlua::Lua::new();
        for (bad, want) in [
            ("if x\n  y = 1\nend", "then"),
            ("function update()\n", "Нет end"),
            ("x = (1 + 2", "закрывающей )"),
            ("x = 1 +", "имя"),
            ("for i = 1, 2\n y = 1 end", "do"),
            ("x = \"abc\n", "кавычкой"),
        ] {
            let mut p = check(&lua, "t.luau", bad).unwrap();
            p.text = russian(&p.text);
            assert!(p.text.contains(want), "{bad:?}: {}", p.text);
        }
    }

    #[test]
    fn comment_toggles_lines() {
        let text = "a\n    b = 1\nc";
        let (t, end) = toggle_comment(text, 2, 5);
        assert_eq!(t, "a\n    -- b = 1\nc");
        assert_eq!(&t[..end], "a\n    -- b = 1");
        let (back, _) = toggle_comment(&t, 2, 5);
        assert_eq!(back, text);
        let (two, _) = toggle_comment(text, 0, text.len());
        assert_eq!(two, "-- a\n    -- b = 1\n-- c");
    }

    #[test]
    fn indent_after_block_start() {
        assert_eq!(next_indent("    if me.alive then"), "        ");
        assert_eq!(next_indent("function update(me, hero, dt)"), "    ");
        assert_eq!(next_indent("    me.x = 1 -- then"), "    ");
        assert_eq!(next_indent("\tx = 1"), "\t");
    }

    #[test]
    fn story_names_complete_and_typos_are_flagged() {
        let names = StoryNames {
            dialogs: vec!["kuznec.privet".into(), "kuznec.ruda".into()],
            quests: vec!["mech".into()],
            vars: vec!["hero.money".into()],
        };
        // После «story.» и «quest.» — функции движка.
        let t = "function update(me) story.ta";
        let (_, typed, items) = complete(t, t.len()).unwrap();
        assert_eq!((typed.as_str(), items), ("ta", vec!["talk".to_owned()]));
        let t = "quest.";
        let items = complete(t, t.len()).unwrap().2;
        assert!(items.contains(&"add".to_owned()) && items.contains(&"state".to_owned()));
        // В кавычках — имена из сюжета проекта.
        let t = "story.talk(\"kuznec.";
        let (at, typed, items) = story_complete(t, t.len(), &names).unwrap();
        assert_eq!((at, typed.as_str()), (12, "kuznec."));
        assert_eq!(
            items,
            vec!["kuznec.privet".to_owned(), "kuznec.ruda".to_owned()]
        );
        let t = "quest.add(\"m";
        assert_eq!(
            story_complete(t, t.len(), &names).unwrap().2,
            vec!["mech".to_owned()]
        );
        // Опечатки: имя не из сюжета и функция, которой нет.
        let t = "story.talk(\"kuznec.privt\")\nquest.add(\"mech\", \"ruda\")\nstory.get('hero.mony')\nquest.strat(\"mech\")";
        let w = story_warnings(t, &names);
        let texts: Vec<&str> = w.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "В сюжете нет диалога «kuznec.privt». Может, kuznec.privet?",
                "В сюжете нет переменной «hero.mony». Может, hero.money?",
            ]
        );
        assert_eq!(w[1].line, 3);
        let w = warnings(t);
        assert!(
            w.iter()
                .any(|w| w.text == "У quest нет функции strat. Может, start?"),
            "{w:?}"
        );
        // Без сюжета в проекте имена не проверяются.
        assert!(story_warnings(t, &StoryNames::default()).is_empty());
        assert!(
            hover("story.talk(x)", 7)
                .unwrap()
                .starts_with("story.talk(диалог): ")
        );
    }
}
