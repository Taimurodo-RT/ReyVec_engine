//! Правки файла сцены 3D (М.8б) прямо в тексте: новый предмет, лампы, материалы.
//! Как и правки моделей (`shape_edit`), меняется только нужное место: комментарии и
//! отступы остаются.
//!
//! Цвета редактор пишет как `"#rrggbb"` (так их видит человек в палитре); числа 0…1 в
//! файле тоже читаются, но после правки цвета становятся записью `"#rrggbb"`.

use std::path::Path;

use reyvec_sdf::space_file::{Color3, LampEntry, MaterialEntry};

use super::ron_edit::{
    EditError, fields, insert_item, items, num, remove_item, set_field_at, set_item_field, skip_ws,
    value_end,
};
use super::shape_edit::{self, quoted, triple};

/// Материал, которого нет в сцене: для первого предмета пустой сцены.
const FALLBACK: &str = "серый";

/// Элементы списка `field` у самой сцены: текст каждого.
fn list<'a>(text: &'a str, field: &str) -> Vec<&'a str> {
    let s = text.as_bytes();
    let root = skip_ws(s, 0);
    let Some(f) = fields(s, root).and_then(|f| f.into_iter().find(|f| f.name == field)) else {
        return Vec::new();
    };
    if s.get(f.value.start) != Some(&b'[') {
        return Vec::new();
    }
    items(s, f.value.start)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|it| Some(&text[it..value_end(s, it)?]))
        .collect()
}

/// Материалы сцены по порядку (`None` — запись не читается).
pub(crate) fn materials(text: &str) -> Vec<Option<MaterialEntry>> {
    list(text, "materials")
        .into_iter()
        .map(|t| ron::from_str(t).ok())
        .collect()
}

/// Лампы сцены по порядку (`None` — запись не читается).
pub(crate) fn lamps(text: &str) -> Vec<Option<LampEntry>> {
    list(text, "lamps")
        .into_iter()
        .map(|t| ron::from_str(t).ok())
        .collect()
}

/// Цвет для палитры (sRGB, байты).
pub(crate) fn srgb(c: &Color3) -> [u8; 3] {
    let lin = c.linear().unwrap_or([0.5; 3]);
    lin.map(|v| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    })
}

/// Цвет для файла: `"#rrggbb"`.
pub(crate) fn hex(c: [u8; 3]) -> String {
    format!("\"#{:02x}{:02x}{:02x}\"", c[0], c[1], c[2])
}

/// Имя, которого ещё нет среди `taken`: `base`, `base 2`, `base 3`…
fn unique(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_owned();
    }
    (2..)
        .map(|i| format!("{base} {i}"))
        .find(|n| !taken.iter().any(|t| t == n))
        .unwrap_or_default()
}

/// Имена предметов сцены.
fn object_names(text: &str) -> Vec<String> {
    shape_edit::parse(text)
        .map(|r| r.children.into_iter().filter_map(|c| c.name).collect())
        .unwrap_or_default()
}

/// Материал для нового предмета: первый материал сцены без сетки (сетка — у пола). В
/// сцене нет материалов — появляется серый.
fn default_material(text: &str) -> Result<(String, String), EditError> {
    let all = materials(text);
    let pick = all
        .iter()
        .flatten()
        .find(|m| !m.grid)
        .or_else(|| all.iter().flatten().next());
    if let Some(m) = pick {
        return Ok((text.to_owned(), m.name.clone()));
    }
    let t = insert_item(
        text,
        &[],
        "materials",
        usize::MAX,
        &format!("(name: {}, color: \"#9a9a9a\")", quoted(FALLBACK)),
    )?;
    Ok((t, FALLBACK.to_owned()))
}

/// Новый предмет в конце списка `objects`: имя (с номером, если такое уже есть), форма
/// текстом и место. Ответ — текст и номер предмета.
pub(crate) fn add_object(
    text: &str,
    name: &str,
    shape: &str,
    at: [f64; 3],
) -> Result<(String, usize), EditError> {
    let (text, material) = default_material(text)?;
    let name = unique(name, &object_names(&text));
    let count = shape_edit::parse(&text)?.children.len();
    let item = format!(
        "(name: {}, material: {}, at: {},\n    shape: {shape})",
        quoted(&name),
        quoted(&material),
        triple(at),
    );
    let t = insert_item(&text, &[], "objects", usize::MAX, &item)?;
    Ok((t, count))
}

/// Как модель из файла `path` называется в `Use` сцены, лежащей в папке `dir`: путь
/// внутри `dir/models` без окончания. `None` — модель не из папки моделей этой сцены.
pub(crate) fn model_for_scene(dir: &Path, path: &Path) -> Option<String> {
    let (root, name) = super::shapes::model_of(path)?;
    let same = match (root.canonicalize(), dir.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => root == dir,
    };
    same.then_some(name)
}

/// Предмет-копия модели `model` на месте `at`; имя — последнее слово пути модели.
pub(crate) fn add_model(
    text: &str,
    model: &str,
    at: [f64; 3],
) -> Result<(String, usize), EditError> {
    let name = model.rsplit('/').next().unwrap_or(model);
    add_object(text, name, &format!("Use(model: {})", quoted(model)), at)
}

/// Новая лампа на месте `at`: тёплый свет средней силы.
pub(crate) fn add_lamp(text: &str, at: [f64; 3]) -> Result<String, EditError> {
    insert_item(
        text,
        &[],
        "lamps",
        usize::MAX,
        &format!(
            "(at: {}, color: \"#fff0dc\", power: 4.0, range: 6.0)",
            triple(at)
        ),
    )
}

/// Поле `field` лампы `i` (значение уже текстом RON).
pub(crate) fn set_lamp(
    text: &str,
    i: usize,
    field: &str,
    value: &str,
) -> Result<String, EditError> {
    set_item_field(text, &[], "lamps", i, field, value)
}

/// Место лампы `i`.
pub(crate) fn move_lamp(text: &str, i: usize, at: [f64; 3]) -> Result<String, EditError> {
    set_lamp(text, i, "at", &triple(at))
}

pub(crate) fn remove_lamp(text: &str, i: usize) -> Result<String, EditError> {
    remove_item(text, &[], "lamps", i)
}

/// Новый материал с именем, которого ещё нет. Ответ — текст и имя.
pub(crate) fn add_material(text: &str) -> Result<(String, String), EditError> {
    let taken: Vec<String> = materials(text)
        .into_iter()
        .flatten()
        .map(|m| m.name)
        .collect();
    let name = unique("материал", &taken);
    let t = insert_item(
        text,
        &[],
        "materials",
        usize::MAX,
        &format!("(name: {}, color: \"#b0a090\")", quoted(&name)),
    )?;
    Ok((t, name))
}

/// Поле `field` материала `i` (значение уже текстом RON).
pub(crate) fn set_material(
    text: &str,
    i: usize,
    field: &str,
    value: &str,
) -> Result<String, EditError> {
    set_item_field(text, &[], "materials", i, field, value)
}

/// Свечение материала (0 — не светится).
pub(crate) fn set_glow(text: &str, i: usize, glow: f64) -> Result<String, EditError> {
    set_material(text, i, "glow", &num(glow.max(0.0)))
}

/// Сетка на материале (как у пола).
pub(crate) fn set_grid(text: &str, i: usize, grid: bool) -> Result<String, EditError> {
    set_material(text, i, "grid", if grid { "true" } else { "false" })
}

/// Сколько предметов красит материал `name`.
pub(crate) fn uses_of(text: &str, name: &str) -> usize {
    shape_edit::parse(text)
        .map(|r| {
            r.children
                .iter()
                .filter(|c| {
                    shape_edit::read_string(text, c.moved, "material").as_deref() == Some(name)
                })
                .count()
        })
        .unwrap_or(0)
}

/// Убрать материал `i`: нельзя, пока им покрашен хоть один предмет.
pub(crate) fn remove_material(text: &str, i: usize) -> Result<String, EditError> {
    let all = materials(text);
    let name = all
        .get(i)
        .and_then(|m| m.as_ref().map(|m| m.name.clone()))
        .ok_or_else(|| EditError(format!("нет материала №{}", i + 1)))?;
    let n = uses_of(text, &name);
    if n > 0 {
        return Err(EditError(format!(
            "материалом «{name}» покрашено предметов: {n}. Сначала смените им материал"
        )));
    }
    remove_item(text, &[], "materials", i)
}

/// Переименовать материал `i`: предметы, покрашенные им, получают новое имя.
pub(crate) fn rename_material(text: &str, i: usize, new: &str) -> Result<String, EditError> {
    let new = new.trim();
    if new.is_empty() {
        return Err(EditError("у материала должно быть имя".into()));
    }
    let all = materials(text);
    let old = all
        .get(i)
        .and_then(|m| m.as_ref().map(|m| m.name.clone()))
        .ok_or_else(|| EditError(format!("нет материала №{}", i + 1)))?;
    if old == new {
        return Ok(text.to_owned());
    }
    if all.iter().flatten().any(|m| m.name == new) {
        return Err(EditError(format!("материал «{new}» уже есть")));
    }
    let mut t = set_material(text, i, "name", &quoted(new))?;
    // Предметы по одному: после каждой правки места в тексте сдвигаются.
    loop {
        let root = shape_edit::parse(&t)?;
        let Some(o) = root.children.iter().find_map(|c| {
            (shape_edit::read_string(&t, c.moved, "material").as_deref() == Some(old.as_str()))
                .then_some(c.moved)
                .flatten()
        }) else {
            return Ok(t);
        };
        t = set_field_at(&t, o, "material", &quoted(new))?;
    }
}

/// Точка, где луч `origin + dir·t` встречает пол (плоскость y = 0) не дальше `far`.
pub(crate) fn on_floor(origin: glam::Vec3, dir: glam::Vec3, far: f32) -> Option<glam::Vec3> {
    if dir.y.abs() < 1e-6 {
        return None;
    }
    let t = -origin.y / dir.y;
    (t > 0.0 && t <= far).then(|| origin + dir * t)
}

/// Точка в метрах, округлённая до миллиметра (для файла).
pub(crate) fn rounded(p: glam::Vec3) -> [f64; 3] {
    p.to_array()
        .map(|v| (f64::from(v) * 1000.0).round() / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_sdf::space_file::SpaceFile;

    const SCENE: &str = r##"// Пример.
(
    format: "reyvec-space",
    version: 1,
    materials: [
        (name: "пол", color: (0.2, 0.2, 0.2), grid: true),
        (name: "дерево", color: "#6b4a2b"),
    ],
    lamps: [
        (at: (1.0, 2.0, 1.0), color: (1.0, 0.9, 0.8), power: 6.0, range: 7.0),
    ],
    objects: [
        (name: "пол", material: "пол", shape: Plane(normal: (0.0, 1.0, 0.0), offset: 0.0)),
        // Пень.
        (name: "пень", material: "дерево", at: (0.0, 0.25, 0.0),
            shape: Cylinder(height: 0.5, radius: 0.22)),
    ],
)
"##;

    fn file(t: &str) -> SpaceFile {
        ron::from_str(t).unwrap_or_else(|e| panic!("{e}\n{t}"))
    }

    #[test]
    fn new_objects_get_unique_names_and_a_material() {
        let (t, i) = add_model(SCENE, "кузница/пень", [0.5, 0.0, -0.25]).unwrap();
        assert_eq!(i, 2);
        let (t, j) = add_model(&t, "кузница/пень", [0.0; 3]).unwrap();
        assert_eq!(j, 3);
        let (t, _) =
            add_object(&t, "Коробка", &shape_edit::part_shape("Box", 0.2), [0.0; 3]).unwrap();
        let f = file(&t);
        let names: Vec<&str> = f.objects.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["пол", "пень", "пень 2", "пень 3", "Коробка"]);
        // Не пол с сеткой, а первый обычный материал.
        assert_eq!(f.objects[2].material, "дерево");
        assert_eq!(f.objects[2].at, [0.5, 0.0, -0.25]);
        assert!(t.contains("// Пень."), "комментарии на месте");
        assert_eq!(shape_edit::parse(&t).unwrap().children.len(), 5);
        // В сцене без материалов появляется серый.
        let bare = "(format: \"reyvec-space\", version: 1, objects: [])";
        let (t, _) = add_model(bare, "склад/ящик", [0.0; 3]).unwrap();
        let f = file(&t);
        assert_eq!(f.materials[0].name, FALLBACK);
        assert_eq!(f.objects[0].material, FALLBACK);
    }

    #[test]
    fn lamps_are_added_moved_recoloured_and_removed() {
        let t = add_lamp(SCENE, [0.0, 1.5, 0.0]).unwrap();
        let t = move_lamp(&t, 1, [0.25, 1.0, -0.5]).unwrap();
        let t = set_lamp(&t, 0, "color", &hex([255, 0, 0])).unwrap();
        let t = set_lamp(&t, 1, "power", "2.5").unwrap();
        let f = file(&t);
        assert_eq!(f.lamps.len(), 2);
        assert_eq!(f.lamps[1].at, [0.25, 1.0, -0.5]);
        assert_eq!(f.lamps[1].power, 2.5);
        assert_eq!(srgb(&f.lamps[0].color), [255, 0, 0]);
        assert_eq!(lamps(&t).len(), 2);
        let t = remove_lamp(&t, 0).unwrap();
        assert_eq!(file(&t).lamps[0].at, [0.25, 1.0, -0.5]);
    }

    #[test]
    fn materials_are_added_renamed_and_kept_while_used() {
        let (t, name) = add_material(SCENE).unwrap();
        assert_eq!(name, "материал");
        let (t, name2) = add_material(&t).unwrap();
        assert_eq!(name2, "материал 2");
        let t = set_glow(&t, 2, 2.0).unwrap();
        let t = set_grid(&t, 2, true).unwrap();
        let f = file(&t);
        assert_eq!((f.materials[2].glow, f.materials[2].grid), (2.0, true));
        // Переименование меняет имя и у предметов.
        let t = rename_material(&t, 1, "дуб").unwrap();
        let f = file(&t);
        assert_eq!(f.materials[1].name, "дуб");
        assert_eq!(f.objects[1].material, "дуб");
        assert!(rename_material(&t, 1, "пол").is_err());
        // Используемый не убрать, свободный — можно.
        assert!(remove_material(&t, 1).is_err());
        let t = remove_material(&t, 3).unwrap();
        assert_eq!(file(&t).materials.len(), 3);
        assert_eq!(uses_of(&t, "пол"), 1);
        // Цвет туда и обратно.
        let c = Color3::Hex("#6b4a2b".into());
        assert_eq!(hex(srgb(&c)), "\"#6b4a2b\"");
    }

    #[test]
    fn models_from_other_folders_do_not_go_into_the_scene() {
        let dir = Path::new("/p/scenes/3d");
        assert_eq!(
            model_for_scene(dir, Path::new("/p/scenes/3d/models/склад/ящик.shape.ron")).as_deref(),
            Some("склад/ящик")
        );
        assert!(model_for_scene(dir, Path::new("/p/other/models/ящик.shape.ron")).is_none());
        let o = glam::Vec3::new(0.0, 2.0, 0.0);
        let hit = on_floor(o, glam::Vec3::new(0.0, -1.0, 1.0).normalize(), 100.0).unwrap();
        assert!((hit - glam::Vec3::new(0.0, 0.0, 2.0)).length() < 1e-5);
        assert!(on_floor(o, glam::Vec3::Y, 100.0).is_none());
        assert_eq!(
            rounded(glam::Vec3::new(0.12345, 0.0, -1.0)),
            [0.123, 0.0, -1.0]
        );
    }
}
