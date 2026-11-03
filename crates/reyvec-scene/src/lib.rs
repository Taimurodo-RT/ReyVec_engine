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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use reyvec_vector::kurbo::{self, Shape as _};
use reyvec_vector::{
    Affine, BezPath, Color, LineCap, LineJoin, SceneItem, Shape, ShapeBatch, ShapeId, ShapeLibrary,
    ShapeSink, Stroke, Tessellator,
};
use serde::Deserialize;

/// Пример сцены «первая комната» (`scenes/first-room.ron`): его показывает песочница,
/// если своего файла нет рядом.
pub const EXAMPLE: &str = include_str!("../../../scenes/first-room.ron");

/// Папка примера в исходниках: от неё считаются пути к рисункам примера (5.5), когда
/// пример берётся встроенным, а не из файла.
pub const EXAMPLE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes");

/// Отметка формата в начале файла.
pub const FORMAT: &str = "reyvec-scene";
/// Версия формата, которую понимает этот движок.
pub const VERSION: u32 = 1;
/// Сколько источников света, объектов и их частей берёт видеокарта за кадр (как в
/// `reyvec-gpu`: `MAX_LIGHTS`, `MAX_OBJECTS`, `MAX_BLOBS`).
pub const MAX_LIGHTS: usize = 16;
pub const MAX_OBJECTS: usize = 16;
pub const MAX_PARTS: usize = 128;
/// Сколько источников и объектов можно задать во всей сцене: в кадр идут только те, что
/// на экране (5.5в), не больше [`MAX_LIGHTS`] и [`MAX_OBJECTS`]. Предел всей сцены — от
/// опечаток (времени процессора на тысячу источников уходит немного).
pub const MAX_SCENE_LIGHTS: usize = 1024;
pub const MAX_SCENE_OBJECTS: usize = 1024;
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
    /// Предметы сцены (5.2): у каждого место, глубина, поворот, размер, свои фигуры,
    /// источники и SDF-объекты, а также вложенные предметы, которые двигаются вместе с ним.
    #[serde(default)]
    pub nodes: Vec<NodeDesc>,
    /// Слои векторных фигур снизу вверх (способ 5.1; слой — это предмет без места).
    #[serde(default)]
    pub layers: Vec<LayerDesc>,
    #[serde(default)]
    pub lights: Vec<LightDesc>,
    /// Объекты из SDF: рисуются под векторными фигурами.
    #[serde(default)]
    pub objects: Vec<ObjectDesc>,
    /// Папка файла сцены: от неё считаются пути к рисункам (5.5). В файле не пишется.
    #[serde(skip)]
    pub dir: Option<PathBuf>,
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

/// Предмет сцены. Место `at`, поворот `angle` и размер `scale` действуют на всё, что в
/// нём: фигуры, источники, SDF-объекты и вложенные предметы (`children`). Глубина
/// `depth` складывается с глубиной родителя: что глубже (больше), то рисуется поверх.
/// `shadow` — фигуры отбрасывают тень; если не задано, берётся у родителя.
/// `opacity` — общая прозрачность, умножается на прозрачность родителя.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDesc {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub at: P,
    #[serde(default)]
    pub depth: f64,
    #[serde(default)]
    pub angle: f64,
    #[serde(default = "one_f64")]
    pub scale: f64,
    #[serde(default)]
    pub shadow: Option<bool>,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default)]
    pub shapes: Vec<ShapeDesc>,
    #[serde(default)]
    pub lights: Vec<LightDesc>,
    #[serde(default)]
    pub objects: Vec<ObjectDesc>,
    #[serde(default)]
    pub children: Vec<NodeDesc>,
    /// Движение (5.3): вращение, градусов в секунду (больше 0 — против часовой).
    #[serde(default)]
    pub spin: f64,
    /// Качание вокруг своего места, как у маятника.
    #[serde(default)]
    pub swing: Option<Swing>,
    /// Покачивание туда-обратно на `by` от своего места.
    #[serde(default)]
    pub bob: Option<Bob>,
    /// Рисунки SVG из файлов (5.5).
    #[serde(default)]
    pub pictures: Vec<PictureDesc>,
    /// Перетекающие фигуры из файлов Motif (5.5).
    #[serde(default)]
    pub morphs: Vec<MorphDesc>,
    /// Невидимые твёрдые прямоугольники (6.1): пол, стены, края уровня.
    #[serde(default)]
    pub solids: Vec<SolidDesc>,
    /// Уровень из тайлов (6.4): клетки буквами, левый верхний угол сетки — место
    /// предмета.
    #[serde(default)]
    pub tiles: Option<TilesDesc>,
    /// Этот предмет — персонаж игрока (6.1): его двигает управление, `at` — середина
    /// прямоугольника столкновений. Только у предмета верхнего уровня, один на сцену.
    #[serde(default)]
    pub player: Option<PlayerDesc>,
}

/// Невидимый твёрдый прямоугольник с серединой `at` и размером `size`.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolidDesc {
    pub at: P,
    pub size: P,
    /// Платформа: держит только сверху.
    #[serde(default)]
    pub oneway: bool,
}

/// Настройки персонажа игрока. Всё можно не писать: тогда берётся как в
/// [`reyvec_game::Tuning::default`]. Расстояния в единицах сцены (высота кадра — 2),
/// время в секундах.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PlayerDesc {
    /// Ширина и высота прямоугольника столкновений.
    pub size: P,
    /// Скорость бега.
    pub speed: f64,
    /// Разгон и торможение на земле, разгон в воздухе (единиц в секунду за секунду).
    pub accel: f64,
    pub decel: f64,
    pub air_accel: f64,
    /// Высота прыжка с зажатой кнопкой и время до верхней точки.
    pub jump: f64,
    pub jump_time: f64,
    /// Во сколько раз сильнее тянет вниз при падении или отпущенной кнопке.
    pub fall: f64,
    pub max_fall: f64,
    /// Время койота и запоминание нажатия прыжка, секунды.
    pub coyote: f64,
    pub buffer: f64,
    /// Рывок: скорость и длительность (секунды; 0 — без рывка).
    pub dash_speed: f64,
    pub dash_time: f64,
    /// Скорость скольжения по стене вниз.
    pub wall_slide: f64,
    /// Прыжок от стены: во сколько раз скорости бега отталкивает вбок (0 — нельзя).
    pub wall_jump: f64,
    /// Можно цепляться за край (6.4).
    pub ledge: bool,
}

impl Default for PlayerDesc {
    fn default() -> Self {
        let t = reyvec_game::Tuning::default();
        Self {
            size: (t.size[0], t.size[1]),
            speed: t.speed,
            accel: t.accel,
            decel: t.decel,
            air_accel: t.air_accel,
            jump: t.jump,
            jump_time: t.jump_time,
            fall: t.fall,
            max_fall: t.max_fall,
            coyote: t.coyote,
            buffer: t.buffer,
            dash_speed: t.dash_speed,
            dash_time: t.dash_time,
            wall_slide: t.wall_slide,
            wall_jump: t.wall_jump,
            ledge: t.ledge,
        }
    }
}

impl PlayerDesc {
    pub(crate) fn tuning(&self) -> reyvec_game::Tuning {
        reyvec_game::Tuning {
            size: [self.size.0, self.size.1],
            speed: self.speed,
            accel: self.accel,
            decel: self.decel,
            air_accel: self.air_accel,
            jump: self.jump,
            jump_time: self.jump_time,
            fall: self.fall,
            max_fall: self.max_fall,
            coyote: self.coyote,
            buffer: self.buffer,
            dash_speed: self.dash_speed,
            dash_time: self.dash_time,
            wall_slide: self.wall_slide,
            wall_jump: self.wall_jump,
            ledge: self.ledge,
        }
    }

    fn check(&self) -> Result<(), String> {
        check_size("ширина size", self.size.0)?;
        check_size("высота size", self.size.1)?;
        let positive = [
            ("speed", self.speed),
            ("accel", self.accel),
            ("decel", self.decel),
            ("air_accel", self.air_accel),
            ("jump", self.jump),
            ("jump_time", self.jump_time),
            ("max_fall", self.max_fall),
            ("dash_speed", self.dash_speed),
            ("wall_slide", self.wall_slide),
        ];
        for (name, v) in positive {
            if !(v.is_finite() && v > 0.0 && v <= 1000.0) {
                return Err(format!("{name} {v} — нужно больше 0 и не больше 1000"));
            }
        }
        if !(self.fall.is_finite() && (1.0..=10.0).contains(&self.fall)) {
            return Err(format!("fall {} — нужно от 1 до 10", self.fall));
        }
        if !(self.wall_jump.is_finite() && (0.0..=10.0).contains(&self.wall_jump)) {
            return Err(format!("wall_jump {} — нужно от 0 до 10", self.wall_jump));
        }
        for (name, v) in [
            ("coyote", self.coyote),
            ("buffer", self.buffer),
            ("dash_time", self.dash_time),
        ] {
            if !(v.is_finite() && (0.0..=1.0).contains(&v)) {
                return Err(format!("{name} {v} — нужно от 0 до 1 секунды"));
            }
        }
        Ok(())
    }
}

/// Качание: на `angle` градусов в обе стороны, полный взмах за `period` секунд.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Swing {
    pub angle: f64,
    pub period: f64,
}

/// Покачивание: сдвиг до `by` в обе стороны, туда и обратно за `period` секунд.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bob {
    pub by: P,
    pub period: f64,
}

impl NodeDesc {
    /// Предмет из слоя способа 5.1: без места, на глубине по номеру слоя.
    fn from_layer(i: usize, layer: &LayerDesc) -> Self {
        Self {
            name: layer.name.clone(),
            at: (0.0, 0.0),
            depth: i as f64,
            angle: 0.0,
            scale: 1.0,
            shadow: Some(layer.shadow),
            opacity: layer.opacity,
            shapes: layer.shapes.clone(),
            lights: Vec::new(),
            objects: Vec::new(),
            children: Vec::new(),
            spin: 0.0,
            swing: None,
            bob: None,
            pictures: Vec::new(),
            morphs: Vec::new(),
            solids: Vec::new(),
            tiles: None,
            player: None,
        }
    }

    /// Все предметы файла: слои 5.1 (как предметы) и предметы 5.2.
    pub(crate) fn all(file: &SceneFile) -> Vec<NodeDesc> {
        file.layers
            .iter()
            .enumerate()
            .map(|(i, l)| Self::from_layer(i, l))
            .chain(file.nodes.iter().cloned())
            .collect()
    }

    fn count(&self, f: &dyn Fn(&NodeDesc) -> usize) -> usize {
        f(self) + self.children.iter().map(|c| c.count(f)).sum::<usize>()
    }

    /// Наибольшее `f` по предмету и всем вложенным.
    fn count_max(&self, f: &dyn Fn(&NodeDesc) -> usize) -> usize {
        self.children
            .iter()
            .map(|c| c.count_max(f))
            .fold(f(self), usize::max)
    }

    fn check(&self, path: &str) -> Result<(), SceneError> {
        let here = if self.name.is_empty() {
            path.to_string()
        } else {
            format!("{path} «{}»", self.name)
        };
        let fail = |m: String| SceneError::new(format!("{here}: {m}"));
        check_point("at", self.at).map_err(fail)?;
        if !(self.depth.is_finite() && self.depth.abs() <= MAX_COORD) {
            return Err(fail(format!(
                "depth {} — нужно от −{MAX_COORD} до {MAX_COORD}",
                self.depth
            )));
        }
        if !self.angle.is_finite() {
            return Err(fail(format!("angle {} — нужно число", self.angle)));
        }
        check_size("scale", self.scale).map_err(fail)?;
        if !(self.spin.is_finite() && self.spin.abs() <= 36_000.0) {
            return Err(fail(format!(
                "spin {} — нужно от −36000 до 36000 градусов в секунду",
                self.spin
            )));
        }
        if let Some(w) = self.swing {
            if !(w.angle.is_finite() && w.angle.abs() <= 360.0) {
                return Err(fail(format!(
                    "swing: angle {} — нужно от −360 до 360",
                    w.angle
                )));
            }
            check_period(w.period).map_err(fail)?;
        }
        if let Some(b) = self.bob {
            check_point("bob: by", b.by).map_err(fail)?;
            check_period(b.period).map_err(fail)?;
        }
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err(fail(format!("opacity {} — нужно от 0 до 1", self.opacity)));
        }
        for (j, shape) in self.shapes.iter().enumerate() {
            shape
                .check()
                .map_err(|m| SceneError::new(format!("{here}, фигура {}: {m}", j + 1)))?;
        }
        for (j, light) in self.lights.iter().enumerate() {
            light
                .check()
                .map_err(|m| SceneError::new(format!("{here}, источник света {}: {m}", j + 1)))?;
        }
        for (j, object) in self.objects.iter().enumerate() {
            object.check().map_err(|m| {
                SceneError::new(format!("{here}, {}: {m}", object_name(j, &object.name)))
            })?;
        }
        for (j, p) in self.pictures.iter().enumerate() {
            p.check()
                .map_err(|m| SceneError::new(format!("{here}, рисунок {}: {m}", j + 1)))?;
        }
        for (j, m) in self.morphs.iter().enumerate() {
            m.check()
                .map_err(|e| SceneError::new(format!("{here}, фигура Motif {}: {e}", j + 1)))?;
        }
        for (j, solid) in self.solids.iter().enumerate() {
            check_point("at", solid.at)
                .and_then(|()| check_size("ширина size", solid.size.0))
                .and_then(|()| check_size("высота size", solid.size.1))
                .map_err(|m| SceneError::new(format!("{here}, твёрдое {}: {m}", j + 1)))?;
        }
        if let Some(t) = &self.tiles {
            t.check()
                .map_err(|m| SceneError::new(format!("{here}, tiles: {m}")))?;
        }
        if let Some(p) = &self.player {
            p.check()
                .map_err(|m| SceneError::new(format!("{here}, player: {m}")))?;
        }
        for (j, child) in self.children.iter().enumerate() {
            if child.player.is_some() {
                return Err(SceneError::new(format!(
                    "{here} → предмет {}: player можно только у предмета верхнего уровня \
                     (прямо в nodes), а не внутри другого",
                    j + 1
                )));
            }
            child.check(&format!("{here} → предмет {}", j + 1))?;
        }
        Ok(())
    }
}

/// Векторная фигура. У всех: `at` — место, `angle` — поворот в градусах, `color`, и
/// `stroke` — если задан, фигура рисуется контуром этой толщины, а не заливкой.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ShapeDesc {
    /// Прямоугольник с серединой в `at`, размером `size`, скруглением углов `corner`.
    /// `solid: true` — твёрдый: персонаж на него встаёт и сквозь него не проходит (6.1).
    /// `oneway: true` — платформа: держит только сверху, снизу и сбоку проходишь (6.2).
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
        #[serde(default)]
        solid: bool,
        #[serde(default)]
        oneway: bool,
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
    /// Сколько сущностей в мире сцены (предметы, фигуры, источники, объекты).
    pub entities: usize,
    /// Персонаж игрока, если он есть в сцене (6.1): за ним идёт камера.
    pub player: Option<PlayerView>,
    /// Последнее нажатие, которое логика уже взяла в шаг (отметка времени ввода, 0 —
    /// не было): кадр с этим снимком — первый, где видно ответ на нажатие (6.3).
    pub input_stamp: u64,
}

/// Что видно о персонаже игрока снаружи мира: для камеры и панели.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerView {
    /// Середина прямоугольника столкновений.
    pub at: [f32; 2],
    pub vel: [f32; 2],
    pub on_ground: bool,
    /// Висит на краю (6.4).
    pub hanging: bool,
    /// Твёрдых прямоугольников в сцене.
    pub solids: usize,
}

/// Готовые формы фигур: одна форма нарезается на треугольники один раз за работу
/// программы, сколько бы раз файл ни сохраняли.
#[derive(Debug, Default)]
pub struct ShapeCache {
    shapes: HashMap<String, ShapeId>,
    /// Рисунки SVG и файлы Motif (5.5) по пути, размеру и времени изменения файла.
    svgs: HashMap<String, Arc<SvgDrawing>>,
    motifs: HashMap<String, Arc<MorphFile>>,
    /// Ключи, которые спросил последний построенный мир ([`SceneWorld::new`]): всё
    /// остальное можно выбросить ([`Self::release_unused`], 5.5б).
    used: HashSet<String>,
}

/// Что выброшено из библиотеки ([`ShapeCache::release_unused`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Released {
    /// Фигур (вместе с уровнями детализации).
    pub shapes: usize,
    /// Рисунков SVG и файлов Motif.
    pub files: usize,
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

    /// Ключи готовых форм: загрузчику в фоне, чтобы не резать их снова
    /// ([`SceneFile::prepare`]).
    pub fn keys(&self) -> HashSet<String> {
        self.shapes
            .keys()
            .chain(self.svgs.keys())
            .chain(self.motifs.keys())
            .cloned()
            .collect()
    }

    /// Сколько разных рисунков SVG и файлов Motif уже загружено.
    pub fn files(&self) -> usize {
        self.svgs.len() + self.motifs.len()
    }

    /// Начало постройки мира: отметки «нужно» сбрасываются, мир ставит их заново.
    pub(crate) fn begin_use(&mut self) {
        self.used.clear();
    }

    /// Отметка: форма или файл с ключом `key` нужны строящемуся миру.
    pub(crate) fn mark_used(&mut self, key: &str) {
        if !self.used.contains(key) {
            self.used.insert(key.to_owned());
        }
    }

    /// Удаляет из кэша и из `library` формы, рисунки и файлы Motif, которые не спросил
    /// последний построенный мир. Звать после того, как этот мир принят (стал рисоваться):
    /// пока рисуется старый мир, его формы нужны. Если постройка не удалась, отметки
    /// неполные — тогда не звать, а дождаться следующего удачного мира.
    pub fn release_unused(&mut self, library: &mut ShapeLibrary) -> Released {
        let mut released = Released::default();
        let used = &self.used;
        self.shapes.retain(|key, id| {
            let keep = used.contains(key);
            if !keep {
                released.shapes += library.remove(*id);
            }
            keep
        });
        self.svgs.retain(|key, drawing| {
            let keep = used.contains(key);
            if !keep {
                released.files += 1;
                released.shapes += drawing.shapes().map(|id| library.remove(id)).sum::<usize>();
            }
            keep
        });
        self.motifs.retain(|key, file| {
            let keep = used.contains(key);
            if !keep {
                released.files += 1;
                released.shapes += file.shape_ids().map(|id| library.remove(id)).sum::<usize>();
            }
            keep
        });
        if released != Released::default() {
            tracing::info!(
                "Сцена: выброшено ненужное — {} фигур, {} файлов",
                released.shapes,
                released.files
            );
        }
        released
    }
}

/// Файл сцены, разобранный в фоне, с уже нарезанными новыми формами (этап 5.4). Потоку
/// кадра остаётся положить формы в набор и построить мир ([`Self::finish`]).
pub struct PreparedScene {
    file: SceneFile,
    batch: ShapeBatch,
    /// Ключ формы и её номер в пачке.
    keys: Vec<(String, ShapeId)>,
    svgs: Vec<(String, SvgDrawing)>,
    motifs: Vec<(String, MorphFile)>,
}

impl PreparedScene {
    /// Сколько новых форм нарезано.
    pub fn new_shapes(&self) -> usize {
        self.batch.len()
    }

    /// Сколько рисунков SVG и файлов Motif прочитано.
    pub fn new_files(&self) -> usize {
        self.svgs.len() + self.motifs.len()
    }

    /// Кладёт нарезанные формы в `library` и строит мир сцены. Резать уже нечего:
    /// все формы найдутся в `cache` (кроме тех, что разбить не удалось — тогда ошибка
    /// та же, что у [`SceneWorld::new`]).
    pub fn finish(
        self,
        library: &mut ShapeLibrary,
        cache: &mut ShapeCache,
    ) -> Result<SceneWorld, SceneError> {
        use std::collections::hash_map::Entry;
        let first = library.append(self.batch);
        // Если такую форму успели положить раньше (сцену сохранили дважды подряд),
        // лишняя копия сразу удаляется.
        for (key, id) in self.keys {
            let id = id.offset(first);
            match cache.shapes.entry(key) {
                Entry::Occupied(_) => _ = library.remove(id),
                Entry::Vacant(v) => _ = v.insert(id),
            }
        }
        for (key, mut drawing) in self.svgs {
            drawing.offset_shapes(first);
            match cache.svgs.entry(key) {
                Entry::Occupied(_) => drawing.shapes().for_each(|id| _ = library.remove(id)),
                Entry::Vacant(v) => _ = v.insert(Arc::new(drawing)),
            }
        }
        for (key, mut motif) in self.motifs {
            motif.offset_shapes(first);
            match cache.motifs.entry(key) {
                Entry::Occupied(_) => motif.shape_ids().for_each(|id| _ = library.remove(id)),
                Entry::Vacant(v) => _ = v.insert(Arc::new(motif)),
            }
        }
        SceneWorld::new(&self.file, library, cache)
    }
}

impl SceneFile {
    /// Тяжёлая часть загрузки, которой не нужен набор фигур: разбор текста, проверка и
    /// нарезка форм, которых нет среди `known` ([`ShapeCache::keys`]). Годится для
    /// фонового потока; потом [`PreparedScene::finish`] в потоке кадра.
    pub fn prepare(
        text: &str,
        dir: Option<&Path>,
        tessellator: Tessellator,
        known: &HashSet<String>,
    ) -> Result<PreparedScene, SceneError> {
        let file = Self::parse_in(text, dir)?;
        let mut batch = ShapeBatch::new(tessellator);
        let mut keys = Vec::new();
        let mut svgs = Vec::new();
        let mut motifs = Vec::new();
        let mut seen = HashSet::new();
        fn walk(node: &NodeDesc, f: &mut dyn FnMut(&NodeDesc)) {
            f(node);
            for child in &node.children {
                walk(child, f);
            }
        }
        // Что не удалось (нет файла, не разбилась форма), не страшно: `finish` попробует
        // ещё раз и скажет, где ошибка.
        for node in NodeDesc::all(&file) {
            walk(&node, &mut |n| {
                let tiles = n
                    .tiles
                    .as_ref()
                    .and_then(|t| t.build().ok())
                    .map(|level| level.shapes)
                    .unwrap_or_default();
                for shape in n.shapes.iter().chain(&tiles) {
                    let (key, path, stroke) = shape.geometry();
                    if known.contains(&key) || !seen.insert(key.clone()) {
                        continue;
                    }
                    if let Ok(id) = batch.add(white_shape(path, stroke)) {
                        keys.push((key, id));
                    }
                }
                for p in &n.pictures {
                    let Ok(f) = assets::picture_file(file.dir.as_deref(), p) else {
                        continue;
                    };
                    if known.contains(&f.key) || !seen.insert(f.key.clone()) {
                        continue;
                    }
                    if let Ok(d) = assets::load_picture(&f, p.height, &mut batch) {
                        svgs.push((f.key, d));
                    }
                }
                for m in &n.morphs {
                    let Ok(f) = assets::motif_file(file.dir.as_deref(), m) else {
                        continue;
                    };
                    if known.contains(&f.key) || !seen.insert(f.key.clone()) {
                        continue;
                    }
                    if let Ok(motif) = assets::load_motif(&f, &mut batch) {
                        motifs.push((f.key, motif));
                    }
                }
            });
        }
        Ok(PreparedScene {
            file,
            batch,
            keys,
            svgs,
            motifs,
        })
    }

    /// Как [`Self::parse`], но для файла из папки `dir`: от неё считаются пути к
    /// рисункам (5.5).
    pub fn parse_in(text: &str, dir: Option<&Path>) -> Result<Self, SceneError> {
        let mut file = Self::parse(text)?;
        file.dir = dir.map(Path::to_path_buf);
        Ok(file)
    }

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
        let nodes: usize = self
            .nodes
            .iter()
            .map(|n| n.count(&|n| n.lights.len()))
            .sum();
        let lights = self.lights.len() + nodes;
        if lights > MAX_SCENE_LIGHTS {
            return Err(SceneError::new(format!(
                "источников света {lights}, а в сцене можно не больше {MAX_SCENE_LIGHTS}"
            )));
        }
        let nodes: usize = self
            .nodes
            .iter()
            .map(|n| n.count(&|n| n.objects.len()))
            .sum();
        let objects = self.objects.len() + nodes;
        if objects > MAX_SCENE_OBJECTS {
            return Err(SceneError::new(format!(
                "SDF-объектов {objects}, а в сцене можно не больше {MAX_SCENE_OBJECTS}"
            )));
        }
        // Частей у одного объекта не больше, чем видеокарта берёт за кадр на все объекты.
        let parts_of = |os: &[ObjectDesc]| os.iter().map(|o| o.parts.len()).max().unwrap_or(0);
        let parts: usize = parts_of(&self.objects).max(
            self.nodes
                .iter()
                .map(|n| n.count_max(&|n| parts_of(&n.objects)))
                .max()
                .unwrap_or(0),
        );
        if parts > MAX_PARTS {
            return Err(SceneError::new(format!(
                "частей у SDF-объекта {parts}, а видеокарта берёт за кадр не больше {MAX_PARTS}"
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
        for (i, node) in self.nodes.iter().enumerate() {
            node.check(&format!("предмет {}", i + 1))?;
        }
        let players = self.nodes.iter().filter(|n| n.player.is_some()).count();
        if players > 1 {
            return Err(SceneError::new(format!(
                "player задан у {players} предметов, а персонаж игрока в сцене один"
            )));
        }
        Ok(())
    }

    /// Превращает описание в сцену для рисования: строит мир сцены ([`SceneWorld`]),
    /// один раз прогоняет его системы и берёт снимок. Новые формы добавляются в `library`
    /// (там они нарезаются на треугольники), уже знакомые берутся из `cache`.
    pub fn build(
        &self,
        library: &mut ShapeLibrary,
        cache: &mut ShapeCache,
    ) -> Result<Scene, SceneError> {
        let mut world = SceneWorld::new(self, library, cache)?;
        world.update();
        Ok(world.snapshot())
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

fn check_period(period: f64) -> Result<(), String> {
    if period.is_finite() && (0.01..=3600.0).contains(&period) {
        Ok(())
    } else {
        Err(format!("period {period} — нужно от 0.01 до 3600 секунд"))
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

    pub(crate) fn placement(&self) -> (P, f64, Hex) {
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

    /// Форма фигуры в библиотеке: знакомая из `cache` или новая (нарезается здесь).
    pub(crate) fn shape_id(
        &self,
        library: &mut ShapeLibrary,
        cache: &mut ShapeCache,
    ) -> Result<ShapeId, String> {
        let (key, path, stroke) = self.geometry();
        cache.mark_used(&key);
        let id = match cache.shapes.get(&key) {
            Some(id) => *id,
            None => {
                let id = library
                    .add(white_shape(path, stroke))
                    .map_err(|e| format!("не удалось разбить на треугольники: {e}"))?;
                cache.shapes.insert(key, id);
                id
            }
        };
        Ok(id)
    }
}

/// Форма фигуры сцены: белая (цвет даёт оттенок экземпляра), обводка с круглыми
/// концами и углами.
fn white_shape(path: BezPath, stroke: Option<f64>) -> Shape {
    match stroke {
        Some(w) => Shape::stroke(
            path,
            Stroke::new(w)
                .with_cap(LineCap::Round)
                .with_join(LineJoin::Round),
            Color::WHITE,
        ),
        None => Shape::fill(path, Color::WHITE),
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

    pub(crate) fn build(&self) -> SceneLight {
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

    pub(crate) fn build(&self) -> SceneObject {
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

mod assets;
mod loader;
mod logic;
pub use assets::{MorphDesc, MorphTrack, PictureDesc};
use reyvec_motif::MorphFile;
use reyvec_svg::SvgDrawing;
mod tiles;
mod world;
pub use loader::{LoadJob, LoadResult, Loaded, Loader};
pub use logic::{STEP, SceneLogic};
/// Игровые системы (этап 6): ввод и контроллер персонажа.
pub use reyvec_game as game;
pub use tiles::TilesDesc;
pub use world::{
    GlobalPlacement, LightSource, Motion, Placement, SceneTime, SceneWorld, SdfShape, SvgPicture,
    VectorShape,
};

#[cfg(test)]
mod tests;
