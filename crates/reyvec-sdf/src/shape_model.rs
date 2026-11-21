//! Модели форм (М.0, `docs/models-3d.md`): дерево [`Shape3`] в своём файле, которое сцена
//! ставит сколько угодно раз ссылкой `Use(model: "имя")`.
//!
//! Файл модели лежит в папке `models` рядом с файлом сцены и называется
//! `имя.shape.ron`; имя может идти через подпапки (`"станки/конвейер"` →
//! `models/станки/конвейер.shape.ron`). Внутри — формат `reyvec-shape`, версия 1:
//!
//! ```text
//! (
//!     format: "reyvec-shape",
//!     version: 1,
//!     materials: [(name: "железо", color: "#3a3d42")],
//!     shape: Union(smooth: 0.03, shapes: [
//!         Paint(material: 0, shape: Box(size: (0.4, 0.2, 0.15), round: 0.01)),
//!         Sphere(radius: 0.08),
//!     ]),
//! )
//! ```
//!
//! Ссылка раскрывается при чтении сцены в обычное дерево, поэтому шейдер, сборка
//! программы и сетки для слабых видеокарт про модели не знают: одинаковые ссылки дают
//! одинаковые деревья, а их кэши уже сливают одинаковые формы.
//!
//! Материалы: `Paint(material: N)` внутри модели — номер в списке `materials` самой
//! модели. При чтении сцены материалы модели дописываются в конец списка сцены под
//! именами `«модель/материал»` (один раз на модель), а номера в `Paint` сдвигаются.
//! Части модели без `Paint` красятся материалом предмета сцены, как обычно. Модель может
//! ссылаться на другие модели (вложенность не глубже [`MAX_NESTING`], без кругов).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::shape3d::Shape3;
use crate::space_file::MaterialEntry;

/// Формат файла модели.
pub const FORMAT: &str = "reyvec-shape";
/// Окончание имени файла модели.
pub const EXTENSION: &str = ".shape.ron";
/// Глубже модели в модели не ссылаются.
pub const MAX_NESTING: usize = 8;

/// Файл модели как он записан.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelFile {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub materials: Vec<MaterialEntry>,
    pub shape: Shape3,
}

impl ModelFile {
    /// Читает текст файла модели. Ошибка — строка для человека.
    pub fn parse(text: &str) -> Result<Self, String> {
        let file: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if file.format != FORMAT {
            return Err(format!("format: \"{}\", а нужен \"{FORMAT}\"", file.format));
        }
        if file.version != 1 {
            return Err(format!(
                "version: {}, а движок знает только 1",
                file.version
            ));
        }
        let mut worst = None;
        max_paint(&file.shape, &mut worst);
        if let Some(n) = worst
            && n as usize >= file.materials.len()
        {
            return Err(format!(
                "Paint(material: {n}), а материалов в модели {} (номера с 0)",
                file.materials.len()
            ));
        }
        Ok(file)
    }
}

/// Путь к файлу модели `name` для сцены из папки `dir`.
pub fn model_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let rel = Path::new(name);
    let plain = !name.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)));
    if !plain {
        return Err(format!(
            "модель «{name}»: имя без «..» и без начала с корня, например \"станки/конвейер\""
        ));
    }
    Ok(dir.join("models").join(format!("{name}{EXTENSION}")))
}

/// Подставляет модели в формы одной сцены: читает каждый файл один раз и дописывает
/// материалы моделей в список сцены.
pub struct Resolver<'a> {
    dir: PathBuf,
    materials: &'a mut Vec<MaterialEntry>,
    /// Модель → её дерево с уже сдвинутыми номерами материалов и раскрытыми ссылками.
    done: HashMap<String, Shape3>,
    /// Файлы прочитанных моделей (для слежки за сохранением).
    pub files: Vec<PathBuf>,
}

impl<'a> Resolver<'a> {
    /// `dir` — папка файла сцены (`None` — рабочая папка), `materials` — список
    /// материалов сцены, в конец которого лягут материалы моделей.
    pub fn new(dir: Option<&Path>, materials: &'a mut Vec<MaterialEntry>) -> Self {
        Self {
            dir: dir.map_or_else(|| PathBuf::from("."), Path::to_path_buf),
            materials,
            done: HashMap::new(),
            files: Vec::new(),
        }
    }

    /// Заменяет все `Use` в форме деревьями моделей.
    pub fn resolve(&mut self, shape: &mut Shape3) -> Result<(), String> {
        self.walk(shape, &mut Vec::new())
    }

    fn walk(&mut self, shape: &mut Shape3, chain: &mut Vec<String>) -> Result<(), String> {
        match shape {
            Shape3::Use { model } => {
                let model = model.clone();
                *shape = self.load(&model, chain)?;
                Ok(())
            }
            Shape3::Union { shapes, .. } | Shape3::Intersect { shapes, .. } => {
                shapes.iter_mut().try_for_each(|s| self.walk(s, chain))
            }
            Shape3::Subtract { base, cut, .. } => {
                self.walk(base, chain)?;
                cut.iter_mut().try_for_each(|s| self.walk(s, chain))
            }
            Shape3::Move { shape, .. }
            | Shape3::Paint { shape, .. }
            | Shape3::Animate { shape, .. } => self.walk(shape, chain),
            _ => Ok(()),
        }
    }

    fn load(&mut self, name: &str, chain: &mut Vec<String>) -> Result<Shape3, String> {
        if let Some(done) = self.done.get(name) {
            return Ok(done.clone());
        }
        if chain.iter().any(|n| n == name) {
            return Err(format!(
                "модели ссылаются по кругу: {} → {name}",
                chain.join(" → ")
            ));
        }
        if chain.len() >= MAX_NESTING {
            return Err(format!(
                "модели вложены глубже {MAX_NESTING}: {} → {name}",
                chain.join(" → ")
            ));
        }
        let path = model_path(&self.dir, name)?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("модель «{name}»: не прочитать {}: {e}", path.display()))?;
        let file = ModelFile::parse(&text).map_err(|e| format!("модель «{name}»: {e}"))?;
        self.files.push(path);
        let base = self.materials.len() as u32;
        for m in &file.materials {
            let mut m = m.clone();
            m.name = format!("{name}/{}", m.name);
            self.materials.push(m);
        }
        let mut shape = file.shape;
        shift_paint(&mut shape, base);
        chain.push(name.to_string());
        let walked = self.walk(&mut shape, chain);
        chain.pop();
        walked.map_err(|e| format!("модель «{name}»: {e}"))?;
        self.done.insert(name.to_string(), shape.clone());
        Ok(shape)
    }
}

/// Сдвигает номера материалов всех `Paint` на `base`. Ссылки `Use` не трогает: их
/// материалы сдвигаются, когда модель читается.
fn shift_paint(shape: &mut Shape3, base: u32) {
    match shape {
        Shape3::Paint { material, shape } => {
            *material += base;
            shift_paint(shape, base);
        }
        Shape3::Union { shapes, .. } | Shape3::Intersect { shapes, .. } => {
            shapes.iter_mut().for_each(|s| shift_paint(s, base));
        }
        Shape3::Subtract { base: b, cut, .. } => {
            shift_paint(b, base);
            cut.iter_mut().for_each(|s| shift_paint(s, base));
        }
        Shape3::Move { shape, .. } | Shape3::Animate { shape, .. } => shift_paint(shape, base),
        _ => {}
    }
}

/// Самый большой номер материала в `Paint` формы.
fn max_paint(shape: &Shape3, worst: &mut Option<u32>) {
    match shape {
        Shape3::Paint { material, shape } => {
            *worst = Some(worst.map_or(*material, |w| w.max(*material)));
            max_paint(shape, worst);
        }
        Shape3::Union { shapes, .. } | Shape3::Intersect { shapes, .. } => {
            shapes.iter().for_each(|s| max_paint(s, worst));
        }
        Shape3::Subtract { base, cut, .. } => {
            max_paint(base, worst);
            cut.iter().for_each(|s| max_paint(s, worst));
        }
        Shape3::Move { shape, .. } | Shape3::Animate { shape, .. } => max_paint(shape, worst),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpaceScene;

    /// Временная папка сцены с файлами моделей; удаляется в конце теста.
    struct TempScene(PathBuf);

    impl TempScene {
        fn new(tag: &str, models: &[(&str, &str)]) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("reyvec-shape-model-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            for (name, text) in models {
                let path = model_path(&dir, name).unwrap();
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            Self(dir)
        }
    }

    impl Drop for TempScene {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scene(materials: &str, objects: &str) -> String {
        format!(
            "(format: \"reyvec-space\", version: 1, materials: [{materials}], objects: [{objects}])"
        )
    }

    const LEGS: &str = "Union(shapes: [Box(size: (3.6, 0.12, 0.8), round: 0.02), \
        Move(at: (-1.6, -0.33, -0.3), shape: Box(size: (0.08, 0.6, 0.08))), \
        Move(at: (1.6, -0.33, 0.3), shape: Box(size: (0.08, 0.6, 0.08)))])";

    #[test]
    fn use_gives_the_same_frame_as_inline_shape() {
        let model = format!("(format: \"reyvec-shape\", version: 1, shape: {LEGS})");
        let tmp = TempScene::new("same", &[("станки/конвейер", &model)]);
        let mat = r#"(name: "лента", color: (0.1, 0.1, 0.1))"#;
        let inline = scene(
            mat,
            &format!(
                r#"(material: "лента", at: (1, 0.5, 0), turn: (0, 30, 0), shape: {LEGS}),
                   (material: "лента", at: (5, 0.5, 0), shape: Move(scale: 2.0, shape: {LEGS}))"#
            ),
        );
        let with_use = scene(
            mat,
            r#"(material: "лента", at: (1, 0.5, 0), turn: (0, 30, 0),
                    shape: Use(model: "станки/конвейер")),
               (material: "лента", at: (5, 0.5, 0),
                    shape: Move(scale: 2.0, shape: Use(model: "станки/конвейер")))"#,
        );
        let a = SpaceScene::parse_in(&inline, Some(&tmp.0)).unwrap();
        let b = SpaceScene::parse_in(&with_use, Some(&tmp.0)).unwrap();
        assert_eq!(a.frame(0.5).shapes, b.frame(0.5).shapes);
        assert_eq!(b.model_files.len(), 1, "файл модели читается один раз");
    }

    #[test]
    fn model_materials_are_appended_and_renumbered() {
        let model = r##"(format: "reyvec-shape", version: 1,
            materials: [(name: "железо", color: "#404040"), (name: "медь", color: "#b87333")],
            shape: Union(shapes: [
                Paint(material: 1, shape: Sphere(radius: 0.2)),
                Paint(material: 0, shape: Box(size: (1, 1, 1))),
                Sphere(radius: 0.1),
            ]))"##;
        let tmp = TempScene::new("mat", &[("наковальня", model)]);
        let text = scene(
            r#"(name: "пол", color: (0.2, 0.2, 0.2)), (name: "камень", color: (0.5, 0.5, 0.5))"#,
            r#"(material: "камень", shape: Use(model: "наковальня")),
               (material: "пол", shape: Use(model: "наковальня"))"#,
        );
        let s = SpaceScene::parse_in(&text, Some(&tmp.0)).unwrap();
        assert_eq!(
            s.material_names[2..],
            ["наковальня/железо", "наковальня/медь"]
        );
        let frame = s.frame(0.0);
        let Shape3::Paint { material: 1, shape } = &frame.shapes[0] else {
            panic!("предмет красится своим материалом: {:?}", frame.shapes[0]);
        };
        let Shape3::Move { shape, .. } = shape.as_ref() else {
            panic!("{shape:?}");
        };
        let Shape3::Union { shapes, .. } = shape.as_ref() else {
            panic!("{shape:?}");
        };
        assert!(matches!(shapes[0], Shape3::Paint { material: 3, .. }));
        assert!(matches!(shapes[1], Shape3::Paint { material: 2, .. }));
    }

    #[test]
    fn nested_models_and_errors() {
        let bolt =
            r#"(format: "reyvec-shape", version: 1, shape: Cylinder(height: 0.1, radius: 0.02))"#;
        let plate = r#"(format: "reyvec-shape", version: 1, shape: Union(shapes: [
            Box(size: (1, 0.05, 1)), Move(at: (0.4, 0, 0.4), shape: Use(model: "болт"))]))"#;
        let ring_a = r#"(format: "reyvec-shape", version: 1, shape: Use(model: "б"))"#;
        let ring_b = r#"(format: "reyvec-shape", version: 1, shape: Use(model: "а"))"#;
        let bad_paint = r#"(format: "reyvec-shape", version: 1, shape: Paint(material: 0, shape: Sphere(radius: 1)))"#;
        let tmp = TempScene::new(
            "nest",
            &[
                ("болт", bolt),
                ("плита", plate),
                ("а", ring_a),
                ("б", ring_b),
                ("краска", bad_paint),
            ],
        );
        let mat = r#"(name: "сталь", color: (0.5, 0.5, 0.5))"#;
        let one = |model: &str| {
            SpaceScene::parse_in(
                &scene(
                    mat,
                    &format!(r#"(material: "сталь", shape: Use(model: "{model}"))"#),
                ),
                Some(&tmp.0),
            )
        };
        let s = one("плита").unwrap();
        assert_eq!(s.model_files.len(), 2);
        let e = one("а").unwrap_err();
        assert!(e.contains("по кругу") && e.contains("а → б → а"), "{e}");
        let e = one("нет").unwrap_err();
        assert!(
            e.contains("модель «нет»") && e.contains("нет.shape.ron"),
            "{e}"
        );
        let e = one("../плита").unwrap_err();
        assert!(e.contains("без «..»"), "{e}");
        let e = one("краска").unwrap_err();
        assert!(e.contains("материалов в модели 0"), "{e}");
    }

    #[test]
    fn unresolved_use_does_not_compile() {
        let e = crate::shape3d::compile(&Shape3::Use {
            model: "станок".into(),
        })
        .unwrap_err();
        assert!(e.to_string().contains("станок"), "{e}");
    }
}
