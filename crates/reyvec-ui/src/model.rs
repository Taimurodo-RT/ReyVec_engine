//! Что значат блоки и модификаторы: из разобранного файла ([`crate::syntax`]) — дерево
//! элементов с готовыми размерами, отступами и оформлением.
//!
//! Понимает блоки `Column`, `Row`, `Stack`, `Rect`, `Text`, `Spacer`, модификаторы
//! раскладки и оформления (И.1), стили, классы, переменные и наследование текста (И.2а,
//! [`crate::style`]). Незнакомое имя — ошибка с местом в файле и подсказкой
//! («может, .padding?»), а не молчаливый пропуск: иначе опечатку не найти.

use reyvec_vector::Color;

use crate::UiError;
use crate::style::{Info, StyleSet, Vars};
use crate::syntax::{Arg, Modifier, Node, Pos, ScreenDecl, ScreenKind, Value};

/// Длина: пиксели экрана 1920×1080 (на другом экране умножаются на высоту / 1080) или
/// доля родителя.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Len {
    #[default]
    Auto,
    Px(f32),
    Percent(f32),
}

/// Край или середина по одной оси.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Start,
    Center,
    End,
    Stretch,
}

/// Как ряд или столбец раскладывает детей вдоль себя.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    Between,
    Around,
}

/// Раскладка элемента: размеры, отступы, место у родителя.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Внутренние отступы: верх, право, низ, лево.
    pub padding: [f32; 4],
    /// Внешние отступы: верх, право, низ, лево.
    pub margin: [f32; 4],
    pub width: Len,
    pub height: Len,
    /// Доля свободного места вдоль ряда или столбца родителя (`Spacer` — 1).
    pub grow: f32,
    /// Место у родителя: по горизонтали и по вертикали. `None` — как решит родитель.
    pub align: (Option<Edge>, Option<Edge>),
    /// Промежуток между детьми ряда или столбца.
    pub spacing: f32,
    /// Как ряд или столбец ставит детей поперёк себя.
    pub cross: Edge,
    pub justify: Justify,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            padding: [0.0; 4],
            margin: [0.0; 4],
            width: Len::Auto,
            height: Len::Auto,
            grow: 0.0,
            align: (None, None),
            spacing: 0.0,
            cross: Edge::Start,
            justify: Justify::Start,
        }
    }
}

/// Оформление прямоугольника элемента.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub background: Option<Color>,
    pub radius: f32,
    /// Обводка: цвет и толщина (внутрь прямоугольника).
    pub border: Option<(Color, f32)>,
    /// Прозрачность элемента и всего внутри него.
    pub opacity: f32,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            background: None,
            radius: 0.0,
            border: None,
            opacity: 1.0,
        }
    }
}

/// Текст элемента `Text`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSpec {
    pub text: String,
    /// Высота буквы (em) в пикселях экрана 1920×1080.
    pub size: f32,
    pub color: Color,
}

/// Вид элемента.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Column,
    Row,
    /// Дети друг на друге; каждый ставится по своему `.align` (по умолчанию — в середину).
    Stack,
    Text(TextSpec),
    Spacer,
}

/// Элемент экрана.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub kind: Kind,
    pub placement: Placement,
    pub look: Look,
    /// Размер и цвет текста этого элемента и (по наследству) его детей.
    pub font: f32,
    pub color: Color,
    pub children: Vec<Element>,
    pub pos: Pos,
    /// Вид блока, как написан в файле (`Rect`, `Button`), и классы.
    pub type_name: String,
    pub classes: Vec<String>,
    /// Откуда взялось каждое свойство: имя модификатора и источник, последний победил.
    pub origins: Vec<(String, String)>,
}

/// Экран: корень — `Stack` на весь экран.
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub name: String,
    pub kind: ScreenKind,
    pub root: Element,
}

const KINDS: &[&str] = &["Column", "Row", "Stack", "Rect", "Text", "Spacer"];
const MODIFIERS: &[&str] = &[
    "class",
    "var",
    "spacing",
    "padding",
    "margin",
    "size",
    "width",
    "height",
    "grow",
    "align",
    "anchor",
    "background",
    "radius",
    "border",
    "opacity",
    "font",
    "color",
];
/// Модификаторы состояний: разбираются, но работают с шага И.2 (мышь и клавиатура).
const STATES: &[&str] = &["hover", "pressed", "focus", "disabled", "checked"];
/// Размер текста без `.font`.
pub const DEFAULT_FONT: f32 = 24.0;

fn meaning(pos: Pos, message: impl Into<String>) -> UiError {
    UiError::Meaning {
        pos,
        message: message.into(),
    }
}

/// Самое похожее имя из списка, если оно правда похоже: для подсказки при опечатке.
pub(crate) fn suggest<'a>(word: &str, known: &[&'a str]) -> Option<&'a str> {
    let distance = |a: &str, b: &str| {
        let a: Vec<char> = a.to_lowercase().chars().collect();
        let b: Vec<char> = b.to_lowercase().chars().collect();
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.iter().enumerate() {
            let mut row = vec![i + 1];
            for (j, cb) in b.iter().enumerate() {
                let cost = usize::from(ca != cb);
                row.push((prev[j] + cost).min(prev[j + 1] + 1).min(row[j] + 1));
            }
            prev = row;
        }
        prev[b.len()]
    };
    known
        .iter()
        .map(|k| (distance(word, k), *k))
        .filter(|(d, k)| *d <= 2.max(k.len() / 3))
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

fn unknown(pos: Pos, what: &str, word: &str, known: &[&str], dot: &str) -> UiError {
    let hint = match suggest(word, known) {
        Some(s) => format!(" Может, {dot}{s}?"),
        None => format!(
            " Известны: {}.",
            known
                .iter()
                .map(|k| format!("{dot}{k}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    meaning(pos, format!("незнакомый {what} «{dot}{word}».{hint}"))
}

fn number(arg: &Arg, what: &str) -> Result<f32, UiError> {
    match arg.value {
        Value::Number(n) => Ok(n),
        ref v => Err(meaning(
            arg.pos,
            format!("{what}: ждал число, а здесь {}", v.kind()),
        )),
    }
}

fn len(arg: &Arg, what: &str) -> Result<Len, UiError> {
    match &arg.value {
        Value::Number(n) => Ok(Len::Px(*n)),
        Value::Percent(p) => Ok(Len::Percent(*p)),
        Value::Name(n) if n == "fill" => Ok(Len::Percent(100.0)),
        Value::Name(n) if n == "auto" => Ok(Len::Auto),
        v => Err(meaning(
            arg.pos,
            format!(
                "{what}: ждал пиксели (200), проценты (50%) или fill, а здесь {}",
                v.kind()
            ),
        )),
    }
}

fn color(arg: &Arg, what: &str) -> Result<Color, UiError> {
    match arg.value {
        Value::Color([r, g, b, a]) => {
            let mut c = Color::from_srgb8(r, g, b);
            c.a = f32::from(a) / 255.0;
            Ok(c)
        }
        ref v => Err(meaning(
            arg.pos,
            format!(
                "{what}: ждал цвет вида #F5A54A, а здесь {}{}",
                v.kind(),
                if matches!(v, Value::Name(n) if n.starts_with("theme.")) {
                    " (переменные темы пишутся var(имя))"
                } else {
                    ""
                }
            ),
        )),
    }
}

/// 1, 2 или 4 числа, как в CSS: все стороны; верх-низ и лево-право; верх, право, низ, лево.
fn sides(m: &Modifier) -> Result<[f32; 4], UiError> {
    let n: Vec<f32> = m
        .args
        .iter()
        .map(|a| number(a, &format!(".{}", m.name)))
        .collect::<Result<_, _>>()?;
    match n[..] {
        [a] => Ok([a; 4]),
        [v, h] => Ok([v, h, v, h]),
        [t, r, b, l] => Ok([t, r, b, l]),
        _ => Err(meaning(
            m.pos,
            format!(
                ".{}: 1, 2 или 4 числа (все стороны; верх-низ и лево-право; верх, право, низ, лево)",
                m.name
            ),
        )),
    }
}

fn edge_word(word: &str) -> Option<Edge> {
    match word {
        "start" | "left" | "top" | "leading" => Some(Edge::Start),
        "center" => Some(Edge::Center),
        "end" | "right" | "bottom" | "trailing" => Some(Edge::End),
        "stretch" | "fill" => Some(Edge::Stretch),
        _ => None,
    }
}

/// `center`, `top`, `bottom_left`, … → (по горизонтали, по вертикали).
fn align_word(arg: &Arg) -> Result<(Option<Edge>, Option<Edge>), UiError> {
    let bad = || {
        meaning(
            arg.pos,
            "место: center, top, bottom, left, right, top_left, top_right, bottom_left, bottom_right, stretch",
        )
    };
    let Value::Name(word) = &arg.value else {
        return Err(bad());
    };
    Ok(match word.as_str() {
        "center" => (Some(Edge::Center), Some(Edge::Center)),
        "stretch" | "fill" => (Some(Edge::Stretch), Some(Edge::Stretch)),
        "top" => (Some(Edge::Center), Some(Edge::Start)),
        "bottom" => (Some(Edge::Center), Some(Edge::End)),
        "left" | "leading" | "start" => (Some(Edge::Start), Some(Edge::Center)),
        "right" | "trailing" | "end" => (Some(Edge::End), Some(Edge::Center)),
        "top_left" => (Some(Edge::Start), Some(Edge::Start)),
        "top_right" => (Some(Edge::End), Some(Edge::Start)),
        "bottom_left" => (Some(Edge::Start), Some(Edge::End)),
        "bottom_right" => (Some(Edge::End), Some(Edge::End)),
        _ => return Err(bad()),
    })
}

fn apply(m: &Modifier, el: &mut Element) -> Result<(), UiError> {
    let name = m.name.as_str();
    if STATES.contains(&name) {
        // Состояния — шаг И.2б: разбор уже есть, применять пока нечего.
        return Ok(());
    }
    if matches!(name, "class" | "var") {
        // Разобраны раньше, до сопоставления стилей.
        return Ok(());
    }
    if !MODIFIERS.contains(&name) {
        return Err(unknown(m.pos, "модификатор", name, MODIFIERS, "."));
    }
    let what = format!(".{name}");
    let count = |n: std::ops::RangeInclusive<usize>| {
        if n.contains(&m.args.len()) {
            Ok(())
        } else {
            Err(meaning(
                m.pos,
                format!(
                    "{what}: нужно {} значения, а здесь {}",
                    if n.start() == n.end() {
                        n.start().to_string()
                    } else {
                        format!("от {} до {}", n.start(), n.end())
                    },
                    m.args.len()
                ),
            ))
        }
    };
    let p = &mut el.placement;
    match name {
        "spacing" => {
            if !matches!(el.kind, Kind::Column | Kind::Row) {
                return Err(meaning(
                    m.pos,
                    format!("{what} бывает только у Column и Row"),
                ));
            }
            count(1..=1)?;
            p.spacing = number(&m.args[0], &what)?;
        }
        "padding" => p.padding = sides(m)?,
        "margin" => p.margin = sides(m)?,
        "size" => {
            count(1..=2)?;
            p.width = len(&m.args[0], &what)?;
            p.height = len(m.args.get(1).unwrap_or(&m.args[0]), &what)?;
        }
        "width" => {
            count(1..=1)?;
            p.width = len(&m.args[0], &what)?;
        }
        "height" => {
            count(1..=1)?;
            p.height = len(&m.args[0], &what)?;
        }
        "grow" => {
            count(0..=1)?;
            p.grow = m.args.first().map_or(Ok(1.0), |a| number(a, &what))?;
        }
        "align" | "anchor" => {
            if m.args.is_empty() {
                return Err(meaning(
                    m.pos,
                    format!("{what}: куда ставить, например center"),
                ));
            }
            p.align = align_word(&m.args[0])?;
            for a in &m.args[1..] {
                match a.name.as_deref() {
                    Some("margin") => p.margin = [number(a, &what)?; 4],
                    _ => {
                        return Err(meaning(
                            a.pos,
                            format!("{what}: после места бывает только margin: 24"),
                        ));
                    }
                }
            }
        }
        "background" => {
            count(1..=1)?;
            el.look.background = Some(color(&m.args[0], &what)?);
        }
        "radius" => {
            count(1..=1)?;
            el.look.radius = number(&m.args[0], &what)?.max(0.0);
        }
        "border" => {
            count(1..=2)?;
            let c = color(&m.args[0], &what)?;
            let w = m.args.get(1).map_or(Ok(1.0), |a| number(a, &what))?;
            el.look.border = Some((c, w.max(0.0)));
        }
        "opacity" => {
            count(1..=1)?;
            el.look.opacity = number(&m.args[0], &what)?.clamp(0.0, 1.0);
        }
        // У контейнера — для текстов внутри него (наследование, как в CSS).
        "font" | "color" => {
            count(1..=1)?;
            if name == "font" {
                el.font = number(&m.args[0], &what)?.max(1.0);
            } else {
                el.color = color(&m.args[0], &what)?;
            }
        }
        _ => unreachable!("имя проверено выше"),
    }
    Ok(())
}

fn container_args(node: &Node, p: &mut Placement) -> Result<(), UiError> {
    for a in &node.args {
        match a.name.as_deref() {
            Some("spacing") => p.spacing = number(a, "spacing")?,
            Some("align") => {
                let Value::Name(w) = &a.value else {
                    return Err(meaning(a.pos, "align: start, center, end или stretch"));
                };
                p.cross = edge_word(w)
                    .ok_or_else(|| meaning(a.pos, "align: start, center, end или stretch"))?;
            }
            Some("justify") => {
                let Value::Name(w) = &a.value else {
                    return Err(meaning(
                        a.pos,
                        "justify: start, center, end, between, around",
                    ));
                };
                p.justify = match w.as_str() {
                    "start" | "top" | "left" => Justify::Start,
                    "center" => Justify::Center,
                    "end" | "bottom" | "right" => Justify::End,
                    "between" => Justify::Between,
                    "around" => Justify::Around,
                    _ => {
                        return Err(meaning(
                            a.pos,
                            "justify: start, center, end, between, around",
                        ));
                    }
                };
            }
            Some(other) => {
                return Err(unknown(
                    a.pos,
                    &format!("параметр {}", node.kind),
                    other,
                    &["spacing", "align", "justify"],
                    "",
                ));
            }
            None if a == &node.args[0] && matches!(a.value, Value::Number(_)) => {
                // `Column(16)` — короткая запись промежутка.
                p.spacing = number(a, "spacing")?;
            }
            None => {
                return Err(meaning(
                    a.pos,
                    format!(
                        "{}: значения пишутся с именем: spacing: 16, align: center",
                        node.kind
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Что блок получает от предков: переменные, текст по наследству и цепочку предков для
/// вложенных стилей.
#[derive(Clone)]
struct Ctx<'a> {
    styles: &'a StyleSet,
    vars: Vars,
    font: f32,
    color: Color,
    ancestors: Vec<Info>,
}

/// Классы из `.class(a, b)` у самого блока.
fn classes_of(modifiers: &[Modifier]) -> Result<Vec<String>, UiError> {
    let mut out = Vec::new();
    for m in modifiers.iter().filter(|m| m.name == "class") {
        if m.args.is_empty() {
            return Err(meaning(
                m.pos,
                ".class: имена классов, например .class(danger)",
            ));
        }
        for a in &m.args {
            match &a.value {
                Value::Name(n)
                    if a.name.is_none()
                        && !n.contains('.')
                        && !n.chars().next().is_some_and(char::is_uppercase) =>
                {
                    out.push(n.clone());
                }
                _ => {
                    return Err(meaning(
                        a.pos,
                        ".class: имя класса с маленькой буквы без кавычек, .class(danger)",
                    ));
                }
            }
        }
    }
    Ok(out)
}

/// Модификатор и откуда он: «style Button (styles/menu.rui, строка 3)» или «у блока».
type Sourced<'m> = (&'m Modifier, String);

/// Модификаторы блока по лестнице: стили (слабые первыми), потом свои. Вместе с
/// источником для «откуда взялось». Переменные `.var` собираются в `vars`.
fn styled<'m>(
    ctx: &Ctx<'m>,
    me: &Info,
    own: &'m [Modifier],
) -> Result<(Vec<Sourced<'m>>, Vec<Sourced<'m>>), UiError> {
    let mut from_styles = Vec::new();
    for r in ctx.styles.matching(me, &ctx.ancestors) {
        for m in &r.modifiers {
            if m.name == "class" {
                return Err(meaning(
                    m.pos,
                    ".class пишется у блока, а не в стиле: стиль сам выбирает блоки по классам",
                ));
            }
            from_styles.push((m, r.origin.clone()));
        }
    }
    let own = own
        .iter()
        .map(|m| (m, format!("у блока (строка {})", m.pos.line)))
        .collect();
    Ok((from_styles, own))
}

/// Переменные блока: родительские и `.var(имя: значение)` из стилей и своих модификаторов
/// по лестнице. Значения считаются от переменных родителя.
fn own_vars(ctx: &Ctx<'_>, mods: &[&Sourced<'_>]) -> Result<Vars, UiError> {
    let mut set: Vec<(String, Value)> = Vec::new();
    for (m, _) in mods.iter().map(|x| (x.0, &x.1)) {
        if m.name != "var" {
            continue;
        }
        if m.args.is_empty() {
            return Err(meaning(
                m.pos,
                ".var: имя и значение, .var(accent: #6BB8F5)",
            ));
        }
        for a in &m.args {
            let Some(name) = &a.name else {
                return Err(meaning(
                    a.pos,
                    ".var: имя и значение, .var(accent: #6BB8F5)",
                ));
            };
            let v = ctx.vars.resolve(&a.value, a.pos)?;
            set.retain(|(n, _)| n != name);
            set.push((name.clone(), v));
        }
    }
    Ok(ctx.vars.with(set))
}

fn record(origins: &mut Vec<(String, String)>, name: &str, from: &str) {
    origins.retain(|(n, _)| n != name);
    origins.push((name.to_owned(), from.to_owned()));
}

fn element(node: &Node, ctx: &Ctx<'_>) -> Result<Element, UiError> {
    let kind_name = node.kind.as_str();
    if !KINDS.contains(&kind_name) {
        return Err(unknown(node.pos, "блок", kind_name, KINDS, ""));
    }
    let mut placement = Placement::default();
    let kind = match kind_name {
        "Column" | "Row" => {
            if kind_name == "Column" {
                Kind::Column
            } else {
                Kind::Row
            }
        }
        "Stack" | "Rect" => {
            if let Some(a) = node.args.first() {
                return Err(meaning(
                    a.pos,
                    format!("у {kind_name} нет значений в скобках: размер и цвет — модификаторами"),
                ));
            }
            Kind::Stack
        }
        "Spacer" => {
            placement.grow = 1.0;
            Kind::Spacer
        }
        "Text" => {
            let text = match node.args.first() {
                Some(Arg {
                    value: Value::Text(t),
                    name: None,
                    ..
                }) if node.args.len() == 1 => t.clone(),
                Some(Arg {
                    value: Value::Call(f, _),
                    pos,
                    ..
                }) if f == "tr" => {
                    return Err(meaning(
                        *pos,
                        "tr(\"ключ\") — тексты на двух языках, шаг И.2",
                    ));
                }
                _ => {
                    return Err(meaning(
                        node.pos,
                        "Text: одна строка в кавычках, Text(\"Играть\")",
                    ));
                }
            };
            Kind::Text(TextSpec {
                text,
                size: ctx.font,
                color: ctx.color,
            })
        }
        _ => unreachable!("имя проверено выше"),
    };
    let me = Info {
        kind: kind_name.to_owned(),
        classes: classes_of(&node.modifiers)?,
    };
    let mut el = Element {
        kind,
        placement: Placement::default(),
        look: Look::default(),
        font: ctx.font,
        color: ctx.color,
        children: Vec::new(),
        pos: node.pos,
        type_name: me.kind.clone(),
        classes: me.classes.clone(),
        origins: Vec::new(),
    };
    // Spacer растягивается, если стиль или сам блок не скажут иначе.
    el.placement.grow = placement.grow;
    let (from_styles, own) = styled(ctx, &me, &node.modifiers)?;
    let vars = own_vars(
        ctx,
        &from_styles.iter().chain(own.iter()).collect::<Vec<_>>(),
    )?;
    // Лестница: стили, потом значения в скобках у Column/Row, потом свои модификаторы.
    for (m, from) in &from_styles {
        apply(&vars.resolve_modifier(m)?, &mut el)?;
        record(&mut el.origins, &m.name, from);
    }
    if matches!(el.kind, Kind::Column | Kind::Row) {
        let resolved = Node {
            args: vars.resolve_args(&node.args)?,
            ..node.clone()
        };
        container_args(&resolved, &mut el.placement)?;
        for a in &node.args {
            let name = a.name.as_deref().unwrap_or("spacing");
            record(
                &mut el.origins,
                name,
                &format!("у блока (строка {})", a.pos.line),
            );
        }
    }
    for (m, from) in &own {
        apply(&vars.resolve_modifier(m)?, &mut el)?;
        record(&mut el.origins, &m.name, from);
    }
    if let Kind::Text(spec) = &mut el.kind {
        spec.size = el.font;
        spec.color = el.color;
    }
    if let Some(children) = &node.children {
        if matches!(el.kind, Kind::Text(_) | Kind::Spacer) {
            return Err(meaning(
                node.pos,
                format!("у {kind_name} не бывает вложенных блоков в {{ }}"),
            ));
        }
        let mut inner = ctx.clone();
        inner.vars = vars;
        inner.font = el.font;
        inner.color = el.color;
        inner.ancestors.push(me);
        el.children = children
            .iter()
            .map(|c| element(c, &inner))
            .collect::<Result<_, _>>()?;
    }
    Ok(el)
}

/// Экран из объявления: корень — `Stack` на весь экран, модификаторы экрана — у корня.
/// Корень сопоставляется со стилями по имени экрана: `style MainMenu { ... }`.
pub fn screen(decl: &ScreenDecl, styles: &StyleSet, vars: &Vars) -> Result<Screen, UiError> {
    let root_ctx = Ctx {
        styles,
        vars: vars.clone(),
        font: DEFAULT_FONT,
        color: Color::WHITE,
        ancestors: Vec::new(),
    };
    for m in &decl.modifiers {
        if matches!(
            m.name.as_str(),
            "size" | "width" | "height" | "align" | "anchor"
        ) {
            return Err(meaning(
                m.pos,
                format!("экран всегда на весь экран: .{} у него не бывает", m.name),
            ));
        }
    }
    // Корень — тот же Stack, только с видом «имя экрана» для стилей.
    let node = Node {
        kind: "Stack".into(),
        args: Vec::new(),
        children: Some(decl.children.clone()),
        modifiers: decl.modifiers.clone(),
        pos: decl.pos,
    };
    let me = Info {
        kind: decl.name.clone(),
        classes: classes_of(&decl.modifiers)?,
    };
    let mut root = Element {
        kind: Kind::Stack,
        placement: Placement {
            width: Len::Percent(100.0),
            height: Len::Percent(100.0),
            ..Placement::default()
        },
        look: Look::default(),
        font: DEFAULT_FONT,
        color: Color::WHITE,
        children: Vec::new(),
        pos: decl.pos,
        type_name: decl.name.clone(),
        classes: me.classes.clone(),
        origins: Vec::new(),
    };
    let (from_styles, own) = styled(&root_ctx, &me, &node.modifiers)?;
    let vars = own_vars(
        &root_ctx,
        &from_styles.iter().chain(own.iter()).collect::<Vec<_>>(),
    )?;
    for (m, from) in from_styles.iter().chain(own.iter()) {
        if matches!(
            m.name.as_str(),
            "size" | "width" | "height" | "align" | "anchor"
        ) {
            return Err(meaning(
                m.pos,
                format!("экран всегда на весь экран: .{} у него не бывает", m.name),
            ));
        }
        apply(&vars.resolve_modifier(m)?, &mut root)?;
        record(&mut root.origins, &m.name, from);
    }
    let inner = Ctx {
        vars,
        font: root.font,
        color: root.color,
        ancestors: vec![me],
        ..root_ctx
    };
    root.children = decl
        .children
        .iter()
        .map(|c| element(c, &inner))
        .collect::<Result<_, _>>()?;
    Ok(Screen {
        name: decl.name.clone(),
        kind: decl.kind,
        root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn first(src: &str) -> Result<Screen, UiError> {
        let doc = parse(src)?;
        let mut styles = StyleSet::default();
        styles.add_rules(&doc.styles, "экран");
        styles.themes = doc.themes.clone();
        let vars = styles.theme_vars((!styles.themes.is_empty()).then_some(0))?;
        screen(&doc.screens[0], &styles, &vars)
    }

    #[test]
    fn modifiers_fill_placement_and_look() {
        let s = first(
            r##"screen A {
                Column(spacing: 12, align: center, justify: between) {
                    Text("Привет").font(40).color(#FF0000)
                    Spacer()
                    Rect().size(200, 50%).background(#00000080).radius(8).border(#FFFFFF, 2)
                }
                .padding(10, 20)
                .align(bottom_left, margin: 24)
                .opacity(0.5)
            }"##,
        )
        .unwrap();
        let col = &s.root.children[0];
        assert_eq!(col.kind, Kind::Column);
        assert_eq!(col.placement.spacing, 12.0);
        assert_eq!(col.placement.cross, Edge::Center);
        assert_eq!(col.placement.justify, Justify::Between);
        assert_eq!(col.placement.padding, [10.0, 20.0, 10.0, 20.0]);
        assert_eq!(col.placement.margin, [24.0; 4]);
        assert_eq!(col.placement.align, (Some(Edge::Start), Some(Edge::End)));
        assert_eq!(col.look.opacity, 0.5);
        let Kind::Text(t) = &col.children[0].kind else {
            panic!("ждали текст");
        };
        assert_eq!((t.text.as_str(), t.size), ("Привет", 40.0));
        assert!(t.color.r > 0.99 && t.color.g < 0.01);
        assert_eq!(col.children[1].placement.grow, 1.0);
        let rect = &col.children[2];
        assert_eq!(rect.placement.width, Len::Px(200.0));
        assert_eq!(rect.placement.height, Len::Percent(50.0));
        assert!((rect.look.background.unwrap().a - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(rect.look.radius, 8.0);
        assert_eq!(rect.look.border.unwrap().1, 2.0);
    }

    #[test]
    fn typos_get_a_hint() {
        let e = first("screen A { Rect().paddng(4) }")
            .unwrap_err()
            .to_string();
        assert!(e.contains("Может, .padding?"), "{e}");
        let e = first("screen A {\n  Colum { }\n}").unwrap_err().to_string();
        assert!(
            e.contains("строка 2") && e.contains("Может, Column?"),
            "{e}"
        );
        let e = first("screen A { Rect().spacing(20) }")
            .unwrap_err()
            .to_string();
        assert!(e.contains("только у Column и Row"), "{e}");
        let e = first("screen A { Rect().background(theme.panel) }")
            .unwrap_err()
            .to_string();
        assert!(e.contains("var(имя)"), "{e}");
        let e = first("screen A { Text(\"a\") { Rect() } }")
            .unwrap_err()
            .to_string();
        assert!(e.contains("не бывает вложенных"), "{e}");
    }

    #[test]
    fn states_are_parsed_but_wait_for_step_two() {
        let s = first("screen A { Rect().background(#000000).hover { .background(#FFFFFF) } }")
            .unwrap();
        assert!(s.root.children[0].look.background.unwrap().r < 0.01);
    }
}
