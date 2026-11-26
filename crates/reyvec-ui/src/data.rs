//! Данные игры для интерфейса (шаг И.3а, `docs/game-ui.md`, «Привязка к движку»).
//!
//! Игра кладёт значения по путям: `game.hero.hp`, `game.hero.coins`. В разметке путь
//! пишется как есть: `Bar(game.hero.hp / game.hero.hp_max)`,
//! `Text("Монеты: \(game.hero.coins)")`, `.when(game.hero.hp < 2) { … }`. Поменялось
//! значение — экран пересчитывает вид и тексты (без разбора файла и без сборки заново).

use std::collections::BTreeMap;

use crate::UiError;
use crate::syntax::{Arg, Pos, Value};

/// С чего начинаются пути к данным игры.
pub const ROOTS: &[&str] = &["game", "settings"];

/// Значения игры по путям.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiData {
    values: BTreeMap<String, Value>,
}

impl UiData {
    /// Число по пути. `true` — значение поменялось.
    pub fn set_number(&mut self, path: &str, v: f64) -> bool {
        self.put(path, Value::Number(v as f32))
    }

    /// Строка по пути.
    pub fn set_text(&mut self, path: &str, v: &str) -> bool {
        self.put(path, Value::Text(v.to_owned()))
    }

    /// Да или нет по пути: в разметке `true`, `false`.
    pub fn set_flag(&mut self, path: &str, v: bool) -> bool {
        self.put(path, Value::Name(if v { "true" } else { "false" }.into()))
    }

    fn put(&mut self, path: &str, v: Value) -> bool {
        debug_assert!(
            is_path(path),
            "путь данных начинается с game. или settings."
        );
        if self.values.get(path) == Some(&v) {
            return false;
        }
        self.values.insert(path.to_owned(), v);
        true
    }

    /// Значение по пути.
    pub fn get(&self, path: &str) -> Option<&Value> {
        self.values.get(path)
    }

    /// Пути, которые игра задала, по алфавиту.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Имя — путь к данным игры (`game.…`, `settings.…`).
pub fn is_path(name: &str) -> bool {
    name.split_once('.')
        .is_some_and(|(root, rest)| ROOTS.contains(&root) && !rest.is_empty())
}

/// В значении есть путь к данным игры: оно «живое» и считается заново, когда данные
/// меняются.
pub fn is_live(v: &Value) -> bool {
    match v {
        Value::Name(n) => is_path(n),
        Value::Call(_, args) => args.iter().any(|a| is_live(&a.value)),
        Value::Op(a, _, b) => is_live(a) || is_live(b),
        Value::If(c, a, b) => is_live(c) || is_live(a) || is_live(b),
        _ => false,
    }
}

/// Считает живое значение по данным `data`: пути — в значения, формулы — в числа,
/// сравнения — в `true`/`false`, выбор — в один из вариантов. Пока игра не дала ни
/// одного значения, путь считается нулём: экран можно смотреть и без игры.
pub fn compute(v: &Value, pos: Pos, data: &UiData) -> Result<Value, UiError> {
    if !is_live(v) {
        return Ok(v.clone());
    }
    match v {
        Value::Name(n) => match data.get(n) {
            Some(v) => Ok(v.clone()),
            None if data.is_empty() => Ok(Value::Number(0.0)),
            None => Err(missing(n, pos, data)),
        },
        Value::Call(f, args) => Ok(Value::Call(
            f.clone(),
            args.iter()
                .map(|a| {
                    Ok(Arg {
                        name: a.name.clone(),
                        value: compute(&a.value, a.pos, data)?,
                        pos: a.pos,
                    })
                })
                .collect::<Result<_, UiError>>()?,
        )),
        Value::If(c, yes, no) => {
            if truth(&compute(c, pos, data)?, pos)? {
                compute(yes, pos, data)
            } else {
                compute(no, pos, data)
            }
        }
        Value::Op(a, op, b) => {
            let a = compute(a, pos, data)?;
            let b = compute(b, pos, data)?;
            operate(&a, *op, &b, pos)
        }
        other => Ok(other.clone()),
    }
}

/// Да или нет: `true`/`false`, число не ноль.
pub fn truth(v: &Value, pos: Pos) -> Result<bool, UiError> {
    match v {
        Value::Name(n) if n == "true" => Ok(true),
        Value::Name(n) if n == "false" => Ok(false),
        Value::Number(n) => Ok(*n != 0.0),
        other => Err(UiError::Meaning {
            pos,
            message: format!(
                "здесь нужно «да или нет» (сравнение, true, false), а получилось: {}",
                other.kind()
            ),
        }),
    }
}

fn flag(b: bool) -> Value {
    Value::Name(if b { "true" } else { "false" }.into())
}

fn operate(a: &Value, op: char, b: &Value, pos: Pos) -> Result<Value, UiError> {
    let cmp = |o: std::cmp::Ordering| -> bool {
        use std::cmp::Ordering::*;
        match op {
            '<' => o == Less,
            '>' => o == Greater,
            '≤' => o != Greater,
            '≥' => o != Less,
            '=' => o == Equal,
            _ => o != Equal,
        }
    };
    let out = match (a, b) {
        (Value::Number(x), Value::Number(y)) => match op {
            '+' => Some(Value::Number(x + y)),
            '-' => Some(Value::Number(x - y)),
            '*' => Some(Value::Number(x * y)),
            '/' => Some(Value::Number(if *y == 0.0 { 0.0 } else { x / y })),
            _ => x.partial_cmp(y).map(|o| flag(cmp(o))),
        },
        (Value::Percent(x), Value::Number(y)) if matches!(op, '*' | '/') => {
            Some(Value::Percent(if op == '*' {
                x * y
            } else if *y == 0.0 {
                0.0
            } else {
                x / y
            }))
        }
        (Value::Number(x), Value::Percent(y)) if op == '*' => Some(Value::Percent(x * y)),
        (Value::Text(x), Value::Text(y)) if op == '+' => Some(Value::Text(format!("{x}{y}"))),
        (x, y) if matches!(op, '=' | '≠') => Some(flag((x == y) == (op == '='))),
        _ => None,
    };
    out.ok_or_else(|| UiError::Meaning {
        pos,
        message: format!(
            "формула {} {op} {} не считается: числа складываются и сравниваются с числами",
            a.kind(),
            b.kind()
        ),
    })
}

/// Значение как текст для вставки в строку: целые без точки.
pub fn show(v: &Value) -> String {
    match v {
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 1e9 => format!("{}", *n as i64),
        Value::Number(n) => format!("{n:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned(),
        Value::Percent(n) => format!("{n}%"),
        Value::Text(t) | Value::Name(t) => t.clone(),
        other => other.kind().to_owned(),
    }
}

fn missing(name: &str, pos: Pos, data: &UiData) -> UiError {
    let known: Vec<&str> = data.paths().collect();
    let hint = match crate::model::suggest(name, &known) {
        Some(s) => format!(" Может, {s}?"),
        None => format!(" Есть: {}.", known.join(", ")),
    };
    UiError::Meaning {
        pos,
        message: format!("игра не даёт «{name}».{hint}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn expr(src: &str) -> Value {
        let doc = parse(&format!("screen S {{ Rect().x({src}) }}")).unwrap();
        doc.screens[0].children[0].modifiers[0].args[0]
            .value
            .clone()
    }

    #[test]
    fn live_values_follow_the_data() {
        let mut d = UiData::default();
        let p = Pos::default();
        // Без данных путь — ноль.
        assert_eq!(
            compute(&expr("game.hero.hp + 1"), p, &d).unwrap(),
            Value::Number(1.0)
        );
        assert!(d.set_number("game.hero.hp", 3.0));
        assert!(!d.set_number("game.hero.hp", 3.0));
        d.set_number("game.hero.hp_max", 4.0);
        assert_eq!(
            compute(&expr("game.hero.hp / game.hero.hp_max"), p, &d).unwrap(),
            Value::Number(0.75)
        );
        assert_eq!(
            compute(&expr("game.hero.hp < 2 ? 10 : 20"), p, &d).unwrap(),
            Value::Number(20.0)
        );
        assert_eq!(
            compute(&expr("game.hero.hp == 3"), p, &d).unwrap(),
            Value::Name("true".into())
        );
        let e = compute(&expr("game.hero.hpp"), p, &d)
            .unwrap_err()
            .to_string();
        assert!(e.contains("Может, game.hero.hp?"), "{e}");
        assert!(!is_live(&expr("width < 1280")));
    }

    #[test]
    fn numbers_are_shown_without_extra_zeros() {
        assert_eq!(show(&Value::Number(12.0)), "12");
        assert_eq!(show(&Value::Number(0.5)), "0.5");
        assert_eq!(show(&Value::Number(1.0 / 3.0)), "0.33");
    }
}
