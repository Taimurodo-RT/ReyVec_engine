//! Стили и темы (шаг И.2а, `docs/game-ui.md`, раздел «Стили и темы»).
//!
//! - **Тема** — набор переменных; `var(имя)` читает ближайшее `.var(имя: …)` вверх по
//!   дереву, а если его нет — переменную активной темы.
//! - **Стиль** — путь селекторов (`MainMenu > Column > Button` при вложенности; звено
//!   «внутри» значит на любой глубине) и модификаторы.
//! - **Лестница силы** снизу вверх: вид → класс → вложенный → модификатор у блока; на
//!   одной ступени — записанное позже. Подсчёта «веса», как в CSS, нет.
//!
//! Сопоставление идёт один раз, когда строится экран, а не каждый кадр.

use std::collections::HashMap;
use std::rc::Rc;

use crate::UiError;
use crate::data::is_live;
use crate::syntax::{Arg, Modifier, Pos, Selector, StyleRule, ThemeDecl, Value};

/// Ступень лестницы силы стиля.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rung {
    /// `style Button`
    Kind,
    /// `style danger`, `style Button.big`
    Class,
    /// `style MainMenu { Button { } }`
    Nested,
}

/// Стиль, развёрнутый из вложенных: полный путь селекторов.
#[derive(Clone, Debug)]
pub struct FlatRule {
    pub path: Vec<Selector>,
    pub modifiers: Vec<Modifier>,
    pub rung: Rung,
    /// Порядок записи по всем файлам: при равной ступени побеждает больший.
    pub order: usize,
    /// Откуда: «style Menu Button (styles/menu.rui, строка 4)».
    pub origin: String,
}

/// Что известно о блоке для сопоставления со стилями.
#[derive(Clone, Debug)]
pub struct Info {
    pub kind: String,
    pub classes: Vec<String>,
}

fn matches(sel: &Selector, info: &Info) -> bool {
    sel.kind.as_ref().is_none_or(|k| *k == info.kind)
        && sel.classes.iter().all(|c| info.classes.contains(c))
}

/// Все стили и темы экрана.
#[derive(Clone, Debug, Default)]
pub struct StyleSet {
    pub rules: Vec<FlatRule>,
    pub themes: Vec<ThemeDecl>,
}

impl StyleSet {
    /// Добавляет стили файла `file_name` (для «откуда взялось») в конец порядка.
    pub fn add_rules(&mut self, rules: &[StyleRule], file_name: &str) {
        for r in rules {
            self.flatten(r, &[], file_name);
        }
    }

    fn flatten(&mut self, rule: &StyleRule, prefix: &[Selector], file_name: &str) {
        let mut path = prefix.to_vec();
        path.push(rule.selector.clone());
        if !rule.modifiers.is_empty() {
            let rung = if path.len() > 1 {
                Rung::Nested
            } else if rule.selector.classes.is_empty() {
                Rung::Kind
            } else {
                Rung::Class
            };
            let name = path
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ");
            self.rules.push(FlatRule {
                path: path.clone(),
                modifiers: rule.modifiers.clone(),
                rung,
                order: self.rules.len(),
                origin: format!("style {name} ({file_name}, строка {})", rule.pos.line),
            });
        }
        for n in &rule.nested {
            self.flatten(n, &path, file_name);
        }
    }

    /// Стили, которые действуют на блок `me` внутри `ancestors` (от корня к родителю), в
    /// порядке применения: слабые первыми.
    pub fn matching(&self, me: &Info, ancestors: &[Info]) -> Vec<&FlatRule> {
        let mut found: Vec<&FlatRule> = self
            .rules
            .iter()
            .filter(|r| {
                let (last, before) = r.path.split_last().expect("путь не пустой");
                if !matches(last, me) {
                    return false;
                }
                // Звенья «внутри»: ближайший подходящий предок для каждого, снизу вверх.
                let mut up = ancestors.iter().rev();
                before
                    .iter()
                    .rev()
                    .all(|sel| up.by_ref().any(|a| matches(sel, a)))
            })
            .collect();
        found.sort_by_key(|r| (r.rung, r.order));
        found
    }

    /// Имена тем по порядку подключения.
    pub fn theme_names(&self) -> impl Iterator<Item = &str> {
        self.themes.iter().map(|t| t.name.as_str())
    }

    /// Переменные темы `theme` (номер в [`Self::themes`]), посчитанные до значений.
    pub fn theme_vars(&self, theme: Option<usize>) -> Result<Vars, UiError> {
        let Some(t) = theme.and_then(|i| self.themes.get(i)) else {
            return Ok(Vars::default());
        };
        let mut done: HashMap<String, Value> = HashMap::new();
        for (name, ..) in &t.vars {
            let mut stack = Vec::new();
            theme_var(t, name, &mut done, &mut stack)?;
        }
        Ok(Vars(Rc::new(done)))
    }
}

/// Считает переменную темы, следя, чтобы переменные не ссылались друг на друга по кругу.
fn theme_var(
    t: &ThemeDecl,
    name: &str,
    done: &mut HashMap<String, Value>,
    stack: &mut Vec<String>,
) -> Result<Option<Value>, UiError> {
    if let Some(v) = done.get(name) {
        return Ok(Some(v.clone()));
    }
    let Some((_, raw, pos)) = t.vars.iter().find(|(n, ..)| n == name) else {
        return Ok(None);
    };
    if stack.iter().any(|s| s == name) {
        stack.push(name.to_owned());
        return Err(UiError::Meaning {
            pos: *pos,
            message: format!(
                "переменные темы {} ссылаются друг на друга по кругу: {}",
                t.name,
                stack.join(" → ")
            ),
        });
    }
    stack.push(name.to_owned());
    let mut lookup = |n: &str, at: Pos| -> Result<Value, UiError> {
        match theme_var(t, n, done, stack)? {
            Some(v) => Ok(v),
            None => Err(missing(n, at, t.vars.iter().map(|(n, ..)| n.as_str()))),
        }
    };
    let v = eval(raw, *pos, &mut lookup)?;
    stack.pop();
    done.insert(name.to_owned(), v.clone());
    Ok(Some(v))
}

fn missing<'a>(name: &str, pos: Pos, known: impl Iterator<Item = &'a str>) -> UiError {
    let known: Vec<&str> = known.collect();
    let hint = match crate::model::suggest(name, &known) {
        Some(s) => format!(" Может, var({s})?"),
        None if known.is_empty() => " В теме нет переменных: подключите тему в head.".into(),
        None => format!(" Есть: {}.", known.join(", ")),
    };
    UiError::Meaning {
        pos,
        message: format!("нет переменной «{name}».{hint}"),
    }
}

/// Переменные, видимые блоку: тема и `.var` предков. Делятся между блоками без копий,
/// пока никто не переопределяет.
#[derive(Clone, Debug, Default)]
pub struct Vars(Rc<HashMap<String, Value>>);

impl Vars {
    /// Копия с переменными `set` поверх.
    pub fn with(&self, set: Vec<(String, Value)>) -> Self {
        if set.is_empty() {
            return self.clone();
        }
        let mut map = (*self.0).clone();
        map.extend(set);
        Self(Rc::new(map))
    }

    /// Подставляет переменные и считает формулы в значении.
    pub fn resolve(&self, v: &Value, pos: Pos) -> Result<Value, UiError> {
        let map = &self.0;
        eval(v, pos, &mut |n, at| {
            map.get(n)
                .cloned()
                .ok_or_else(|| missing(n, at, map.keys().map(String::as_str)))
        })
    }

    /// Модификатор с подставленными переменными, вместе с вложенными (состояния).
    pub fn resolve_modifier(&self, m: &Modifier) -> Result<Modifier, UiError> {
        Ok(Modifier {
            name: m.name.clone(),
            args: self.resolve_args(&m.args)?,
            inner: match &m.inner {
                Some(inner) => Some(
                    inner
                        .iter()
                        .map(|i| self.resolve_modifier(i))
                        .collect::<Result<_, _>>()?,
                ),
                None => None,
            },
            pos: m.pos,
        })
    }

    pub fn resolve_args(&self, args: &[Arg]) -> Result<Vec<Arg>, UiError> {
        args.iter()
            .map(|a| {
                Ok(Arg {
                    name: a.name.clone(),
                    value: self.resolve(&a.value, a.pos)?,
                    pos: a.pos,
                })
            })
            .collect()
    }
}

/// `var(…)` → значение, формулы → число.
fn eval(
    v: &Value,
    pos: Pos,
    lookup: &mut dyn FnMut(&str, Pos) -> Result<Value, UiError>,
) -> Result<Value, UiError> {
    match v {
        Value::Call(f, args) if f == "var" => match &args[..] {
            [
                Arg {
                    name: None,
                    value: Value::Name(n),
                    pos,
                },
            ] => lookup(n, *pos),
            _ => Err(UiError::Meaning {
                pos,
                message: "var: одно имя без кавычек, var(accent)".into(),
            }),
        },
        Value::Call(f, args) => {
            let args = args
                .iter()
                .map(|a| {
                    Ok(Arg {
                        name: a.name.clone(),
                        value: eval(&a.value, a.pos, lookup)?,
                        pos: a.pos,
                    })
                })
                .collect::<Result<_, UiError>>()?;
            Ok(Value::Call(f.clone(), args))
        }
        Value::Op(a, op, b) if matches!(op, '<' | '>' | '≤' | '≥' | '=' | '≠') => {
            // Сравнение `.when(width < var(narrow))`: слева имя окна или данные игры,
            // посчитает его `.when` или [`crate::data::compute`].
            let a = match a.as_ref() {
                Value::Name(_) => a.as_ref().clone(),
                other => eval(other, pos, lookup)?,
            };
            Ok(Value::Op(Box::new(a), *op, Box::new(eval(b, pos, lookup)?)))
        }
        Value::If(c, yes, no) => {
            let c = eval(c, pos, lookup)?;
            if is_live(&c) {
                return Ok(Value::If(
                    Box::new(c),
                    Box::new(eval(yes, pos, lookup)?),
                    Box::new(eval(no, pos, lookup)?),
                ));
            }
            if crate::data::truth(&c, pos)? {
                eval(yes, pos, lookup)
            } else {
                eval(no, pos, lookup)
            }
        }
        Value::Op(a, op, b) => {
            let a = eval(a, pos, lookup)?;
            let b = eval(b, pos, lookup)?;
            if is_live(&a) || is_live(&b) {
                // Данные игры: досчитается, когда они придут.
                return Ok(Value::Op(Box::new(a), *op, Box::new(b)));
            }
            arith(&a, *op, &b).ok_or_else(|| UiError::Meaning {
                pos,
                message: format!(
                    "формула {} {op} {}: считаются числа с числами и проценты с процентами или числами (умножение и деление)",
                    a.kind(),
                    b.kind()
                ),
            })
        }
        other => Ok(other.clone()),
    }
}

fn arith(a: &Value, op: char, b: &Value) -> Option<Value> {
    let f = |x: f32, y: f32| match op {
        '+' => x + y,
        '-' => x - y,
        '*' => x * y,
        _ => {
            if y == 0.0 {
                0.0
            } else {
                x / y
            }
        }
    };
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => Some(Value::Number(f(*x, *y))),
        (Value::Percent(x), Value::Percent(y)) if matches!(op, '+' | '-') => {
            Some(Value::Percent(f(*x, *y)))
        }
        (Value::Percent(x), Value::Number(y)) if matches!(op, '*' | '/') => {
            Some(Value::Percent(f(*x, *y)))
        }
        (Value::Number(x), Value::Percent(y)) if op == '*' => Some(Value::Percent(f(*x, *y))),
        _ => None,
    }
}
