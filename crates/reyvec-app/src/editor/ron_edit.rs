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
    let s = text.as_bytes();
    let open = node_open(s, path)?;
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
}
