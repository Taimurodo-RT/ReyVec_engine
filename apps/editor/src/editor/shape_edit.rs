//! Части модели формы в тексте её файла (М.3): дерево частей для панели «Иерархия»,
//! числа частей для «Инспектора» и правки текста — так же точечно, как правки сцены
//! (`ron_edit`): меняется только нужное число, комментарии и отступы остаются.
//!
//! Сдвиг `Move` и цвет `Paint` вокруг детали в дереве не показываются отдельными
//! строками: это «положение» и «цвет» самой детали в инспекторе. Нет сдвига, а положение
//! поменяли — деталь оборачивается в `Move(at: …, shape: …)`.

use std::ops::Range;

use super::ron_edit::{
    EditError, fields, insert_item_at, items, num, remove_span, set_field_at, skip_ws, value_end,
};

/// Часть модели в тексте файла.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Part {
    /// Вид части, как в файле: `Box`, `Union`, `Mirror`…
    pub kind: String,
    /// Скобка `(` самой части (`None` — части без скобок).
    pub open: Option<usize>,
    /// Вся часть с её сдвигом и цветом (что убирается и во что оборачивается).
    pub outer: Range<usize>,
    /// Скобка `(` сдвига `Move` вокруг части.
    pub moved: Option<usize>,
    /// Скобка `(` цвета `Paint` вокруг части.
    pub painted: Option<usize>,
    /// Часть — элемент списка (её можно убрать).
    pub removable: bool,
    /// Скобка `[` списка, куда добавляются новые детали (у объединения и пересечения —
    /// `shapes`, у вырезания — `cut`).
    pub list: Option<usize>,
    pub children: Vec<Part>,
}

impl Part {
    /// Часть по пути из номеров вложенных.
    pub(crate) fn at(&self, path: &[usize]) -> Option<&Part> {
        path.iter().try_fold(self, |p, &i| p.children.get(i))
    }
}

/// Имя для людей.
pub(crate) fn label(kind: &str) -> &str {
    match kind {
        "Box" => "Коробка",
        "Sphere" => "Шар",
        "Cylinder" => "Цилиндр",
        "Capsule" => "Капсула",
        "Torus" => "Тор",
        "Cone" => "Конус",
        "Plane" => "Плоскость",
        "Union" => "Объединение",
        "Subtract" => "Вырезание",
        "Intersect" => "Пересечение",
        "Mirror" => "Зеркало",
        "Repeat" => "Повтор",
        "Move" => "Сдвиг",
        "Paint" => "Цвет",
        "Use" => "Модель",
        "Extrude" => "Выдавливание",
        "Revolve" => "Выточка",
        "Animate" => "Движение",
        other => other,
    }
}

/// Какое число у части.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Param {
    /// Одно число и его значение по умолчанию.
    Num(f64),
    /// Три числа.
    Vec3([f64; 3]),
    /// Целое (сколько копий).
    Count(u32),
    /// Ось зеркала.
    Axis,
}

/// Числа части `kind`: поле в файле, подпись, вид.
pub(crate) fn params(kind: &str) -> &'static [(&'static str, &'static str, Param)] {
    use Param::*;
    match kind {
        "Sphere" => &[("radius", "Радиус", Num(0.5))],
        "Box" => &[
            ("size", "Размер", Vec3([1.0; 3])),
            ("round", "Скругление", Num(0.0)),
        ],
        "Capsule" => &[
            ("height", "Высота", Num(1.0)),
            ("radius", "Радиус", Num(0.25)),
        ],
        "Cylinder" => &[
            ("height", "Высота", Num(1.0)),
            ("radius", "Радиус", Num(0.5)),
            ("round", "Скругление", Num(0.0)),
        ],
        "Torus" => &[
            ("radius", "Радиус", Num(0.5)),
            ("thickness", "Толщина", Num(0.1)),
        ],
        "Cone" => &[
            ("height", "Высота", Num(1.0)),
            ("bottom", "Радиус внизу", Num(0.5)),
            ("top", "Радиус вверху", Num(0.0)),
            ("round", "Скругление", Num(0.0)),
        ],
        "Union" | "Subtract" | "Intersect" => &[("smooth", "Плавный шов", Num(0.0))],
        "Mirror" => &[("axis", "Ось", Axis), ("smooth", "Плавный шов", Num(0.0))],
        "Repeat" => &[
            ("count", "Копий", Count(2)),
            ("step", "Шаг", Vec3([1.0, 0.0, 0.0])),
            ("smooth", "Плавный шов", Num(0.0)),
        ],
        "Move" => &[
            ("at", "Положение", Vec3([0.0; 3])),
            ("turn", "Поворот, °", Vec3([0.0; 3])),
            ("scale", "Масштаб", Num(1.0)),
        ],
        _ => &[],
    }
}

/// Имя (`Box`) с позиции `at` и скобка `(` за ним.
fn head(s: &[u8], at: usize) -> Option<(String, Option<usize>)> {
    let mut i = at;
    while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'_') {
        i += 1;
    }
    if i == at {
        return None;
    }
    let name = String::from_utf8_lossy(&s[at..i]).into_owned();
    let j = skip_ws(s, i);
    Some((name, (s.get(j) == Some(&b'(')).then_some(j)))
}

/// Начало значения поля `name` в скобках `open`.
fn field_start(s: &[u8], open: usize, name: &str) -> Option<usize> {
    fields(s, open)?
        .into_iter()
        .find(|f| f.name == name)
        .map(|f| f.value.start)
}

/// Часть, записанная с позиции `start`.
fn part(s: &[u8], start: usize, removable: bool, depth: usize) -> Option<Part> {
    if depth > 64 {
        return None;
    }
    let outer = start..value_end(s, start)?;
    let (mut moved, mut painted) = (None, None);
    let mut at = start;
    let (kind, open) = loop {
        let (name, open) = head(s, at)?;
        let wrapper = match name.as_str() {
            "Move" if moved.is_none() => &mut moved,
            "Paint" if painted.is_none() => &mut painted,
            _ => break (name, open),
        };
        let Some(o) = open else {
            break (name, open);
        };
        let Some(inner) = field_start(s, o, "shape") else {
            break (name, open);
        };
        *wrapper = Some(o);
        at = inner;
    };
    let mut children = Vec::new();
    let mut list = None;
    if let Some(o) = open {
        for f in fields(s, o)? {
            match f.name.as_str() {
                "shape" | "base" => children.push(part(s, f.value.start, false, depth + 1)?),
                "shapes" | "cut" if s.get(f.value.start) == Some(&b'[') => {
                    list = Some(f.value.start);
                    for it in items(s, f.value.start)? {
                        children.push(part(s, it, true, depth + 1)?);
                    }
                }
                _ => {}
            }
        }
    }
    Some(Part {
        kind,
        open,
        outer,
        moved,
        painted,
        removable,
        list,
        children,
    })
}

/// Скобка `(` всего файла.
fn root_open(s: &[u8]) -> Option<usize> {
    let i = skip_ws(s, 0);
    (s.get(i) == Some(&b'(')).then_some(i)
}

/// Дерево частей модели из текста файла.
pub(crate) fn parse(text: &str) -> Result<Part, EditError> {
    let s = text.as_bytes();
    let err = |m: &str| EditError(m.into());
    let root = root_open(s).ok_or_else(|| err("файл модели не начинается со скобки"))?;
    let start = field_start(s, root, "shape").ok_or_else(|| err("в модели нет поля shape"))?;
    part(s, start, false, 0).ok_or_else(|| err("не разобрать форму модели"))
}

/// Имена материалов модели по порядку (номер в `Paint` — номер здесь).
pub(crate) fn materials(text: &str) -> Vec<String> {
    let s = text.as_bytes();
    let Some(list) = root_open(s).and_then(|r| field_start(s, r, "materials")) else {
        return Vec::new();
    };
    if s.get(list) != Some(&b'[') {
        return Vec::new();
    }
    items(s, list)
        .unwrap_or_default()
        .into_iter()
        .map(|it| {
            (s.get(it) == Some(&b'('))
                .then(|| field_start(s, it, "name"))
                .flatten()
                .and_then(|v| ron::from_str::<String>(&text[v..value_end(s, v)?]).ok())
                .unwrap_or_else(|| "?".into())
        })
        .collect()
}

/// Материалы модели текстом, как они записаны в файле (для сцены подставки).
pub(crate) fn material_items(text: &str) -> Vec<String> {
    let s = text.as_bytes();
    let Some(list) = root_open(s).and_then(|r| field_start(s, r, "materials")) else {
        return Vec::new();
    };
    if s.get(list) != Some(&b'[') {
        return Vec::new();
    }
    items(s, list)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|it| Some(text[it..value_end(s, it)?].to_owned()))
        .collect()
}

/// Текст значения поля `name` в скобках `open`.
fn field_text<'a>(text: &'a str, open: usize, name: &str) -> Option<&'a str> {
    let s = text.as_bytes();
    let f = fields(s, open)?.into_iter().find(|f| f.name == name)?;
    Some(&text[f.value])
}

/// Число поля (нет поля или не число — `default`).
pub(crate) fn read_num(text: &str, open: Option<usize>, name: &str, default: f64) -> f64 {
    open.and_then(|o| field_text(text, o, name))
        .and_then(|v| ron::from_str::<f64>(v).ok())
        .unwrap_or(default)
}

/// Три числа поля.
pub(crate) fn read_vec3(
    text: &str,
    open: Option<usize>,
    name: &str,
    default: [f64; 3],
) -> [f64; 3] {
    open.and_then(|o| field_text(text, o, name))
        .and_then(|v| ron::from_str::<(f64, f64, f64)>(v).ok())
        .map_or(default, |(x, y, z)| [x, y, z])
}

/// Имя-значение поля (ось `X`).
pub(crate) fn read_word(text: &str, open: Option<usize>, name: &str) -> Option<String> {
    open.and_then(|o| field_text(text, o, name))
        .map(|v| v.trim().to_owned())
}

/// Три числа для файла: `(x, y, z)`.
pub(crate) fn triple(v: [f64; 3]) -> String {
    format!("({}, {}, {})", num(v[0]), num(v[1]), num(v[2]))
}

/// Новое значение поля части (в её собственных скобках).
pub(crate) fn set(text: &str, part: &Part, field: &str, value: &str) -> Result<String, EditError> {
    let open = part
        .open
        .ok_or_else(|| EditError("у части нет скобок".into()))?;
    set_field_at(text, open, field, value)
}

/// Поле сдвига детали (`at`, `turn`, `scale`); сдвига нет — деталь в него оборачивается.
pub(crate) fn set_moved(
    text: &str,
    part: &Part,
    field: &str,
    value: &str,
) -> Result<String, EditError> {
    match part.moved {
        Some(open) => set_field_at(text, open, field, value),
        None => Ok(wrap(
            text,
            part,
            &format!("Move({field}: {value}, shape: "),
            ")",
        )),
    }
}

/// Цвет детали — материал модели номер `material`; `None` — убрать свой цвет (деталь
/// берёт цвет снаружи).
pub(crate) fn set_paint(
    text: &str,
    part: &Part,
    material: Option<usize>,
) -> Result<String, EditError> {
    match (part.painted, material) {
        (Some(open), Some(m)) => set_field_at(text, open, "material", &m.to_string()),
        (None, Some(m)) => Ok(wrap(
            text,
            part,
            &format!("Paint(material: {m}, shape: "),
            ")",
        )),
        (Some(open), None) => {
            // Paint(material: n, shape: X) → X.
            let s = text.as_bytes();
            let inner = field_start(s, open, "shape")
                .ok_or_else(|| EditError("у цвета нет shape".into()))?;
            let inner_end = value_end(s, inner).ok_or_else(|| EditError("не разобрать".into()))?;
            let paint_start = text[..open].trim_end().len() - "Paint".len();
            let paint_end =
                value_end(s, paint_start).ok_or_else(|| EditError("не разобрать цвет".into()))?;
            Ok(format!(
                "{}{}{}",
                &text[..paint_start],
                &text[inner..inner_end],
                &text[paint_end..]
            ))
        }
        (None, None) => Ok(text.to_owned()),
    }
}

/// Оборачивает часть целиком (со сдвигом и цветом): `before` + часть + `after`.
pub(crate) fn wrap(text: &str, part: &Part, before: &str, after: &str) -> String {
    format!(
        "{}{before}{}{after}{}",
        &text[..part.outer.start],
        &text[part.outer.clone()],
        &text[part.outer.end..]
    )
}

/// Убирает деталь из списка.
pub(crate) fn remove(text: &str, part: &Part) -> Result<String, EditError> {
    if !part.removable {
        return Err(EditError(
            "эту часть не убрать: она не в списке деталей".into(),
        ));
    }
    Ok(remove_span(text, part.outer.start, part.outer.end))
}

/// Добавляет деталь `item` в список части (в конец).
pub(crate) fn add(text: &str, part: &Part, item: &str) -> Result<String, EditError> {
    let list = part
        .list
        .ok_or_else(|| EditError("у части нет списка деталей".into()))?;
    insert_item_at(text, list, usize::MAX, item)
}

/// Копия детали сразу за ней в том же списке (с её сдвигом и цветом). Деталь на своей
/// строке — копия тоже на своей, с тем же отступом.
pub(crate) fn duplicate(text: &str, part: &Part) -> Result<String, EditError> {
    if !part.removable {
        return Err(EditError(
            "эту часть не повторить: она не в списке деталей".into(),
        ));
    }
    let (start, end) = (part.outer.start, part.outer.end);
    let line = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line..start];
    let sep = if indent.trim().is_empty() {
        format!(",\n{indent}")
    } else {
        ", ".to_owned()
    };
    Ok(format!(
        "{}{sep}{}{}",
        &text[..end],
        &text[start..end],
        &text[end..]
    ))
}

/// Меняет местами две детали одного списка (`a` раньше `b` в тексте).
pub(crate) fn swap(text: &str, a: &Part, b: &Part) -> Result<String, EditError> {
    if !(a.removable && b.removable) || a.outer.end > b.outer.start {
        return Err(EditError("детали не переставить".into()));
    }
    Ok(format!(
        "{}{}{}{}{}",
        &text[..a.outer.start],
        &text[b.outer.clone()],
        &text[a.outer.end..b.outer.start],
        &text[a.outer.clone()],
        &text[b.outer.end..]
    ))
}

/// Во что обернуть деталь.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Wrap {
    /// Объединение из одной детали (дальше в него добавляют).
    Union,
    /// Зеркало через плоскость YZ: деталь и её отражение.
    Mirror,
    /// Две копии со сдвигом `step` по X.
    Repeat(f64),
}

/// Оборачивает деталь (со сдвигом и цветом) в объединение, зеркало или повтор.
pub(crate) fn wrap_in(text: &str, part: &Part, how: Wrap) -> String {
    match how {
        Wrap::Union => wrap(text, part, "Union(shapes: [", "])"),
        Wrap::Mirror => wrap(text, part, "Mirror(axis: X, shape: ", ")"),
        Wrap::Repeat(step) => wrap(
            text,
            part,
            &format!("Repeat(count: 2, step: ({}, 0.0, 0.0), shape: ", num(step)),
            ")",
        ),
    }
}

/// У части одна вложенная, и обёртку можно снять.
pub(crate) fn can_unwrap(part: &Part) -> bool {
    part.children.len() == 1
        && matches!(
            part.kind.as_str(),
            "Union" | "Intersect" | "Subtract" | "Mirror" | "Repeat" | "Move" | "Paint" | "Animate"
        )
}

/// Снимает обёртку: на месте части остаётся её единственная вложенная (сдвиг и цвет
/// самой части остаются).
pub(crate) fn unwrap(text: &str, part: &Part) -> Result<String, EditError> {
    let err = || EditError("обёртку не снять".into());
    let (true, Some(open)) = (can_unwrap(part), part.open) else {
        return Err(err());
    };
    let head = text[..open].trim_end();
    let start = head.len().checked_sub(part.kind.len()).ok_or_else(err)?;
    if head[start..] != part.kind {
        return Err(err());
    }
    let end = value_end(text.as_bytes(), start).ok_or_else(err)?;
    let inner = &part.children[0].outer;
    Ok(format!(
        "{}{}{}",
        &text[..start],
        &text[inner.clone()],
        &text[end..]
    ))
}

/// Правки текста: что поставить на место каких байтов.
type Patches = Vec<(Range<usize>, String)>;

/// Значение поля `name` в скобках `open`: заменить или (поля нет) вписать первым.
fn put(patches: &mut Patches, text: &str, open: usize, name: &str, value: String) {
    let s = text.as_bytes();
    match fields(s, open).and_then(|f| f.into_iter().find(|f| f.name == name)) {
        Some(f) => patches.push((f.value, value)),
        None => patches.push((open + 1..open + 1, format!("{name}: {value}, "))),
    }
}

/// Множитель, сильнее всех отличный от 1 (для того, что тянется только целиком).
fn strongest(fs: &[f64]) -> f64 {
    fs.iter()
        .copied()
        .max_by(|a, b| (a.ln().abs()).total_cmp(&b.ln().abs()))
        .unwrap_or(1.0)
}

/// Растягивает часть по её осям в `f` раз (X, Y, Z): размеры деталей, места вложенных
/// и шаги повторов. Середина части (её `at`) остаётся на месте. Шар и другие детали без
/// размера по осям меняются целиком (по сильнейшему множителю), у цилиндра, капсулы и
/// конуса ось — Y, у тора — плоскость XZ.
pub(crate) fn stretch(text: &str, part: &Part, f: [f64; 3]) -> Result<String, EditError> {
    if f.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err(EditError("растяжение должно быть больше нуля".into()));
    }
    let mut patches = Patches::new();
    stretch_into(text, part, f, &mut patches);
    patches.sort_by_key(|(r, _)| std::cmp::Reverse((r.start, r.end)));
    let mut out = text.to_owned();
    for (r, v) in patches {
        out.replace_range(r, &v);
    }
    Ok(out)
}

/// Множители растяжения в осях вложенной части, повёрнутой на `turn`: каждой её оси —
/// множитель той оси родителя, вдоль которой она идёт больше всего.
fn turned_factors(f: [f64; 3], turn: [f64; 3]) -> [f64; 3] {
    let r = reyvec_sdf::shape3d::rotation(turn.map(|v| v as f32));
    std::array::from_fn(|j| {
        let col = r.col(j).abs();
        let i = if col.x >= col.y && col.x >= col.z {
            0
        } else if col.y >= col.z {
            1
        } else {
            2
        };
        f[i]
    })
}

fn stretch_into(text: &str, part: &Part, f: [f64; 3], patches: &mut Patches) {
    let Some(open) = part.open else { return };
    let num_of = |name: &str, default: f64| read_num(text, Some(open), name, default);
    let radial = strongest(&[f[0], f[2]]);
    let mut set = |name: &str, v: f64| put(patches, text, open, name, num(v));
    match part.kind.as_str() {
        "Box" => {
            let size = read_vec3(text, Some(open), "size", [1.0; 3]);
            let size = std::array::from_fn(|i| size[i] * f[i]);
            put(patches, text, open, "size", triple(size));
        }
        "Sphere" => set("radius", num_of("radius", 0.5) * strongest(&f)),
        "Cylinder" => {
            set("height", num_of("height", 1.0) * f[1]);
            set("radius", num_of("radius", 0.5) * radial);
        }
        "Capsule" => {
            let (h, r) = (num_of("height", 1.0), num_of("radius", 0.25));
            let r2 = r * radial;
            set("height", ((h + 2.0 * r) * f[1] - 2.0 * r2).max(0.0));
            set("radius", r2);
        }
        "Cone" => {
            set("height", num_of("height", 1.0) * f[1]);
            set("bottom", num_of("bottom", 0.5) * radial);
            let top = num_of("top", 0.0);
            if top != 0.0 {
                set("top", top * radial);
            }
        }
        "Torus" => {
            set("radius", num_of("radius", 0.5) * radial);
            set("thickness", num_of("thickness", 0.1) * strongest(&f));
        }
        "Repeat" => {
            let step = read_vec3(text, Some(open), "step", [0.0; 3]);
            let step = std::array::from_fn(|i| step[i] * f[i]);
            put(patches, text, open, "step", triple(step));
        }
        _ => {}
    }
    // Вложенная часть со сдвигом `Move` (своим или отдельной частью): её место
    // растягивается, внутри — множители в её осях.
    let moved = |p: &Part| if p.kind == "Move" { p.open } else { p.moved };
    for child in &part.children {
        let mut cf = f;
        if let Some(o) = moved(child) {
            let at = read_vec3(text, Some(o), "at", [0.0; 3]);
            if at != [0.0; 3] {
                let at = std::array::from_fn(|i| at[i] * f[i]);
                put(patches, text, o, "at", triple(at));
            }
            cf = turned_factors(f, read_vec3(text, Some(o), "turn", [0.0; 3]));
        }
        stretch_into(text, child, cf, patches);
    }
}

/// Повтор, который можно разделить на отдельные детали.
pub(crate) fn can_split(part: &Part) -> bool {
    part.kind == "Repeat" && part.open.is_some() && part.children.len() == 1
}

/// Делит повтор на отдельные детали: `Repeat(count: n, step: s, shape: X)` становится
/// объединением из `n` копий `X`, каждая на своём месте (`k · s`). Копии больше не
/// связаны: каждую можно выбрать, сдвинуть и поменять. Сдвиг и цвет самого повтора
/// остаются на объединении.
pub(crate) fn split_repeat(text: &str, part: &Part) -> Result<String, EditError> {
    let err = || EditError("повтор не разделить".into());
    let (true, Some(open)) = (can_split(part), part.open) else {
        return Err(err());
    };
    let head = text[..open].trim_end();
    let start = head.len().checked_sub(part.kind.len()).ok_or_else(err)?;
    if head[start..] != part.kind {
        return Err(err());
    }
    let end = value_end(text.as_bytes(), start).ok_or_else(err)?;
    let count = read_num(text, Some(open), "count", 1.0)
        .round()
        .clamp(1.0, f64::from(reyvec_sdf::shape3d::MAX_REPEAT)) as u32;
    let step = read_vec3(text, Some(open), "step", [0.0; 3]);
    let smooth = read_num(text, Some(open), "smooth", 0.0);
    let child = &part.children[0];
    let at = read_vec3(text, child.moved, "at", [0.0; 3]);
    let mut copies = Vec::new();
    for k in 0..count {
        let k = f64::from(k);
        let place = [
            at[0] + step[0] * k,
            at[1] + step[1] * k,
            at[2] + step[2] * k,
        ];
        let copy = match child.moved {
            // У детали свой сдвиг: место копии — в нём же.
            Some(o) => {
                let moved = set_field_at(text, o, "at", &triple(place))?;
                let grow = moved.len() as isize - text.len() as isize;
                let end = (child.outer.end as isize + grow) as usize;
                moved[child.outer.start..end].to_owned()
            }
            None if k == 0.0 => text[child.outer.clone()].to_owned(),
            None => format!(
                "Move(at: {}, shape: {})",
                triple(place),
                &text[child.outer.clone()]
            ),
        };
        copies.push(copy);
    }
    let line = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[line..start]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let inner = format!("{indent}    ");
    let list = copies
        .iter()
        .map(|c| format!("\n{inner}{c},"))
        .collect::<String>();
    let smooth = if smooth != 0.0 {
        format!(", smooth: {}", num(smooth))
    } else {
        String::new()
    };
    Ok(format!(
        "{}Union(shapes: [{list}\n{indent}]{smooth}){}",
        &text[..start],
        &text[end..]
    ))
}

/// Как сложить новую деталь с выбранной частью.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Combine {
    /// Добавить: объединение.
    Add,
    /// Вырезать новую деталь из выбранной.
    Cut,
    /// Оставить только общее с новой деталью.
    Intersect,
}

/// Ляжет ли новая деталь в список самой части (тогда её место считается от части, а не
/// от её родителя).
pub(crate) fn goes_inside(part: &Part, how: Combine) -> bool {
    let kind = match how {
        Combine::Add => "Union",
        Combine::Cut => "Subtract",
        Combine::Intersect => "Intersect",
    };
    part.kind == kind && part.list.is_some()
}

/// Новая деталь `item` к части `part` (её родитель — `parent`). Добавить: в список
/// самой части, если он есть, иначе в объединение-родителя, иначе часть и деталь
/// оборачиваются в новое объединение. Вырезать и пересечь: у вырезания деталь идёт в
/// его список `cut`, у пересечения — в его `shapes`, иначе часть оборачивается.
pub(crate) fn combine(
    text: &str,
    part: &Part,
    parent: Option<&Part>,
    how: Combine,
    item: &str,
) -> Result<String, EditError> {
    let has_list = |p: &Part, kind: &str| p.kind == kind && p.list.is_some();
    match how {
        _ if goes_inside(part, how) => add(text, part, item),
        Combine::Add if parent.is_some_and(|p| has_list(p, "Union") && part.removable) => {
            add(text, parent.expect("проверено"), item)
        }
        Combine::Add => Ok(wrap(text, part, "Union(shapes: [", &format!(", {item}])"))),
        Combine::Cut => Ok(wrap(
            text,
            part,
            "Subtract(base: ",
            &format!(", cut: [{item}])"),
        )),
        Combine::Intersect => Ok(wrap(
            text,
            part,
            "Intersect(shapes: [",
            &format!(", {item}])"),
        )),
    }
}

/// Новые детали из палитры «+»: вид и текст для файла (на месте `at`).
pub(crate) const PALETTE: [&str; 6] = ["Box", "Sphere", "Cylinder", "Capsule", "Torus", "Cone"];

/// Текст новой детали `kind` с размером около `size` метров, сдвинутой на `at`.
pub(crate) fn new_part(kind: &str, size: f64, at: [f64; 3]) -> String {
    let s = num(size);
    let h = num(size * 0.5);
    let q = num(size * 0.25);
    let shape = match kind {
        "Sphere" => format!("Sphere(radius: {h})"),
        "Cylinder" => format!("Cylinder(height: {s}, radius: {h})"),
        "Capsule" => format!("Capsule(height: {s}, radius: {q})"),
        "Torus" => format!("Torus(radius: {h}, thickness: {})", num(size * 0.1)),
        "Cone" => format!("Cone(height: {s}, bottom: {h}, top: 0.0)"),
        _ => format!("Box(size: ({s}, {s}, {s}))"),
    };
    format!("Move(at: {}, shape: {shape})", triple(at))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = r##"// Табурет.
(
    format: "reyvec-shape",
    version: 1,
    materials: [
        (name: "дерево", color: "#6b4a2b"),
        (name: "сталь", color: (0.5, 0.5, 0.55)),
    ],
    shape: Union(smooth: 0.02, shapes: [
        // Сиденье.
        Paint(material: 0, shape: Move(at: (0.0, 0.45, 0.0),
            shape: Cylinder(height: 0.05, radius: 0.2))),
        Mirror(axis: X, shape: Move(at: (0.12, 0.22, 0.0), shape: Box(size: (0.04, 0.44, 0.04)))),
        Sphere(radius: 0.05),
    ]),
)
"##;

    fn reparse(text: &str) -> reyvec_sdf::Shape3 {
        #[derive(serde::Deserialize)]
        struct File {
            shape: reyvec_sdf::Shape3,
        }
        ron::from_str::<File>(text)
            .unwrap_or_else(|e| panic!("{e}\n{text}"))
            .shape
    }

    #[test]
    fn tree_merges_move_and_paint() {
        let root = parse(MODEL).unwrap();
        assert_eq!(root.kind, "Union");
        assert_eq!(root.children.len(), 3);
        let seat = &root.children[0];
        assert_eq!(seat.kind, "Cylinder");
        assert!(seat.moved.is_some() && seat.painted.is_some() && seat.removable);
        assert_eq!(read_num(MODEL, seat.open, "radius", 0.0), 0.2);
        assert_eq!(
            read_vec3(MODEL, seat.moved, "at", [0.0; 3]),
            [0.0, 0.45, 0.0]
        );
        let mirror = &root.children[1];
        assert_eq!(mirror.kind, "Mirror");
        assert_eq!(read_word(MODEL, mirror.open, "axis").as_deref(), Some("X"));
        assert_eq!(mirror.children[0].kind, "Box");
        assert!(!mirror.children[0].removable);
        assert_eq!(read_num(MODEL, root.open, "smooth", 0.0), 0.02);
        assert_eq!(read_num(MODEL, root.children[2].open, "missing", 7.0), 7.0);
        assert_eq!(materials(MODEL), vec!["дерево", "сталь"]);
        assert_eq!(root.at(&[1, 0]).map(|p| p.kind.as_str()), Some("Box"));
    }

    #[test]
    fn edits_keep_comments_and_parse() {
        let root = parse(MODEL).unwrap();
        let seat = &root.children[0];
        let t = set(MODEL, seat, "radius", "0.25").unwrap();
        assert!(
            t.contains("radius: 0.25") && t.contains("// Сиденье.") && t.contains("// Табурет.")
        );
        reparse(&t);
        // Положение у детали без сдвига: она оборачивается.
        let ball = &root.children[2];
        let t = set_moved(MODEL, ball, "at", &triple([0.0, 0.6, 0.0])).unwrap();
        assert!(t.contains("Move(at: (0.0, 0.6, 0.0), shape: Sphere(radius: 0.05))"));
        reparse(&t);
        // Цвет: поменять, дать, убрать.
        let t = set_paint(MODEL, seat, Some(1)).unwrap();
        assert!(t.contains("Paint(material: 1"));
        let t = set_paint(MODEL, ball, Some(1)).unwrap();
        assert!(t.contains("Paint(material: 1, shape: Sphere(radius: 0.05))"));
        let t = set_paint(MODEL, seat, None).unwrap();
        assert!(!t.contains("Paint("), "{t}");
        let back = parse(&t).unwrap();
        assert_eq!(back.children[0].kind, "Cylinder");
        assert!(back.children[0].painted.is_none() && back.children[0].moved.is_some());
        reparse(&t);
        // Убрать деталь.
        let t = remove(MODEL, ball).unwrap();
        assert_eq!(parse(&t).unwrap().children.len(), 2);
        reparse(&t);
        assert!(remove(MODEL, &root.children[1].children[0]).is_err());
    }

    #[test]
    fn combine_adds_cuts_and_intersects() {
        let root = parse(MODEL).unwrap();
        let item = new_part("Box", 0.2, [0.0, 0.5, 0.0]);
        // В объединение — в его список.
        let t = combine(MODEL, &root, None, Combine::Add, &item).unwrap();
        assert_eq!(parse(&t).unwrap().children.len(), 4);
        reparse(&t);
        // К детали в объединении — тоже в список родителя.
        let seat = &root.children[0];
        let t = combine(MODEL, seat, Some(&root), Combine::Add, &item).unwrap();
        assert_eq!(parse(&t).unwrap().children.len(), 4);
        // Вырезать из сиденья: оно оборачивается в вырезание.
        let hole = new_part("Cylinder", 0.1, [0.0, 0.45, 0.0]);
        let t = combine(MODEL, seat, Some(&root), Combine::Cut, &hole).unwrap();
        let cut = parse(&t).unwrap();
        assert_eq!(cut.children[0].kind, "Subtract");
        assert_eq!(cut.children[0].children.len(), 2);
        assert!(cut.children[0].children[1].removable);
        reparse(&t);
        // Ещё одна дырка — в тот же список cut.
        let t2 = combine(&t, &cut.children[0], Some(&cut), Combine::Cut, &hole).unwrap();
        assert_eq!(parse(&t2).unwrap().children[0].children.len(), 3);
        reparse(&t2);
        // Пересечь шарик.
        let t = combine(
            MODEL,
            &root.children[2],
            Some(&root),
            Combine::Intersect,
            &item,
        )
        .unwrap();
        assert_eq!(parse(&t).unwrap().children[2].kind, "Intersect");
        reparse(&t);
        // Модель из одной детали: «добавить» делает объединение.
        let single = "(format: \"reyvec-shape\", version: 1, shape: Sphere(radius: 0.3))";
        let one = parse(single).unwrap();
        let t = combine(single, &one, None, Combine::Add, &item).unwrap();
        assert_eq!(parse(&t).unwrap().kind, "Union");
        reparse(&t);
        for kind in PALETTE {
            reparse(&format!(
                "(format: \"reyvec-shape\", version: 1, shape: {})",
                new_part(kind, 0.3, [0.1, 0.2, 0.3])
            ));
        }
    }

    #[test]
    fn real_models_parse() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenes/3d/models");
        let mut n = 0;
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            for f in std::fs::read_dir(&d).unwrap() {
                let path = f.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !path.to_string_lossy().ends_with(".shape.ron") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let root = parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert!(!root.kind.is_empty());
                n += 1;
            }
        }
        assert!(n >= 10, "моделей {n}");
    }

    #[test]
    fn tree_actions_duplicate_swap_wrap_unwrap() {
        let root = parse(MODEL).unwrap();
        let seat = root.at(&[0]).unwrap();
        // Копия сиденья сразу за ним, на своей строке.
        let t = duplicate(MODEL, seat).unwrap();
        let r = parse(&t).unwrap();
        assert_eq!(r.children.len(), 4);
        assert_eq!(r.children[1].kind, "Cylinder");
        assert!(t.contains("radius: 0.2))),\n        Paint("), "{t}");
        reparse(&t);
        assert!(duplicate(MODEL, &root).is_err(), "корень не в списке");
        // Ножки выше сиденья.
        let t = swap(MODEL, seat, root.at(&[1]).unwrap()).unwrap();
        let r = parse(&t).unwrap();
        assert_eq!(r.children[0].kind, "Mirror");
        assert_eq!(r.children[1].kind, "Cylinder");
        reparse(&t);
        // Шар в зеркало, потом обёртку снять — текст как был.
        let ball = root.at(&[2]).unwrap();
        let t = wrap_in(MODEL, ball, Wrap::Mirror);
        let r = parse(&t).unwrap();
        assert_eq!(r.children[2].kind, "Mirror");
        reparse(&t);
        assert_eq!(unwrap(&t, r.at(&[2]).unwrap()).unwrap(), MODEL);
        let t = wrap_in(MODEL, ball, Wrap::Repeat(0.25));
        assert!(t.contains("Repeat(count: 2, step: (0.25, 0.0, 0.0), shape: Sphere"));
        reparse(&t);
        let t = wrap_in(MODEL, ball, Wrap::Union);
        reparse(&t);
        // Зеркало ножки снимается: остаётся одна ножка со своим сдвигом.
        let legs = root.at(&[1]).unwrap();
        assert!(can_unwrap(legs) && !can_unwrap(ball) && !can_unwrap(&root));
        let t = unwrap(MODEL, legs).unwrap();
        assert!(
            t.contains("        Move(at: (0.12, 0.22, 0.0), shape: Box"),
            "{t}"
        );
        reparse(&t);
    }

    #[test]
    fn split_repeat_makes_separate_parts_in_the_same_places() {
        let model = "(\n    shape: Union(shapes: [\n        // ряд ящиков\n        \
                     Move(at: (0.0, 1.0, 0.0), shape: Repeat(count: 3, step: (0.5, 0.0, 0.0), \
                     smooth: 0.02, shape: Move(at: (0.1, 0.0, 0.0), shape: Box(size: (0.2, 0.2, 0.2))))),\n        \
                     Repeat(count: 2, step: (0.0, 0.0, 0.4), shape: Sphere(radius: 0.1)),\n    ]),\n)\n";
        let before = reparse(model);
        let root = parse(model).unwrap();
        let boxes = root.at(&[0]).unwrap();
        let balls = root.at(&[1]).unwrap();
        assert!(can_split(boxes) && can_split(balls) && !can_split(&root));
        let t = split_repeat(model, boxes).unwrap();
        let r = parse(&t).unwrap();
        let row = r.at(&[0]).unwrap();
        assert_eq!(row.kind, "Union");
        assert!(row.moved.is_some(), "сдвиг повтора остался на объединении");
        assert_eq!(row.children.len(), 3);
        assert!(t.contains("Move(at: (1.1, 0.0, 0.0), shape: Box"), "{t}");
        assert!(t.contains("smooth: 0.02"), "{t}");
        assert!(t.contains("// ряд ящиков"));
        let t = split_repeat(&t, parse(&t).unwrap().at(&[1]).unwrap()).unwrap();
        let r = parse(&t).unwrap();
        assert_eq!(r.at(&[1]).unwrap().children.len(), 2);
        assert!(t.contains("Move(at: (0.0, 0.0, 0.4), shape: Sphere"), "{t}");
        let after = reparse(&t);
        for p in [
            glam::Vec3::new(0.0, 1.0, 0.0),
            glam::Vec3::new(0.6, 1.1, 0.05),
            glam::Vec3::new(1.2, 0.9, -0.1),
            glam::Vec3::new(0.0, 0.0, 0.45),
            glam::Vec3::new(-0.3, 0.4, 0.2),
        ] {
            let (a, b) = (before.distance(p), after.distance(p));
            assert!((a - b).abs() < 1e-5, "{p}: {a} против {b}");
        }
        assert!(split_repeat(model, &root).is_err());
    }

    #[test]
    fn stretch_scales_sizes_places_and_steps_along_axes() {
        let model = "(\n    shape: Union(shapes: [\n        \
                     Move(at: (0.5, 1.0, 0.0), shape: Box(size: (0.2, 0.4, 0.2))),\n        \
                     Move(at: (0.0, 0.5, 0.0), turn: (0.0, 0.0, 90.0), shape: Cylinder(height: 1.0, radius: 0.1)),\n        \
                     Repeat(count: 3, step: (0.3, 0.0, 0.0), shape: Sphere(radius: 0.1)),\n        \
                     Capsule(height: 0.6, radius: 0.2),\n    ]),\n)\n";
        let root = parse(model).unwrap();
        let t = stretch(model, &root, [2.0, 1.0, 1.0]).unwrap();
        // Места и шаги по X вдвое дальше, коробка вдвое шире.
        assert!(
            t.contains("Move(at: (1.0, 1.0, 0.0), shape: Box(size: (0.4, 0.4, 0.2)))"),
            "{t}"
        );
        assert!(t.contains("step: (0.6, 0.0, 0.0)"), "{t}");
        // Цилиндр повёрнут на 90° вокруг Z: его ось Y лежит вдоль X — длиннее он.
        assert!(t.contains("Cylinder(height: 2.0, radius: 0.1)"), "{t}");
        // Шар меняется целиком, капсула вдоль Y не меняется.
        assert!(t.contains("Sphere(radius: 0.2)"), "{t}");
        assert!(t.contains("Capsule(height: 0.2, radius: 0.4)"), "{t}");
        reparse(&t);
        // Коробка без поля размера получает его.
        let t = stretch(
            "(shape: Box())",
            &parse("(shape: Box())").unwrap(),
            [1.0, 3.0, 1.0],
        )
        .unwrap();
        assert!(t.contains("Box(size: (1.0, 3.0, 1.0), )"), "{t}");
        assert!(stretch(model, &root, [0.0, 1.0, 1.0]).is_err());
    }
}
