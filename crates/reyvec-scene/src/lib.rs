//! Сцена из файла (этап 5, задача 5.1).
//!
//! Сцена описывается текстовым файлом в формате RON (похож на запись данных в Rust, но
//! без кода): векторные фигуры в слоях (слой может отбрасывать тень), источники света и
//! объекты из SDF (капли, слизь, светящиеся трубки). Файл правится в любом блокноте, а
//! [`SceneWatcher`] замечает сохранение, и песочница сразу показывает новую сцену.
//!
//! Устройство:
//!
//! - [`SceneFile::parse`] читает текст и проверяет его: формат и версия файла, числа в
//!   допустимых пределах, не больше источников и объектов, чем берёт видеокарта. Ошибка
//!   приходит со строкой и столбцом и по-русски ([`SceneError`]).
//! - [`SceneFile::build`] превращает описание в то, что рисует движок: список
//!   [`SceneItem`] для векторного слоя и короткие списки источников и объектов.
//!   Фигуры нарезаются на треугольники один раз: [`ShapeCache`] помнит каждую форму
//!   (без места, поворота и цвета), поэтому сдвиг или перекраска фигуры при правке файла
//!   новых фигур не создаёт.
//!
//! Координаты как у камеры 2D: y вверх, кадр по высоте от −1 до 1, углы в градусах
//! против часовой стрелки. Цвета — строки `"#rrggbb"` или `"#rrggbbaa"` (sRGB, как в
//! редакторах); яркость источников и светящихся объектов задаёт `power`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use reyvec_vector::kurbo::{self, Shape as _};
use reyvec_vector::{
    Affine, BezPath, BlendMode, Color, Instance, LineCap, LineJoin, SceneItem, Shape, ShapeId,
    ShapeLibrary, Stroke,
};
use serde::Deserialize;

/// Пример сцены «первая комната» (`scenes/first-room.ron`): его показывает песочница,
/// если своего файла нет рядом.
pub const EXAMPLE: &str = include_str!("../../../scenes/first-room.ron");

/// Отметка формата в начале файла.
pub const FORMAT: &str = "reyvec-scene";
/// Версия формата, которую понимает этот движок.
pub const VERSION: u32 = 1;
/// Сколько источников света, объектов и их частей берёт видеокарта за кадр (как в
/// `reyvec-gpu`: `MAX_LIGHTS`, `MAX_OBJECTS`, `MAX_BLOBS`).
pub const MAX_LIGHTS: usize = 16;
pub const MAX_OBJECTS: usize = 16;
pub const MAX_PARTS: usize = 128;
/// Самая большая координата и размер: сцена 2D в единицах камеры (кадр — 2 единицы по
/// высоте), дальше почти наверняка опечатка.
const MAX_COORD: f64 = 1000.0;

/// Ошибка в файле сцены: где (строка и столбец, если известны) и что не так.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}{message}", place(*.line, *.col))]
pub struct SceneError {
    pub line: Option<usize>,
    pub col: Option<usize>,
    pub message: String,
}

fn place(line: Option<usize>, col: Option<usize>) -> String {
    match (line, col) {
        (Some(l), Some(c)) => format!("строка {l}, столбец {c}: "),
        (Some(l), None) => format!("строка {l}: "),
        _ => String::new(),
    }
}

impl SceneError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            line: None,
            col: None,
            message: message.into(),
        }
    }
}

/// Цвет sRGB из строки `"#rgb"`, `"#rrggbb"` или `"#rrggbbaa"`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hex(pub Color);

impl Hex {
    /// Разбирает строку цвета.
    pub fn parse(s: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "цвет «{s}»: нужна строка вида \"#rrggbb\" или \"#rrggbbaa\" (например \"#ffcc88\")"
            )
        };
        let digits = s.strip_prefix('#').ok_or_else(bad)?;
        if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).map_err(|_| bad());
        let (r, g, b, a) = match digits.len() {
            3 => {
                let n = |i: usize| {
                    u8::from_str_radix(&digits[i..=i], 16)
                        .map(|v| v * 17)
                        .map_err(|_| bad())
                };
                (n(0)?, n(1)?, n(2)?, 255)
            }
            6 => (byte(0)?, byte(2)?, byte(4)?, 255),
            8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
            _ => return Err(bad()),
        };
        Ok(Self(
            Color::from_srgb8(r, g, b).with_alpha(f32::from(a) / 255.0),
        ))
    }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Hex::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Фон сцены.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Background {
    /// Светло-серый пол: его красит свет, так лучше видно источники и тени.
    #[default]
    Plain,
    /// Каменная стена и дощатый пол, как в «лаборатории мага».
    Lab,
}

/// Файл сцены целиком.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneFile {
    /// Должно быть [`FORMAT`].
    pub format: String,
    /// Версия формата (сейчас [`VERSION`]).
    pub version: u32,
    #[serde(default)]
    pub background: Background,
    /// Общий тусклый свет, который есть везде, даже в тени.
    #[serde(default = "default_ambient")]
    pub ambient: Hex,
    /// Слои векторных фигур снизу вверх.
    #[serde(default)]
    pub layers: Vec<LayerDesc>,
    #[serde(default)]
    pub lights: Vec<LightDesc>,
    /// Объекты из SDF: рисуются под векторными фигурами.
    #[serde(default)]
    pub objects: Vec<ObjectDesc>,
}

fn default_ambient() -> Hex {
    // Как у сцен этапа 4 (линейный 0,06 / 0,065 / 0,08).
    Hex(Color::linear(0.06, 0.065, 0.08, 1.0))
}

fn one() -> f32 {
    1.0
}

fn one_f64() -> f64 {
    1.0
}

/// Слой фигур.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerDesc {
    /// Имя для сообщений об ошибках.
    #[serde(default)]
    pub name: String,
    /// Фигуры слоя отбрасывают тень от источников света.
    #[serde(default)]
    pub shadow: bool,
    /// Общая прозрачность слоя (1 — непрозрачный).
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default)]
    pub shapes: Vec<ShapeDesc>,
}

type P = (f64, f64);

/// Векторная фигура. У всех: `at` — место, `angle` — поворот в градусах, `color`, и
/// `stroke` — если задан, фигура рисуется контуром этой толщины, а не заливкой.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ShapeDesc {
    /// Прямоугольник с серединой в `at`, размером `size`, скруглением углов `corner`.
    Rect {
        at: P,
        size: P,
        #[serde(default)]
        angle: f64,
        #[serde(default)]
        corner: f64,
        color: Hex,
        #[serde(default)]
        stroke: Option<f64>,
    },
    /// Круг с центром `at` и радиусом `r`.
    Circle {
        at: P,
        r: f64,
        color: Hex,
        #[serde(default)]
        stroke: Option<f64>,
    },
    /// Многоугольник по точкам `points` (относительно `at`).
    Polygon {
        points: Vec<P>,
        #[serde(default)]
        at: P,
        #[serde(default)]
        angle: f64,
        color: Hex,
        #[serde(default)]
        stroke: Option<f64>,
    },
    /// Ломаная линия толщиной `width` по точкам `points` (концы и углы круглые).
    Line {
        points: Vec<P>,
        width: f64,
        #[serde(default)]
        at: P,
        #[serde(default)]
        angle: f64,
        color: Hex,
    },
    /// Контур в записи SVG (`"M 0 0 Q 0.5 1 1 0 Z"`), но y вверх, как во всей сцене.
    /// `scale` увеличивает контур (толщина `stroke` не меняется).
    Path {
        svg: String,
        #[serde(default)]
        at: P,
        #[serde(default)]
        angle: f64,
        #[serde(default = "one_f64")]
        scale: f64,
        color: Hex,
        #[serde(default)]
        stroke: Option<f64>,
    },
}

/// Источник света. `color` и `power` (яркость, может быть больше 1), `reach` — докуда
/// достаёт свет, `size` — размер источника: чем больше, тем мягче края теней.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum LightDesc {
    /// Лампа в точке.
    Point {
        at: P,
        color: Hex,
        #[serde(default = "one")]
        power: f32,
        reach: f32,
        #[serde(default = "default_light_size")]
        size: f32,
    },
    /// Светящийся отрезок (неоновая трубка).
    Tube {
        from: P,
        to: P,
        color: Hex,
        #[serde(default = "one")]
        power: f32,
        reach: f32,
        #[serde(default = "default_light_size")]
        size: f32,
    },
    /// Свет издалека (солнце, луна) со стороны `from` (направление на источник). Не
    /// гаснет; `reach` — длина теней.
    Sun {
        from: P,
        color: Hex,
        #[serde(default = "one")]
        power: f32,
        reach: f32,
        #[serde(default = "default_light_size")]
        size: f32,
    },
}

fn default_light_size() -> f32 {
    0.02
}

/// Объект из SDF: части, слитые плавно (где части рядом, между ними перемычка, как у
/// капель). `blend` — размер перемычки, `glow` — светится сам (не затеняется, даёт
/// свечение), `shadow` — отбрасывает тень.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectDesc {
    #[serde(default)]
    pub name: String,
    pub parts: Vec<PartDesc>,
    #[serde(default = "default_blend")]
    pub blend: f32,
    pub color: Hex,
    #[serde(default = "one")]
    pub power: f32,
    #[serde(default)]
    pub glow: bool,
    #[serde(default)]
    pub shadow: bool,
}

fn default_blend() -> f32 {
    0.04
}

/// Часть SDF-объекта.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PartDesc {
    /// Круг.
    Circle { at: P, r: f32 },
    /// Капсула: отрезок от `from` до `to` толщиной `r`.
    Capsule { from: P, to: P, r: f32 },
}

/// Вид источника для видеокарты.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    /// Лампа (`a == b`) или трубка от `a` до `b`.
    Local,
    /// Свет издалека, `a` — единичное направление на источник.
    Directional,
}

/// Источник, готовый для видеокарты (как `Light` в `reyvec-gpu`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneLight {
    pub kind: LightKind,
    pub a: [f32; 2],
    pub b: [f32; 2],
    /// Линейный цвет, умноженный на яркость.
    pub color: [f32; 3],
    pub reach: f32,
    pub softness: f32,
}

/// Часть объекта: капсула от `a` до `b` толщиной `r` (у круга `a == b`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScenePart {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub r: f32,
}

/// Объект из SDF, готовый для видеокарты.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneObject {
    pub parts: Vec<ScenePart>,
    pub k: f32,
    /// Линейный цвет (у светящихся умножен на яркость) и прозрачность.
    pub color: [f32; 4],
    pub glow: bool,
    pub shadow: bool,
}

/// Сцена, готовая к рисованию.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub background: Background,
    /// Линейный цвет общего света.
    pub ambient: [f32; 3],
    pub items: Vec<SceneItem>,
    pub lights: Vec<SceneLight>,
    pub objects: Vec<SceneObject>,
    /// Сколько векторных фигур в сцене.
    pub shapes: usize,
}

/// Готовые формы фигур: одна форма нарезается на треугольники один раз за работу
/// программы, сколько бы раз файл ни сохраняли.
#[derive(Debug, Default)]
pub struct ShapeCache {
    shapes: HashMap<String, ShapeId>,
}

impl ShapeCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Сколько разных форм уже в библиотеке.
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

impl SceneFile {
    /// Читает и проверяет текст файла сцены.
    pub fn parse(text: &str) -> Result<Self, SceneError> {
        // Блокнот Windows может записать в начало файла отметку UTF-8 (BOM).
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let options = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: SceneFile = options.from_str(text).map_err(|e| SceneError {
            line: Some(e.span.start.line),
            col: Some(e.span.start.col),
            message: russian(&e.code),
        })?;
        file.check()?;
        Ok(file)
    }

    fn check(&self) -> Result<(), SceneError> {
        if self.format != FORMAT {
            return Err(SceneError::new(format!(
                "format: «{}», а нужно \"{FORMAT}\"",
                self.format
            )));
        }
        if self.version != VERSION {
            return Err(SceneError::new(format!(
                "version: {} — эта версия движка понимает только версию {VERSION}",
                self.version
            )));
        }
        if self.lights.len() > MAX_LIGHTS {
            return Err(SceneError::new(format!(
                "источников света {}, а видеокарта берёт не больше {MAX_LIGHTS}",
                self.lights.len()
            )));
        }
        if self.objects.len() > MAX_OBJECTS {
            return Err(SceneError::new(format!(
                "SDF-объектов {}, а видеокарта берёт не больше {MAX_OBJECTS}",
                self.objects.len()
            )));
        }
        let parts: usize = self.objects.iter().map(|o| o.parts.len()).sum();
        if parts > MAX_PARTS {
            return Err(SceneError::new(format!(
                "частей у SDF-объектов всего {parts}, а видеокарта берёт не больше {MAX_PARTS}"
            )));
        }
        for (i, layer) in self.layers.iter().enumerate() {
            let at = || layer_name(i, &layer.name);
            if !(0.0..=1.0).contains(&layer.opacity) {
                return Err(SceneError::new(format!(
                    "{}: opacity {} — нужно от 0 до 1",
                    at(),
                    layer.opacity
                )));
            }
            for (j, shape) in layer.shapes.iter().enumerate() {
                shape
                    .check()
                    .map_err(|m| SceneError::new(format!("{}, фигура {}: {m}", at(), j + 1)))?;
            }
        }
        for (i, light) in self.lights.iter().enumerate() {
            light
                .check()
                .map_err(|m| SceneError::new(format!("источник света {}: {m}", i + 1)))?;
        }
        for (i, object) in self.objects.iter().enumerate() {
            object
                .check()
                .map_err(|m| SceneError::new(format!("{}: {m}", object_name(i, &object.name))))?;
        }
        Ok(())
    }

    /// Превращает описание в сцену для рисования. Новые формы добавляются в `library`
    /// (там они нарезаются на треугольники), уже знакомые берутся из `cache`.
    pub fn build(
        &self,
        library: &mut ShapeLibrary,
        cache: &mut ShapeCache,
    ) -> Result<Scene, SceneError> {
        let mut items = Vec::new();
        let mut shapes = 0;
        for (i, layer) in self.layers.iter().enumerate() {
            let own_layer = layer.shadow || layer.opacity < 1.0;
            if own_layer {
                items.push(SceneItem::BeginLayer {
                    opacity: layer.opacity,
                    blend: BlendMode::Normal,
                    casts_shadow: layer.shadow,
                });
            }
            for (j, shape) in layer.shapes.iter().enumerate() {
                let item = shape.build(library, cache).map_err(|m| {
                    SceneError::new(format!(
                        "{}, фигура {}: {m}",
                        layer_name(i, &layer.name),
                        j + 1
                    ))
                })?;
                items.push(item);
                shapes += 1;
            }
            if own_layer {
                items.push(SceneItem::EndLayer);
            }
        }
        let a = self.ambient.0;
        Ok(Scene {
            background: self.background,
            ambient: [a.r, a.g, a.b],
            items,
            lights: self.lights.iter().map(LightDesc::build).collect(),
            objects: self.objects.iter().map(ObjectDesc::build).collect(),
            shapes,
        })
    }
}

fn layer_name(i: usize, name: &str) -> String {
    if name.is_empty() {
        format!("слой {}", i + 1)
    } else {
        format!("слой {} «{name}»", i + 1)
    }
}

fn object_name(i: usize, name: &str) -> String {
    if name.is_empty() {
        format!("SDF-объект {}", i + 1)
    } else {
        format!("SDF-объект {} «{name}»", i + 1)
    }
}

fn check_point(what: &str, p: P) -> Result<(), String> {
    if p.0.is_finite() && p.1.is_finite() && p.0.abs() <= MAX_COORD && p.1.abs() <= MAX_COORD {
        Ok(())
    } else {
        Err(format!(
            "{what} ({}, {}) — координаты должны быть от −{MAX_COORD} до {MAX_COORD}",
            p.0, p.1
        ))
    }
}

fn check_size(what: &str, v: f64) -> Result<(), String> {
    if v.is_finite() && v > 0.0 && v <= MAX_COORD {
        Ok(())
    } else {
        Err(format!(
            "{what} {v} — нужно больше 0 и не больше {MAX_COORD}"
        ))
    }
}

fn check_stroke(stroke: Option<f64>) -> Result<(), String> {
    stroke.map_or(Ok(()), |w| check_size("stroke", w))
}

impl ShapeDesc {
    fn check(&self) -> Result<(), String> {
        match self {
            Self::Rect {
                at,
                size,
                corner,
                stroke,
                ..
            } => {
                check_point("at", *at)?;
                check_size("ширина size", size.0)?;
                check_size("высота size", size.1)?;
                if !(corner.is_finite() && *corner >= 0.0) {
                    return Err(format!("corner {corner} — нужно 0 или больше"));
                }
                check_stroke(*stroke)
            }
            Self::Circle { at, r, stroke, .. } => {
                check_point("at", *at)?;
                check_size("r", *r)?;
                check_stroke(*stroke)
            }
            Self::Polygon {
                points, at, stroke, ..
            } => {
                check_point("at", *at)?;
                if points.len() < 3 {
                    return Err(format!(
                        "у многоугольника {} точек, нужно хотя бы 3",
                        points.len()
                    ));
                }
                points.iter().try_for_each(|p| check_point("точка", *p))?;
                check_stroke(*stroke)
            }
            Self::Line {
                points, width, at, ..
            } => {
                check_point("at", *at)?;
                if points.len() < 2 {
                    return Err(format!("у линии {} точек, нужно хотя бы 2", points.len()));
                }
                points.iter().try_for_each(|p| check_point("точка", *p))?;
                check_size("width", *width)
            }
            Self::Path {
                svg,
                at,
                scale,
                stroke,
                ..
            } => {
                check_point("at", *at)?;
                check_size("scale", *scale)?;
                BezPath::from_svg(svg).map_err(|e| format!("контур svg: {e}"))?;
                check_stroke(*stroke)
            }
        }
    }

    /// Форма без места, поворота и цвета: ключ для [`ShapeCache`] и контур.
    fn geometry(&self) -> (String, BezPath, Option<f64>) {
        match self {
            Self::Rect {
                size,
                corner,
                stroke,
                ..
            } => {
                let (w, h) = *size;
                let rect = kurbo::Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0);
                let path = if *corner > 0.0 {
                    kurbo::RoundedRect::from_rect(rect, corner.min(w.min(h) / 2.0)).to_path(1e-6)
                } else {
                    rect.to_path(1e-6)
                };
                (format!("rect {w} {h} {corner} {stroke:?}"), path, *stroke)
            }
            Self::Circle { r, stroke, .. } => (
                format!("circle {r} {stroke:?}"),
                kurbo::Circle::new((0.0, 0.0), *r).to_path(1e-6),
                *stroke,
            ),
            Self::Polygon { points, stroke, .. } => {
                let mut path = polyline(points);
                path.close_path();
                (format!("polygon {points:?} {stroke:?}"), path, *stroke)
            }
            Self::Line { points, width, .. } => (
                format!("line {points:?} {width}"),
                polyline(points),
                Some(*width),
            ),
            Self::Path {
                svg, scale, stroke, ..
            } => {
                let mut path = BezPath::from_svg(svg).unwrap_or_default();
                path.apply_affine(Affine::scale(*scale));
                (format!("path {svg} {scale} {stroke:?}"), path, *stroke)
            }
        }
    }

    fn placement(&self) -> (P, f64, Hex) {
        match self {
            Self::Rect {
                at, angle, color, ..
            }
            | Self::Polygon {
                at, angle, color, ..
            }
            | Self::Line {
                at, angle, color, ..
            }
            | Self::Path {
                at, angle, color, ..
            } => (*at, *angle, *color),
            Self::Circle { at, color, .. } => (*at, 0.0, *color),
        }
    }

    fn build(
        &self,
        library: &mut ShapeLibrary,
        cache: &mut ShapeCache,
    ) -> Result<SceneItem, String> {
        let (key, path, stroke) = self.geometry();
        let id = match cache.shapes.get(&key) {
            Some(id) => *id,
            None => {
                let shape = match stroke {
                    Some(w) => Shape::stroke(
                        path,
                        Stroke::new(w)
                            .with_cap(LineCap::Round)
                            .with_join(LineJoin::Round),
                        Color::WHITE,
                    ),
                    None => Shape::fill(path, Color::WHITE),
                };
                let id = library
                    .add(shape)
                    .map_err(|e| format!("не удалось разбить на треугольники: {e}"))?;
                cache.shapes.insert(key, id);
                id
            }
        };
        let (at, angle, color) = self.placement();
        let transform = Affine::translate(at) * Affine::rotate(angle.to_radians());
        Ok(Instance::new(id, transform).with_tint(color.0).into())
    }
}

fn polyline(points: &[P]) -> BezPath {
    let mut path = BezPath::new();
    for (i, p) in points.iter().enumerate() {
        if i == 0 {
            path.move_to(*p);
        } else {
            path.line_to(*p);
        }
    }
    path
}

fn check_light_numbers(power: f32, reach: f32, size: f32) -> Result<(), String> {
    if !(power.is_finite() && (0.0..=100.0).contains(&power)) {
        return Err(format!("power {power} — нужно от 0 до 100"));
    }
    check_size("reach", f64::from(reach))?;
    if !(size.is_finite() && (0.0..=10.0).contains(&size)) {
        return Err(format!("size {size} — нужно от 0 до 10"));
    }
    Ok(())
}

fn f2(p: P) -> [f32; 2] {
    [p.0 as f32, p.1 as f32]
}

fn light_color(color: Hex, power: f32) -> [f32; 3] {
    let c = color.0;
    [c.r * power, c.g * power, c.b * power]
}

impl LightDesc {
    fn check(&self) -> Result<(), String> {
        match self {
            Self::Point {
                at,
                color: _,
                power,
                reach,
                size,
            } => {
                check_point("at", *at)?;
                check_light_numbers(*power, *reach, *size)
            }
            Self::Tube {
                from,
                to,
                color: _,
                power,
                reach,
                size,
            } => {
                check_point("from", *from)?;
                check_point("to", *to)?;
                check_light_numbers(*power, *reach, *size)
            }
            Self::Sun {
                from,
                color: _,
                power,
                reach,
                size,
            } => {
                check_point("from", *from)?;
                if from.0.hypot(from.1) < 1e-6 {
                    return Err(
                        "from (0, 0) — нужно направление на солнце, например (-1, 1)".into(),
                    );
                }
                check_light_numbers(*power, *reach, *size)
            }
        }
    }

    fn build(&self) -> SceneLight {
        match *self {
            Self::Point {
                at,
                color,
                power,
                reach,
                size,
            } => SceneLight {
                kind: LightKind::Local,
                a: f2(at),
                b: f2(at),
                color: light_color(color, power),
                reach,
                softness: size,
            },
            Self::Tube {
                from,
                to,
                color,
                power,
                reach,
                size,
            } => SceneLight {
                kind: LightKind::Local,
                a: f2(from),
                b: f2(to),
                color: light_color(color, power),
                reach,
                softness: size,
            },
            Self::Sun {
                from,
                color,
                power,
                reach,
                size,
            } => {
                let len = from.0.hypot(from.1).max(1e-6);
                SceneLight {
                    kind: LightKind::Directional,
                    a: [(from.0 / len) as f32, (from.1 / len) as f32],
                    b: [0.0, 0.0],
                    color: light_color(color, power),
                    reach,
                    softness: size,
                }
            }
        }
    }
}

impl ObjectDesc {
    fn check(&self) -> Result<(), String> {
        if self.parts.is_empty() {
            return Err("нет частей (parts)".into());
        }
        for part in &self.parts {
            let (points, r) = match *part {
                PartDesc::Circle { at, r } => (vec![at], r),
                PartDesc::Capsule { from, to, r } => (vec![from, to], r),
            };
            points.iter().try_for_each(|p| check_point("часть", *p))?;
            check_size("r", f64::from(r))?;
        }
        if !(self.blend.is_finite() && (0.0..=10.0).contains(&self.blend)) {
            return Err(format!("blend {} — нужно от 0 до 10", self.blend));
        }
        if !(self.power.is_finite() && (0.0..=100.0).contains(&self.power)) {
            return Err(format!("power {} — нужно от 0 до 100", self.power));
        }
        Ok(())
    }

    fn build(&self) -> SceneObject {
        let c = self.color.0;
        let p = self.power;
        SceneObject {
            parts: self
                .parts
                .iter()
                .map(|part| match *part {
                    PartDesc::Circle { at, r } => ScenePart {
                        a: f2(at),
                        b: f2(at),
                        r,
                    },
                    PartDesc::Capsule { from, to, r } => ScenePart {
                        a: f2(from),
                        b: f2(to),
                        r,
                    },
                })
                .collect(),
            k: self.blend,
            color: [c.r * p, c.g * p, c.b * p, c.a],
            glow: self.glow,
            shadow: self.shadow,
        }
    }
}

/// Ошибка разбора RON по-русски.
fn russian(e: &ron::Error) -> String {
    use ron::Error as E;
    let list = |names: &[&str]| {
        names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match e {
        E::Message(m) => m.clone(),
        E::Eof => "файл кончился раньше времени: не хватает закрывающей скобки?".into(),
        E::ExpectedArray => "здесь нужен список в квадратных скобках `[ ... ]`".into(),
        E::ExpectedArrayEnd => "здесь нужна `]` (или запятая между элементами)".into(),
        E::ExpectedComma => "не хватает запятой".into(),
        E::ExpectedFloat | E::ExpectedInteger => "здесь нужно число".into(),
        E::ExpectedString | E::ExpectedStringEnd => {
            "здесь нужна строка в кавычках, например \"#ffcc88\"".into()
        }
        E::ExpectedStructLike | E::ExpectedNamedStructLike(_) => {
            "здесь нужны круглые скобки `( ... )`".into()
        }
        E::ExpectedStructLikeEnd => "здесь нужна `)` (или запятая между полями)".into(),
        E::ExpectedIdentifier => "здесь нужно имя поля или вида, например `at` или `Rect`".into(),
        E::ExpectedDifferentLength { expected, found } => {
            format!("нужно {expected}, а здесь {found} чисел")
        }
        E::NoSuchEnumVariant {
            expected, found, ..
        } => format!("«{found}»: такого нет, можно {}", list(expected)),
        E::NoSuchStructField {
            expected, found, ..
        } => format!("поле «{found}»: такого нет, можно {}", list(expected)),
        E::MissingStructField { field, outer } => match outer {
            Some(o) => format!("у {o} не хватает поля `{field}`"),
            None => format!("не хватает поля `{field}`"),
        },
        E::DuplicateStructField { field, .. } => format!("поле `{field}` записано дважды"),
        E::UnexpectedChar(c) => format!("лишний знак «{c}»"),
        E::UnclosedBlockComment => "комментарий /* не закрыт */".into(),
        E::TrailingCharacters => "после конца сцены ещё что-то написано".into(),
        E::InvalidValueForType { expected, found } => {
            format!("нужно {expected}, а здесь {found}")
        }
        other => other.to_string(),
    }
}

/// Следит за файлом сцены: после сохранения [`SceneWatcher::poll`] отдаёт новый текст.
///
/// Следим за папкой, а не за самим файлом: многие редакторы сохраняют через новый файл и
/// переименование, и слежка за старым файлом при этом теряется. Сохранение часто идёт
/// несколькими записями подряд, поэтому текст читается, когда записи стихли на
/// [`SceneWatcher::QUIET`].
pub struct SceneWatcher {
    path: PathBuf,
    _watcher: notify::RecommendedWatcher,
    events: Receiver<()>,
    pending: Option<Instant>,
}

impl SceneWatcher {
    /// Сколько ждать тишины после последней записи в файл.
    pub const QUIET: Duration = Duration::from_millis(120);

    pub fn new(path: &Path) -> Result<Self, notify::Error> {
        use notify::Watcher as _;

        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let name = path.file_name().map(ToOwned::to_owned);
        let (tx, rx) = channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else { return };
                if !(event.kind.is_modify() || event.kind.is_create()) {
                    return;
                }
                if event
                    .paths
                    .iter()
                    .any(|p| p.file_name().map(ToOwned::to_owned) == name)
                {
                    let _ = tx.send(());
                }
            })?;
        watcher.watch(&dir, notify::RecursiveMode::NonRecursive)?;
        tracing::info!(
            "Слежу за файлом сцены {}: после сохранения сцена обновится сама",
            path.display()
        );
        Ok(Self {
            path,
            _watcher: watcher,
            events: rx,
            pending: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Новый текст файла, если его сохранили и записи стихли. Ошибка чтения тоже
    /// приходит сюда (её стоит показать человеку).
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let now = Instant::now();
        while self.events.try_recv().is_ok() {
            self.pending = Some(now);
        }
        let since = self.pending?;
        if now.duration_since(since) < Self::QUIET {
            return None;
        }
        self.pending = None;
        Some(
            std::fs::read_to_string(&self.path)
                .map_err(|e| format!("не прочитать {}: {e}", self.path.display())),
        )
    }
}

#[cfg(test)]
mod tests;
