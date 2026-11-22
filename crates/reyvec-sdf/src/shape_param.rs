//! Ручки модели (М.6, `docs/models-3d.md`): числа, которые сцена задаёт каждой копии
//! модели по-своему. Один «конвейер» в сцене стоит длиной 2,4 м, другой — 4 м.
//!
//! Ручка записана в файле модели: имя, значение (под него нарисована модель в файле),
//! пределы и связи. Связь — «число детали меняется на `per` на каждую единицу ручки»:
//!
//! ```text
//! params: [
//!     (name: "длина", value: 3.6, min: 1.2, max: 6.0, links: [
//!         (part: [0], field: "size.x", per: 1.0),
//!         (part: [1], field: "at.x", per: -0.5),
//!     ]),
//! ],
//! ```
//!
//! В сцене: `Use(model: "завод/конвейер", params: {"длина": 2.4})`. При чтении сцены
//! дерево модели копируется и каждое связанное число сдвигается на
//! `per · (значение в сцене − value)`, поэтому шейдер про ручки не знает.
//!
//! Деталь `part` — путь из номеров вложенных деталей, как в «Иерархии» редактора:
//! сдвиг `Move` и цвет `Paint` вокруг детали отдельными деталями не считаются (поля
//! сдвига — `at.x`, `turn.y`, `scale` самой детали). Нет у детали сдвига, а связь
//! двигает её, — сдвиг добавляется. Номера внутри объединения и пересечения — по списку
//! `shapes`, у вырезания 0 — основа, дальше вырезы.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::shape3d::{MAX_REPEAT, Shape3};

/// Ручка модели.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelParam {
    pub name: String,
    /// Значение, под которое нарисована модель в файле.
    pub value: f32,
    pub min: f32,
    pub max: f32,
    #[serde(default)]
    pub links: Vec<Link>,
}

/// Связь ручки с числом детали.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// Путь к детали (номера вложенных, пустой — вся модель).
    pub part: Vec<usize>,
    /// Число детали: `size.x`, `radius`, `at.y`, `count`…
    pub field: String,
    /// На сколько меняется число на единицу ручки.
    pub per: f32,
}

/// Число детали для правки.
pub enum Num<'a> {
    Real(&'a mut f32),
    /// Число копий повтора: целое, от 1 до [`MAX_REPEAT`].
    Count(&'a mut u32),
}

impl Num<'_> {
    /// Значение числом.
    pub fn get(&self) -> f32 {
        match self {
            Self::Real(x) => **x,
            Self::Count(c) => **c as f32,
        }
    }

    /// Сдвинуть на `by` (повтор округляется и остаётся в пределах).
    pub fn add(self, by: f32) {
        match self {
            Self::Real(x) => *x += by,
            Self::Count(c) => {
                *c = (*c as f32 + by).round().clamp(1.0, MAX_REPEAT as f32) as u32;
            }
        }
    }
}

/// Поля сдвига детали.
pub const MOVE_FIELDS: [&str; 7] = [
    "at.x", "at.y", "at.z", "turn.x", "turn.y", "turn.z", "scale",
];

const XYZ: [&str; 3] = ["x", "y", "z"];

/// Число детали, которое можно связать с ручкой, по пути `path`.
pub fn number_mut<'a>(root: &'a mut Shape3, path: &[usize], field: &str) -> Option<Num<'a>> {
    let mut node = root;
    for &i in path {
        node = child_mut(unwrap_mut(node), i)?;
    }
    part_number(node, field)
}

/// Все числа модели, которые можно связать с ручкой: путь детали, поле и значение. У
/// детали без сдвига поля сдвига есть тоже (0 и масштаб 1).
pub fn numbers(shape: &Shape3) -> Vec<(Vec<usize>, &'static str, f32)> {
    fn collect(
        node: &mut Shape3,
        path: &mut Vec<usize>,
        out: &mut Vec<(Vec<usize>, &'static str, f32)>,
    ) {
        for f in MOVE_FIELDS {
            if let Some(n) = part_number(node, f) {
                out.push((path.clone(), f, n.get()));
            }
        }
        let core = unwrap_mut(node);
        for f in own_fields(core) {
            if let Some(n) = own_number(core, f) {
                out.push((path.clone(), f, n.get()));
            }
        }
        for i in 0.. {
            let Some(c) = child_mut(core, i) else {
                break;
            };
            path.push(i);
            collect(c, path, out);
            path.pop();
        }
    }
    let mut copy = shape.clone();
    let mut out = Vec::new();
    collect(&mut copy, &mut Vec::new(), &mut out);
    out
}

/// Проверка ручек модели: имена, пределы и что каждая связь находит своё число.
pub fn check(shape: &Shape3, params: &[ModelParam]) -> Result<(), String> {
    for (i, p) in params.iter().enumerate() {
        if p.name.trim().is_empty() {
            return Err(format!("у ручки номер {} нет имени", i + 1));
        }
        if params[..i].iter().any(|q| q.name == p.name) {
            return Err(format!("ручка «{}» записана дважды", p.name));
        }
        if !(p.min <= p.value && p.value <= p.max) {
            return Err(format!(
                "ручка «{}»: значение {} не между min {} и max {}",
                p.name, p.value, p.min, p.max
            ));
        }
        let mut copy = shape.clone();
        for l in &p.links {
            if number_mut(&mut copy, &l.part, &l.field).is_none() {
                return Err(format!(
                    "ручка «{}»: у детали {:?} нет числа «{}»",
                    p.name, l.part, l.field
                ));
            }
        }
    }
    Ok(())
}

/// Подставляет значения ручек `values` в дерево модели. Значение вне пределов ручки
/// прижимается к пределу; ручки, которой у модели нет, — ошибка.
pub fn apply(
    shape: &mut Shape3,
    params: &[ModelParam],
    values: &BTreeMap<String, f32>,
) -> Result<(), String> {
    for (name, &v) in values {
        let Some(p) = params.iter().find(|p| &p.name == name) else {
            let known: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
            return Err(if known.is_empty() {
                format!("у модели нет ручек, а в сцене задана «{name}»")
            } else {
                format!("у модели нет ручки «{name}», есть: {}", known.join(", "))
            });
        };
        let d = v.clamp(p.min, p.max) - p.value;
        if d == 0.0 {
            continue;
        }
        for l in &p.links {
            let n = number_mut(shape, &l.part, &l.field).ok_or_else(|| {
                format!(
                    "ручка «{name}»: у детали {:?} нет числа «{}»",
                    l.part, l.field
                )
            })?;
            n.add(l.per * d);
        }
    }
    Ok(())
}

/// Число детали `node`: поле её сдвига (сдвига нет — он добавляется) или её собственное.
fn part_number<'a>(node: &'a mut Shape3, field: &str) -> Option<Num<'a>> {
    if MOVE_FIELDS.contains(&field) {
        let Shape3::Move {
            at, turn, scale, ..
        } = folded_move(node, false)
        else {
            return None;
        };
        return Some(match field {
            "scale" => Num::Real(scale),
            f => {
                let (v, axis) = f.split_once('.')?;
                let i = XYZ.iter().position(|a| *a == axis)?;
                Num::Real(if v == "at" { &mut at[i] } else { &mut turn[i] })
            }
        });
    }
    own_number(unwrap_mut(node), field)
}

/// Сдвиг `Move` вокруг детали (под её цветом `Paint`, если он снаружи). Сдвига нет —
/// деталь оборачивается в пустой сдвиг.
fn folded_move(node: &mut Shape3, painted: bool) -> &mut Shape3 {
    if !painted && matches!(node, Shape3::Paint { .. }) {
        let Shape3::Paint { shape, .. } = node else {
            unreachable!()
        };
        return folded_move(shape, true);
    }
    if !matches!(node, Shape3::Move { .. }) {
        let core = std::mem::replace(node, Shape3::Sphere { radius: 0.0 });
        *node = Shape3::Move {
            at: [0.0; 3],
            turn: [0.0; 3],
            scale: 1.0,
            shape: Box::new(core),
        };
    }
    node
}

/// Поля самой детали (без сдвига), которые можно связать с ручкой.
fn own_fields(core: &Shape3) -> &'static [&'static str] {
    match core {
        Shape3::Sphere { .. } => &["radius"],
        Shape3::Box { .. } => &["size.x", "size.y", "size.z", "round"],
        Shape3::Capsule { .. } => &["height", "radius"],
        Shape3::Cylinder { .. } => &["height", "radius", "round"],
        Shape3::Torus { .. } => &["radius", "thickness"],
        Shape3::Cone { .. } => &["height", "bottom", "top", "round"],
        Shape3::Plane { .. } => &["offset"],
        Shape3::Extrude { .. } => &["depth", "round"],
        Shape3::Revolve { .. } => &["offset"],
        Shape3::Union { .. }
        | Shape3::Subtract { .. }
        | Shape3::Intersect { .. }
        | Shape3::Mirror { .. } => &["smooth"],
        Shape3::Repeat { .. } => &["count", "step.x", "step.y", "step.z", "smooth"],
        _ => &[],
    }
}

fn own_number<'a>(core: &'a mut Shape3, field: &str) -> Option<Num<'a>> {
    let axis = |f: &str, name: &str| {
        f.strip_prefix(name)
            .and_then(|a| a.strip_prefix('.'))
            .and_then(|a| XYZ.iter().position(|x| *x == a))
    };
    let r = match (core, field) {
        (Shape3::Sphere { radius }, "radius")
        | (Shape3::Capsule { radius, .. }, "radius")
        | (Shape3::Cylinder { radius, .. }, "radius")
        | (Shape3::Torus { radius, .. }, "radius") => radius,
        (Shape3::Box { round, .. }, "round")
        | (Shape3::Cylinder { round, .. }, "round")
        | (Shape3::Cone { round, .. }, "round")
        | (Shape3::Extrude { round, .. }, "round") => round,
        (Shape3::Capsule { height, .. }, "height")
        | (Shape3::Cylinder { height, .. }, "height")
        | (Shape3::Cone { height, .. }, "height") => height,
        (Shape3::Torus { thickness, .. }, "thickness") => thickness,
        (Shape3::Cone { bottom, .. }, "bottom") => bottom,
        (Shape3::Cone { top, .. }, "top") => top,
        (Shape3::Plane { offset, .. }, "offset") | (Shape3::Revolve { offset, .. }, "offset") => {
            offset
        }
        (Shape3::Extrude { depth, .. }, "depth") => depth,
        (Shape3::Union { smooth, .. }, "smooth")
        | (Shape3::Subtract { smooth, .. }, "smooth")
        | (Shape3::Intersect { smooth, .. }, "smooth")
        | (Shape3::Mirror { smooth, .. }, "smooth")
        | (Shape3::Repeat { smooth, .. }, "smooth") => smooth,
        (Shape3::Repeat { count, .. }, "count") => return Some(Num::Count(count)),
        (Shape3::Box { size, .. }, f) => &mut size[axis(f, "size")?],
        (Shape3::Repeat { step, .. }, f) => &mut step[axis(f, "step")?],
        _ => return None,
    };
    Some(Num::Real(r))
}

/// Деталь без своего сдвига и цвета (не больше одного каждого, как в «Иерархии»).
fn unwrap_mut(node: &mut Shape3) -> &mut Shape3 {
    fn go(node: &mut Shape3, moved: bool, painted: bool) -> &mut Shape3 {
        let what = match node {
            Shape3::Move { .. } if !moved => 1,
            Shape3::Paint { .. } if !painted => 2,
            _ => 0,
        };
        match (what, node) {
            (1, Shape3::Move { shape, .. }) => go(shape, true, painted),
            (2, Shape3::Paint { shape, .. }) => go(shape, moved, true),
            (_, n) => n,
        }
    }
    go(node, false, false)
}

/// Вложенная деталь номер `i`.
fn child_mut(node: &mut Shape3, i: usize) -> Option<&mut Shape3> {
    match node {
        Shape3::Union { shapes, .. } | Shape3::Intersect { shapes, .. } => shapes.get_mut(i),
        Shape3::Subtract { base, cut, .. } => {
            if i == 0 {
                Some(base)
            } else {
                cut.get_mut(i - 1)
            }
        }
        Shape3::Move { shape, .. }
        | Shape3::Paint { shape, .. }
        | Shape3::Mirror { shape, .. }
        | Shape3::Repeat { shape, .. }
        | Shape3::Animate { shape, .. }
            if i == 0 =>
        {
            Some(shape)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conveyor() -> Shape3 {
        ron::from_str(
            "Union(shapes: [Box(size: (3.6, 0.12, 0.8)), \
             Move(at: (-1.6, -0.33, 0), shape: Box(size: (0.08, 0.6, 0.08))), \
             Paint(material: 0, shape: Box(size: (0.08, 0.6, 0.08))), \
             Repeat(count: 3, step: (0.5, 0, 0), shape: Sphere(radius: 0.1))])",
        )
        .unwrap()
    }

    fn link(part: &[usize], field: &str, per: f32) -> Link {
        Link {
            part: part.to_vec(),
            field: field.into(),
            per,
        }
    }

    #[test]
    fn links_shift_numbers_and_add_moves() {
        let mut s = conveyor();
        let params = vec![ModelParam {
            name: "длина".into(),
            value: 3.6,
            min: 1.2,
            max: 6.0,
            links: vec![
                link(&[0], "size.x", 1.0),
                link(&[1], "at.x", -0.5),
                link(&[2], "at.x", 0.5),
                link(&[3], "count", 1.0),
            ],
        }];
        check(&s, &params).unwrap();
        let values = BTreeMap::from([("длина".to_string(), 5.6)]);
        apply(&mut s, &params, &values).unwrap();
        let get = |s: &Shape3, p: &[usize], f: &str| {
            numbers(s)
                .into_iter()
                .find(|(q, g, _)| q == p && *g == f)
                .map(|x| x.2)
                .unwrap()
        };
        assert!((get(&s, &[0], "size.x") - 5.6).abs() < 1e-5);
        assert!((get(&s, &[1], "at.x") + 2.6).abs() < 1e-5);
        assert!((get(&s, &[1], "at.y") + 0.33).abs() < 1e-5);
        assert!((get(&s, &[2], "at.x") - 1.0).abs() < 1e-5);
        assert_eq!(get(&s, &[3], "count"), 5.0);
        // Сдвиг добавился под цветом: цвет детали остался снаружи.
        let Shape3::Union { shapes, .. } = &s else {
            panic!()
        };
        assert!(
            matches!(&shapes[2], Shape3::Paint { shape, .. } if matches!(**shape, Shape3::Move { .. }))
        );
        // Вне пределов — прижимается к max.
        let mut t = conveyor();
        apply(&mut t, &params, &BTreeMap::from([("длина".into(), 100.0)])).unwrap();
        assert!((get(&t, &[0], "size.x") - 6.0).abs() < 1e-5);
    }

    #[test]
    fn bad_params_are_reported() {
        let s = conveyor();
        let p = |name: &str, value: f32, links: Vec<Link>| ModelParam {
            name: name.into(),
            value,
            min: 0.0,
            max: 1.0,
            links,
        };
        let e = check(&s, &[p("а", 2.0, vec![])]).unwrap_err();
        assert!(e.contains("не между"), "{e}");
        let e = check(&s, &[p("а", 0.5, vec![]), p("а", 0.5, vec![])]).unwrap_err();
        assert!(e.contains("дважды"), "{e}");
        let e = check(&s, &[p("а", 0.5, vec![link(&[0], "radius", 1.0)])]).unwrap_err();
        assert!(e.contains("нет числа «radius»"), "{e}");
        let e = check(&s, &[p("а", 0.5, vec![link(&[9], "at.x", 1.0)])]).unwrap_err();
        assert!(e.contains("[9]"), "{e}");
        let mut t = s.clone();
        let e = apply(
            &mut t,
            &[p("а", 0.5, vec![])],
            &BTreeMap::from([("б".into(), 1.0)]),
        )
        .unwrap_err();
        assert!(e.contains("нет ручки «б», есть: а"), "{e}");
    }
}
