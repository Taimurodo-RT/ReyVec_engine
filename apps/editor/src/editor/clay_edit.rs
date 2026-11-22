//! Глина в «Формах» (М.9б): деталь модели превращается в глину (сетку расстояний в
//! файле `.clay` рядом с моделью), кисть лепит её мышью. Здесь — то, что не про экран:
//! память о прочитанной глине, превращение детали, мазки и запись файла.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::Vec3;
use reyvec_sdf::brick3d::{Brick, BrickRef, MAX_SIDE};
use reyvec_sdf::clay::{Brush, EXTENSION};
use reyvec_sdf::shape_model::{Resolver, clay_path};
use reyvec_sdf::shape3d::Shape3;

use super::ron_edit::{skip_ws, value_end};
use super::shape_edit::{self, Part};

thread_local! {
    /// Глина по имени файла (`кузница/заготовка.clay`): по ней выбор мышью и кисть видят
    /// форму, а пока тянут кисть — и самые свежие мазки, ещё не записанные в файл.
    static CLAY: RefCell<HashMap<String, BrickRef>> = RefCell::new(HashMap::new());
}

/// Глина из памяти.
pub(crate) fn brick(file: &str) -> Option<BrickRef> {
    CLAY.with_borrow(|c| c.get(file).cloned())
}

/// Запомнить глину (после мазка или отмены).
pub(crate) fn put(file: &str, brick: Brick) {
    CLAY.with_borrow_mut(|c| c.insert(file.to_owned(), BrickRef(Arc::new(brick))));
    super::shapes::forget_whole();
}

/// Перечитать из файлов глину, на которую ссылается модель `text` (её свои детали;
/// глина вложенных моделей приходит вместе с ними).
pub(crate) fn remember(text: &str, root: &Path) {
    let Ok(tree) = shape_edit::parse(text) else {
        return;
    };
    let mut stack = vec![&tree];
    while let Some(p) = stack.pop() {
        stack.extend(p.children.iter());
        let Some(file) = file_of(text, p) else {
            continue;
        };
        let read = clay_path(root, &file)
            .and_then(|path| std::fs::read(&path).map_err(|e| e.to_string()))
            .and_then(|b| Brick::from_clay_bytes(&b));
        match read {
            Ok(b) => {
                CLAY.with_borrow_mut(|c| c.insert(file, BrickRef(Arc::new(b))));
            }
            Err(e) => tracing::warn!("Глина «{file}» не читается: {e}"),
        }
    }
    super::shapes::forget_whole();
}

/// Файл глины детали (`None` — деталь не глина).
pub(crate) fn file_of(text: &str, part: &Part) -> Option<String> {
    (part.kind == "Clay")
        .then(|| shape_edit::read_string(text, part.open, "file"))
        .flatten()
}

/// Можно ли превратить деталь в глину: у неё есть размер и она не глина уже.
pub(crate) fn can_turn(part: &Part) -> bool {
    !matches!(part.kind.as_str(), "Clay" | "Plane" | "Scene" | "Use") && part.open.is_some()
}

/// Где в тексте сама деталь, без её сдвига и цвета: от имени вида до закрывающей скобки.
fn own_span(text: &str, part: &Part) -> Option<std::ops::Range<usize>> {
    let s = text.as_bytes();
    let open = part.open?;
    let start = open.checked_sub(part.kind.len())?;
    if text[start..open] != part.kind {
        // Между именем и скобкой пробелы: ищем имя перед ними.
        let name_end = text[..open].trim_end().len();
        let start = name_end.checked_sub(part.kind.len())?;
        (text[start..name_end] == part.kind).then_some(())?;
        return Some(start..value_end(s, skip_ws(s, start))?);
    }
    Some(start..value_end(s, start)?)
}

/// Свободное имя файла глины для детали модели `model` (`кузница/наковальня`):
/// `кузница/наковальня-глина.clay`, занято — `…-глина-2.clay` и дальше.
fn free_name(root: &Path, model: &str) -> String {
    (1..)
        .map(|n| {
            if n == 1 {
                format!("{model}-глина{EXTENSION}")
            } else {
                format!("{model}-глина-{n}{EXTENSION}")
            }
        })
        .find(|name| clay_path(root, name).is_ok_and(|p| !p.exists()))
        .unwrap_or_default()
}

/// Деталь, превращённая в глину.
pub(crate) struct Turned {
    /// Новый текст модели: на месте детали `Clay(file: …)`, сдвиг и цвет те же.
    pub text: String,
    /// Имя файла глины для `Clay`.
    pub file: String,
    /// Куда записать файл.
    pub path: PathBuf,
    pub brick: Brick,
}

/// Превратить деталь `path` модели `model` (текст `text`, папка с `models` — `root`)
/// в глину. Ошибка — почему нельзя.
pub(crate) fn turn(text: &str, path: &[usize], root: &Path, model: &str) -> Result<Turned, String> {
    let tree = shape_edit::parse(text).map_err(|e| e.0)?;
    let part = tree.at(path).ok_or("детали нет")?;
    if !can_turn(part) {
        return Err(format!(
            "{} в глину не превращается",
            shape_edit::label(&part.kind)
        ));
    }
    let span = own_span(text, part).ok_or("не найти деталь в тексте")?;
    let mut shape: Shape3 =
        ron::from_str(&text[span.clone()]).map_err(|e| format!("деталь не читается: {e}"))?;
    let mut materials = Vec::new();
    Resolver::new(Some(root), &mut materials).resolve(&mut shape)?;
    let brick = Brick::from_shape(&shape, MAX_SIDE)?;
    let file = free_name(root, model);
    let path = clay_path(root, &file)?;
    let new = format!(
        "{}Clay(file: {}){}",
        &text[..span.start],
        shape_edit::quoted(&file),
        &text[span.end..]
    );
    Ok(Turned {
        text: new,
        file,
        path,
        brick,
    })
}

/// Записать глину в файл (папки создаются).
pub(crate) fn save(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

/// Мазок кисти, как его прислал интерфейс: файл глины и точка в её координатах.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Dab {
    pub file: String,
    pub kind: Brush,
    pub at: Vec3,
    /// Радиус в координатах глины.
    pub radius: f32,
    pub strength: f32,
}

/// Один проход кистью (от нажатия до отпускания): копия глины, которую правят мазки, и
/// файл до прохода (для отмены).
pub(crate) struct Stroke {
    pub file: String,
    pub before: Vec<u8>,
    brick: Brick,
    /// Есть мазки, ещё не записанные в файл.
    pub dirty: bool,
}

impl Stroke {
    /// Начать проход по глине `file` (она должна быть в памяти).
    pub(crate) fn start(file: &str) -> Option<Self> {
        let brick = brick(file)?.0.duplicate();
        Some(Self {
            file: file.to_owned(),
            before: brick.to_clay_bytes(),
            brick,
            dirty: false,
        })
    }

    /// Мазок: глина в памяти сразу новая.
    pub(crate) fn dab(&mut self, d: &Dab) {
        if self.brick.brush(d.kind, d.at, d.radius, d.strength) {
            self.dirty = true;
            put(&self.file, self.brick.duplicate());
        }
    }

    /// Файл глины сейчас.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.brick.to_clay_bytes()
    }
}

/// Шаг истории правок модели: текст файла модели или файл глины до правки.
pub(crate) enum ShapeStep {
    Text(String),
    Clay { file: String, bytes: Vec<u8> },
}

impl super::Editor {
    /// Вернуть шаг истории `step` (текст модели сейчас — `current`); ответ — шаг, который
    /// вернёт как было.
    pub(super) fn shape_step(&mut self, step: ShapeStep, current: String) -> ShapeStep {
        match step {
            ShapeStep::Text(text) => {
                self.write_shape(&text);
                ShapeStep::Text(current)
            }
            ShapeStep::Clay { file, bytes } => {
                let now = brick(&file)
                    .map(|b| b.0.to_clay_bytes())
                    .unwrap_or_default();
                match Brick::from_clay_bytes(&bytes) {
                    Ok(b) => {
                        put(&file, b);
                        self.write_clay(&file, &bytes);
                    }
                    Err(e) => tracing::warn!("Глина «{file}» не вернулась: {e}"),
                }
                ShapeStep::Clay { file, bytes: now }
            }
        }
    }

    /// Деталь `path` открытой модели — в глину: файл глины рядом с моделью, в тексте
    /// модели `Clay` на месте детали (одним шагом истории).
    pub(super) fn turn_to_clay(&mut self, path: &[usize]) {
        let Some(shape) = self.shape.as_ref() else {
            return;
        };
        let turned = match turn(&shape.text, path, &shape.root, &shape.name) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("В глину не превратить: {e}");
                return;
            }
        };
        if let Err(e) = save(&turned.path, &turned.brick.to_clay_bytes()) {
            tracing::warn!("Глина не записана в {}: {e}", turned.path.display());
            return;
        }
        tracing::info!("Деталь стала глиной: {}", turned.file);
        put(&turned.file, turned.brick);
        self.shape_action(super::ui::Action::ShapeText {
            text: turned.text,
            commit: true,
        });
    }

    /// Мазок кисти: глина в памяти сразу новая, в файл и движку — не чаще
    /// [`super::PREVIEW_INTERVAL`].
    pub(super) fn clay_dab(&mut self, dab: &Dab) {
        if self
            .clay_stroke
            .as_ref()
            .is_some_and(|s| s.file != dab.file)
        {
            self.clay_end();
        }
        if self.clay_stroke.is_none() {
            self.clay_stroke = Stroke::start(&dab.file);
        }
        let Some(stroke) = self.clay_stroke.as_mut() else {
            return;
        };
        stroke.dab(dab);
        if self.clay_written.elapsed() >= super::PREVIEW_INTERVAL {
            self.clay_flush();
        }
    }

    /// Мазки прохода — в файл глины и на подставку.
    fn clay_flush(&mut self) {
        let Some(stroke) = self.clay_stroke.as_mut().filter(|s| s.dirty) else {
            return;
        };
        stroke.dirty = false;
        let (file, bytes) = (stroke.file.clone(), stroke.bytes());
        self.write_clay(&file, &bytes);
    }

    /// Кисть отпустили: дописать файл, проход — шаг истории.
    pub(super) fn clay_end(&mut self) {
        self.clay_flush();
        let Some(stroke) = self.clay_stroke.take() else {
            return;
        };
        if stroke.bytes() != stroke.before {
            self.shape_undo.push(ShapeStep::Clay {
                file: stroke.file,
                bytes: stroke.before,
            });
            self.shape_redo.clear();
        }
    }

    /// Записать файл глины и показать модель с ним на подставке.
    fn write_clay(&mut self, file: &str, bytes: &[u8]) {
        self.clay_written = std::time::Instant::now();
        let (Some(shape), Some(r)) = (self.shape.as_ref(), self.renderer.as_mut()) else {
            return;
        };
        let path = match clay_path(&shape.root, file) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("{e}");
                return;
            }
        };
        if let Err(e) = save(&path, bytes) {
            tracing::warn!("Глина не записана в {}: {e}", path.display());
            return;
        }
        r.set_space_file_in(&shape.stand_scene(), Some(shape.root.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = r#"(
    format: "reyvec-shape",
    version: 1,
    materials: [(name: "сталь", color: (0.5, 0.5, 0.5))],
    shape: Union(shapes: [
        Move(at: (0.0, 0.25, 0.0), shape: Paint(material: 0, shape: Box (size: (0.4, 0.2, 0.2)))),
        Sphere(radius: 0.1),
    ]),
)"#;

    #[test]
    fn part_turns_into_clay_and_brush_changes_it() {
        let dir = std::env::temp_dir().join(format!("reyvec-clay-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("models/кузница")).unwrap();
        let tree = shape_edit::parse(MODEL).unwrap();
        assert!(can_turn(tree.at(&[0]).unwrap()));
        let t = turn(MODEL, &[0], &dir, "кузница/брусок").unwrap();
        assert_eq!(t.file, "кузница/брусок-глина.clay");
        // Сдвиг и цвет остались, вместо коробки — глина.
        assert!(
            t.text
                .contains("Paint(material: 0, shape: Clay(file: \"кузница/брусок-глина.clay\"))"),
            "{}",
            t.text
        );
        let tree = shape_edit::parse(&t.text).unwrap();
        let part = tree.at(&[0]).unwrap();
        assert_eq!(part.kind, "Clay");
        assert_eq!(file_of(&t.text, part).as_deref(), Some(t.file.as_str()));
        assert!(!can_turn(part));
        // Сетка повторяет коробку: внутри минус, снаружи плюс.
        assert!(t.brick.sample(Vec3::ZERO) < 0.0);
        assert!(t.brick.sample(Vec3::new(0.0, 0.2, 0.0)) > 0.0);
        save(&t.path, &t.brick.to_clay_bytes()).unwrap();
        // Занятое имя не берётся второй раз.
        assert_eq!(
            free_name(&dir, "кузница/брусок"),
            "кузница/брусок-глина-2.clay"
        );
        // Модель читается с глиной, кисть меняет её в памяти.
        remember(&t.text, &dir);
        let mut s = Stroke::start(&t.file).expect("глина в памяти");
        let top = Vec3::new(0.0, 0.1, 0.0);
        let before = brick(&t.file).unwrap().0.sample(top + Vec3::Y * 0.03);
        s.dab(&Dab {
            file: t.file.clone(),
            kind: Brush::Add,
            at: top,
            radius: 0.06,
            strength: 0.8,
        });
        assert!(s.dirty);
        let after = brick(&t.file).unwrap().0.sample(top + Vec3::Y * 0.03);
        assert!(after < before, "{before} → {after}");
        assert_ne!(s.bytes(), s.before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forge_billet_is_clay_on_the_stand() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scenes/3d/models/кузница/заготовка.shape.ron");
        let mut view = super::super::shapes::ShapeView::open(path).expect("модель");
        view.load().expect("модель читается");
        let tree = shape_edit::parse(&view.text).unwrap();
        assert_eq!(
            file_of(&view.text, &tree).as_deref(),
            Some("кузница/заготовка.clay")
        );
        // Луч сверху попадает в глину: выбор мышью и кисть её видят.
        let (lo, hi) = super::super::shapes::part_bounds(&view.text, &[]).expect("коробка");
        let mid = (lo + hi) * 0.5;
        let p = super::super::shapes::hit(
            &view.text,
            &[],
            Vec3::new(mid.x, hi.y + 1.0, mid.z),
            Vec3::NEG_Y,
            10.0,
        )
        .expect("попали в заготовку");
        let frame = super::super::shapes::part_frame(&view.text, &[]).unwrap();
        let b = brick("кузница/заготовка.clay").unwrap();
        let local = frame.inverse().transform_point3(p);
        assert!(b.0.sample(local).abs() < b.0.cell() * 2.0);
    }
}
