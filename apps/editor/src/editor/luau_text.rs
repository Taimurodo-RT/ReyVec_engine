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
const KNOWN: [&str; 17] = [
    "me", "hero", "dt", "update", "math", "string", "table", "print", "pairs", "ipairs",
    "tostring", "tonumber", "type", "select", "error", "pcall", "bit32",
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

/// Поле, которое знает движок: имя, что значит, пишет ли его скрипт.
pub(crate) struct Hint {
    pub name: &'static str,
    pub about: &'static str,
    pub write: bool,
}

const fn read(name: &'static str, about: &'static str) -> Hint {
    Hint {
        name,
        about,
        write: false,
    }
}

const fn write(name: &'static str, about: &'static str) -> Hint {
    Hint {
        name,
        about,
        write: true,
    }
}

/// Поля `me`: что движок пишет перед вызовом и что читает после (`reyvec-script`).
pub(crate) const ME: [Hint; 15] = [
    read(
        "x",
        "где предмет по X, метры; скрипт предмета может записать новое место",
    ),
    read("y", "где предмет по Y, метры; можно записать новое место"),
    read("vx", "скорость по X, м/с"),
    read("vy", "скорость по Y, м/с"),
    read("on_ground", "стоит на земле"),
    read("wall", "стена сбоку: -1 слева, 1 справа, 0 нет"),
    read("hp", "здоровье"),
    read("alive", "цел (не разбит)"),
    read("touching", "касается героя"),
    read("time", "время сцены, секунды"),
    write("move", "бег: от -1 (влево) до 1 (вправо)"),
    write("jump", "прыгнуть в этом шаге"),
    write("hidden", "не виден"),
    write("collected", "подобран: движок один раз даёт звон и блёстки"),
    write("…", "свои поля можно хранить тут же: me.home, me.dir"),
];

/// Поля `hero`.
pub(crate) const HERO: [Hint; 5] = [
    read("x", "где герой по X, метры"),
    read("y", "где герой по Y, метры"),
    read("vx", "скорость героя по X"),
    read("vy", "скорость героя по Y"),
    read("striking", "герой бьёт"),
];

/// Самое нужное из `math`.
const MATH: [&str; 16] = [
    "abs", "ceil", "clamp", "cos", "floor", "huge", "max", "min", "noise", "pi", "random", "round",
    "sign", "sin", "sqrt", "atan2",
];

/// Что подсказать на месте курсора: начало слова (байт), что уже набрано и варианты.
pub(crate) fn complete(text: &str, cursor: usize) -> Option<(usize, String, Vec<&'static str>)> {
    let before = text.get(..cursor)?;
    let word_start = before
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    let typed = &before[word_start..];
    let base_end = word_start.checked_sub(1)?;
    if before.as_bytes().get(base_end) != Some(&b'.') {
        return None;
    }
    let base_start = before[..base_end]
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    // Внутри комментария или строки не подсказывать.
    let inside = tokens(text)
        .into_iter()
        .any(|(r, t)| matches!(t, Tok::Comment | Tok::Str) && r.start < cursor && cursor < r.end);
    if inside {
        return None;
    }
    let names: Vec<&'static str> = match &before[base_start..base_end] {
        "me" => ME.iter().map(|h| h.name).filter(|n| *n != "…").collect(),
        "hero" => HERO.iter().map(|h| h.name).collect(),
        "math" => MATH.to_vec(),
        _ => return None,
    };
    let found: Vec<&'static str> = names
        .into_iter()
        .filter(|n| n.starts_with(typed) && *n != typed)
        .collect();
    (!found.is_empty()).then(|| (word_start, typed.to_owned(), found))
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
    fn indent_after_block_start() {
        assert_eq!(next_indent("    if me.alive then"), "        ");
        assert_eq!(next_indent("function update(me, hero, dt)"), "    ");
        assert_eq!(next_indent("    me.x = 1 -- then"), "    ");
        assert_eq!(next_indent("\tx = 1"), "\t");
    }
}
