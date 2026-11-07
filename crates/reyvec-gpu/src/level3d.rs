//! 2.5D-вид уровня (этап 7.9): тот же уровень из файла сцены (этап 6), только
//! нарисованный реймарчингом в объёме. Игра не меняется: логика, столкновения и герой те
//! же, меняется лишь картинка — каждый кадр снимок мира превращается в объёмную сцену.
//!
//! Как плоское становится объёмным:
//! - **твёрдое** (столкновения уровня: пол, стены, полки, тайлы) — бруски глубиной
//!   [`SOLID_BACK`]…[`SOLID_FRONT`] по Z. По ним герой и ходит, поэтому в объёме видно,
//!   на что он встаёт;
//! - **фигуры** векторного слоя (герой, предметы, рисунок тайлов) — выдавленные пластины
//!   толщиной [`PLATE`] у переднего края брусков. Порядок рисования сохраняется: каждая
//!   следующая фигура на [`LAYER_STEP`] ближе к камере, как в 2D она рисуется поверх;
//! - **лампы** — лампы объёмной сцены перед уровнем; первые две (ближние к камере)
//!   отбрасывают тени;
//! - **SDF-объекты** (капли слизи, лампочки) — капсулы, светящиеся остаются светящимися;
//! - сзади **стена**, снизу **земля**.
//!
//! Плоская сцена в своих единицах (герой ростом 0,28), а свет и тени объёма настроены на
//! метры (затенение в углах видит на метр вокруг). Поэтому уровень увеличен в
//! [`LEVEL_SCALE`] раз: герой около 1,1 м.
//!
//! Форма фигуры (её поле расстояний) готовится один раз на фигуру библиотеки и потом
//! только переставляется: сдвиг, поворот, размер и отражение героя, бегущего влево.

use std::collections::HashMap;

use glam::Vec3;
use reyvec_sdf::profile2d::Profile;
use reyvec_sdf::shape3d::Shape3;
use reyvec_sdf::space_file::SpaceFrame;
use reyvec_sdf::{Camera3D, Lamp3, Material3};
use reyvec_vector::{
    Affine, BezPath, Color, Paint, PathEl, SceneItem, ShapeId, ShapeLibrary, kurbo,
};

/// Во сколько раз уровень больше в объёме, чем на плоскости.
pub(crate) const LEVEL_SCALE: f32 = 4.0;
/// Передний и задний края твёрдого, метры по Z.
pub(crate) const SOLID_FRONT: f32 = 1.0;
pub(crate) const SOLID_BACK: f32 = -0.6;
/// Толщина пластины фигуры, метры.
const PLATE: f32 = 0.3;
/// Насколько каждая следующая по порядку рисования фигура ближе к камере, метры.
const LAYER_STEP: f32 = 0.002;
/// Самая мелкая клетка поля фигуры в единицах плоской сцены (4 мм в объёме).
const PROFILE_CELL: f32 = 0.001;
/// Самая мелкая клетка поля, если фигура мелкая: поле не больше нужного.
const MIN_PROFILE_CELLS: f32 = 24.0;
/// Наклон камеры вниз, градусы: видно верх брусков, но вид остаётся «сбоку».
const CAMERA_PITCH_DEG: f32 = 12.0;
/// Ламп в кадре не больше (как в сцене «3D» из файла: свет перебирается на каждую точку).
const MAX_LAMPS: usize = 8;
/// Где по Z стоят лампы, метры: перед уровнем, чтобы светили на лицевые стороны.
const LAMP_Z: f32 = SOLID_FRONT + 0.8;
/// Светящийся объект ближе этого к лампе (единицы 2D) — её огонёк.
const LAMP_NEAR: f32 = 0.05;
/// Больше форм в кэше не держим: после смены сцены старые фигуры не нужны.
const MAX_CACHED: usize = 8192;

/// Цвета фона «лаборатории» (каменная стена, тёмная земля) и твёрдого (тёсаный камень),
/// линейные.
const WALL: [f32; 3] = [0.10, 0.075, 0.06];
const GROUND: [f32; 3] = [0.035, 0.028, 0.022];
const SOLID: [f32; 3] = [0.10, 0.085, 0.07];

/// Форма фигуры в её собственных координатах, без толщины.
#[derive(Clone, Debug)]
enum Flat {
    /// Прямоугольник по осям: середина и размер (самый частый случай — без поля).
    Rect { center: [f32; 2], size: [f32; 2] },
    /// Любой контур: запечённое поле.
    Profile(Profile),
}

/// 2.5D-вид: формы фигур, готовые к перестановке.
#[derive(Default)]
pub(crate) struct Level3d {
    flats: HashMap<ShapeId, Option<Flat>>,
}

/// Объёмная сцена одного кадра и её материалы.
pub(crate) struct LevelFrame {
    pub frame: SpaceFrame,
    pub materials: Vec<Material3>,
}

/// Материалы кадра: одинаковые цвета — один материал.
#[derive(Default)]
struct Materials {
    list: Vec<Material3>,
    index: HashMap<([u16; 3], u32), u32>,
}

impl Materials {
    fn get(&mut self, color: [f32; 3], glow: f32) -> u32 {
        let key = (
            color.map(|c| (c.clamp(0.0, 4.0) * 4096.0) as u16),
            glow.to_bits(),
        );
        *self.index.entry(key).or_insert_with(|| {
            self.list.push(Material3 {
                color,
                glow,
                grid: false,
            });
            self.list.len() as u32 - 1
        })
    }
}

impl Level3d {
    /// Забывает формы фигур (сцена сменилась).
    pub fn clear(&mut self) {
        self.flats.clear();
    }

    /// Объёмная сцена из снимка мира `scene`; формы фигур берутся из `library`.
    pub fn frame(&mut self, scene: &reyvec_scene::Scene, library: &ShapeLibrary) -> LevelFrame {
        if self.flats.len() > MAX_CACHED {
            self.flats.clear();
        }
        let mut materials = Materials::default();
        let mut shapes = Vec::new();
        let s = LEVEL_SCALE;

        // Стена и земля.
        let wall = materials.get(WALL, 0.0);
        shapes.push(
            Shape3::Plane {
                normal: [0.0, 0.0, 1.0],
                offset: SOLID_BACK,
            }
            .paint(wall),
        );
        let ground_y = scene
            .solids
            .iter()
            .map(|c| c.aabb.min[1] as f32)
            .fold(f32::INFINITY, f32::min);
        if ground_y.is_finite() {
            let ground = materials.get(GROUND, 0.0);
            shapes.push(
                Shape3::Plane {
                    normal: [0.0, 1.0, 0.0],
                    offset: ground_y * s,
                }
                .paint(ground),
            );
        }

        // Твёрдое: бруски.
        let solid = materials.get(SOLID, 0.0);
        for c in &scene.solids {
            if let Some(shape) = solid_shape(c) {
                shapes.push(shape.paint(solid));
            }
        }

        // Фигуры: пластины по порядку рисования.
        let mut layer = 0u32;
        let mut mask_depth = 0u32;
        for item in &scene.items {
            match item {
                SceneItem::BeginMask { .. } => mask_depth += 1,
                SceneItem::EndMask => mask_depth = mask_depth.saturating_sub(1),
                SceneItem::Shape(instance) if mask_depth == 0 => {
                    let Some(color) = shape_color(library, instance.shape, instance.tint) else {
                        continue;
                    };
                    let Some(flat) = self.flat(library, instance.shape) else {
                        continue;
                    };
                    let front = SOLID_FRONT + PLATE * 0.5 + layer as f32 * LAYER_STEP;
                    layer += 1;
                    if let Some(shape) = plate(&flat, instance.transform, front) {
                        shapes.push(shape.paint(materials.get(color, 0.0)));
                    }
                }
                _ => {}
            }
        }

        // SDF-объекты: капсулы у переднего края. Светящийся объект у лампы (огонёк бра,
        // шар-светильник) — на глубине лампы: луч тени не доходит до лампы 35 см, и
        // огонёк не бросает тень от своей же лампы на стену.
        let lights: Vec<[f32; 2]> = scene
            .lights
            .iter()
            .filter(|l| l.kind == reyvec_scene::LightKind::Local)
            .map(|l| [(l.a[0] + l.b[0]) * 0.5, (l.a[1] + l.b[1]) * 0.5])
            .collect();
        for o in &scene.objects {
            let at_lamp = o.glow
                && o.parts.iter().any(|p| {
                    let c = [(p.a[0] + p.b[0]) * 0.5, (p.a[1] + p.b[1]) * 0.5];
                    lights
                        .iter()
                        .any(|l| (l[0] - c[0]).hypot(l[1] - c[1]) < p.r + LAMP_NEAR)
                });
            let z = if at_lamp {
                LAMP_Z
            } else {
                SOLID_FRONT + PLATE * 0.5
            };
            if let Some((shape, color, glow)) = object_shape(o, z) {
                shapes.push(shape.paint(materials.get(color, glow)));
            }
        }

        LevelFrame {
            frame: SpaceFrame {
                shapes,
                lamps: Vec::new(),
                meshes: Vec::new(),
                proxies: Vec::new(),
            },
            materials: materials.list,
        }
    }

    /// Форма фигуры `id` (готовится при первой встрече).
    fn flat(&mut self, library: &ShapeLibrary, id: ShapeId) -> Option<Flat> {
        self.flats
            .entry(id)
            .or_insert_with(|| library.shape(id).and_then(flat_of))
            .clone()
    }
}

/// Лампы уровня: ближние к `target` (середине вида) первыми — у первых двух тени.
pub(crate) fn lamps(scene: &reyvec_scene::Scene, target: [f32; 2]) -> Vec<Lamp3> {
    let s = LEVEL_SCALE;
    let mut lamps: Vec<(f32, Lamp3)> = scene
        .lights
        .iter()
        .filter(|l| l.kind == reyvec_scene::LightKind::Local)
        .filter_map(|l| {
            let at = [(l.a[0] + l.b[0]) * 0.5, (l.a[1] + l.b[1]) * 0.5];
            let strength = l.color.iter().copied().fold(0.0f32, f32::max);
            if strength <= 1e-4 {
                return None;
            }
            let distance = ((at[0] - target[0]).powi(2) + (at[1] - target[1]).powi(2)).sqrt();
            Some((
                distance,
                Lamp3 {
                    at: [at[0] * s, at[1] * s, LAMP_Z],
                    color: l.color.map(|c| c / strength),
                    // Яркость плоской лампы — у её места; в объёме лампа на LAMP_Z перед
                    // уровнем, и свет до стены идёт дальше.
                    power: strength * 4.0,
                    range: (l.reach * s * 1.5).max(1.0),
                },
            ))
        })
        .collect();
    lamps.sort_by(|a, b| a.0.total_cmp(&b.0));
    lamps.into_iter().take(MAX_LAMPS).map(|(_, l)| l).collect()
}

/// Камера 2.5D-вида: сбоку с лёгким наклоном сверху, смотрит на середину 2D-камеры
/// `center`; при увеличении `zoom` видна та же высота уровня, что в плоском виде.
/// `orbit` — облёт для осмотра: поворот вокруг середины влево-вправо и вверх-вниз,
/// градусы (0, 0 — обычный вид).
pub(crate) fn camera(center: [f64; 2], zoom: f64, orbit: [f32; 2]) -> Camera3D {
    let s = LEVEL_SCALE;
    let target = Vec3::new(center[0] as f32 * s, center[1] as f32 * s, SOLID_FRONT);
    // Плоский вид показывает по высоте 2 / zoom единиц.
    let half_height = s / zoom.max(1e-3) as f32;
    let distance = half_height / (reyvec_sdf::camera3d::DEFAULT_FOV_Y * 0.5).tan();
    let pitch = (CAMERA_PITCH_DEG + orbit[1]).to_radians();
    let yaw = orbit[0].to_radians();
    let eye = target
        + Vec3::new(
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            pitch.cos() * yaw.cos(),
        ) * distance;
    Camera3D::looking_at(eye, target)
}

/// Брусок твёрдого (склон — выдавленный многоугольник).
fn solid_shape(c: &reyvec_scene::game::Collider) -> Option<Shape3> {
    let s = LEVEL_SCALE;
    let (lo, hi) = (c.aabb.min, c.aabb.max);
    let (w, h) = ((hi[0] - lo[0]) as f32, (hi[1] - lo[1]) as f32);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let depth = SOLID_FRONT - SOLID_BACK;
    let z = (SOLID_FRONT + SOLID_BACK) * 0.5;
    if c.slope == 0.0 {
        return Some(
            Shape3::Box {
                size: [w * s, h * s, depth],
                round: 0.0,
            }
            .at([
                (lo[0] + hi[0]) as f32 * 0.5 * s,
                (lo[1] + hi[1]) as f32 * 0.5 * s,
                z,
            ]),
        );
    }
    // Склон: верх идёт от одного верхнего угла вниз на |slope| у другого края.
    let drop = c.slope.abs().min(f64::from(h));
    let (left_top, right_top) = if c.slope > 0.0 {
        (hi[1] - drop, hi[1])
    } else {
        (hi[1], hi[1] - drop)
    };
    let mut path = BezPath::new();
    path.move_to((lo[0], lo[1]));
    path.line_to((hi[0], lo[1]));
    path.line_to((hi[0], right_top));
    path.line_to((lo[0], left_top));
    path.close_path();
    let profile = Profile::from_bez(&path, PROFILE_CELL * 10.0).ok()?;
    Some(Shape3::Move {
        at: [0.0, 0.0, z],
        turn: [0.0, 0.0, 0.0],
        scale: s,
        shape: Box::new(Shape3::Extrude {
            profile,
            depth: depth / s,
            round: 0.0,
        }),
    })
}

/// Цвет фигуры (линейный) с оттенком; `None` — почти прозрачная, не рисуется.
fn shape_color(library: &ShapeLibrary, id: ShapeId, tint: Color) -> Option<[f32; 3]> {
    let shape = library.shape(id)?;
    let base = match &shape.paint {
        Paint::Solid(c) => *c,
        Paint::Gradient(g) => {
            // Градиент — его средний цвет.
            let n = g.stops.len().max(1) as f32;
            g.stops
                .iter()
                .fold(Color::linear(0.0, 0.0, 0.0, 0.0), |acc, s| {
                    Color::linear(
                        acc.r + s.color.r / n,
                        acc.g + s.color.g / n,
                        acc.b + s.color.b / n,
                        acc.a + s.color.a / n,
                    )
                })
        }
    };
    let alpha = base.a * tint.a;
    (alpha >= 0.05).then_some([base.r * tint.r, base.g * tint.g, base.b * tint.b])
}

/// Форма фигуры без толщины: прямоугольник по осям или поле контура (обводка — поле
/// её полосы).
fn flat_of(shape: &reyvec_vector::Shape) -> Option<Flat> {
    let path = match shape.style {
        reyvec_vector::Style::Fill(_) => {
            if let Some(rect) = axis_rect(&shape.path) {
                return Some(rect);
            }
            shape.path.clone()
        }
        reyvec_vector::Style::Stroke(stroke) => kurbo::stroke(
            shape.path.iter(),
            &kurbo::Stroke::new(stroke.width),
            &kurbo::StrokeOpts::default(),
            stroke.width * 0.05,
        ),
    };
    let size = kurbo::Shape::bounding_box(&path).size();
    let long = size.width.max(size.height) as f32;
    let cell = (long / MIN_PROFILE_CELLS).clamp(PROFILE_CELL, PROFILE_CELL * 10.0);
    Profile::from_bez(&path, cell).ok().map(Flat::Profile)
}

/// Прямоугольник по осям: четыре угла и замыкание.
fn axis_rect(path: &BezPath) -> Option<Flat> {
    let mut points = Vec::new();
    for el in path.elements() {
        match el {
            PathEl::MoveTo(p) if points.is_empty() => points.push(*p),
            PathEl::LineTo(p) => points.push(*p),
            PathEl::ClosePath => {}
            _ => return None,
        }
    }
    if points.len() == 5 && points[4].distance(points[0]) < 1e-9 {
        points.pop();
    }
    if points.len() != 4 {
        return None;
    }
    let axis =
        |a: kurbo::Point, b: kurbo::Point| (a.x - b.x).abs() < 1e-9 || (a.y - b.y).abs() < 1e-9;
    if !(0..4).all(|i| axis(points[i], points[(i + 1) % 4])) {
        return None;
    }
    let (mut lo, mut hi) = (points[0], points[0]);
    for p in &points {
        lo = kurbo::Point::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = kurbo::Point::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let size = [(hi.x - lo.x) as f32, (hi.y - lo.y) as f32];
    (size[0] > 0.0 && size[1] > 0.0).then_some(Flat::Rect {
        center: [(lo.x + hi.x) as f32 * 0.5, (lo.y + hi.y) as f32 * 0.5],
        size,
    })
}

/// Где стоит фигура в объёме: сдвиг, поворот вокруг Z, размер и отражение по X из
/// преобразования `transform` (наклон и неравный размер — по среднему). Отражение — это
/// поворот на 180° вокруг вертикали: пластина симметрична по Z, поэтому он отражает её
/// по X, не меняя расстояний.
pub(crate) fn placement(transform: Affine) -> ([f32; 2], f32, f32, bool) {
    let [a, b, c, d, e, f] = transform.as_coeffs();
    let det = a * d - b * c;
    let mirror = det < 0.0;
    // Первый столбец (куда уходит X фигуры), у отражённой — с минусом.
    let (x0, y0) = if mirror { (-a, -b) } else { (a, b) };
    let angle = y0.atan2(x0);
    let scale = det.abs().sqrt();
    (
        [e as f32, f as f32],
        angle.to_degrees() as f32,
        scale as f32,
        mirror,
    )
}

/// Пластина фигуры толщиной [`PLATE`] с передней стороной на `front`.
fn plate(flat: &Flat, transform: Affine, front: f32) -> Option<Shape3> {
    let ([x, y], angle, scale, mirror) = placement(transform);
    if scale <= 1e-6 {
        return None;
    }
    let s = LEVEL_SCALE * scale;
    let depth = PLATE / s;
    let local = match flat {
        Flat::Rect { center, size } => Shape3::Box {
            size: [size[0], size[1], depth],
            round: 0.0,
        }
        .at([center[0], center[1], 0.0]),
        Flat::Profile(profile) => Shape3::Extrude {
            profile: profile.clone(),
            depth,
            round: 0.0,
        },
    };
    Some(Shape3::Move {
        at: [x * LEVEL_SCALE, y * LEVEL_SCALE, front - PLATE * 0.5],
        turn: [0.0, if mirror { 180.0 } else { 0.0 }, angle],
        scale: s,
        shape: Box::new(local),
    })
}

/// SDF-объект: плавное объединение капсул, цвет и свечение.
fn object_shape(o: &reyvec_scene::SceneObject, z: f32) -> Option<(Shape3, [f32; 3], f32)> {
    if o.parts.is_empty() || o.color[3] < 0.05 {
        return None;
    }
    let parts: Vec<Shape3> = o
        .parts
        .iter()
        .map(|p| {
            let (dx, dy) = (p.b[0] - p.a[0], p.b[1] - p.a[1]);
            let length = (dx * dx + dy * dy).sqrt();
            // Капсула вдоль Y: поворот так, чтобы Y смотрел от a к b.
            let angle = dx.atan2(dy).to_degrees();
            Shape3::Capsule {
                height: length,
                radius: p.r,
            }
            .placed(
                [(p.a[0] + p.b[0]) * 0.5, (p.a[1] + p.b[1]) * 0.5, 0.0],
                [0.0, 0.0, -angle],
            )
        })
        .collect();
    let shape = Shape3::Move {
        at: [0.0, 0.0, z],
        turn: [0.0, 0.0, 0.0],
        scale: LEVEL_SCALE,
        shape: Box::new(Shape3::Union {
            shapes: parts,
            smooth: o.k,
        }),
    };
    let rgb = [o.color[0], o.color[1], o.color[2]];
    let bright = rgb.iter().copied().fold(0.0f32, f32::max);
    if o.glow {
        let unit = rgb.map(|c| c / bright.max(1e-4));
        Some((shape, unit, bright.max(1.0) * 2.0))
    } else {
        Some((shape, rgb.map(|c| c.min(1.0)), 0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Отражённая и повёрнутая фигура в объёме стоит там же, где на плоскости.
    #[test]
    fn placement_matches_affine() {
        let flat = Flat::Rect {
            center: [0.1, 0.0],
            size: [0.2, 0.05],
        };
        for transform in [
            Affine::translate((1.0, 2.0)),
            Affine::translate((1.0, 2.0)) * Affine::rotate(0.7),
            Affine::translate((-0.5, 0.3)) * Affine::scale_non_uniform(-1.0, 1.0),
            Affine::translate((0.2, 0.1))
                * Affine::rotate(-0.4)
                * Affine::scale_non_uniform(-1.0, 1.0)
                * Affine::scale(1.5),
        ] {
            let shape = plate(&flat, transform, 1.0).unwrap();
            // Точка внутри фигуры (её правый край) и точка снаружи.
            let inside = transform * kurbo::Point::new(0.19, 0.0);
            let outside = transform * kurbo::Point::new(-0.05, 0.0);
            let at = |p: kurbo::Point| {
                Vec3::new(
                    p.x as f32 * LEVEL_SCALE,
                    p.y as f32 * LEVEL_SCALE,
                    1.0 - PLATE * 0.5,
                )
            };
            assert!(shape.distance(at(inside)) < 0.0, "{transform:?}");
            assert!(shape.distance(at(outside)) > 0.0, "{transform:?}");
        }
    }

    #[test]
    fn rectangles_need_no_field() {
        let rect = kurbo::Rect::new(-0.1, -0.2, 0.3, 0.4);
        let path = kurbo::Shape::to_path(&rect, 0.1);
        let Some(Flat::Rect { center, size }) = axis_rect(&path) else {
            panic!("не прямоугольник");
        };
        assert!((center[0] - 0.1).abs() < 1e-6 && (center[1] - 0.1).abs() < 1e-6);
        assert!((size[0] - 0.4).abs() < 1e-6 && (size[1] - 0.6).abs() < 1e-6);
        let circle = kurbo::Shape::to_path(&kurbo::Circle::new((0.0, 0.0), 1.0), 0.1);
        assert!(axis_rect(&circle).is_none());
    }

    #[test]
    fn camera_shows_same_height_as_flat_view() {
        let cam = camera([2.0, 0.5], 1.0, [0.0, 0.0]);
        let aspect = 16.0 / 9.0;
        // Точки на высоте ±1 от середины плоского вида — у краёв кадра по вертикали.
        for (dy, want) in [(1.0f32, 1.0f32), (-1.0, -1.0)] {
            let p = Vec3::new(2.0 * LEVEL_SCALE, (0.5 + dy) * LEVEL_SCALE, SOLID_FRONT);
            let screen = cam.project(p, aspect).unwrap();
            assert!((screen.y - want).abs() < 0.2, "{dy}: {screen}");
        }
    }

    #[test]
    fn orbit_turns_around_view_center() {
        let center = Vec3::new(2.0 * LEVEL_SCALE, 0.5 * LEVEL_SCALE, SOLID_FRONT);
        let home = camera([2.0, 0.5], 1.0, [0.0, 0.0]);
        // Облёт вправо на 60°: камера правее середины, на том же расстоянии, и середина
        // вида всё так же посередине кадра.
        let turned = camera([2.0, 0.5], 1.0, [60.0, 0.0]);
        assert!(turned.eye.x > center.x + 1.0, "{}", turned.eye);
        let d = |c: &Camera3D| (c.eye - center).length();
        assert!((d(&home) - d(&turned)).abs() < 1e-3);
        let screen = turned.project(center, 16.0 / 9.0).unwrap();
        assert!(screen.truncate().length() < 1e-3, "{screen}");
    }
}
