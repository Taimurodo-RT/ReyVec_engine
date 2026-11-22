//! Ручки модели в тексте её файла (М.6, `reyvec_sdf::shape_param`): прочитать и
//! записать список `params`, нарисовать модель при другом значении ручки и «научить»
//! ручку.
//!
//! Обучение: ручку ставят на другое значение, а детали двигают и растягивают как
//! обычно, пока модель не станет такой, какой должна быть при этом значении. «Запомнить»
//! сравнивает числа деталей с тем, что было до обучения: каждое изменившееся число
//! становится связью `per = изменение / (новое значение − значение в файле)`, а сами
//! числа в тексте возвращаются к прежним (модель в файле — при значении из файла).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use reyvec_sdf::Shape3;
use reyvec_sdf::shape_model::ModelFile;
use reyvec_sdf::shape_param::{self, Link, MOVE_FIELDS, ModelParam};

use super::ron_edit::{EditError, fields, num, remove_field, skip_ws};
use super::shape_edit::{self, Part};

/// Меньше этого изменение числа при обучении не считается.
const EPS: f32 = 1e-5;

/// Ручки модели из текста (не читается — пусто).
pub(crate) fn read(text: &str) -> Vec<ModelParam> {
    ModelFile::parse(text).map(|f| f.params).unwrap_or_default()
}

/// Ручки текстом для файла.
fn format(params: &[ModelParam]) -> String {
    let mut out = String::from("[\n");
    for p in params {
        let _ = write!(
            out,
            "        (name: {}, value: {}, min: {}, max: {}, links: [",
            shape_edit::quoted(&p.name),
            num(f64::from(p.value)),
            num(f64::from(p.min)),
            num(f64::from(p.max)),
        );
        if p.links.is_empty() {
            out.push_str("]),\n");
            continue;
        }
        out.push('\n');
        for l in &p.links {
            let part: Vec<String> = l.part.iter().map(usize::to_string).collect();
            let _ = writeln!(
                out,
                "            (part: [{}], field: {}, per: {}),",
                part.join(", "),
                shape_edit::quoted(&l.field),
                per(l.per),
            );
        }
        out.push_str("        ]),\n");
    }
    out.push_str("    ]");
    out
}

/// Связь точнее, чем числа деталей: до миллионной.
fn per(x: f32) -> String {
    let r = (f64::from(x) * 1e6).round() / 1e6;
    if r == r.trunc() {
        format!("{r:.1}")
    } else {
        format!("{r}")
    }
}

/// Записать ручки в текст: поле `params` заменяется, а нет его — встаёт перед `shape`.
/// Пустой список убирает поле.
pub(crate) fn write(text: &str, params: &[ModelParam]) -> Result<String, EditError> {
    if params.is_empty() {
        return remove_field(text, &[], "params");
    }
    let s = text.as_bytes();
    let root = skip_ws(s, 0);
    let err = || EditError("не разобрать поля модели".into());
    let fs = fields(s, root).ok_or_else(err)?;
    let value = format(params);
    if let Some(f) = fs.iter().find(|f| f.name == "params") {
        return Ok(format!(
            "{}{value}{}",
            &text[..f.value.start],
            &text[f.value.end..]
        ));
    }
    let shape = fs.iter().find(|f| f.name == "shape").ok_or_else(err)?;
    Ok(format!(
        "{}params: {value},\n    {}",
        &text[..shape.name_start],
        &text[shape.name_start..]
    ))
}

/// Записать число детали `path` (поле как в связи: `size.x`, `at.y`, `radius`).
pub(crate) fn set_number(
    text: &str,
    path: &[usize],
    field: &str,
    value: f32,
) -> Result<String, EditError> {
    let root = shape_edit::parse(text)?;
    let part = root
        .at(path)
        .ok_or_else(|| EditError(format!("нет детали {path:?}")))?;
    let v = f64::from(value);
    let axis = |a: &str| ["x", "y", "z"].iter().position(|x| *x == a);
    if MOVE_FIELDS.contains(&field) {
        if field == "scale" {
            return shape_edit::set_moved(text, part, "scale", &num(v));
        }
        let (name, a) = field.split_once('.').unwrap_or((field, "x"));
        let mut xyz = shape_edit::read_vec3(text, part.moved, name, [0.0; 3]);
        xyz[axis(a).unwrap_or(0)] = v;
        return shape_edit::set_moved(text, part, name, &shape_edit::triple(xyz));
    }
    if let Some((name, a)) = field.split_once('.') {
        let mut xyz = shape_edit::read_vec3(text, part.open, name, [0.0; 3]);
        xyz[axis(a).unwrap_or(0)] = v;
        return shape_edit::set(text, part, name, &shape_edit::triple(xyz));
    }
    let value = if field == "count" {
        (value.round().max(1.0) as u32).to_string()
    } else {
        num(v)
    };
    shape_edit::set(text, part, field, &value)
}

type Numbers = BTreeMap<(Vec<usize>, &'static str), f32>;

fn numbers(shape: &Shape3) -> Numbers {
    shape_param::numbers(shape)
        .into_iter()
        .map(|(p, f, v)| ((p, f), v))
        .collect()
}

/// Текст, где числа `from` заменены на числа `to` там, где они разные.
fn write_numbers(text: &str, from: &Numbers, to: &Numbers) -> Result<String, EditError> {
    let mut out = text.to_owned();
    for (key, &v) in to {
        if from.get(key).is_some_and(|&old| (old - v).abs() > EPS) {
            out = set_number(&out, &key.0, key.1, v)?;
        }
    }
    Ok(out)
}

/// Текст модели при значениях ручек `values` (числа деталей сдвинуты по связям).
pub(crate) fn applied(text: &str, values: &BTreeMap<String, f32>) -> Result<String, String> {
    let file = ModelFile::parse(text)?;
    let mut shape = file.shape.clone();
    shape_param::apply(&mut shape, &file.params, values)?;
    write_numbers(text, &numbers(&file.shape), &numbers(&shape)).map_err(|e| e.0)
}

/// Одинаково ли устроены модели: те же детали тех же видов на тех же местах.
fn same_parts(a: &Part, b: &Part) -> bool {
    a.kind == b.kind
        && a.children.len() == b.children.len()
        && a.children
            .iter()
            .zip(&b.children)
            .all(|(x, y)| same_parts(x, y))
}

/// Итог обучения: новый текст и сколько чисел связалось.
pub(crate) struct Learned {
    pub text: String,
    pub links: usize,
}

/// Запомнить обучение ручки `name`: `before` — текст до обучения, `now` — после правок
/// при значении `target`.
pub(crate) fn learn(before: &str, now: &str, name: &str, target: f32) -> Result<Learned, String> {
    let a = ModelFile::parse(before)?;
    let b = ModelFile::parse(now)?;
    let (pa, pb) = (
        shape_edit::parse(before).map_err(|e| e.0)?,
        shape_edit::parse(now).map_err(|e| e.0)?,
    );
    if !same_parts(&pa, &pb) {
        return Err(
            "во время обучения детали добавили или убрали: так не запомнить. «Отмена» вернёт \
             модель, как была"
                .into(),
        );
    }
    let mut params = b.params.clone();
    let p = params
        .iter_mut()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("ручки «{name}» больше нет"))?;
    let dv = target - p.value;
    if dv.abs() < EPS {
        return Err("ручка стоит на значении из файла: поставьте другое".into());
    }
    let (na, nb) = (numbers(&a.shape), numbers(&b.shape));
    p.links = nb
        .iter()
        .filter_map(|(key, &v)| {
            let old = *na.get(key)?;
            ((v - old).abs() > EPS).then(|| Link {
                part: key.0.clone(),
                field: key.1.to_string(),
                per: (v - old) / dv,
            })
        })
        .collect();
    let links = p.links.len();
    let text = write_numbers(now, &nb, &na).map_err(|e| e.0)?;
    let text = write(&text, &params).map_err(|e| e.0)?;
    ModelFile::parse(&text).map_err(|e| format!("после обучения модель не читается: {e}"))?;
    Ok(Learned { text, links })
}

/// Новая ручка с именем, которого ещё нет.
pub(crate) fn new_param(params: &[ModelParam]) -> ModelParam {
    let name = (1..)
        .map(|i| format!("ручка {i}"))
        .find(|n| params.iter().all(|p| &p.name != n))
        .unwrap_or_default();
    ModelParam {
        name,
        value: 1.0,
        min: 0.5,
        max: 2.0,
        links: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RACK: &str = r#"// Стеллаж.
(
    format: "reyvec-shape",
    version: 1,
    shape: Union(shapes: [
        Move(at: (-0.95, 0.00, 0.00), shape: Box(size: (0.06, 2.00, 0.80))),
        Move(at: (0.95, 0.00, 0.00), shape: Box(size: (0.06, 2.00, 0.80))),
        // Полка.
        Box(size: (1.96, 0.05, 0.80)),
    ]),
)
"#;

    #[test]
    fn params_are_written_before_shape_and_read_back() {
        let mut p = new_param(&[]);
        p.name = "длина".into();
        let t = write(RACK, &[p.clone()]).unwrap();
        assert!(t.contains("params: [\n        (name: \"длина\""), "{t}");
        assert!(t.find("params").unwrap() < t.find("shape:").unwrap());
        assert_eq!(read(&t), vec![p.clone()]);
        let mut q = p.clone();
        q.max = 3.0;
        let t2 = write(&t, &[q.clone(), new_param(&[q.clone()])]).unwrap();
        let back = read(&t2);
        assert_eq!(back[0], q);
        assert_eq!(back[1].name, "ручка 1");
        let empty = write(&t2, &[]).unwrap();
        assert!(!empty.contains("params"), "{empty}");
        assert!(empty.contains("// Полка."));
    }

    #[test]
    fn learning_turns_edits_into_links() {
        let p = ModelParam {
            name: "длина".into(),
            value: 1.96,
            min: 1.0,
            max: 4.0,
            links: Vec::new(),
        };
        let before = write(RACK, &[p]).unwrap();
        // При длине 2.96: стойки разошлись на 0.5, полка длиннее на 1, у полки появился
        // сдвиг вверх (у неё не было Move).
        let mut now = set_number(&before, &[0], "at.x", -1.45).unwrap();
        now = set_number(&now, &[1], "at.x", 1.45).unwrap();
        now = set_number(&now, &[2], "size.x", 2.96).unwrap();
        now = set_number(&now, &[2], "at.y", 0.1).unwrap();
        let l = learn(&before, &now, "длина", 2.96).unwrap();
        assert_eq!(l.links, 4);
        assert!(l.text.contains("// Полка."), "комментарии на месте");
        // Числа в файле вернулись к длине из файла.
        let file = ModelFile::parse(&l.text).unwrap();
        let base = ModelFile::parse(&before).unwrap();
        let close = |a: &Shape3, b: &Shape3| {
            let pts = [
                glam::Vec3::new(1.2, 0.0, 0.0),
                glam::Vec3::new(-0.95, 0.5, 0.0),
                glam::Vec3::ZERO,
            ];
            pts.iter()
                .all(|&q| (a.distance(q) - b.distance(q)).abs() < 1e-4)
        };
        assert!(close(&file.shape, &base.shape));
        // При длине 2.96 модель — как после правок.
        let at = applied(&l.text, &BTreeMap::from([("длина".into(), 2.96)])).unwrap();
        let want = ModelFile::parse(&now).unwrap();
        assert!(close(&ModelFile::parse(&at).unwrap().shape, &want.shape));
        let links = &file.params[0].links;
        let stand = links
            .iter()
            .find(|l| l.part == [1] && l.field == "at.x")
            .unwrap();
        assert!((stand.per - 0.5).abs() < 1e-4, "{stand:?}");
        // Новая деталь во время обучения — ошибка.
        let item = shape_edit::new_part("Sphere", 0.2, [0.0; 3]);
        let root = shape_edit::parse(&now).unwrap();
        let more = shape_edit::add(&now, &root, &item).unwrap();
        assert!(learn(&before, &more, "длина", 2.96).is_err());
    }
}
