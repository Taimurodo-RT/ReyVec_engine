//! Точечная правка текста файла сцены (.ron): меняется только значение нужного поля
//! нужного предмета, всё остальное (комментарии, порядок полей, отступы, пустые строки)
//! остаётся как было. Поэтому сохранение из редактора не портит файл, который Роман
//! правит в блокноте.
//!
//! Разбор здесь нарочно простой: он понимает только скобки, строки, комментарии и
//! `имя: значение`. Правильность самого значения проверяет обычный разбор сцены после
//! правки.

use std::fmt::Write as _;

/// Не нашлось, что править.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EditError(pub String);

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Поле структуры: где имя и где значение (байты текста).
#[derive(Clone, Debug)]
struct Field {
    name: String,
    name_start: usize,
    value: std::ops::Range<usize>,
}

/// Пропускает пробелы и комментарии.
fn skip_ws(s: &[u8], mut i: usize) -> usize {
    loop {
        while i < s.len() && s[i].is_ascii_whitespace() {
            i += 1;
        }
        if s[i..].starts_with(b"//") {
            while i < s.len() && s[i] != b'\n' {
                i += 1;
            }
        } else if s[i..].starts_with(b"/*") {
            // Комментарии /* */ в RON вкладываются.
            let mut depth = 0;
            while i < s.len() {
                if s[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if s[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else {
            return i;
        }
    }
}

/// Конец строки в кавычках, начинающейся в `i` (`"…"` или `r#"…"#`).
fn string_end(s: &[u8], i: usize) -> Option<usize> {
    if s[i] == b'r' {
        let mut j = i + 1;
        let mut hashes = 0;
        while j < s.len() && s[j] == b'#' {
            hashes += 1;
            j += 1;
        }
        if s.get(j) != Some(&b'"') {
            return None;
        }
        j += 1;
        while j < s.len() {
            if s[j] == b'"'
                && s[j + 1..]
                    .iter()
                    .take(hashes)
                    .filter(|&&c| c == b'#')
                    .count()
                    == hashes
            {
                return Some(j + 1 + hashes);
            }
            j += 1;
        }
        return None;
    }
    let mut j = i + 1;
    while j < s.len() {
        match s[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// Конец значения, начинающегося в `i`: скобки целиком, строка, число или имя (а за
/// именем — его скобки: `Some(…)`, `Rect(…)`).
fn value_end(s: &[u8], i: usize) -> Option<usize> {
    match s.get(i)? {
        b'(' | b'[' | b'{' => group_end(s, i),
        b'"' => string_end(s, i),
        b'r' if matches!(s.get(i + 1), Some(b'"' | b'#')) => string_end(s, i),
        _ => {
            let mut j = i;
            while j < s.len()
                && !matches!(s[j], b',' | b')' | b']' | b'}' | b'(' | b'[' | b'{')
                && !s[j].is_ascii_whitespace()
                && !s[j..].starts_with(b"//")
            {
                j += 1;
            }
            if j == i {
                return None;
            }
            let k = skip_ws(s, j);
            if s.get(k) == Some(&b'(') {
                return group_end(s, k);
            }
            Some(j)
        }
    }
}

/// Конец скобок, открытых в `open` (за закрывающей скобкой).
fn group_end(s: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < s.len() {
        let j = skip_ws(s, i);
        if j != i {
            i = j;
            continue;
        }
        match s[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b'"' => {
                i = string_end(s, i)?;
                continue;
            }
            b'r' if matches!(s.get(i + 1), Some(b'"' | b'#')) => {
                i = string_end(s, i)?;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Поля структуры в скобках, открытых в `open`.
fn fields(s: &[u8], open: usize) -> Option<Vec<Field>> {
    let mut out = Vec::new();
    let mut i = skip_ws(s, open + 1);
    loop {
        if s.get(i)? == &b')' {
            return Some(out);
        }
        let name_start = i;
        while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'_') {
            i += 1;
        }
        if i == name_start {
            return None;
        }
        let name = String::from_utf8_lossy(&s[name_start..i]).into_owned();
        i = skip_ws(s, i);
        if s.get(i)? != &b':' {
            return None;
        }
        let start = skip_ws(s, i + 1);
        let end = value_end(s, start)?;
        out.push(Field {
            name,
            name_start,
            value: start..end,
        });
        i = skip_ws(s, end);
        if s.get(i) == Some(&b',') {
            i = skip_ws(s, i + 1);
        }
    }
}

/// Начала элементов списка в скобках `[`, открытых в `open`.
fn items(s: &[u8], open: usize) -> Option<Vec<usize>> {
    let mut out = Vec::new();
    let mut i = skip_ws(s, open + 1);
    loop {
        if s.get(i)? == &b']' {
            return Some(out);
        }
        out.push(i);
        let end = value_end(s, i)?;
        i = skip_ws(s, end);
        if s.get(i) == Some(&b',') {
            i = skip_ws(s, i + 1);
        }
    }
}

/// Открывающая скобка предмета по пути из номеров (`[2, 0]` — первый вложенный в
/// третий предмет списка `nodes`).
fn node_open(s: &[u8], path: &[usize]) -> Result<usize, EditError> {
    let err = |what: &str| EditError(what.to_owned());
    let root = skip_ws(s, 0);
    if s.get(root) != Some(&b'(') {
        return Err(err("файл сцены должен начинаться со скобки «(»"));
    }
    let mut list_field = "nodes";
    let mut open = root;
    for (depth, &n) in path.iter().enumerate() {
        let f = fields(s, open).ok_or_else(|| err("не разобрать поля в файле сцены"))?;
        let list = f
            .iter()
            .find(|f| f.name == list_field)
            .ok_or_else(|| err("в файле нет такого предмета"))?;
        if s.get(list.value.start) != Some(&b'[') {
            return Err(err("список предметов должен быть в скобках [ ]"));
        }
        let it = items(s, list.value.start).ok_or_else(|| err("не разобрать список предметов"))?;
        let at = *it.get(n).ok_or_else(|| {
            err(&format!(
                "в файле нет предмета №{} на уровне {}",
                n + 1,
                depth + 1
            ))
        })?;
        if s.get(at) != Some(&b'(') {
            return Err(err("предмет должен быть в скобках ( )"));
        }
        open = at;
        list_field = "children";
    }
    if path.is_empty() {
        return Err(err("пустой путь к предмету"));
    }
    Ok(open)
}

/// Отступ строки, в которой стоит байт `at`.
fn indent_of(s: &[u8], at: usize) -> &str {
    let line = s[..at]
        .iter()
        .rposition(|&c| c == b'\n')
        .map_or(0, |p| p + 1);
    let end = s[line..at]
        .iter()
        .position(|c| !matches!(c, b' ' | b'\t'))
        .map_or(at, |p| line + p);
    std::str::from_utf8(&s[line..end]).unwrap_or("")
}

/// Ставит полю `field` предмета `path` значение `value` (уже записанное по правилам
/// RON). Поля нет — оно добавляется первым, с отступом соседних полей.
pub(crate) fn set_field(
    text: &str,
    path: &[usize],
    field: &str,
    value: &str,
) -> Result<String, EditError> {
    let open = node_open(text.as_bytes(), path)?;
    set_field_at(text, open, field, value)
}

/// Как [`set_field`], но у самой сцены (`background`, `ambient`).
pub(crate) fn set_root_field(text: &str, field: &str, value: &str) -> Result<String, EditError> {
    let open = root_open(text.as_bytes())?;
    set_field_at(text, open, field, value)
}

/// Скобка `(` всей сцены.
fn root_open(s: &[u8]) -> Result<usize, EditError> {
    let root = skip_ws(s, 0);
    if s.get(root) != Some(&b'(') {
        return Err(EditError(
            "файл сцены должен начинаться со скобки «(»".into(),
        ));
    }
    Ok(root)
}

/// Скобка `(` предмета `path` или всей сцены (`path` пустой).
fn owner_open(s: &[u8], path: &[usize]) -> Result<usize, EditError> {
    if path.is_empty() {
        root_open(s)
    } else {
        node_open(s, path)
    }
}

/// Поле `field` структуры в скобках, открытых в `open`.
fn set_field_at(text: &str, open: usize, field: &str, value: &str) -> Result<String, EditError> {
    let s = text.as_bytes();
    let f = fields(s, open).ok_or_else(|| EditError("не разобрать поля предмета".into()))?;
    let mut out = String::with_capacity(text.len() + value.len() + 16);
    if let Some(found) = f.iter().find(|f| f.name == field) {
        out.push_str(&text[..found.value.start]);
        out.push_str(value);
        out.push_str(&text[found.value.end..]);
        return Ok(out);
    }
    match f.first() {
        Some(first) => {
            let multiline = s[open..first.name_start].contains(&b'\n');
            out.push_str(&text[..first.name_start]);
            if multiline {
                let indent = indent_of(s, first.name_start);
                let _ = write!(out, "{field}: {value},\n{indent}");
            } else {
                let _ = write!(out, "{field}: {value}, ");
            }
            out.push_str(&text[first.name_start..]);
        }
        None => {
            out.push_str(&text[..=open]);
            let _ = write!(out, "{field}: {value}");
            out.push_str(&text[open + 1..]);
        }
    }
    Ok(out)
}

/// Конец предмета, открытого в `open` (за закрывающей скобкой).
fn node_end(s: &[u8], open: usize) -> Result<usize, EditError> {
    group_end(s, open).ok_or_else(|| EditError("не закрыта скобка предмета".into()))
}

/// Начало строки, в которой стоит байт `at`.
fn line_start(s: &[u8], at: usize) -> usize {
    s[..at]
        .iter()
        .rposition(|&c| c == b'\n')
        .map_or(0, |p| p + 1)
}

/// Текст предмета `path` для буфера обмена: без отступа его строки, так что его можно
/// вставить на любой уровень.
pub(crate) fn node_text(text: &str, path: &[usize]) -> Result<String, EditError> {
    let s = text.as_bytes();
    let open = node_open(s, path)?;
    let end = node_end(s, open)?;
    let indent = indent_of(s, open);
    let mut out = String::new();
    for (i, line) in text[open..end].split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(line.strip_prefix(indent).unwrap_or(line));
        } else {
            out.push_str(line);
        }
    }
    Ok(out)
}

/// Убирает предмет `path` вместе с вложенными (и запятую за ним, и комментарий в конце
/// его строки).
pub(crate) fn remove_node(text: &str, path: &[usize]) -> Result<String, EditError> {
    let s = text.as_bytes();
    let open = node_open(s, path)?;
    let end = node_end(s, open)?;
    Ok(remove_span(text, open, end))
}

/// Вырезает кусок `open..end` (элемент списка или поле), запятую за ним и комментарий в
/// конце строки; если он занимал свои строки — строки целиком.
fn remove_span(text: &str, open: usize, end: usize) -> String {
    let s = text.as_bytes();
    let spaces = |mut i: usize| {
        while i < s.len() && matches!(s[i], b' ' | b'\t') {
            i += 1;
        }
        i
    };
    let mut j = spaces(end);
    if s.get(j) == Some(&b',') {
        j = spaces(j + 1);
    }
    let ls = line_start(s, open);
    let alone_before = s[ls..open].iter().all(|c| matches!(c, b' ' | b'\t'));
    if s[j..].starts_with(b"//") {
        while j < s.len() && s[j] != b'\n' {
            j += 1;
        }
    }
    let (from, to) = if alone_before && (j == s.len() || s[j] == b'\n') {
        // Предмет занимал свои строки: убираем их целиком.
        (ls, (j + 1).min(s.len()))
    } else {
        (open, j)
    };
    format!("{}{}", &text[..from], &text[to..])
}

/// Список `field` предмета `path` (или сцены): начала его элементов.
fn list_items(s: &[u8], path: &[usize], field: &str) -> Result<Vec<usize>, EditError> {
    let open = list_in(s, path, field)?
        .ok_or_else(|| EditError(format!("у предмета нет списка {field}")))?;
    items(s, open).ok_or_else(|| EditError("не разобрать список".into()))
}

/// Убирает элемент `index` списка `field` предмета `path` (фигуру, свет, часть).
pub(crate) fn remove_item(
    text: &str,
    path: &[usize],
    field: &str,
    index: usize,
) -> Result<String, EditError> {
    let s = text.as_bytes();
    let it = list_items(s, path, field)?;
    let at = *it
        .get(index)
        .ok_or_else(|| EditError(format!("в списке {field} нет №{}", index + 1)))?;
    let end = value_end(s, at).ok_or_else(|| EditError("не разобрать элемент".into()))?;
    Ok(remove_span(text, at, end))
}

/// Убирает поле `field` предмета `path` (например, `body` или `touch`).
pub(crate) fn remove_field(text: &str, path: &[usize], field: &str) -> Result<String, EditError> {
    let s = text.as_bytes();
    let open = owner_open(s, path)?;
    let f = fields(s, open).ok_or_else(|| EditError("не разобрать поля".into()))?;
    match f.iter().find(|f| f.name == field) {
        Some(found) => Ok(remove_span(text, found.name_start, found.value.end)),
        None => Ok(text.to_owned()),
    }
}

/// Ставит поле `sub` элементу `index` списка `field` (`Rect(…)`, `Point(…)`, `(…)`).
pub(crate) fn set_item_field(
    text: &str,
    path: &[usize],
    field: &str,
    index: usize,
    sub: &str,
    value: &str,
) -> Result<String, EditError> {
    let s = text.as_bytes();
    let it = list_items(s, path, field)?;
    let mut at = *it
        .get(index)
        .ok_or_else(|| EditError(format!("в списке {field} нет №{}", index + 1)))?;
    if s[at] != b'(' {
        // `Rect(` — имя, а за ним скобки.
        while at < s.len() && s[at] != b'(' {
            at += 1;
        }
    }
    set_field_at(text, at, sub, value)
}

/// Скобка `[` списка `name` у предмета `path` (или у сцены, если `path` пустой).
fn list_in(s: &[u8], path: &[usize], name: &str) -> Result<Option<usize>, EditError> {
    let open = owner_open(s, path)?;
    let f = fields(s, open).ok_or_else(|| EditError("не разобрать поля".into()))?;
    match f.iter().find(|f| f.name == name) {
        Some(list) if s.get(list.value.start) == Some(&b'[') => Ok(Some(list.value.start)),
        Some(_) => Err(EditError(
            "список предметов должен быть в скобках [ ]".into(),
        )),
        None => Ok(None),
    }
}

/// Вставляет предмет (текст в скобках, как из [`node_text`]) в список `parent` на место
/// `index` (дальше конца — в конец). У предмета без вложенных появляется `children`.
pub(crate) fn insert_node(
    text: &str,
    parent: &[usize],
    index: usize,
    item: &str,
) -> Result<String, EditError> {
    let field = if parent.is_empty() {
        "nodes"
    } else {
        "children"
    };
    insert_item(text, parent, field, index, item)
}

/// Вставляет элемент в список `field` предмета `path` (или сцены) на место `index`;
/// списка нет — он появляется.
pub(crate) fn insert_item(
    text: &str,
    path: &[usize],
    field: &str,
    index: usize,
    item: &str,
) -> Result<String, EditError> {
    let s = text.as_bytes();
    let Some(open) = list_in(s, path, field)? else {
        let owner = owner_open(s, path)?;
        let t = set_field_at(text, owner, field, "[]")?;
        return insert_item(&t, path, field, index, item);
    };
    let close = group_end(s, open).ok_or_else(|| EditError("не закрыт список".into()))? - 1;
    let it = items(s, open).ok_or_else(|| EditError("не разобрать список предметов".into()))?;
    let base = indent_of(s, open).to_owned();
    let multiline = it.is_empty() || s[open..close].contains(&b'\n');
    let indent = match it.first() {
        Some(&first) if multiline => indent_of(s, first).to_owned(),
        _ => format!("{base}    "),
    };
    let item = item.replace('\n', &format!("\n{indent}"));
    let mut out = String::with_capacity(text.len() + item.len() + 16);
    if let Some(&at) = it.get(index) {
        out.push_str(&text[..at]);
        if multiline {
            let _ = write!(out, "{item},\n{indent}");
        } else {
            let _ = write!(out, "{item}, ");
        }
        out.push_str(&text[at..]);
    } else if let Some(&last) = it.last() {
        let end = value_end(s, last).ok_or_else(|| EditError("не разобрать предмет".into()))?;
        let mut j = end;
        while j < close && matches!(s[j], b' ' | b'\t') {
            j += 1;
        }
        let comma = s.get(j) == Some(&b',');
        if multiline {
            // За концом строки последнего предмета (после его комментария, если есть).
            let mut at = if comma { j + 1 } else { end };
            let nl = s[at..close]
                .iter()
                .position(|&c| c == b'\n')
                .map(|p| at + p);
            out.push_str(&text[..end]);
            if !comma {
                out.push(',');
            }
            if let Some(nl) = nl {
                out.push_str(&text[end..nl]);
                at = nl;
            }
            let _ = write!(out, "\n{indent}{item},");
            out.push_str(&text[at..]);
        } else {
            let at = if comma { j + 1 } else { end };
            out.push_str(&text[..at]);
            out.push_str(if comma { " " } else { ", " });
            out.push_str(&item);
            out.push_str(&text[at..]);
        }
    } else {
        let inner = &text[open + 1..close];
        out.push_str(&text[..=open]);
        if inner.trim().is_empty() {
            let _ = write!(out, "\n{indent}{item},\n{base}");
            out.push_str(&text[close..]);
        } else {
            // В пустом списке только комментарии: предмет — перед закрывающей скобкой.
            out.push_str(inner.trim_end());
            let _ = write!(out, "\n{indent}{item},\n{base}");
            out.push_str(&text[close..]);
        }
    }
    Ok(out)
}

/// Число для файла: не больше 4 знаков после запятой, всегда с точкой (`1.0`, `-0.25`).
pub(crate) fn num(x: f64) -> String {
    let x = (x * 10_000.0).round() / 10_000.0;
    let x = if x == 0.0 { 0.0 } else { x };
    let mut t = format!("{x:.4}");
    while t.ends_with('0') {
        t.pop();
    }
    if t.ends_with('.') {
        t.push('0');
    }
    t
}

/// Пара чисел для файла: `(x, y)`.
pub(crate) fn pair(p: [f64; 2]) -> String {
    format!("({}, {})", num(p[0]), num(p[1]))
}

/// Строка для файла в кавычках.
pub(crate) fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_scene::SceneFile;

    const SMALL: &str = r##"// шапка
(
    format: "reyvec-scene",
    version: 1,
    nodes: [
        // стол
        (
            name: "стол", // имя
            at: (0.5, -0.2),
            shapes: [Rect(at: (0, 0), size: (1, 0.1), color: "#884422")],
            children: [
                (name: "банка", at: (0.1, 0.1)),
                (shapes: [Circle(at: (0, 0), r: 0.05, color: "#ff0000")]),
            ],
        ),
        (name: "пусто"),
    ],
)
"##;

    #[test]
    fn replaces_value_and_keeps_comments() {
        let t = set_field(SMALL, &[0], "at", &pair([1.25, -0.5])).unwrap();
        assert!(t.contains("at: (1.25, -0.5),"));
        assert!(t.contains("// стол") && t.contains("// имя") && t.starts_with("// шапка"));
        let f = SceneFile::parse(&t).unwrap();
        assert_eq!(f.nodes[0].at, (1.25, -0.5));
        // Остальное не тронуто: разница только в этой строке.
        assert_eq!(t.lines().count(), SMALL.lines().count());
    }

    #[test]
    fn child_and_missing_fields() {
        let t = set_field(SMALL, &[0, 1], "angle", &num(30.0)).unwrap();
        let t = set_field(&t, &[0, 0], "scale", &num(2.0)).unwrap();
        let t = set_field(&t, &[1], "depth", &num(-1.5)).unwrap();
        let f = SceneFile::parse(&t).unwrap();
        assert_eq!(f.nodes[0].children[1].angle, 30.0);
        assert_eq!(f.nodes[0].children[0].scale, 2.0);
        assert_eq!(f.nodes[1].depth, -1.5);
        // В многострочный предмет поле встаёт отдельной строкой с тем же отступом.
        let t = set_field(SMALL, &[0], "angle", "15.0").unwrap();
        assert!(t.contains("            angle: 15.0,\n            name: \"стол\""));
    }

    #[test]
    fn names_with_quotes() {
        let t = set_field(SMALL, &[1], "name", &string("ящик \"А\"")).unwrap();
        let f = SceneFile::parse(&t).unwrap();
        assert_eq!(f.nodes[1].name, "ящик \"А\"");
    }

    #[test]
    fn errors_name_the_problem() {
        assert!(set_field(SMALL, &[5], "at", "(0.0, 0.0)").is_err());
        assert!(set_field(SMALL, &[0, 9], "at", "(0.0, 0.0)").is_err());
        assert!(set_field(SMALL, &[], "at", "(0.0, 0.0)").is_err());
        assert!(set_field("нет скобки", &[0], "at", "(0.0, 0.0)").is_err());
    }

    #[test]
    fn numbers() {
        assert_eq!(num(1.0), "1.0");
        assert_eq!(num(-0.25), "-0.25");
        assert_eq!(num(0.123456), "0.1235");
        assert_eq!(num(-0.00001), "0.0");
        assert_eq!(pair([0.5, 2.0]), "(0.5, 2.0)");
    }

    #[test]
    fn real_scene_every_node_is_found() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenes/first-room.ron"),
        )
        .unwrap();
        let file = SceneFile::parse(&text).unwrap();
        fn walk(text: &str, nodes: &[reyvec_scene::NodeDesc], path: &mut Vec<usize>) {
            for (i, n) in nodes.iter().enumerate() {
                path.push(i);
                let moved = [n.at.0 + 0.5, n.at.1 - 0.25];
                let t = set_field(text, path, "at", &pair(moved)).unwrap();
                let f = SceneFile::parse(&t).unwrap();
                let got = crate::editor::outline::node_at(&f, path).unwrap();
                assert!((got.at.0 - moved[0]).abs() < 1e-4 && (got.at.1 - moved[1]).abs() < 1e-4);
                walk(text, &n.children, path);
                path.pop();
            }
        }
        walk(&text, &file.nodes, &mut Vec::new());
    }

    #[test]
    fn remove_keeps_neighbours_and_comments() {
        let t = remove_node(SMALL, &[0, 0]).unwrap();
        let f = SceneFile::parse(&t).unwrap();
        assert_eq!(f.nodes[0].children.len(), 1);
        assert!(!t.contains("банка") && t.contains("// имя"));
        assert_eq!(t.lines().count(), SMALL.lines().count() - 1);
        let t = remove_node(SMALL, &[0]).unwrap();
        let f = SceneFile::parse(&t).unwrap();
        assert_eq!(f.nodes.len(), 1);
        assert_eq!(f.nodes[0].name, "пусто");
        let t = remove_node(&t, &[0]).unwrap();
        assert!(SceneFile::parse(&t).unwrap().nodes.is_empty());
        // В одну строку.
        let line = r#"(format: "reyvec-scene", version: 1, nodes: [(name: "а"), (name: "б")])"#;
        let t = remove_node(line, &[0]).unwrap();
        assert_eq!(
            t,
            r#"(format: "reyvec-scene", version: 1, nodes: [(name: "б")])"#
        );
    }

    #[test]
    fn copy_and_insert_anywhere() {
        let table = node_text(SMALL, &[0]).unwrap();
        assert!(table.starts_with('(') && table.ends_with(')'));
        assert!(table.contains("\n    name: \"стол\""), "{table}");
        // В конец, в начало и внутрь предмета без вложенных.
        let t = insert_node(SMALL, &[], 99, &table).unwrap();
        let t = insert_node(&t, &[], 0, "(name: \"первый\")").unwrap();
        let t = insert_node(&t, &[2], 0, "(name: \"внутри\")").unwrap();
        let jar = node_text(&t, &[1, 0]).unwrap();
        let t = insert_node(&t, &[1], 1, &jar).unwrap();
        let f = SceneFile::parse(&t).unwrap();
        let names: Vec<_> = f.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["первый", "стол", "пусто", "стол"], "{t}");
        assert_eq!(f.nodes[2].children[0].name, "внутри");
        assert_eq!(f.nodes[1].children.len(), 3);
        assert_eq!(f.nodes[1].children[1].name, "банка");
        assert_eq!(f.nodes[3].children.len(), 2);
        // Вставленное выровнено, как соседи.
        assert!(t.contains("\n        (\n            name: \"стол\""), "{t}");
        let line = r#"(format: "reyvec-scene", version: 1, nodes: [(name: "а")])"#;
        let t = insert_node(line, &[], 5, "(name: \"б\")").unwrap();
        assert_eq!(
            t,
            r#"(format: "reyvec-scene", version: 1, nodes: [(name: "а"), (name: "б")])"#
        );
        let t = insert_node(SMALL, &[], 99, "(name: \"в\")").unwrap();
        assert!(
            t.contains("(name: \"пусто\"),\n        (name: \"в\"),\n    ],"),
            "{t}"
        );
    }

    #[test]
    fn real_scene_survives_cut_and_paste() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenes/first-room.ron"),
        )
        .unwrap();
        let file = SceneFile::parse(&text).unwrap();
        for i in 0..file.nodes.len() {
            let item = node_text(&text, &[i]).unwrap();
            let cut = remove_node(&text, &[i]).unwrap();
            let f = SceneFile::parse(&cut).unwrap();
            assert_eq!(f.nodes.len(), file.nodes.len() - 1);
            let back = insert_node(&cut, &[], i, &item).unwrap();
            let f = SceneFile::parse(&back).unwrap();
            assert_eq!(f.nodes.len(), file.nodes.len());
            assert_eq!(f.nodes[i].name, file.nodes[i].name);
            assert_eq!(f.nodes[i].children.len(), file.nodes[i].children.len());
        }
    }

    #[test]
    fn parts_and_scene_fields() {
        let t = insert_item(
            SMALL,
            &[1],
            "shapes",
            0,
            "Circle(at: (0.0, 0.0), r: 0.1, color: \"#ffffff\")",
        )
        .unwrap();
        let t = insert_item(
            &t,
            &[0],
            "lights",
            9,
            "Point(at: (0.0, 0.1), color: \"#ffcc88\", reach: 0.5)",
        )
        .unwrap();
        let t = set_item_field(&t, &[0], "shapes", 0, "color", &string("#123456")).unwrap();
        let t = set_item_field(&t, &[1], "shapes", 0, "r", &num(0.25)).unwrap();
        let t = set_field(&t, &[1], "body", "(size: (0.1, 0.1))").unwrap();
        let t = set_root_field(&t, "ambient", &string("#101010")).unwrap();
        let f = SceneFile::parse(&t).unwrap();
        assert_eq!(f.nodes[1].shapes.len(), 1);
        assert_eq!(f.nodes[0].lights.len(), 1);
        assert!(f.nodes[1].body.is_some());
        assert!(t.contains("color: \"#123456\"") && t.contains("r: 0.25"));
        assert!(t.contains("ambient: \"#101010\""));
        let t = remove_item(&t, &[1], "shapes", 0).unwrap();
        let t = remove_field(&t, &[1], "body").unwrap();
        let t = remove_item(&t, &[0], "lights", 0).unwrap();
        let f = SceneFile::parse(&t).unwrap();
        assert!(f.nodes[1].shapes.is_empty() && f.nodes[1].body.is_none());
        assert!(f.nodes[0].lights.is_empty());
        assert!(remove_item(&t, &[0], "shapes", 5).is_err());
    }
}
