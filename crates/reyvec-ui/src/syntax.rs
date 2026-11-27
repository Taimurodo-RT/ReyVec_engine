//! Разбор текста `.rui` в дерево блоков (вид как у SwiftUI, решение 0022).
//!
//! ```text
//! screen Demo {
//!     Column(spacing: 16, align: center) {
//!         Text("Привет").font(32)
//!         Rect().size(200, 40).background(#F5A54A).radius(8)
//!     }
//!     .align(center)
//! }
//! ```
//!
//! Блок — имя с большой буквы, в скобках главные значения (по порядку или `имя: значение`),
//! в фигурных скобках вложенные блоки, дальше модификаторы `.имя(...)` через точку, каждый
//! можно с новой строки. Модификатор может нести свой блок модификаторов:
//! `.hover { .background(#fff) }` (состояния, шаг И.2). Комментарии — `//` до конца строки.
//!
//! Здесь только разбор: что значат имена и значения, решает [`crate::model`].

use crate::UiError;

/// Место в файле: строка и столбец (с единицы), для понятных ошибок.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub column: u32,
    /// Какой файл: 0 — сам экран, дальше — подключённые в `head` по порядку чтения.
    pub file: u16,
}

impl std::fmt::Display for Pos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "строка {}, столбец {}", self.line, self.column)
    }
}

/// Значение в скобках.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Число: пиксели экрана 1920×1080, доли, секунды — как поймёт модификатор.
    Number(f32),
    /// Число со знаком `%`: доля от размера родителя.
    Percent(f32),
    /// Строка в кавычках.
    Text(String),
    /// Цвет `#RRGGBB` или `#RRGGBBAA` в sRGB, как в редакторах.
    Color([u8; 4]),
    /// Имя или путь через точку: `center`, `theme.accent`, `game.hero.stamina`.
    Name(String),
    /// Вызов: `tr("menu.title")`, `image("...")`, `var(accent)`.
    Call(String, Vec<Arg>),
    /// Формула: `var(gap) * 2`, `100% - 32`. Знак: `+`, `-`, `*`, `/`; сравнения `<`,
    /// `>`, `≤`, `≥`, `=` (`==`) и `≠` (`!=`).
    Op(Box<Value>, char, Box<Value>),
    /// Выбор: `game.hero.hp < 2 ? var(danger) : var(accent)`.
    If(Box<Value>, Box<Value>, Box<Value>),
    /// Связь в обе стороны: `$settings.audio.volume`. Ползунок показывает значение по
    /// пути и сам пишет туда новое.
    Bind(String),
}

impl Value {
    /// Как значение называется в ошибках.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Number(_) => "число",
            Value::Percent(_) => "проценты",
            Value::Text(_) => "строка",
            Value::Color(_) => "цвет",
            Value::Name(_) => "имя",
            Value::Call(..) => "вызов",
            Value::Op(..) => "формула",
            Value::If(..) => "выбор ? :",
            Value::Bind(_) => "связь $",
        }
    }
}

/// Значение в скобках с необязательным именем: `16` или `spacing: 16`.
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Value,
    pub pos: Pos,
}

/// Модификатор `.имя(аргументы) { модификаторы }`.
#[derive(Clone, Debug, PartialEq)]
pub struct Modifier {
    pub name: String,
    pub args: Vec<Arg>,
    /// Модификаторы в фигурных скобках: для состояний (`.hover { ... }`).
    pub inner: Option<Vec<Modifier>>,
    pub pos: Pos,
}

/// Блок: `Имя(аргументы) { блоки }` и модификаторы.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub kind: String,
    pub args: Vec<Arg>,
    /// Вложенные блоки; `None` — фигурных скобок не было.
    pub children: Option<Vec<Node>>,
    pub modifiers: Vec<Modifier>,
    pub pos: Pos,
}

/// Чем объявлен экран: `screen` (экран со стопкой «Назад») или `layer` (слой поверх игры).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenKind {
    Screen,
    Layer,
}

/// Объявление верхнего уровня: `screen Имя { блоки }` и модификаторы экрана.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenDecl {
    pub kind: ScreenKind,
    pub name: String,
    pub children: Vec<Node>,
    pub modifiers: Vec<Modifier>,
    pub pos: Pos,
}

/// Что подключает строка блока `head`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncludeKind {
    /// `theme("themes/forge")`: файл с темами (наборами переменных).
    Theme,
    /// `styles("styles/menu")`: файл со стилями.
    Styles,
}

/// Строка блока `head`: что и откуда подключить. Путь — от папки `ui/`, без `.rui`.
#[derive(Clone, Debug, PartialEq)]
pub struct Include {
    pub kind: IncludeKind,
    pub path: String,
    pub pos: Pos,
}

/// Звено селектора стиля: вид блока или экран (с большой буквы) и классы (с маленькой).
/// `Button`, `danger`, `Button.big.red`.
#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    pub kind: Option<String>,
    pub classes: Vec<String>,
    pub pos: Pos,
}

impl std::fmt::Display for Selector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        if let Some(k) = &self.kind {
            f.write_str(k)?;
            first = false;
        }
        for c in &self.classes {
            if !first {
                f.write_str(".")?;
            }
            f.write_str(c)?;
            first = false;
        }
        Ok(())
    }
}

/// Стиль: `style Селектор { модификаторы и вложенные стили }`.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleRule {
    pub selector: Selector,
    pub modifiers: Vec<Modifier>,
    /// Вложенные стили: действуют на блоки внутри блока этого селектора.
    pub nested: Vec<StyleRule>,
    pub pos: Pos,
}

/// Тема: `theme Имя { имя = значение ... }` — набор переменных.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeDecl {
    pub name: String,
    pub vars: Vec<(String, Value, Pos)>,
    pub pos: Pos,
}

/// Разобранный файл.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    /// Подключения из блока `head` по порядку.
    pub head: Vec<Include>,
    pub screens: Vec<ScreenDecl>,
    pub themes: Vec<ThemeDecl>,
    pub styles: Vec<StyleRule>,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Number(f32),
    Percent(f32),
    Text(String),
    Color([u8; 4]),
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,
    Eq,
    Plus,
    Minus,
    Star,
    Slash,
    /// Сравнение для `.when`: `<`, `>`, `≤` (`<=`), `≥` (`>=`).
    Cmp(char),
    /// `?` в выборе `a ? b : c`.
    Question,
    /// `$` перед путём: связь в обе стороны.
    Dollar,
    /// Строка со вставками `"Монеты: \(game.hero.coins)"`: куски и значения.
    Interp(Vec<Arg>),
    Eof,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Ident(s) => format!("«{s}»"),
            Tok::Number(n) => format!("число {n}"),
            Tok::Percent(n) => format!("{n}%"),
            Tok::Text(_) => "строка".into(),
            Tok::Color(_) => "цвет".into(),
            Tok::LParen => "«(»".into(),
            Tok::RParen => "«)»".into(),
            Tok::LBrace => "«{»".into(),
            Tok::RBrace => "«}»".into(),
            Tok::Comma => "запятая".into(),
            Tok::Colon => "двоеточие".into(),
            Tok::Dot => "точка".into(),
            Tok::Eq => "«=»".into(),
            Tok::Plus => "«+»".into(),
            Tok::Minus => "«-»".into(),
            Tok::Star => "«*»".into(),
            Tok::Slash => "«/»".into(),
            Tok::Cmp(c) => format!("«{c}»"),
            Tok::Question => "«?»".into(),
            Tok::Dollar => "«$»".into(),
            Tok::Interp(_) => "строка".into(),
            Tok::Eof => "конец файла".into(),
        }
    }
}

/// Значение вставки `\(…)` в строке; места — от начала вставки в файле.
fn inserted(src: &str, at: Pos, file: u16) -> Result<Value, UiError> {
    let shift = |p: Pos| Pos {
        line: at.line + p.line - 1,
        column: if p.line == 1 {
            at.column + 1 + p.column
        } else {
            p.column
        },
        file,
    };
    let toks = lex(src, file)
        .map_err(|e| match e {
            UiError::Syntax { pos, message } => UiError::Syntax {
                pos: shift(pos),
                message,
            },
            other => other,
        })?
        .into_iter()
        .map(|(t, p)| (t, shift(p)))
        .collect();
    let mut p = Parser { toks, at: 0 };
    if *p.peek() == Tok::Eof {
        return Err(UiError::Syntax {
            pos: at,
            message: "пустая вставка \\(): внутри пишется значение, \\(game.hero.coins)".into(),
        });
    }
    let v = p.value()?;
    match p.next() {
        (Tok::Eof, _) => Ok(v),
        (t, pos) => Err(UiError::Syntax {
            pos,
            message: format!("во вставке \\(…) одно значение, а дальше {}", t.describe()),
        }),
    }
}

fn lex(src: &str, file: u16) -> Result<Vec<(Tok, Pos)>, UiError> {
    let mut out = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let (mut i, mut line, mut col) = (0usize, 1u32, 1u32);
    let err = |pos: Pos, message: String| UiError::Syntax { pos, message };
    while i < chars.len() {
        let c = chars[i];
        let pos = Pos {
            line,
            column: col,
            file,
        };
        // Минус перед числом — знак числа, если перед ним не стоит значение: `-4`, но
        // `100% - 4` и `var(a)-4` — вычитание.
        let after_value = matches!(
            out.last(),
            Some((
                Tok::Number(_) | Tok::Percent(_) | Tok::Ident(_) | Tok::RParen | Tok::Color(_),
                _
            ))
        );
        let step = |n: usize, i: &mut usize, col: &mut u32| {
            *i += n;
            *col += n as u32;
        };
        match c {
            '\n' => {
                i += 1;
                line += 1;
                col = 1;
            }
            c if c.is_whitespace() => step(1, &mut i, &mut col),
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '=' | '!' if chars.get(i + 1) == Some(&'=') => {
                out.push((Tok::Cmp(if c == '=' { '=' } else { '≠' }), pos));
                step(2, &mut i, &mut col);
            }
            '?' => {
                out.push((Tok::Question, pos));
                step(1, &mut i, &mut col);
            }
            '$' => {
                out.push((Tok::Dollar, pos));
                step(1, &mut i, &mut col);
            }
            '(' | ')' | '{' | '}' | ',' | ':' | '=' | '+' | '*' | '/' => {
                out.push((
                    match c {
                        '(' => Tok::LParen,
                        ')' => Tok::RParen,
                        '{' => Tok::LBrace,
                        '}' => Tok::RBrace,
                        ',' => Tok::Comma,
                        '=' => Tok::Eq,
                        '+' => Tok::Plus,
                        '*' => Tok::Star,
                        '/' => Tok::Slash,
                        _ => Tok::Colon,
                    },
                    pos,
                ));
                step(1, &mut i, &mut col);
            }
            '.' if !chars.get(i + 1).is_some_and(char::is_ascii_digit) => {
                out.push((Tok::Dot, pos));
                step(1, &mut i, &mut col);
            }
            '"' => {
                let mut text = String::new();
                // Вставки `\(…)`: куски строки и значения по порядку.
                let mut parts: Vec<Arg> = Vec::new();
                i += 1;
                col += 1;
                loop {
                    match chars.get(i) {
                        None | Some('\n') => {
                            return Err(err(pos, "строка не закрыта кавычкой \"".into()));
                        }
                        Some('"') => {
                            i += 1;
                            col += 1;
                            break;
                        }
                        Some('\\') if chars.get(i + 1) == Some(&'(') => {
                            let at = Pos {
                                line,
                                column: col,
                                file,
                            };
                            let start = i + 2;
                            let mut depth = 1;
                            let mut end = start;
                            while end < chars.len() && chars[end] != '\n' {
                                match chars[end] {
                                    '(' => depth += 1,
                                    ')' => {
                                        depth -= 1;
                                        if depth == 0 {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                                end += 1;
                            }
                            if depth != 0 {
                                return Err(err(at, "вставка \\( в строке не закрыта «)»".into()));
                            }
                            let inner: String = chars[start..end].iter().collect();
                            let value = inserted(&inner, at, file)?;
                            if !text.is_empty() {
                                parts.push(Arg {
                                    name: None,
                                    value: Value::Text(std::mem::take(&mut text)),
                                    pos: at,
                                });
                            }
                            parts.push(Arg {
                                name: None,
                                value,
                                pos: at,
                            });
                            let n = end + 1 - i;
                            i += n;
                            col += n as u32;
                        }
                        Some('\\') => {
                            let e = chars.get(i + 1).copied();
                            text.push(match e {
                                Some('n') => '\n',
                                Some('"') => '"',
                                Some('\\') => '\\',
                                _ => {
                                    return Err(err(
                                        Pos {
                                            line,
                                            column: col,
                                            file,
                                        },
                                        "в строке после \\ бывает только n, \", \\ или вставка \\(…)".into(),
                                    ));
                                }
                            });
                            i += 2;
                            col += 2;
                        }
                        Some(&ch) => {
                            text.push(ch);
                            i += 1;
                            col += 1;
                        }
                    }
                }
                if parts.is_empty() {
                    out.push((Tok::Text(text), pos));
                } else {
                    if !text.is_empty() {
                        parts.push(Arg {
                            name: None,
                            value: Value::Text(text),
                            pos,
                        });
                    }
                    out.push((Tok::Interp(parts), pos));
                }
            }
            '#' => {
                let start = i + 1;
                let mut end = start;
                while end < chars.len() && chars[end].is_ascii_hexdigit() {
                    end += 1;
                }
                let hex: String = chars[start..end].iter().collect();
                let color = parse_hex(&hex).ok_or_else(|| {
                    err(
                        pos,
                        format!("цвет пишется #RRGGBB или #RRGGBBAA, а здесь «#{hex}»"),
                    )
                })?;
                out.push((Tok::Color(color), pos));
                let n = end - i;
                step(n, &mut i, &mut col);
            }
            '<' | '>' => {
                let eq = chars.get(i + 1) == Some(&'=');
                let op = match (c, eq) {
                    ('<', false) => '<',
                    ('>', false) => '>',
                    ('<', true) => '≤',
                    _ => '≥',
                };
                out.push((Tok::Cmp(op), pos));
                step(1 + usize::from(eq), &mut i, &mut col);
            }
            '-' if after_value
                || !chars
                    .get(i + 1)
                    .is_some_and(|n| n.is_ascii_digit() || *n == '.') =>
            {
                out.push((Tok::Minus, pos));
                step(1, &mut i, &mut col);
            }
            c if c.is_ascii_digit() || c == '-' || c == '.' => {
                let start = i;
                let mut end = i + 1;
                while end < chars.len() && (chars[end].is_ascii_digit() || chars[end] == '.') {
                    end += 1;
                }
                let text: String = chars[start..end].iter().collect();
                let value: f32 = text
                    .parse()
                    .map_err(|_| err(pos, format!("«{text}» — не число")))?;
                let percent = chars.get(end) == Some(&'%');
                out.push((
                    if percent {
                        Tok::Percent(value)
                    } else {
                        Tok::Number(value)
                    },
                    pos,
                ));
                let n = end - i + usize::from(percent);
                step(n, &mut i, &mut col);
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                let mut end = i;
                while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                    end += 1;
                }
                out.push((Tok::Ident(chars[start..end].iter().collect()), pos));
                let n = end - i;
                step(n, &mut i, &mut col);
            }
            other => return Err(err(pos, format!("непонятный знак «{other}»"))),
        }
    }
    out.push((
        Tok::Eof,
        Pos {
            line,
            column: col,
            file,
        },
    ));
    Ok(out)
}

fn parse_hex(hex: &str) -> Option<[u8; 4]> {
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let byte = |k: usize| u8::from_str_radix(&hex[k..k + 2], 16).ok();
    Some([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if hex.len() == 8 { byte(6)? } else { 255 },
    ])
}

struct Parser {
    toks: Vec<(Tok, Pos)>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.at].0
    }

    fn pos(&self) -> Pos {
        self.toks[self.at].1
    }

    fn next(&mut self) -> (Tok, Pos) {
        let t = self.toks[self.at].clone();
        if self.at + 1 < self.toks.len() {
            self.at += 1;
        }
        t
    }

    fn error(&self, message: String) -> UiError {
        UiError::Syntax {
            pos: self.pos(),
            message,
        }
    }

    fn expect(&mut self, tok: Tok, what: &str) -> Result<Pos, UiError> {
        if *self.peek() == tok {
            Ok(self.next().1)
        } else {
            Err(self.error(format!("ждал {what}, а здесь {}", self.peek().describe())))
        }
    }

    fn ident(&mut self, what: &str) -> Result<(String, Pos), UiError> {
        match self.next() {
            (Tok::Ident(s), pos) => Ok((s, pos)),
            (t, pos) => Err(UiError::Syntax {
                pos,
                message: format!("ждал {what}, а здесь {}", t.describe()),
            }),
        }
    }

    fn document(&mut self) -> Result<Document, UiError> {
        let mut doc = Document::default();
        loop {
            match self.peek().clone() {
                Tok::Eof => return Ok(doc),
                Tok::Ident(word) if word == "screen" || word == "layer" => {
                    let pos = self.next().1;
                    let kind = if word == "screen" {
                        ScreenKind::Screen
                    } else {
                        ScreenKind::Layer
                    };
                    let (name, _) = self.ident("имя экрана после «screen»")?;
                    let children = self.children()?;
                    let modifiers = self.modifiers()?;
                    if doc.screens.iter().any(|s| s.name == name) {
                        return Err(UiError::Syntax {
                            pos,
                            message: format!("экран «{name}» уже есть в этом файле"),
                        });
                    }
                    doc.screens.push(ScreenDecl {
                        kind,
                        name,
                        children,
                        modifiers,
                        pos,
                    });
                }
                Tok::Ident(word) if word == "head" => {
                    let pos = self.next().1;
                    if !doc.head.is_empty()
                        || !doc.screens.is_empty()
                        || !doc.themes.is_empty()
                        || !doc.styles.is_empty()
                    {
                        return Err(UiError::Syntax {
                            pos,
                            message: "head пишется один раз, в самом начале файла".into(),
                        });
                    }
                    doc.head = self.head()?;
                }
                Tok::Ident(word) if word == "theme" => {
                    let pos = self.next().1;
                    let (name, _) = self.ident("имя темы после «theme»")?;
                    if doc.themes.iter().any(|t| t.name == name) {
                        return Err(UiError::Syntax {
                            pos,
                            message: format!("тема «{name}» уже есть в этом файле"),
                        });
                    }
                    let vars = self.theme_vars()?;
                    doc.themes.push(ThemeDecl { name, vars, pos });
                }
                Tok::Ident(word) if word == "style" => {
                    self.next();
                    let rule = self.style_rule()?;
                    doc.styles.push(rule);
                }
                other => {
                    return Err(self.error(format!(
                        "файл состоит из head {{ ... }}, «screen Имя {{ ... }}», «layer Имя {{ ... }}», «theme Имя {{ ... }}» и «style Имя {{ ... }}», а здесь {}",
                        other.describe()
                    )));
                }
            }
        }
    }

    /// `{ theme("путь") styles("путь") }`
    fn head(&mut self) -> Result<Vec<Include>, UiError> {
        self.expect(Tok::LBrace, "«{» после head")?;
        let mut out = Vec::new();
        loop {
            match self.next() {
                (Tok::RBrace, _) => return Ok(out),
                (Tok::Ident(word), pos) => {
                    let kind = match word.as_str() {
                        "theme" => IncludeKind::Theme,
                        "styles" => IncludeKind::Styles,
                        "import" => {
                            return Err(UiError::Meaning {
                                pos,
                                message: "import (свои блоки из других файлов) появится вместе с компонентами, позже".into(),
                            });
                        }
                        other => {
                            return Err(UiError::Syntax {
                                pos,
                                message: format!(
                                    "в head бывают theme(\"путь\") и styles(\"путь\"), а здесь «{other}»"
                                ),
                            });
                        }
                    };
                    let args = self.args()?;
                    let path = match &args[..] {
                        [
                            Arg {
                                name: None,
                                value: Value::Text(t),
                                ..
                            },
                        ] => t.clone(),
                        _ => {
                            return Err(UiError::Syntax {
                                pos,
                                message: format!(
                                    "{word}: один путь в кавычках от папки ui/, например {word}(\"styles/menu\")"
                                ),
                            });
                        }
                    };
                    out.push(Include { kind, path, pos });
                }
                (Tok::Eof, pos) => {
                    return Err(UiError::Syntax {
                        pos,
                        message: "не хватает «}»: head не закрыт".into(),
                    });
                }
                (t, pos) => {
                    return Err(UiError::Syntax {
                        pos,
                        message: format!(
                            "в head бывают theme(\"путь\") и styles(\"путь\"), а здесь {}",
                            t.describe()
                        ),
                    });
                }
            }
        }
    }

    /// `{ имя = значение ... }`
    fn theme_vars(&mut self) -> Result<Vec<(String, Value, Pos)>, UiError> {
        self.expect(Tok::LBrace, "«{» после имени темы")?;
        let mut vars: Vec<(String, Value, Pos)> = Vec::new();
        loop {
            match self.next() {
                (Tok::RBrace, _) => return Ok(vars),
                (Tok::Ident(name), pos) => {
                    self.expect(Tok::Eq, &format!("«=» после «{name}»: {name} = значение"))?;
                    let value = self.value()?;
                    if vars.iter().any(|(n, ..)| *n == name) {
                        return Err(UiError::Syntax {
                            pos,
                            message: format!("переменная «{name}» уже есть в этой теме"),
                        });
                    }
                    vars.push((name, value, pos));
                }
                (Tok::Eof, pos) => {
                    return Err(UiError::Syntax {
                        pos,
                        message: "не хватает «}»: тема не закрыта".into(),
                    });
                }
                (t, pos) => {
                    return Err(UiError::Syntax {
                        pos,
                        message: format!(
                            "в теме пишутся переменные «имя = значение», а здесь {}",
                            t.describe()
                        ),
                    });
                }
            }
        }
    }

    /// `Button.big`, `danger`, `MainMenu`.
    fn selector(&mut self) -> Result<Selector, UiError> {
        let (first, pos) = self.ident("имя вида блока (Button) или класса (danger)")?;
        let mut sel = Selector {
            kind: None,
            classes: Vec::new(),
            pos,
        };
        if first.chars().next().is_some_and(char::is_uppercase) {
            sel.kind = Some(first);
        } else {
            sel.classes.push(first);
        }
        while *self.peek() == Tok::Dot {
            self.next();
            let (class, pos) = self.ident("имя класса после точки")?;
            if class.chars().next().is_some_and(char::is_uppercase) {
                return Err(UiError::Syntax {
                    pos,
                    message: format!(
                        "после точки в стиле — класс с маленькой буквы, а «{class}» с большой: вид блока пишется первым, Button.big"
                    ),
                });
            }
            sel.classes.push(class);
        }
        Ok(sel)
    }

    /// `Селектор { модификаторы и вложенные стили }` (слово `style` уже прочитано).
    fn style_rule(&mut self) -> Result<StyleRule, UiError> {
        let selector = self.selector()?;
        let pos = selector.pos;
        self.expect(Tok::LBrace, &format!("«{{» после «{selector}»"))?;
        let mut modifiers = Vec::new();
        let mut nested = Vec::new();
        loop {
            match self.peek() {
                Tok::RBrace => {
                    self.next();
                    return Ok(StyleRule {
                        selector,
                        modifiers,
                        nested,
                        pos,
                    });
                }
                Tok::Dot => modifiers.extend(self.modifiers()?),
                // `when(width < 1280) { ... }` — то же, что `.when(...) { ... }`.
                Tok::Ident(w)
                    if w == "when"
                        && matches!(self.toks.get(self.at + 1), Some((Tok::LParen, _))) =>
                {
                    let pos = self.next().1;
                    let args = self.args()?;
                    self.expect(Tok::LBrace, "«{» после when(...)")?;
                    let inner = self.modifiers()?;
                    self.expect(Tok::RBrace, "«}» после модификаторов when")?;
                    modifiers.push(Modifier {
                        name: "when".into(),
                        args,
                        inner: Some(inner),
                        pos,
                    });
                }
                Tok::Ident(_) => nested.push(self.style_rule()?),
                Tok::Eof => {
                    return Err(
                        self.error(format!("не хватает «}}»: стиль «{selector}» не закрыт"))
                    );
                }
                other => {
                    return Err(self.error(format!(
                        "в стиле пишутся модификаторы (.background(#262B36)) и вложенные стили (Button {{ ... }}), а здесь {}",
                        other.describe()
                    )));
                }
            }
        }
    }

    /// `{ блоки }`
    fn children(&mut self) -> Result<Vec<Node>, UiError> {
        self.expect(Tok::LBrace, "«{»")?;
        let mut nodes = Vec::new();
        loop {
            match self.peek() {
                Tok::RBrace => {
                    self.next();
                    return Ok(nodes);
                }
                Tok::Eof => {
                    return Err(self.error("не хватает «}»: блок не закрыт".into()));
                }
                Tok::Dot => {
                    return Err(self.error(
                        "модификатор «.» без блока: он пишется сразу после блока, например Text(\"…\").font(24)".into(),
                    ));
                }
                _ => nodes.push(self.node()?),
            }
        }
    }

    fn node(&mut self) -> Result<Node, UiError> {
        let (kind, pos) = self.ident("имя блока, например Column, Row, Text")?;
        let args = if *self.peek() == Tok::LParen {
            self.args()?
        } else {
            Vec::new()
        };
        let children = if *self.peek() == Tok::LBrace {
            Some(self.children()?)
        } else {
            None
        };
        let modifiers = self.modifiers()?;
        Ok(Node {
            kind,
            args,
            children,
            modifiers,
            pos,
        })
    }

    fn modifiers(&mut self) -> Result<Vec<Modifier>, UiError> {
        let mut mods = Vec::new();
        while *self.peek() == Tok::Dot {
            let pos = self.next().1;
            let (name, _) = self.ident("имя модификатора после точки")?;
            let args = if *self.peek() == Tok::LParen {
                self.args()?
            } else {
                Vec::new()
            };
            let inner = if *self.peek() == Tok::LBrace {
                self.next();
                let inner = self.modifiers()?;
                self.expect(Tok::RBrace, "«}» после модификаторов состояния")?;
                Some(inner)
            } else {
                None
            };
            mods.push(Modifier {
                name,
                args,
                inner,
                pos,
            });
        }
        Ok(mods)
    }

    /// `(значение, имя: значение, ...)`
    fn args(&mut self) -> Result<Vec<Arg>, UiError> {
        self.expect(Tok::LParen, "«(»")?;
        let mut args = Vec::new();
        if *self.peek() == Tok::RParen {
            self.next();
            return Ok(args);
        }
        loop {
            let pos = self.pos();
            let name = match (
                self.peek().clone(),
                self.toks.get(self.at + 1).map(|t| &t.0),
            ) {
                (Tok::Ident(n), Some(Tok::Colon)) => {
                    self.next();
                    self.next();
                    Some(n)
                }
                _ => None,
            };
            let value = self.value()?;
            args.push(Arg { name, value, pos });
            match self.next() {
                (Tok::Comma, _) => {}
                (Tok::RParen, _) => return Ok(args),
                (t, pos) => {
                    return Err(UiError::Syntax {
                        pos,
                        message: format!(
                            "между значениями ставится запятая, а в конце «)», а здесь {}",
                            t.describe()
                        ),
                    });
                }
            }
        }
    }

    /// Значение с формулой и, для `.when`, сравнением: `width < 1280`.
    fn value(&mut self) -> Result<Value, UiError> {
        let mut left = self.sum()?;
        if let Tok::Cmp(op) = *self.peek() {
            self.next();
            let right = self.sum()?;
            left = Value::Op(Box::new(left), op, Box::new(right));
        }
        if *self.peek() == Tok::Question {
            self.next();
            let yes = self.value()?;
            self.expect(Tok::Colon, "«:» между вариантами выбора, a ? b : c")?;
            let no = self.value()?;
            return Ok(Value::If(Box::new(left), Box::new(yes), Box::new(no)));
        }
        Ok(left)
    }

    /// Формула: `+` и `-` слабее `*` и `/`, как в школе.
    fn sum(&mut self) -> Result<Value, UiError> {
        let mut left = self.term()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => '+',
                Tok::Minus => '-',
                _ => return Ok(left),
            };
            self.next();
            let right = self.term()?;
            left = Value::Op(Box::new(left), op, Box::new(right));
        }
    }

    fn term(&mut self) -> Result<Value, UiError> {
        let mut left = self.atom()?;
        loop {
            let op = match self.peek() {
                Tok::Star => '*',
                Tok::Slash => '/',
                _ => return Ok(left),
            };
            self.next();
            let right = self.atom()?;
            left = Value::Op(Box::new(left), op, Box::new(right));
        }
    }

    fn atom(&mut self) -> Result<Value, UiError> {
        match self.next() {
            (Tok::Number(n), _) => Ok(Value::Number(n)),
            (Tok::Percent(n), _) => Ok(Value::Percent(n)),
            (Tok::Text(s), _) => Ok(Value::Text(s)),
            (Tok::Interp(parts), _) => Ok(Value::Call("text".into(), parts)),
            (Tok::Color(c), _) => Ok(Value::Color(c)),
            (Tok::Ident(first), _) => {
                let mut path = first;
                while *self.peek() == Tok::Dot {
                    self.next();
                    let (part, _) = self.ident("имя после точки")?;
                    path.push('.');
                    path.push_str(&part);
                }
                if *self.peek() == Tok::LParen {
                    let args = self.args()?;
                    Ok(Value::Call(path, args))
                } else {
                    Ok(Value::Name(path))
                }
            }
            (Tok::Dollar, _) => {
                let (first, _) = self.ident("путь после $, например $settings.audio.volume")?;
                let mut path = first;
                while *self.peek() == Tok::Dot {
                    self.next();
                    let (part, _) = self.ident("имя после точки")?;
                    path.push('.');
                    path.push_str(&part);
                }
                Ok(Value::Bind(path))
            }
            (Tok::LParen, _) => {
                let v = self.value()?;
                self.expect(Tok::RParen, "«)» после формулы")?;
                Ok(v)
            }
            (t, pos) => Err(UiError::Syntax {
                pos,
                message: format!("ждал значение, а здесь {}", t.describe()),
            }),
        }
    }
}

/// Разбирает текст файла `.rui`.
pub fn parse(src: &str) -> Result<Document, UiError> {
    parse_file(src, 0)
}

/// Разбирает текст файла `.rui`; `file` попадает в места ошибок ([`Pos::file`]).
pub fn parse_file(src: &str, file: u16) -> Result<Document, UiError> {
    let toks = lex(src, file)?;
    Parser { toks, at: 0 }.document()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swiftui_like_screen_parses() {
        let doc = parse(
            r##"
            // главное меню
            screen MainMenu {
                Column(spacing: 16, align: center) {
                    Text("Играть").font(32).color(#F5A54A)
                    Rect()
                        .size(200, 50%)
                        .hover { .background(#FFFFFF80) }
                }
                .align(center)
            }
            layer Hud { }
            "##,
        )
        .unwrap();
        assert_eq!(doc.screens.len(), 2);
        let menu = &doc.screens[0];
        assert_eq!(menu.kind, ScreenKind::Screen);
        assert_eq!(menu.name, "MainMenu");
        let column = &menu.children[0];
        assert_eq!(column.kind, "Column");
        assert_eq!(column.args[0].name.as_deref(), Some("spacing"));
        assert_eq!(column.args[0].value, Value::Number(16.0));
        assert_eq!(column.args[1].value, Value::Name("center".into()));
        assert_eq!(column.modifiers[0].name, "align");
        assert_eq!(column.pos.line, 4);
        let kids = column.children.as_ref().unwrap();
        assert_eq!(kids[0].args[0].value, Value::Text("Играть".into()));
        assert_eq!(
            kids[0].modifiers[1].args[0].value,
            Value::Color([0xF5, 0xA5, 0x4A, 255])
        );
        let rect = &kids[1];
        assert_eq!(rect.modifiers[0].args[1].value, Value::Percent(50.0));
        let hover = rect.modifiers[1].inner.as_ref().unwrap();
        assert_eq!(hover[0].args[0].value, Value::Color([255, 255, 255, 0x80]));
        assert_eq!(doc.screens[1].kind, ScreenKind::Layer);
    }

    #[test]
    fn paths_and_calls_are_values() {
        let doc = parse(r#"screen A { Text(tr("menu.title")).color(theme.accent).opacity(-0.5) }"#)
            .unwrap();
        let text = &doc.screens[0].children[0];
        assert_eq!(
            text.args[0].value,
            Value::Call(
                "tr".into(),
                vec![Arg {
                    name: None,
                    value: Value::Text("menu.title".into()),
                    pos: Pos {
                        line: 1,
                        column: 20,
                        file: 0,
                    }
                }]
            )
        );
        assert_eq!(
            text.modifiers[0].args[0].value,
            Value::Name("theme.accent".into())
        );
        assert_eq!(text.modifiers[1].args[0].value, Value::Number(-0.5));
    }

    #[test]
    fn errors_name_the_line_in_russian() {
        let e = parse("screen A {\n  Text(\"x\"\n}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("строка 3"), "{e}");
        assert!(e.contains("запятая"), "{e}");
        let e = parse("screen A {\n  Text(\"x\").font(24)\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("не хватает «}»"), "{e}");
        let e = parse("screen A { Rect().background(#12345) }")
            .unwrap_err()
            .to_string();
        assert!(e.contains("#RRGGBB"), "{e}");
        let e = parse("Column { }").unwrap_err().to_string();
        assert!(e.contains("screen Имя"), "{e}");
        let e = parse("screen A { Text(\"нет конца) }")
            .unwrap_err()
            .to_string();
        assert!(e.contains("не закрыта"), "{e}");
    }
}
