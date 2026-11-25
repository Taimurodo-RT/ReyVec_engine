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
    /// Вызов: `tr("menu.title")`, `image("...")`.
    Call(String, Vec<Arg>),
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

/// Разобранный файл.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub screens: Vec<ScreenDecl>,
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
            Tok::Eof => "конец файла".into(),
        }
    }
}

fn lex(src: &str) -> Result<Vec<(Tok, Pos)>, UiError> {
    let mut out = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let (mut i, mut line, mut col) = (0usize, 1u32, 1u32);
    let err = |pos: Pos, message: String| UiError::Syntax { pos, message };
    while i < chars.len() {
        let c = chars[i];
        let pos = Pos { line, column: col };
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
            '(' | ')' | '{' | '}' | ',' | ':' => {
                out.push((
                    match c {
                        '(' => Tok::LParen,
                        ')' => Tok::RParen,
                        '{' => Tok::LBrace,
                        '}' => Tok::RBrace,
                        ',' => Tok::Comma,
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
                        Some('\\') => {
                            let e = chars.get(i + 1).copied();
                            text.push(match e {
                                Some('n') => '\n',
                                Some('"') => '"',
                                Some('\\') => '\\',
                                _ => {
                                    return Err(err(
                                        Pos { line, column: col },
                                        "в строке после \\ бывает только n, \" или \\".into(),
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
                out.push((Tok::Text(text), pos));
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
    out.push((Tok::Eof, Pos { line, column: col }));
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
                other => {
                    return Err(self.error(format!(
                        "файл состоит из «screen Имя {{ ... }}» и «layer Имя {{ ... }}», а здесь {}",
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

    fn value(&mut self) -> Result<Value, UiError> {
        match self.next() {
            (Tok::Number(n), _) => Ok(Value::Number(n)),
            (Tok::Percent(n), _) => Ok(Value::Percent(n)),
            (Tok::Text(s), _) => Ok(Value::Text(s)),
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
            (t, pos) => Err(UiError::Syntax {
                pos,
                message: format!("ждал значение, а здесь {}", t.describe()),
            }),
        }
    }
}

/// Разбирает текст файла `.rui`.
pub fn parse(src: &str) -> Result<Document, UiError> {
    let toks = lex(src)?;
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
                        column: 20
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
