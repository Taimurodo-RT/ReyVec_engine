//! Объёмные формы из полей расстояний (этап 7.2): дерево из примитивов и операций, его
//! «байт-код» для видеокарты и расчёт того же байт-кода на процессоре.
//!
//! Форма описывается данными ([`Shape3`]), а не кодом шейдера: так её можно менять на лету,
//! читать из файла сцены (7.5) и собирать мышкой в будущем редакторе. Дерево:
//!
//! - **примитивы**: шар, коробка (со скруглением), капсула, цилиндр, тор, плоскость,
//!   выдавленная и повёрнутая 2D-фигура (7.7), поле слоя (7.18б);
//! - **операции**: объединить, вырезать, пересечь — резко или плавно (`smooth` — ширина
//!   плавного перехода в метрах);
//! - **сдвиг, поворот и масштаб** ([`Shape3::Move`]) и **материал** ([`Shape3::Paint`]).
//!
//! [`compile`] превращает дерево в короткую программу для стековой машины: каждый
//! примитив кладёт на стек расстояние и материал, операция снимает два и кладёт одно.
//! Сдвиги и повороты сворачиваются в одну матрицу на примитив, так что видеокарта не
//! обходит дерево. Шейдер `include/sdf3d.slang` выполняет ту же программу, что и
//! [`Program::distance`] здесь: формулы — зеркало друг друга, менять вместе.
//!
//! После плавных операций и масштаба поле остаётся «снизу» (не больше настоящего
//! расстояния), поэтому по нему можно шагать лучом и не проскочить поверхность.

use std::sync::Arc;

use glam::{Mat3, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::brick3d::{BRICK_PRIM_WORDS, BrickRef};
use crate::grid3d::GridOptions;
use crate::layer_field::LayerField;
use crate::profile2d::{FIELD_HEADER_WORDS, Profile, field_distance};

/// Объёмная форма: примитив, операция над формами, сдвиг или материал.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape3 {
    /// Шар радиусом `radius` с серединой в начале координат.
    Sphere { radius: f32 },
    /// Коробка размером `size` (полная ширина, высота, глубина), углы скруглены на `round`.
    Box {
        size: [f32; 3],
        #[serde(default)]
        round: f32,
    },
    /// Капсула вдоль Y: отрезок длиной `height` (без полусфер) с радиусом `radius`.
    Capsule { height: f32, radius: f32 },
    /// Цилиндр вдоль Y высотой `height` и радиусом `radius`, края скруглены на `round`.
    Cylinder {
        height: f32,
        radius: f32,
        #[serde(default)]
        round: f32,
    },
    /// Тор в плоскости XZ: радиус кольца `radius`, толщина `thickness` (радиус трубки).
    Torus { radius: f32, thickness: f32 },
    /// Усечённый конус вдоль Y (М.0б): высота `height`, радиус внизу `bottom` и вверху
    /// `top` (0 — острая вершина), края скруглены на `round`.
    Cone {
        height: f32,
        bottom: f32,
        #[serde(default)]
        top: f32,
        #[serde(default)]
        round: f32,
    },
    /// Плоскость: всё по ту сторону от `normal` дальше `offset` от начала — внутри.
    /// `normal: (0, 1, 0), offset: 0` — пол на высоте 0.
    Plane { normal: [f32; 3], offset: f32 },
    /// Выдавленная 2D-фигура (7.7): контур `profile` в плоскости XY (как в SVG, только
    /// в метрах и Y вверх) толщиной `depth` по Z, рёбра скруглены на `round`.
    Extrude {
        profile: Profile,
        depth: f32,
        #[serde(default)]
        round: f32,
    },
    /// Тело вращения (7.7): контур `profile` (X — от оси, Y — вверх) повёрнут вокруг
    /// оси Y. `offset` отодвигает контур от оси: с ним круг становится бубликом.
    Revolve {
        profile: Profile,
        #[serde(default)]
        offset: f32,
    },
    /// Поле слоя (7.18б, `docs/decisions/0020-level-field.md`): выдавливание 2D-поля,
    /// которое лежит в картинке видеокарты ([`LayerField`]), толщиной `depth` по Z, рёбра
    /// скруглены на `round`. За сеткой расстояние считается от её края. Если у поля задан
    /// прямоугольник фигуры, шар-граница — как у выдавливания этого прямоугольника; поле
    /// без него (поле уровня, 7.18в) бесконечно. В файл сцены не записывается — его создаёт код
    /// (подмена выдавливания в сцене «3D», запекание уровня 7.18в).
    #[serde(skip)]
    Field {
        field: FieldRef,
        depth: f32,
        #[serde(default)]
        round: f32,
    },
    /// «Кирпич поля» (8.15, [`crate::brick3d`]): запечённое расстояние до полигональной
    /// модели, чуть внутри неё. Невидимый заменитель модели для теней и затенения в
    /// углах. В файл сцены не записывается — его печёт чтение модели.
    #[serde(skip)]
    Brick { brick: BrickRef },
    /// Объединение: всё, что есть хотя бы в одной форме.
    Union {
        shapes: Vec<Shape3>,
        #[serde(default)]
        smooth: f32,
    },
    /// Из `base` вырезаны формы `cut`.
    Subtract {
        base: Box<Shape3>,
        cut: Vec<Shape3>,
        #[serde(default)]
        smooth: f32,
    },
    /// Пересечение: только то, что есть во всех формах.
    Intersect {
        shapes: Vec<Shape3>,
        #[serde(default)]
        smooth: f32,
    },
    /// Форма, сдвинутая в `at`, повёрнутая на `turn` (градусы вокруг X, потом Y, потом Z)
    /// и увеличенная в `scale` раз.
    Move {
        #[serde(default)]
        at: [f32; 3],
        #[serde(default)]
        turn: [f32; 3],
        #[serde(default = "one")]
        scale: f32,
        shape: Box<Shape3>,
    },
    /// Материал `material` (номер в списке материалов) для всех примитивов внутри.
    Paint { material: u32, shape: Box<Shape3> },
    /// Форма в движении (7.5): покачивание `bob`, движение по кругу `circle` и вращение
    /// `spin` (градусов в секунду вокруг X, Y, Z). Положение в момент времени даёт
    /// [`Shape3::at_time`]; без времени (расчёт по дереву, сборка программы) — момент 0.
    Animate {
        #[serde(default)]
        bob: Bob,
        #[serde(default)]
        circle: Circle,
        #[serde(default)]
        spin: [f32; 3],
        shape: Box<Shape3>,
    },
    /// Зеркало (М.0б): форма и её отражение через плоскость `axis = 0` (у `X` — через
    /// плоскость YZ), сложенные с плавным швом `smooth`. Половина станка — весь станок.
    /// Видеокарте это две копии частей формы, вторая с отражённой матрицей.
    Mirror {
        axis: Axis,
        #[serde(default)]
        smooth: f32,
        shape: Box<Shape3>,
    },
    /// Повтор (М.0б): `count` копий формы, каждая следующая сдвинута на `step` (ножки,
    /// зубья, болты), сложенные с плавным швом `smooth`. Видеокарте это `count` копий
    /// частей формы, поэтому `count` не больше [`MAX_REPEAT`].
    Repeat {
        count: u32,
        step: [f32; 3],
        #[serde(default)]
        smooth: f32,
        shape: Box<Shape3>,
    },
    /// Модель из своего файла (М.0, [`crate::shape_model`]): `models/<model>.shape.ron`
    /// рядом с файлом сцены. Раскрывается в дерево модели при чтении сцены; до этого у
    /// неё нет формы, и программа из неё не собирается.
    Use { model: String },
}

fn one() -> f32 {
    1.0
}

/// Ось зеркала [`Shape3::Mirror`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    /// Отражение точки через плоскость `axis = 0`.
    pub fn reflect(self, p: Vec3) -> Vec3 {
        match self {
            Self::X => Vec3::new(-p.x, p.y, p.z),
            Self::Y => Vec3::new(p.x, -p.y, p.z),
            Self::Z => Vec3::new(p.x, p.y, -p.z),
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Больше копий у [`Shape3::Repeat`] не бывает.
pub const MAX_REPEAT: u32 = 64;

impl Shape3 {
    /// Копии формы у [`Shape3::Repeat`]: объединение сдвинутых форм.
    fn repeat_union(count: u32, step: [f32; 3], smooth: f32, shape: &Shape3) -> Shape3 {
        Shape3::Union {
            shapes: (0..count.clamp(1, MAX_REPEAT))
                .map(|k| Shape3::Move {
                    at: (Vec3::from(step) * k as f32).to_array(),
                    turn: [0.0; 3],
                    scale: 1.0,
                    shape: Box::new(shape.clone()),
                })
                .collect(),
            smooth,
        }
    }
}

/// Поле слоя, общее для формы и её программы: программа держит его, чтобы расчёт на
/// процессоре ([`Program::distance`]) читал те же числа, что видеокарта из картинки.
/// Две ссылки равны, если это одно и то же поле (по адресу).
#[derive(Clone)]
pub struct FieldRef(pub Arc<LayerField>);

impl PartialEq for FieldRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for FieldRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

/// Покачивание туда-обратно: сдвиг `by · sin(2π·t / period)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bob {
    #[serde(default)]
    pub by: [f32; 3],
    /// Секунд на полный взмах туда и обратно (0 — стоит на месте).
    #[serde(default)]
    pub period: f32,
}

/// Движение по кругу (или овалу) в плоскости XZ: сдвиг
/// `(rx · cos(2π·t / period), 0, rz · sin(2π·t / period))`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Circle {
    /// Радиусы по X и по Z.
    #[serde(default)]
    pub radius: [f32; 2],
    /// Секунд на круг (0 — стоит на месте; меньше нуля — по часовой).
    #[serde(default)]
    pub period: f32,
}

/// Сдвиг от покачивания и движения по кругу в момент `time`.
pub fn motion_offset(bob: &Bob, circle: &Circle, time: f32) -> Vec3 {
    let mut out = Vec3::ZERO;
    if bob.period != 0.0 {
        out += Vec3::from(bob.by) * (std::f32::consts::TAU * time / bob.period).sin();
    }
    if circle.period != 0.0 {
        let a = std::f32::consts::TAU * time / circle.period;
        out += Vec3::new(circle.radius[0] * a.cos(), 0.0, circle.radius[1] * a.sin());
    }
    out
}

impl Shape3 {
    /// Сдвинутая копия формы.
    pub fn at(self, at: [f32; 3]) -> Self {
        Self::Move {
            at,
            turn: [0.0; 3],
            scale: 1.0,
            shape: Box::new(self),
        }
    }

    /// Сдвинутая и повёрнутая копия (градусы вокруг X, Y, Z).
    pub fn placed(self, at: [f32; 3], turn: [f32; 3]) -> Self {
        Self::Move {
            at,
            turn,
            scale: 1.0,
            shape: Box::new(self),
        }
    }

    /// Форма с материалом `material`.
    pub fn paint(self, material: u32) -> Self {
        Self::Paint {
            material,
            shape: Box::new(self),
        }
    }

    /// Расстояние до формы прямо по дереву (медленно, для проверок): то же, что даёт
    /// скомпилированная программа.
    pub fn distance(&self, p: Vec3) -> f32 {
        match self {
            Self::Sphere { radius } => sd_sphere(p, *radius),
            Self::Box { size, round } => sd_box(p, Vec3::from(*size) * 0.5, *round),
            Self::Capsule { height, radius } => sd_capsule(p, *height * 0.5, *radius),
            Self::Cylinder {
                height,
                radius,
                round,
            } => sd_cylinder(p, *height * 0.5, *radius, *round),
            Self::Torus { radius, thickness } => sd_torus(p, *radius, *thickness),
            Self::Cone {
                height,
                bottom,
                top,
                round,
            } => sd_cone(p, *height * 0.5, *bottom, *top, *round),
            Self::Plane { normal, offset } => sd_plane(p, Vec3::from(*normal), *offset),
            Self::Extrude {
                profile,
                depth,
                round,
            } => sd_extrude(
                p,
                profile.distance(Vec2::new(p.x, p.y)),
                *depth * 0.5,
                *round,
            ),
            Self::Revolve { profile, offset } => profile.distance(revolve_point(p, *offset)),
            Self::Field {
                field,
                depth,
                round,
            } => field.0.sample(p, *depth * 0.5, *round, 0).0,
            Self::Brick { brick } => brick.0.sample(p) + brick.0.inset(),
            Self::Union { shapes, smooth } => shapes
                .iter()
                .map(|s| s.distance(p))
                .reduce(|a, b| op_union(a, b, *smooth))
                .unwrap_or(f32::MAX),
            Self::Subtract { base, cut, smooth } => cut.iter().fold(base.distance(p), |a, c| {
                op_subtract(a, c.distance(p), *smooth)
            }),
            Self::Intersect { shapes, smooth } => shapes
                .iter()
                .map(|s| s.distance(p))
                .reduce(|a, b| op_intersect(a, b, *smooth))
                .unwrap_or(f32::MAX),
            Self::Move {
                at,
                turn,
                scale,
                shape,
            } => {
                let local = rotation(*turn).transpose() * (p - Vec3::from(*at)) / *scale;
                shape.distance(local) * *scale
            }
            Self::Paint { shape, .. } => shape.distance(p),
            Self::Animate { .. } => self.at_time(0.0).distance(p),
            Self::Mirror {
                axis,
                smooth,
                shape,
            } => op_union(shape.distance(p), shape.distance(axis.reflect(p)), *smooth),
            Self::Repeat {
                count,
                step,
                smooth,
                shape,
            } => Self::repeat_union(*count, *step, *smooth, shape).distance(p),
            Self::Use { .. } => f32::MAX,
        }
    }

    /// Меняется ли форма во времени сама, а не только едет целиком (8.20е): внутри
    /// объединения, вычитания или пересечения есть [`Shape3::Animate`] — части движутся
    /// друг относительно друга. `Animate` снаружи всех операций — движение целиком.
    pub fn morphs(&self) -> bool {
        match self {
            Self::Move { shape, .. } | Self::Paint { shape, .. } | Self::Animate { shape, .. } => {
                shape.morphs()
            }
            Self::Union { shapes, .. } | Self::Intersect { shapes, .. } => {
                shapes.iter().any(Self::has_animate)
            }
            Self::Subtract { base, cut, .. } => {
                base.has_animate() || cut.iter().any(Self::has_animate)
            }
            _ => false,
        }
    }

    /// Есть ли в форме [`Shape3::Animate`] на любой глубине.
    fn has_animate(&self) -> bool {
        match self {
            Self::Animate { .. } => true,
            Self::Move { shape, .. } | Self::Paint { shape, .. } => shape.has_animate(),
            Self::Union { shapes, .. } | Self::Intersect { shapes, .. } => {
                shapes.iter().any(Self::has_animate)
            }
            Self::Subtract { base, cut, .. } => {
                base.has_animate() || cut.iter().any(Self::has_animate)
            }
            _ => false,
        }
    }

    /// Форма в момент `time` (секунды): движения [`Shape3::Animate`] заменены сдвигами
    /// и поворотами на этот момент.
    pub fn at_time(&self, time: f32) -> Self {
        let all = |shapes: &[Shape3]| shapes.iter().map(|s| s.at_time(time)).collect();
        match self {
            Self::Union { shapes, smooth } => Self::Union {
                shapes: all(shapes),
                smooth: *smooth,
            },
            Self::Intersect { shapes, smooth } => Self::Intersect {
                shapes: all(shapes),
                smooth: *smooth,
            },
            Self::Subtract { base, cut, smooth } => Self::Subtract {
                base: Box::new(base.at_time(time)),
                cut: all(cut),
                smooth: *smooth,
            },
            Self::Move {
                at,
                turn,
                scale,
                shape,
            } => Self::Move {
                at: *at,
                turn: *turn,
                scale: *scale,
                shape: Box::new(shape.at_time(time)),
            },
            Self::Paint { material, shape } => Self::Paint {
                material: *material,
                shape: Box::new(shape.at_time(time)),
            },
            Self::Mirror {
                axis,
                smooth,
                shape,
            } => Self::Mirror {
                axis: *axis,
                smooth: *smooth,
                shape: Box::new(shape.at_time(time)),
            },
            Self::Repeat {
                count,
                step,
                smooth,
                shape,
            } => Self::Repeat {
                count: *count,
                step: *step,
                smooth: *smooth,
                shape: Box::new(shape.at_time(time)),
            },
            Self::Animate {
                bob,
                circle,
                spin,
                shape,
            } => Self::Move {
                at: motion_offset(bob, circle, time).to_array(),
                turn: spin.map(|s| s * time),
                scale: 1.0,
                shape: Box::new(shape.at_time(time)),
            },
            plain => plain.clone(),
        }
    }
}

impl Shape3 {
    /// Шар, в котором форма целиком (середина и радиус), или `None` для бесконечной
    /// формы (плоскость). Снаружи шара расстояние до формы не меньше расстояния до шара:
    /// шейдер по этому пропускает дальние объекты (7.6).
    pub fn bounds(&self) -> Option<(Vec3, f32)> {
        match self {
            Self::Sphere { radius } => Some((Vec3::ZERO, radius.abs())),
            Self::Box { size, .. } => Some((Vec3::ZERO, (Vec3::from(*size) * 0.5).length())),
            Self::Capsule { height, radius } => {
                Some((Vec3::ZERO, height.abs() * 0.5 + radius.abs()))
            }
            Self::Cylinder { height, radius, .. } => Some((
                Vec3::ZERO,
                (height * height * 0.25 + radius * radius).sqrt(),
            )),
            Self::Torus { radius, thickness } => Some((Vec3::ZERO, radius.abs() + thickness.abs())),
            Self::Cone {
                height,
                bottom,
                top,
                ..
            } => {
                let r = bottom.abs().max(top.abs());
                Some((Vec3::ZERO, (height * height * 0.25 + r * r).sqrt()))
            }
            Self::Plane { .. } => None,
            // Поле фигуры с прямоугольником — шар как у выдавливания: за сеткой расстояние
            // не меньше, чем до прямоугольника. Поле слоя без него бесконечно: за сеткой
            // оно продолжается оценкой от края (7.18б).
            Self::Field { field, depth, .. } => field.0.rect().map(|(lo, hi)| {
                let half = depth.abs() * 0.5;
                // Второй слой — свой отрезок по Z: шар накрывает оба.
                let (z0, z1) = field.0.second().map_or((-half, half), |s| {
                    ((-half).min(s.shift - s.half), half.max(s.shift + s.half))
                });
                let (c, r) = extrude_bounds(lo, hi, z1 - z0);
                (c + Vec3::Z * (z0 + z1) * 0.5, r)
            }),
            Self::Extrude { profile, depth, .. } => {
                let (lo, hi) = profile.bounds();
                Some(extrude_bounds(lo, hi, *depth))
            }
            Self::Brick { brick } => Some(brick.0.bounds()),
            Self::Revolve { profile, offset } => {
                let (lo, hi) = profile.bounds();
                let r = (offset + lo.x.abs().max(hi.x.abs())).abs();
                let half = (hi.y - lo.y) * 0.5;
                Some((
                    Vec3::new(0.0, (lo.y + hi.y) * 0.5, 0.0),
                    (r * r + half * half).sqrt(),
                ))
            }
            Self::Union { shapes, smooth } => {
                // Плавное объединение выпирает между частями не больше чем на smooth/4;
                // берём с запасом.
                let mut all = shapes.iter().map(Shape3::bounds);
                let first = all.next()??;
                let (c, r) = all.try_fold(first, |a, b| Some(enclose(a, b?)))?;
                Some((c, r + smooth.abs() * 0.5))
            }
            // Вырезание и пересечение только уменьшают форму.
            Self::Subtract { base, .. } => base.bounds(),
            Self::Intersect { shapes, .. } => shapes
                .iter()
                .filter_map(Shape3::bounds)
                .min_by(|a, b| a.1.total_cmp(&b.1)),
            Self::Move {
                at,
                turn,
                scale,
                shape,
            } => {
                let s = scale.max(1e-6);
                shape
                    .bounds()
                    .map(|(c, r)| (Vec3::from(*at) + rotation(*turn) * (c * s), r * s))
            }
            Self::Paint { shape, .. } => shape.bounds(),
            Self::Animate { .. } => self.at_time(0.0).bounds(),
            Self::Mirror {
                axis,
                smooth,
                shape,
            } => shape.bounds().map(|(c, r)| {
                let (c, r) = enclose((c, r), (axis.reflect(c), r));
                (c, r + smooth.abs() * 0.5)
            }),
            Self::Repeat {
                count,
                step,
                smooth,
                shape,
            } => Self::repeat_union(*count, *step, *smooth, shape).bounds(),
            Self::Use { .. } => Some((Vec3::ZERO, 0.0)),
        }
    }
}

impl Shape3 {
    /// Коробка, в которой форма целиком (наименьший и наибольший углы, 8.8а), или `None`
    /// для бесконечной формы. Плотнее шара для длинных и плоских предметов: у стены 60 м
    /// шар радиусом 30 м, а коробка — толщиной в саму стену. Повёрнутая коробка берётся
    /// по восьми углам, поэтому для повёрнутых частей она может быть шире шара: сетка
    /// отсева берёт пересечение обеих ([`compile`]).
    pub fn bounds_box(&self) -> Option<(Vec3, Vec3)> {
        let centered = |half: Vec3| Some((-half.abs(), half.abs()));
        match self {
            Self::Sphere { radius } => centered(Vec3::splat(*radius)),
            Self::Box { size, .. } => centered(Vec3::from(*size) * 0.5),
            Self::Capsule { height, radius } => {
                let r = radius.abs();
                centered(Vec3::new(r, height.abs() * 0.5 + r, r))
            }
            Self::Cylinder { height, radius, .. } => {
                centered(Vec3::new(*radius, height * 0.5, *radius))
            }
            Self::Torus { radius, thickness } => {
                let t = thickness.abs();
                let r = radius.abs() + t;
                centered(Vec3::new(r, t, r))
            }
            Self::Cone {
                height,
                bottom,
                top,
                ..
            } => {
                let r = bottom.abs().max(top.abs());
                centered(Vec3::new(r, height * 0.5, r))
            }
            Self::Plane { .. } => None,
            Self::Field { field, depth, .. } => field.0.rect().map(|(lo, hi)| {
                let half = depth.abs() * 0.5;
                let (z0, z1) = field.0.second().map_or((-half, half), |s| {
                    ((-half).min(s.shift - s.half), half.max(s.shift + s.half))
                });
                (lo.extend(z0), hi.extend(z1))
            }),
            Self::Extrude { profile, depth, .. } => {
                let (lo, hi) = profile.bounds();
                let half = depth.abs() * 0.5;
                Some((lo.extend(-half), hi.extend(half)))
            }
            Self::Brick { brick } => Some(brick.0.bounds_box()),
            Self::Revolve { profile, offset } => {
                let (lo, hi) = profile.bounds();
                let r = (offset + lo.x.abs().max(hi.x.abs())).abs();
                Some((Vec3::new(-r, lo.y, -r), Vec3::new(r, hi.y, r)))
            }
            Self::Union { shapes, smooth } => {
                let mut all = shapes.iter().map(Shape3::bounds_box);
                let first = all.next()??;
                let (lo, hi) = all.try_fold(first, |a, b| {
                    let b = b?;
                    Some((a.0.min(b.0), a.1.max(b.1)))
                })?;
                let pad = Vec3::splat(smooth.abs() * 0.5);
                Some((lo - pad, hi + pad))
            }
            Self::Subtract { base, .. } => base.bounds_box(),
            Self::Intersect { shapes, .. } => {
                let (lo, hi) = shapes
                    .iter()
                    .filter_map(Shape3::bounds_box)
                    .reduce(|a, b| (a.0.max(b.0), a.1.min(b.1)))?;
                // Пустое пересечение — точка: формы там нет, граница не мешает.
                Some((lo, hi.max(lo)))
            }
            Self::Move {
                at,
                turn,
                scale,
                shape,
            } => {
                let s = scale.max(1e-6);
                let (lo, hi) = shape.bounds_box()?;
                let m = rotation(*turn);
                let center = Vec3::from(*at) + m * ((lo + hi) * 0.5 * s);
                let half = (hi - lo) * 0.5 * s;
                // Полуразмер повёрнутой коробки по оси — сумма |m_ij| · half_j.
                let abs = Mat3::from_cols(m.x_axis.abs(), m.y_axis.abs(), m.z_axis.abs());
                let half = abs * half;
                Some((center - half, center + half))
            }
            Self::Paint { shape, .. } => shape.bounds_box(),
            Self::Animate { .. } => self.at_time(0.0).bounds_box(),
            Self::Mirror {
                axis,
                smooth,
                shape,
            } => {
                let (mut lo, mut hi) = shape.bounds_box()?;
                // Отражённая коробка по оси — от −hi до −lo: вместе с исходной она
                // симметрична, от −max(|lo|, |hi|) до того же со знаком плюс.
                let i = axis.index();
                let m = lo[i].abs().max(hi[i].abs());
                lo[i] = -m;
                hi[i] = m;
                let pad = Vec3::splat(smooth.abs() * 0.5);
                Some((lo - pad, hi + pad))
            }
            Self::Repeat {
                count,
                step,
                smooth,
                shape,
            } => Self::repeat_union(*count, *step, *smooth, shape).bounds_box(),
            Self::Use { .. } => Some((Vec3::ZERO, Vec3::ZERO)),
        }
    }
}

/// Шар-граница выдавленного прямоугольника `lo..hi` толщиной `depth`.
fn extrude_bounds(lo: Vec2, hi: Vec2, depth: f32) -> (Vec3, f32) {
    let half = ((hi - lo) * 0.5).extend(depth.abs() * 0.5);
    (((lo + hi) * 0.5).extend(0.0), half.length())
}

/// Наименьший шар, в котором оба шара.
fn enclose(a: (Vec3, f32), b: (Vec3, f32)) -> (Vec3, f32) {
    let between = b.0 - a.0;
    let d = between.length();
    if d + b.1 <= a.1 {
        return a;
    }
    if d + a.1 <= b.1 {
        return b;
    }
    let r = (d + a.1 + b.1) * 0.5;
    (a.0 + between * ((r - a.1) / d), r)
}

/// Поворот на `turn` градусов: сначала вокруг X, потом Y, потом Z.
pub fn rotation(turn: [f32; 3]) -> Mat3 {
    let [x, y, z] = turn.map(f32::to_radians);
    Mat3::from_rotation_z(z) * Mat3::from_rotation_y(y) * Mat3::from_rotation_x(x)
}

// Формулы примитивов и операций: те же, что в include/sdf3d.slang.

pub fn sd_sphere(p: Vec3, r: f32) -> f32 {
    p.length() - r
}

pub fn sd_box(p: Vec3, half: Vec3, round: f32) -> f32 {
    let q = p.abs() - half + Vec3::splat(round);
    q.max(Vec3::ZERO).length() + q.max_element().min(0.0) - round
}

pub fn sd_capsule(p: Vec3, half_height: f32, r: f32) -> f32 {
    let y = p.y.clamp(-half_height, half_height);
    (p - Vec3::new(0.0, y, 0.0)).length() - r
}

pub fn sd_cylinder(p: Vec3, half_height: f32, r: f32, round: f32) -> f32 {
    let d = glam::Vec2::new(
        glam::Vec2::new(p.x, p.z).length() - r + round,
        p.y.abs() - half_height + round,
    );
    d.max(glam::Vec2::ZERO).length() + d.max_element().min(0.0) - round
}

pub fn sd_torus(p: Vec3, big: f32, small: f32) -> f32 {
    glam::Vec2::new(glam::Vec2::new(p.x, p.z).length() - big, p.y).length() - small
}

/// Усечённый конус вдоль Y: половина высоты `half`, радиусы внизу `r1` и вверху `r2`,
/// края скруглены на `round` (формула Иниго Килеса `sdCappedCone`, конус уменьшен на
/// `round` и раздут обратно).
pub fn sd_cone(p: Vec3, half: f32, r1: f32, r2: f32, round: f32) -> f32 {
    let h = (half - round).max(0.0);
    let (r1, r2) = ((r1 - round).max(0.0), (r2 - round).max(0.0));
    let q = Vec2::new(Vec2::new(p.x, p.z).length(), p.y);
    let k1 = Vec2::new(r2, h);
    let k2 = Vec2::new(r2 - r1, 2.0 * h);
    let ca = Vec2::new(
        q.x - q.x.min(if q.y < 0.0 { r1 } else { r2 }),
        q.y.abs() - h,
    );
    let t = ((k1 - q).dot(k2) / k2.dot(k2).max(1e-12)).clamp(0.0, 1.0);
    let cb = q - k1 + k2 * t;
    let s = if cb.x < 0.0 && ca.y < 0.0 { -1.0 } else { 1.0 };
    s * ca.dot(ca).min(cb.dot(cb)).sqrt() - round
}

pub fn sd_plane(p: Vec3, normal: Vec3, offset: f32) -> f32 {
    p.dot(normal.normalize_or(Vec3::Y)) - offset
}

/// Выдавливание: `d2` — расстояние до 2D-фигуры в точке (x, y), толщина ±`half` по Z.
pub fn sd_extrude(p: Vec3, d2: f32, half: f32, round: f32) -> f32 {
    let w = Vec2::new(d2 + round, p.z.abs() - half + round);
    w.max(Vec2::ZERO).length() + w.max_element().min(0.0) - round
}

/// Точка контура тела вращения: расстояние от оси Y (минус `offset`) и высота.
pub fn revolve_point(p: Vec3, offset: f32) -> Vec2 {
    Vec2::new(Vec2::new(p.x, p.z).length() - offset, p.y)
}

pub fn op_union(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    lerp(b, a, h) - k * h * (1.0 - h)
}

pub fn op_subtract(a: f32, b: f32, k: f32) -> f32 {
    -op_union(-a, b, k)
}

pub fn op_intersect(a: f32, b: f32, k: f32) -> f32 {
    -op_union(-a, -b, k)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Коды команд программы. Слово команды: код в младших 8 битах, вид примитива в следующих.
pub mod op {
    /// Примитив: слово команды, материал, 12 чисел матрицы «мир → форма» (три строки по
    /// четыре: поворот и сдвиг), множитель расстояния (масштаб), затем 4 параметра. Биты
    /// 16–31 слова команды — сколько слов данных идёт следом (поле 2D-фигуры у
    /// выдавливания и вращения, 7.7); у остальных примитивов ноль.
    pub const PRIM: u32 = 1;
    /// Операции: слово команды и ширина плавного перехода.
    pub const UNION: u32 = 2;
    pub const SUBTRACT: u32 = 3;
    pub const INTERSECT: u32 = 4;
    /// Граница части (М.1): слово команды (биты 8–15 — сколько итогов объединений с
    /// вершины стека сравнивать, биты 16–31 — сколько слов части идёт следом), середина
    /// шара-границы части в мире (3 числа), радиус и ширина плавного шва. Если точка от
    /// шара не ближе наименьшего из итогов плюс ширина шва, часть итог не изменит: машина
    /// кладёт на стек расстояние до шара и перепрыгивает слова части (её операция
    /// объединения выполняется как обычно).
    pub const SKIP: u32 = 5;
}

/// Виды примитивов (биты 8–15 слова команды).
pub mod prim {
    pub const SPHERE: u32 = 0;
    pub const BOX: u32 = 1;
    pub const CAPSULE: u32 = 2;
    pub const CYLINDER: u32 = 3;
    pub const TORUS: u32 = 4;
    pub const PLANE: u32 = 5;
    pub const EXTRUDE: u32 = 6;
    pub const REVOLVE: u32 = 7;
    /// Поле слоя (7.18б): 2D-поле в картинке, выдавленное в толщину слоя.
    pub const FIELD: u32 = 8;
    /// «Кирпич поля» (8.15): объёмная сетка расстояний модели в картинке.
    pub const BRICK: u32 = 9;
    /// Усечённый конус (М.0б).
    pub const CONE: u32 = 10;
}

/// Слов на примитив: команда, материал, 12 матрицы, масштаб, 4 параметра (без данных
/// поля 2D-фигуры).
pub const PRIM_WORDS: usize = 19;
/// Слов данных у примитива «поле слоя» (7.18б; `FIELD_PRIM_WORDS` в `include/sdf3d.slang`,
/// менять вместе): номер картинки поля, номер картинки материалов (0 — нет), левый нижний
/// угол сетки (x, y), шаг, `nx | ny << 16`, прямоугольник фигуры (левый нижний и правый
/// верхний углы; пустой — прямоугольника нет), 2 запасных. Параметры примитива: половина
/// толщины, скругление рёбер, половина толщины второго слоя (запас), полоса `B`.
pub const FIELD_PRIM_WORDS: usize = 12;
/// Материал «только оценка» у поля слоя (7.18б): поле в точке обрезано полосой `B`,
/// поверхность может быть дальше, материал не известен. Шейдер в этом случае отдаёт
/// материал −1 (тени не пишут полутень), на процессоре [`crate::grid3d::Grid::near`]
/// отдаёт `None`.
pub const ESTIMATE_MATERIAL: u32 = u32::MAX;
/// Длиннее программа одной формы не бывает: шейдер дальше не читает (MAX_PROGRAM_WORDS
/// в `include/space.slang`, менять вместе).
pub const MAX_PROGRAM_WORDS: usize = 16384;
/// Слов на операцию.
pub const OP_WORDS: usize = 2;
/// Слов на границу части ([`op::SKIP`]).
pub const SKIP_WORDS: usize = 6;

/// Границы частей внутри формы (М.1): `false` — программа без слов [`op::SKIP`], как до
/// М.1 (переключатель на панели, для сравнения времени).
pub static PART_BOUNDS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Включены ли границы частей ([`PART_BOUNDS`]).
pub fn part_bounds() -> bool {
    PART_BOUNDS.load(std::sync::atomic::Ordering::Relaxed)
}
/// Глубина стека машины в шейдере: столько форм может ждать операции одновременно.
pub const STACK: usize = 8;

/// Программа одной формы: слова для видеокарты.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    pub words: Vec<u32>,
    /// Шар, в котором форма целиком (середина, радиус); `None` — форма бесконечна.
    pub bounds: Option<(Vec3, f32)>,
    /// Коробка, в которой форма целиком (наименьший и наибольший углы, 8.8а): по ней
    /// строится сетка отсева. Пересечение [`Shape3::bounds_box`] с коробкой шара.
    pub bounds_box: Option<(Vec3, Vec3)>,
    /// Невидимый заменитель полигональной модели (7.6ж): только тени и затенение в
    /// углах, лучи глаза его не видят.
    pub shadow_only: bool,
    /// Светящийся предмет (огонёк, шар-светильник; 7.9б): виден, но теней и затенения
    /// в углах не даёт. Иначе он у самой лампы и бросает от неё тень на всё вокруг.
    pub no_shadow: bool,
    /// Неподвижный предмет (8.8б): сетка отсева раскладывает его один раз
    /// ([`crate::grid3d::GridCache`]).
    pub still: bool,
    /// Поля слоя (7.18б) в порядке примитивов `PRIM_FIELD` в словах: по ним считает
    /// зеркало на процессоре, видеокарта читает картинки по номерам из слов.
    pub fields: Vec<FieldRef>,
    /// Кирпичи поля (8.15) в порядке примитивов `PRIM_BRICK`: по ним считает зеркало на
    /// процессоре.
    pub bricks: Vec<BrickRef>,
}

/// Почему форму нельзя превратить в программу.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    #[error("форма слишком глубоко вложена: шейдеру хватает стека на {STACK} частей")]
    TooDeep,
    #[error("пустая операция: в объединении или пересечении нет форм")]
    Empty,
    #[error("форма слишком большая: больше {MAX_PROGRAM_WORDS} слов (много выдавленных фигур?)")]
    TooLong,
    #[error("модель «{0}» не подставлена: ссылки Use раскрывает чтение файла сцены")]
    Unresolved(String),
    #[error("повтор на {0} копий, больше {MAX_REPEAT} не бывает")]
    TooManyCopies(u32),
}

/// Преобразование «мир → форма» для примитива: матрица поворота, сдвиг и масштаб.
#[derive(Clone, Copy)]
struct Placement {
    /// Мир → форма: `local = rot · p + shift`.
    rot: Mat3,
    shift: Vec3,
    /// Во сколько раз форма больше своих чисел: расстояние формы умножается на него.
    scale: f32,
}

impl Placement {
    const IDENTITY: Self = Self {
        rot: Mat3::IDENTITY,
        shift: Vec3::ZERO,
        scale: 1.0,
    };

    /// Вложенный сдвиг: точка мира сначала переводится в систему родителя, потом в свою.
    fn then(self, at: [f32; 3], turn: [f32; 3], scale: f32) -> Self {
        // local' = R^T (local - at) / s, local = rot p + shift.
        let inv = rotation(turn).transpose() / scale;
        Self {
            rot: inv * self.rot,
            shift: inv * (self.shift - Vec3::from(at)),
            scale: self.scale * scale,
        }
    }
}

/// Дерево формы → программа.
pub fn compile(shape: &Shape3) -> Result<Program, CompileError> {
    compile_with(shape, part_bounds())
}

/// То же с границами частей (М.1) или без них, как ни стоит [`PART_BOUNDS`].
pub fn compile_with(shape: &Shape3, part_bounds: bool) -> Result<Program, CompileError> {
    let mut out = Emit {
        part_bounds,
        ..Emit::default()
    };
    emit(shape, Placement::IDENTITY, 0, 0, &mut out)?;
    if out.words.len() > MAX_PROGRAM_WORDS {
        return Err(CompileError::TooLong);
    }
    Ok(Program {
        words: out.words,
        bounds: shape.bounds(),
        bounds_box: tight_box(shape),
        shadow_only: false,
        no_shadow: false,
        still: false,
        fields: out.fields,
        bricks: out.bricks,
    })
}

/// Коробка формы для сетки отсева: её коробка, подрезанная коробкой шара-границы (обе
/// честные, значит и пересечение).
fn tight_box(shape: &Shape3) -> Option<(Vec3, Vec3)> {
    let (c, r) = shape.bounds()?;
    let (lo, hi) = shape.bounds_box()?;
    let lo = lo.max(c - r);
    Some((lo, hi.min(c + r).max(lo)))
}

/// Куда пишется программа: слова, поля слоя и кирпичи по порядку их примитивов.
#[derive(Default)]
struct Emit {
    words: Vec<u32>,
    fields: Vec<FieldRef>,
    bricks: Vec<BrickRef>,
    /// Писать ли границы частей ([`op::SKIP`]).
    part_bounds: bool,
    /// Сколько итогов жёстких объединений лежит на стеке прямо под пишущейся формой
    /// (М.1, [`union_slots`]).
    hard: usize,
}

/// Пишет команды формы и возвращает, сколько места на стеке ей понадобилось (`base` уже
/// занято ждущими формами).
fn emit(
    shape: &Shape3,
    at: Placement,
    material: u32,
    base: usize,
    out: &mut Emit,
) -> Result<usize, CompileError> {
    if base >= STACK {
        return Err(CompileError::TooDeep);
    }
    let prim_with = |kind: u32, params: [f32; 4], data: &[u32], out: &mut Emit| {
        if data.len() > 0xffff {
            return Err(CompileError::TooLong);
        }
        let out = &mut out.words;
        out.push(op::PRIM | (kind << 8) | ((data.len() as u32) << 16));
        out.push(material);
        for row in 0..3 {
            let r = at.rot.row(row);
            out.extend([r.x, r.y, r.z, at.shift[row]].map(f32::to_bits));
        }
        out.push(at.scale.to_bits());
        out.extend(params.map(f32::to_bits));
        out.extend_from_slice(data);
        Ok(1)
    };
    let prim = |kind: u32, params: [f32; 4], out: &mut Emit| prim_with(kind, params, &[], out);
    match shape {
        Shape3::Sphere { radius } => prim(prim::SPHERE, [*radius, 0.0, 0.0, 0.0], out),
        Shape3::Box { size, round } => prim(
            prim::BOX,
            [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5, *round],
            out,
        ),
        Shape3::Capsule { height, radius } => {
            prim(prim::CAPSULE, [*height * 0.5, *radius, 0.0, 0.0], out)
        }
        Shape3::Cylinder {
            height,
            radius,
            round,
        } => prim(prim::CYLINDER, [*height * 0.5, *radius, *round, 0.0], out),
        Shape3::Torus { radius, thickness } => {
            prim(prim::TORUS, [*radius, *thickness, 0.0, 0.0], out)
        }
        Shape3::Cone {
            height,
            bottom,
            top,
            round,
        } => prim(prim::CONE, [*height * 0.5, *bottom, *top, *round], out),
        Shape3::Plane { normal, offset } => {
            let n = Vec3::from(*normal).normalize_or(Vec3::Y);
            prim(prim::PLANE, [n.x, n.y, n.z, *offset], out)
        }
        Shape3::Extrude {
            profile,
            depth,
            round,
        } => prim_with(
            prim::EXTRUDE,
            [*depth * 0.5, *round, 0.0, 0.0],
            profile.words(),
            out,
        ),
        Shape3::Revolve { profile, offset } => prim_with(
            prim::REVOLVE,
            [*offset, 0.0, 0.0, 0.0],
            profile.words(),
            out,
        ),
        Shape3::Field {
            field,
            depth,
            round,
        } => {
            let header = field.0.header_words();
            let half = *depth * 0.5;
            // a.z — половина толщины второго слоя (0 — его нет), его место и материал —
            // в словах 10 и 11 заголовка.
            let second = field.0.second().map_or(0.0, |s| s.half);
            prim_with(
                prim::FIELD,
                [half, *round, second, field.0.band()],
                &header,
                out,
            )?;
            out.fields.push(field.clone());
            Ok(1)
        }
        // Кирпич без картинки (видеокарта её не загрузила) — тени нет: точка.
        Shape3::Brick { brick } => match brick.0.texture() {
            Some(_) => {
                let b = &brick.0;
                prim_with(
                    prim::BRICK,
                    [b.inset(), b.band(), 0.0, 0.0],
                    &b.header_words(),
                    out,
                )?;
                out.bricks.push(brick.clone());
                Ok(1)
            }
            None => prim(prim::SPHERE, [0.0; 4], out),
        },
        Shape3::Union { shapes, smooth } => {
            chain(shapes, op::UNION, *smooth, at, material, base, out)
        }
        Shape3::Intersect { shapes, smooth } => {
            chain(shapes, op::INTERSECT, *smooth, at, material, base, out)
        }
        Shape3::Subtract {
            base: b,
            cut,
            smooth,
        } => {
            // Внутри вырезания итоги объединений снаружи для границ частей не годятся.
            // (При ошибке программа не нужна, восстанавливать счёт незачем.)
            let hard = std::mem::take(&mut out.hard);
            let mut need = emit(b, at, material, base, out)?;
            for c in cut {
                need = need.max(1 + emit(c, at, material, base + 1, out)?);
                out.words.extend([op::SUBTRACT, smooth.to_bits()]);
            }
            out.hard = hard;
            Ok(need)
        }
        Shape3::Move {
            at: to,
            turn,
            scale,
            shape,
        } => emit(
            shape,
            at.then(*to, *turn, scale.max(1e-6)),
            material,
            base,
            out,
        ),
        Shape3::Paint { material, shape } => emit(shape, at, *material, base, out),
        Shape3::Animate { .. } => emit(&shape.at_time(0.0), at, material, base, out),
        Shape3::Mirror {
            axis,
            smooth,
            shape,
        } => {
            // Отражённая копия: точка мира сначала переводится в систему формы, потом
            // отражается (отражение — тоже движение без растяжения, расстояние то же).
            let s = Mat3::from_diagonal(axis.reflect(Vec3::ONE));
            let mirrored = Placement {
                rot: s * at.rot,
                shift: s * at.shift,
                scale: at.scale,
            };
            let (first, second) = union_slots(out.hard, *smooth);
            let need = part(shape, at, material, base, out, first)?;
            let need = need.max(1 + part(shape, mirrored, material, base + 1, out, second)?);
            out.words.extend([op::UNION, smooth.to_bits()]);
            Ok(need)
        }
        Shape3::Repeat {
            count,
            step,
            smooth,
            shape,
        } => {
            if *count > MAX_REPEAT {
                return Err(CompileError::TooManyCopies(*count));
            }
            emit(
                &Shape3::repeat_union(*count, *step, *smooth, shape),
                at,
                material,
                base,
                out,
            )
        }
        Shape3::Use { model } => Err(CompileError::Unresolved(model.clone())),
    }
}

fn chain(
    shapes: &[Shape3],
    code: u32,
    smooth: f32,
    at: Placement,
    material: u32,
    base: usize,
    out: &mut Emit,
) -> Result<usize, CompileError> {
    let (first, rest) = shapes.split_first().ok_or(CompileError::Empty)?;
    let (head, tail) = if code == op::UNION {
        union_slots(out.hard, smooth)
    } else {
        (Slot::NONE, Slot::NONE)
    };
    let mut need = part(first, at, material, base, out, head)?;
    for s in rest {
        need = need.max(1 + part(s, at, material, base + 1, out, tail)?);
        out.words.extend([code, smooth.to_bits()]);
    }
    Ok(need)
}

/// С чем часть сравнивает свою границу (М.1): с наименьшим из `count` итогов объединений
/// на вершине стека плюс ширина шва `smooth`. `hard` — сколько итогов жёстких объединений
/// лежит под частями внутри неё.
#[derive(Clone, Copy)]
struct Slot {
    count: usize,
    smooth: f32,
    hard: usize,
}

impl Slot {
    const NONE: Self = Self {
        count: 0,
        smooth: 0.0,
        hard: 0,
    };
}

/// Места первой и следующих частей объединения со швом `smooth`, если под ним лежат
/// `hard` итогов жёстких объединений, в которые оно входит.
///
/// Жёсткое объединение — просто наименьшее: часть, до шара которой не ближе какого-то
/// итога снаружи, итог всего объединения не изменит, и вместо неё можно положить на стек
/// расстояние до шара (оно не меньше итога). Поэтому у жёстких объединений часть смотрит
/// на все итоги снаружи, первая часть тоже. У плавного шов смешивает части, и обойти
/// часть можно, только если она дальше итога этого же объединения на ширину шва (там
/// плавное объединение — уже просто наименьшее); первую часть — нельзя, а внутри частей
/// итоги снаружи не годятся.
fn union_slots(hard: usize, smooth: f32) -> (Slot, Slot) {
    if smooth == 0.0 {
        let first = Slot {
            count: hard,
            smooth: 0.0,
            hard,
        };
        let rest = Slot {
            count: hard + 1,
            smooth: 0.0,
            hard: hard + 1,
        };
        (first, rest)
    } else {
        let rest = Slot {
            count: 1,
            smooth: smooth.max(0.0),
            hard: 0,
        };
        (Slot::NONE, rest)
    }
}

/// Пишет часть объединения с её границей ([`op::SKIP`]), если граница нужна.
fn part(
    shape: &Shape3,
    at: Placement,
    material: u32,
    base: usize,
    out: &mut Emit,
    slot: Slot,
) -> Result<usize, CompileError> {
    let header = (out.part_bounds && slot.count > 0)
        .then(|| part_bound(shape, at, slot))
        .flatten();
    let mark = out.words.len();
    if let Some(words) = header {
        out.words.extend(words);
    }
    let hard = std::mem::replace(&mut out.hard, slot.hard);
    let need = emit(shape, at, material, base, out)?;
    out.hard = hard;
    if header.is_some() {
        let len = out.words.len() - mark - SKIP_WORDS;
        if len > 0xffff {
            return Err(CompileError::TooLong);
        }
        out.words[mark] |= (len as u32) << 16;
    }
    Ok(need)
}

/// Слова границы части `shape` (М.1), если она нужна: часть из нескольких примитивов
/// (одиночный простой примитив считается не дольше самой проверки), конечная, без полей
/// слоя и кирпичей (их зеркало на процессоре читает поля по порядку). Длина пропуска
/// дописывается после части.
fn part_bound(shape: &Shape3, at: Placement, slot: Slot) -> Option<[u32; SKIP_WORDS]> {
    if single_primitive(shape) || holds_textures(shape) || slot.count > 0xff {
        return None;
    }
    let (c, r) = shape.bounds()?;
    // Мир → форма: local = rot · p + shift, значит p = rot⁻¹ · (local − shift).
    let inv = at.rot.inverse();
    if !inv.is_finite() {
        return None;
    }
    let center = inv * (c - at.shift);
    let radius = r * at.scale;
    Some([
        op::SKIP | (slot.count as u32) << 8,
        center.x.to_bits(),
        center.y.to_bits(),
        center.z.to_bits(),
        radius.to_bits(),
        slot.smooth.to_bits(),
    ])
}

/// Форма — один простой примитив (со сдвигами и материалом вокруг).
fn single_primitive(shape: &Shape3) -> bool {
    match shape {
        Shape3::Move { shape, .. } | Shape3::Paint { shape, .. } => single_primitive(shape),
        Shape3::Sphere { .. }
        | Shape3::Box { .. }
        | Shape3::Capsule { .. }
        | Shape3::Cylinder { .. }
        | Shape3::Torus { .. }
        | Shape3::Cone { .. }
        | Shape3::Plane { .. } => true,
        _ => false,
    }
}

/// В форме есть поле слоя или кирпич поля.
fn holds_textures(shape: &Shape3) -> bool {
    match shape {
        Shape3::Field { .. } | Shape3::Brick { .. } => true,
        Shape3::Union { shapes, .. } | Shape3::Intersect { shapes, .. } => {
            shapes.iter().any(holds_textures)
        }
        Shape3::Subtract { base, cut, .. } => {
            holds_textures(base) || cut.iter().any(holds_textures)
        }
        Shape3::Move { shape, .. }
        | Shape3::Paint { shape, .. }
        | Shape3::Animate { shape, .. }
        | Shape3::Mirror { shape, .. }
        | Shape3::Repeat { shape, .. } => holds_textures(shape),
        _ => false,
    }
}

impl Program {
    /// Поле уровня (7.18д3): вся программа — один примитив «поле слоя» без прямоугольника
    /// фигуры (бесконечный), без поворота и масштаба (только сдвиг), обычный объект (не
    /// заменитель и не светящийся). Такой объект шейдер считает до перебора объектов
    /// (`level_field_near` в `include/sdf3d.slang`), без матрицы и лестницы видов.
    pub fn level_field(&self) -> bool {
        let w = &self.words;
        if self.bounds.is_some()
            || self.shadow_only
            || self.no_shadow
            || self.fields.len() != 1
            || w.len() != PRIM_WORDS + FIELD_PRIM_WORDS
            || w[0] != op::PRIM | (prim::FIELD << 8) | ((FIELD_PRIM_WORDS as u32) << 16)
        {
            return false;
        }
        let f = |i: usize| f32::from_bits(w[i]);
        let rot = [f(2), f(3), f(4), f(6), f(7), f(8), f(10), f(11), f(12)];
        let at = PRIM_WORDS;
        let rect = f(at + 6) <= f(at + 8) && f(at + 7) <= f(at + 9);
        rot == [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] && f(14) == 1.0 && !rect
    }

    /// Если вся форма — одна плоскость: плоскость в мире `(n, c)`, расстояние до неё
    /// `dot(n, p) + c` (`n` единичная). Такую форму луч глаза находит формулой (7.6и).
    pub fn plane(&self) -> Option<(Vec3, f32)> {
        let w = &self.words;
        if w.len() != PRIM_WORDS || w[0] != op::PRIM | (prim::PLANE << 8) {
            return None;
        }
        let f = |i: usize| f32::from_bits(w[i]);
        let row = |r: usize| Vec3::new(f(2 + 4 * r), f(3 + 4 * r), f(4 + 4 * r));
        let (rot, shift) = (
            Mat3::from_cols(row(0), row(1), row(2)).transpose(),
            Vec3::new(f(5), f(9), f(13)),
        );
        let (scale, n, offset) = (f(14), Vec3::new(f(15), f(16), f(17)), f(18));
        // d = scale · (n · (rot p + shift) − offset).
        let world = rot.transpose() * n * scale;
        let len = world.length();
        if len.is_nan() || len <= 1e-6 {
            return None;
        }
        Some((world / len, scale * (n.dot(shift) - offset) / len))
    }

    /// Программа — цепочка «форма, форма, операция, форма, операция…»: на стеке машины
    /// никогда не больше двух значений. Шейдер считает такую без стека (`sdf_chain`).
    /// Одна форма без операций — не цепочка: её шейдер и так считает напрямую.
    pub fn chain(&self) -> bool {
        let w = &self.words;
        let mut depth = 0usize;
        let mut ops = 0usize;
        let mut i = 0;
        while i < w.len() {
            if w[i] & 0xff == op::SKIP {
                i += SKIP_WORDS;
            } else if w[i] & 0xff == op::PRIM {
                depth += 1;
                if depth > 2 {
                    return false;
                }
                i += PRIM_WORDS + (w[i] >> 16) as usize;
            } else {
                if depth < 2 {
                    return false;
                }
                depth -= 1;
                ops += 1;
                i += OP_WORDS;
            }
        }
        depth == 1 && ops > 0
    }

    /// Расстояние до формы в точке `p` и материал ближайшей части: то же, что считает
    /// шейдер по этой программе. Материал [`ESTIMATE_MATERIAL`] — ближайшая часть —
    /// поле слоя, и там оно только оценка снизу (7.18б).
    pub fn distance(&self, p: Vec3) -> (f32, u32) {
        self.distance_counted(p).0
    }

    /// То же и сколько примитивов пришлось посчитать (М.1: счётчик «примитивов на шаг»).
    pub fn distance_counted(&self, p: Vec3) -> ((f32, u32), usize) {
        let mut prims = 0;
        let w = &self.words;
        let f = |i: usize| f32::from_bits(w[i]);
        let mut stack: Vec<(f32, u32)> = Vec::with_capacity(STACK);
        let mut i = 0;
        let mut fields = self.fields.iter();
        let mut bricks = self.bricks.iter();
        while i < w.len() {
            let code = w[i] & 0xff;
            if code == op::SKIP {
                // Граница части (М.1): то же правило, что в шейдере.
                let count = ((w[i] >> 8) & 0xff) as usize;
                let len = (w[i] >> 16) as usize;
                if count > 0 && stack.len() >= count {
                    let near = stack[stack.len() - count..]
                        .iter()
                        .map(|s| s.0)
                        .fold(f32::MAX, f32::min);
                    let center = Vec3::new(f(i + 1), f(i + 2), f(i + 3));
                    let outside = (p - center).length() - f(i + 4);
                    if outside >= near + f(i + 5) {
                        stack.push((outside, 0));
                        i += SKIP_WORDS + len;
                        continue;
                    }
                }
                i += SKIP_WORDS;
            } else if code == op::PRIM {
                prims += 1;
                let kind = (w[i] >> 8) & 0xff;
                let mut material = w[i + 1];
                let m = |r: usize| Vec3::new(f(i + 2 + 4 * r), f(i + 3 + 4 * r), f(i + 4 + 4 * r));
                let local = Vec3::new(
                    m(0).dot(p) + f(i + 5),
                    m(1).dot(p) + f(i + 9),
                    m(2).dot(p) + f(i + 13),
                );
                let scale = f(i + 14);
                let a = [f(i + 15), f(i + 16), f(i + 17), f(i + 18)];
                let data = (w[i] >> 16) as usize;
                let field = &w[(i + PRIM_WORDS).min(w.len())..];
                let d = match kind {
                    prim::SPHERE => sd_sphere(local, a[0]),
                    prim::BOX => sd_box(local, Vec3::new(a[0], a[1], a[2]), a[3]),
                    prim::CAPSULE => sd_capsule(local, a[0], a[1]),
                    prim::CYLINDER => sd_cylinder(local, a[0], a[1], a[2]),
                    prim::TORUS => sd_torus(local, a[0], a[1]),
                    prim::CONE => sd_cone(local, a[0], a[1], a[2], a[3]),
                    prim::EXTRUDE if data >= FIELD_HEADER_WORDS => sd_extrude(
                        local,
                        field_distance(field, Vec2::new(local.x, local.y)),
                        a[0],
                        a[1],
                    ),
                    prim::REVOLVE if data >= FIELD_HEADER_WORDS => {
                        field_distance(field, revolve_point(local, a[0]))
                    }
                    prim::FIELD if data >= FIELD_PRIM_WORDS => {
                        // Числа поля — из картинки, которую держит программа; без неё
                        // (испорченная программа) поля нет: бесконечно далеко.
                        match fields.next() {
                            Some(f) => {
                                let (d, m) = f.0.sample(local, a[0], a[1], material);
                                material = m;
                                d
                            }
                            None => f32::MAX,
                        }
                    }
                    prim::BRICK if data >= BRICK_PRIM_WORDS => match bricks.next() {
                        Some(b) => b.0.sample(local) + a[0],
                        None => f32::MAX,
                    },
                    _ => {
                        debug_assert!(kind == prim::PLANE, "неизвестный вид примитива {kind}");
                        sd_plane(local, Vec3::new(a[0], a[1], a[2]), a[3])
                    }
                };
                stack.push((d * scale, material));
                i += PRIM_WORDS + data;
            } else {
                let k = f(i + 1);
                let (b, a) = (
                    stack.pop().unwrap_or((f32::MAX, 0)),
                    stack.pop().unwrap_or((f32::MAX, 0)),
                );
                let d = match code {
                    op::UNION => op_union(a.0, b.0, k),
                    op::SUBTRACT => op_subtract(a.0, b.0, k),
                    _ => op_intersect(a.0, b.0, k),
                };
                // Материал: у объединения — ближайшей части, иначе — основы.
                let material = if code == op::UNION && b.0 < a.0 {
                    b.1
                } else {
                    a.1
                };
                stack.push((d, material));
                i += OP_WORDS;
            }
        }
        (stack.pop().unwrap_or((f32::MAX, 0)), prims)
    }
}

/// Материал поверхности.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material3 {
    /// Цвет (линейный, 0…1).
    pub color: [f32; 3],
    /// Свечение: сколько своего света (0 — не светится). Светящаяся поверхность не
    /// темнеет в тени, а ярче 1 даёт ореол свечения (bloom) при выводе.
    #[serde(default)]
    pub glow: f32,
    /// Рисовать на поверхности сетку через 1 и 10 м (пол сцены «3D»).
    #[serde(default)]
    pub grid: bool,
    /// Текстура цвета (8.14б2): номер в `SpaceScene::textures` плюс 1, 0 — без текстуры.
    /// Ставится только у материалов моделей из файлов, в файле сцены не пишется. В буфере
    /// сцены видеокарта заменяет его номером картинки в общем списке текстур.
    #[serde(skip)]
    pub texture: u32,
}

/// Флаг материала: сетка на поверхности.
pub const MATERIAL_GRID: u32 = 1;
/// Флаг материала (8.14б2): у полигонов с ним цвет умножается на текстуру; номер
/// текстуры — слово 5 записи материала.
pub const MATERIAL_TEXTURED: u32 = 2;

/// Лампа: точечный источник света с мягкой тенью.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lamp3 {
    /// Где стоит.
    pub at: [f32; 3],
    /// Цвет света (линейный).
    pub color: [f32; 3],
    /// Яркость на расстоянии 1 м.
    pub power: f32,
    /// Дальше этого расстояния (м) лампа не светит: свет плавно гаснет к границе.
    pub range: f32,
}

/// Сколько ламп отбрасывают тени (остальные светят без теней): тень — это ещё один
/// проход лучом по сцене на каждую точку экрана.
pub const SHADOW_LAMPS: usize = 2;

/// Слов в записи материала и лампы.
const MATERIAL_WORDS: usize = 8;
/// Сколько материалов различает буфер геометрии сцены «3D» (7.20б: 12 бит номера,
/// `GBUFFER_MATERIALS` в space.slang). Материалы с большими номерами рисуются последним;
/// файл сцены с большим числом не загружается, сцена из уровня пишет предупреждение.
pub const MAX_MATERIALS: usize = 4096;
const LAMP_WORDS: usize = 8;
/// Слов в записи объекта.
pub const OBJECT_WORDS: usize = 8;
/// Флаг объекта: заменитель полигональной модели, только для теней (как в
/// `include/space.slang`).
pub const OBJECT_SHADOW_ONLY: u32 = 1;
/// Флаг объекта: объект — одна плоскость (пол, стена). Луч глаза находит её точку
/// формулой и не опрашивает её на каждом шаге (7.6и); в слове 3 и словах 4–6 записи
/// объекта вместо шара-границы — плоскость в мире: `dot(n, p) + c` (c, потом n).
pub const OBJECT_PLANE: u32 = 2;
/// Флаг объекта: программа — цепочка, где операции ждёт не больше одной формы
/// ([`Program::chain`]); шейдер считает её без стека, в регистрах (7.6т).
pub const OBJECT_CHAIN: u32 = 4;
/// Флаг объекта: теней и затенения в углах не даёт ([`Program::no_shadow`]).
pub const OBJECT_NO_SHADOW: u32 = 8;
/// Больше плоскостей, считаемых формулой, не бывает (шейдер дальше не читает).
pub const MAX_PLANES: usize = 8;
/// Больше объектов шейдер не читает (`MAX_OBJECTS` в `include/space.slang`: число
/// объектов из заголовка режется до этого предела молча). Список для видеокарты
/// обрезается до него на процессоре заранее, с предупреждением (7.18б). Было 1024; для
/// сцены завода на 1000+ предметов (8.7) поднято до 4096 — номер объекта в списках клеток
/// и так 16 бит, а в кадре столько объектов бывает, только когда камера видит весь цех.
pub const MAX_OBJECTS: usize = 4096;
/// Слов в заголовке сцены.
const HEADER_WORDS: usize = 14;

/// Поле уровня (7.18д3): первый объект, который шейдер считает отдельно, до перебора
/// объектов ([`Program::level_field`]); у 2.5D-вида это поле уровня, но подойдёт и
/// такое же поле в сцене «3D». В сетку отсева он не идёт.
pub fn level_field_object(objects: &[Program]) -> Option<usize> {
    objects.iter().position(Program::level_field)
}

/// Объекты-плоскости, которые луч глаза находит формулой ([`OBJECT_PLANE`]): первые
/// [`MAX_PLANES`] видимых объектов из одной плоскости.
pub fn plane_objects(objects: &[Program]) -> Vec<u32> {
    (0..objects.len() as u32)
        .filter(|&i| {
            let p = &objects[i as usize];
            !p.shadow_only && p.plane().is_some()
        })
        .take(MAX_PLANES)
        .collect()
}

/// Сцена для видеокарты: объекты (у каждого своя программа), материалы и лампы одним
/// списком слов. Раскладка (читает `include/space.slang`):
///
/// - заголовок, 14 слов: число объектов, где начинаются объекты, где материалы, сколько
///   их, где лампы, сколько их, где сетка отсева (0 — сетки нет), где полигоны (пишет
///   `reyvec-gpu`), где список плоскостей и сколько их ([`OBJECT_PLANE`]), где раздел
///   запечённого поля (7.10, пишет `reyvec-gpu`; 0 — поля нет), флаги кадра (пишет
///   `reyvec-gpu`), номер объекта поля уровня + 1 (0 — его нет) и начало его программы
///   (7.18д3, [`level_field_object`]);
/// - объекты по 8 слов ([`OBJECT_WORDS`]): начало программы, её длина в словах, флаги
///   ([`OBJECT_SHADOW_ONLY`], [`OBJECT_PLANE`]), 1 запасное, шар-граница (середина и
///   радиус; радиус -1 — граница не нужна, форма бесконечна);
/// - материалы по 8 слов: цвет (3 числа), флаги ([`MATERIAL_GRID`]), свечение, 3 запасных;
/// - лампы по 8 слов: место (3 числа), дальность, цвет (3 числа), яркость;
/// - программы подряд;
/// - список плоскостей: номера объектов (не больше [`MAX_PLANES`]);
/// - сетка отсева ([`crate::grid3d::Grid::write`]).
pub fn pack_scene(objects: &[Program], materials: &[Material3], lamps: &[Lamp3]) -> Vec<u32> {
    pack_scene_with(objects, materials, lamps, true, GridOptions::default())
}

/// То же; `level_field: false` — поле уровня считается в общем переборе, как до 7.18д3
/// (для сравнения в тестах); `grid` — как строить сетку отсева (для сравнения на
/// панели: без свободного расстояния, как до 7.18д4; по шарам, как до 8.8а).
pub fn pack_scene_with(
    objects: &[Program],
    materials: &[Material3],
    lamps: &[Lamp3],
    level_field: bool,
    grid: GridOptions,
) -> Vec<u32> {
    pack_scene_cached(
        objects,
        materials,
        lamps,
        level_field,
        grid,
        &mut crate::grid3d::GridCache::default(),
    )
}

/// То же; сетка отсева берёт готовый вклад неподвижного из `cache` (8.8б,
/// [`crate::grid3d::Grid::build_cached`]). Слова те же, что у [`pack_scene_with`].
pub fn pack_scene_cached(
    objects: &[Program],
    materials: &[Material3],
    lamps: &[Lamp3],
    level_field: bool,
    grid: GridOptions,
    cache: &mut crate::grid3d::GridCache,
) -> Vec<u32> {
    let objects_at = HEADER_WORDS;
    let materials_at = objects_at + objects.len() * OBJECT_WORDS;
    let lamps_at = materials_at + materials.len() * MATERIAL_WORDS;
    let code_at = lamps_at + lamps.len() * LAMP_WORDS;
    let mut out = vec![
        objects.len() as u32,
        objects_at as u32,
        materials_at as u32,
        materials.len() as u32,
        lamps_at as u32,
        lamps.len() as u32,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    let mut start = code_at;
    let field = level_field.then(|| level_field_object(objects)).flatten();
    let planes = plane_objects(objects);
    for (i, p) in objects.iter().enumerate() {
        let mut flags = if p.shadow_only { OBJECT_SHADOW_ONLY } else { 0 };
        let plane = p.plane().filter(|_| planes.contains(&(i as u32)));
        if plane.is_some() {
            flags |= OBJECT_PLANE;
        }
        if p.chain() {
            flags |= OBJECT_CHAIN;
        }
        if p.no_shadow {
            flags |= OBJECT_NO_SHADOW;
        }
        let (n, c) = plane.unwrap_or((Vec3::ZERO, 0.0));
        out.extend([start as u32, p.words.len() as u32, flags, c.to_bits()]);
        let (center, r) = p.bounds.unwrap_or((n, -1.0));
        out.extend([center.x, center.y, center.z, r].map(f32::to_bits));
        if field == Some(i) {
            out[12] = i as u32 + 1;
            out[13] = start as u32;
        }
        start += p.words.len();
    }
    for m in materials {
        out.extend(m.color.map(f32::to_bits));
        out.push(
            (u32::from(m.grid) * MATERIAL_GRID) | (u32::from(m.texture != 0) * MATERIAL_TEXTURED),
        );
        out.extend([m.glow.to_bits(), m.texture, 0, 0]);
    }
    for l in lamps {
        out.extend(l.at.map(f32::to_bits));
        out.push(l.range.to_bits());
        out.extend(l.color.map(f32::to_bits));
        out.push(l.power.to_bits());
    }
    for p in objects {
        out.extend(&p.words);
    }
    out[8] = out.len() as u32;
    out[9] = planes.len() as u32;
    out.extend(&planes);
    let field = field.map(|i| i as u32);
    let grid = crate::grid3d::Grid::build_cached(objects, field, grid, cache);
    if !grid.cells.is_empty() {
        out[6] = out.len() as u32;
        let at = out.len();
        grid.write(at, &mut out);
    }
    out
}

/// Освещённость от лампы `lamp` в точке на расстоянии `distance` (без тени и наклона):
/// обратный квадрат, плавно гаснущий к дальности. Зеркало `lamp_falloff` в
/// `include/space.slang`.
pub fn lamp_falloff(lamp: &Lamp3, distance: f32) -> f32 {
    let x = (distance / lamp.range.max(1e-3)).min(1.0);
    let fade = (1.0 - x * x * x * x).max(0.0);
    lamp.power * fade * fade / (distance * distance + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points() -> Vec<Vec3> {
        let mut out = Vec::new();
        for x in -4..=4 {
            for y in -2..=4 {
                for z in -4..=4 {
                    out.push(Vec3::new(x as f32 * 0.61, y as f32 * 0.53, z as f32 * 0.47));
                }
            }
        }
        out
    }

    /// Формы М.0б: конус, зеркало, повтор — в том числе внутри сдвига с поворотом.
    fn m0b_shapes() -> Vec<Shape3> {
        let cone = Shape3::Cone {
            height: 1.2,
            bottom: 0.6,
            top: 0.15,
            round: 0.05,
        };
        let leg = Shape3::Move {
            at: [0.7, 0.2, -0.3],
            turn: [10.0, 25.0, -15.0],
            scale: 1.0,
            shape: Box::new(Shape3::Box {
                size: [0.2, 0.9, 0.3],
                round: 0.02,
            }),
        };
        vec![
            cone.clone(),
            Shape3::Cone {
                height: 0.8,
                bottom: 0.5,
                top: 0.0,
                round: 0.0,
            },
            Shape3::Mirror {
                axis: Axis::X,
                smooth: 0.1,
                shape: Box::new(leg.clone()),
            },
            Shape3::Move {
                at: [0.3, -0.4, 0.5],
                turn: [0.0, 40.0, 20.0],
                // Масштаб 1: плавный шов под масштабом дерево и программа и до М.0б
                // считают чуть по-разному (ширина шва в программе не масштабируется).
                scale: 1.0,
                shape: Box::new(Shape3::Mirror {
                    axis: Axis::Z,
                    smooth: 0.0,
                    shape: Box::new(Shape3::Union {
                        shapes: vec![leg.clone(), cone.clone()],
                        smooth: 0.05,
                    }),
                }),
            },
            Shape3::Repeat {
                count: 5,
                step: [0.45, 0.0, 0.1],
                smooth: 0.0,
                shape: Box::new(Shape3::Move {
                    at: [-1.0, 0.0, 0.0],
                    turn: [0.0; 3],
                    scale: 1.0,
                    shape: Box::new(Shape3::Sphere { radius: 0.2 }),
                }),
            },
            Shape3::Mirror {
                axis: Axis::Y,
                smooth: 0.0,
                shape: Box::new(Shape3::Repeat {
                    count: 3,
                    step: [0.0, 0.0, 0.6],
                    smooth: 0.08,
                    shape: Box::new(leg),
                }),
            },
        ]
    }

    #[test]
    fn m0b_program_matches_tree() {
        for shape in m0b_shapes() {
            let program = compile(&shape).unwrap();
            for p in points() {
                let (a, b) = (shape.distance(p), program.distance(p).0);
                assert!(
                    (a - b).abs() < 1e-4,
                    "{shape:?} в {p}: дерево {a}, программа {b}"
                );
            }
        }
    }

    #[test]
    fn m0b_bounds_hold_the_shape() {
        for shape in m0b_shapes() {
            let (c, r) = shape.bounds().unwrap();
            let (lo, hi) = shape.bounds_box().unwrap();
            for x in -30..=30 {
                for y in -30..=30 {
                    for z in -30..=30 {
                        let p = Vec3::new(x as f32, y as f32, z as f32) * 0.07;
                        if shape.distance(p) < 0.0 {
                            assert!((p - c).length() <= r + 1e-4, "{shape:?}: {p} вне шара");
                            assert!(
                                p.cmpge(lo - 1e-4).all() && p.cmple(hi + 1e-4).all(),
                                "{shape:?}: {p} вне коробки {lo} {hi}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Стеллаж из многих деталей (М.1): стойки, полки, ящики с плавными швами, повтор,
    /// зеркало, сдвиг с поворотом и масштабом.
    fn m1_rack() -> Shape3 {
        let post = |x: f32, z: f32| Shape3::Move {
            at: [x, 1.0, z],
            turn: [0.0; 3],
            scale: 1.0,
            shape: Box::new(Shape3::Box {
                size: [0.08, 2.0, 0.08],
                round: 0.01,
            }),
        };
        let shelf = |y: f32| Shape3::Move {
            at: [0.0, y, 0.0],
            turn: [0.0; 3],
            scale: 1.0,
            shape: Box::new(Shape3::Box {
                size: [1.0, 0.05, 0.5],
                round: 0.0,
            }),
        };
        let crate_ = |x: f32, y: f32| {
            Shape3::Move {
                at: [x, y, 0.0],
                turn: [0.0, 15.0, 0.0],
                scale: 1.0,
                shape: Box::new(Shape3::Subtract {
                    base: Box::new(Shape3::Box {
                        size: [0.3, 0.3, 0.3],
                        round: 0.02,
                    }),
                    cut: vec![Shape3::Sphere { radius: 0.17 }],
                    smooth: 0.02,
                }),
            }
            .paint(2)
        };
        let unit = Shape3::Union {
            shapes: vec![
                Shape3::Mirror {
                    axis: Axis::X,
                    smooth: 0.0,
                    shape: Box::new(Shape3::Union {
                        shapes: vec![post(0.48, 0.22), post(0.48, -0.22)],
                        smooth: 0.0,
                    }),
                }
                .paint(1),
                Shape3::Union {
                    shapes: vec![shelf(0.3), shelf(1.0), shelf(1.7)],
                    smooth: 0.03,
                },
                crate_(-0.25, 0.48),
                crate_(0.2, 1.18),
            ],
            smooth: 0.02,
        };
        Shape3::Move {
            at: [-2.0, 0.0, 0.5],
            turn: [0.0, 30.0, 5.0],
            scale: 1.3,
            shape: Box::new(Shape3::Repeat {
                count: 6,
                step: [1.1, 0.0, 0.0],
                smooth: 0.0,
                shape: Box::new(unit),
            }),
        }
    }

    #[test]
    fn m1_part_bounds_keep_distance_and_material() {
        let mut shapes = m0b_shapes();
        shapes.push(m1_rack());
        for shape in shapes {
            let plain = compile_with(&shape, false).unwrap();
            let fast = compile_with(&shape, true).unwrap();
            for x in -40..=40 {
                for y in -10..=30 {
                    for z in -12..=12 {
                        let p = Vec3::new(x as f32 * 0.17, y as f32 * 0.11, z as f32 * 0.19);
                        let (a, b) = (plain.distance(p), fast.distance(p));
                        assert!(
                            (a.0 - b.0).abs() <= 1e-5 * a.0.abs().max(1.0),
                            "{p}: без границ {a:?}, с границами {b:?}"
                        );
                        assert_eq!(a.1, b.1, "{p}: материал");
                    }
                }
            }
        }
    }

    #[test]
    fn m1_part_bounds_cut_primitives_per_step() {
        let shape = m1_rack();
        let plain = compile_with(&shape, false).unwrap();
        let fast = compile_with(&shape, true).unwrap();
        assert!(fast.words.len() > plain.words.len());
        // Без границ каждая точка считает все примитивы.
        let all = plain.distance_counted(Vec3::ZERO).1;
        assert_eq!(all, 6 * 11);
        let (mut sum, mut n) = (0, 0);
        for x in -40..=40 {
            for y in -10..=30 {
                let p = Vec3::new(x as f32 * 0.17, y as f32 * 0.11, 0.3);
                assert_eq!(plain.distance_counted(p).1, all);
                sum += fast.distance_counted(p).1;
                n += 1;
            }
        }
        let mean = sum as f32 / n as f32;
        assert!(
            mean < all as f32 * 0.4,
            "в среднем {mean} примитивов из {all}"
        );
    }

    #[test]
    fn cone_formula() {
        // Без скругления и с равными радиусами конус — цилиндр.
        let cyl = Shape3::Cylinder {
            height: 1.0,
            radius: 0.4,
            round: 0.0,
        };
        let same = Shape3::Cone {
            height: 1.0,
            bottom: 0.4,
            top: 0.4,
            round: 0.0,
        };
        for p in points() {
            let (a, b) = (cyl.distance(p), same.distance(p));
            assert!((a - b).abs() < 1e-4, "{p}: {a} {b}");
        }
        let cone = Shape3::Cone {
            height: 2.0,
            bottom: 1.0,
            top: 0.0,
            round: 0.0,
        };
        assert!((cone.distance(Vec3::new(0.0, 1.5, 0.0)) - 0.5).abs() < 1e-5);
        assert!((cone.distance(Vec3::new(0.0, -1.5, 0.0)) - 0.5).abs() < 1e-5);
        assert!((cone.distance(Vec3::new(2.0, -1.0, 0.0)) - 1.0).abs() < 1e-5);
        assert!(cone.distance(Vec3::ZERO) < 0.0);
        // Точка на боку: середина образующей.
        assert!(cone.distance(Vec3::new(0.5, 0.0, 0.0)).abs() < 1e-5);
    }

    #[test]
    fn mirror_and_repeat_are_copies() {
        let ball = Shape3::Move {
            at: [0.8, 0.1, 0.0],
            turn: [0.0; 3],
            scale: 1.0,
            shape: Box::new(Shape3::Sphere { radius: 0.3 }),
        };
        let mirror = Shape3::Mirror {
            axis: Axis::X,
            smooth: 0.0,
            shape: Box::new(ball.clone()),
        };
        assert!(mirror.distance(Vec3::new(-0.8, 0.1, 0.0)) < -0.29);
        assert!(mirror.distance(Vec3::new(0.8, 0.1, 0.0)) < -0.29);
        assert!(mirror.distance(Vec3::new(0.0, 0.1, 0.0)) > 0.4);
        let repeat = Shape3::Repeat {
            count: 4,
            step: [0.0, 1.0, 0.0],
            smooth: 0.0,
            shape: Box::new(ball),
        };
        for k in 0..4 {
            assert!(repeat.distance(Vec3::new(0.8, 0.1 + k as f32, 0.0)) < -0.29);
        }
        assert!(repeat.distance(Vec3::new(0.8, 4.1, 0.0)) > 0.6);
        let too_many = Shape3::Repeat {
            count: MAX_REPEAT + 1,
            step: [1.0, 0.0, 0.0],
            smooth: 0.0,
            shape: Box::new(Shape3::Sphere { radius: 0.1 }),
        };
        assert_eq!(
            compile(&too_many).unwrap_err(),
            CompileError::TooManyCopies(MAX_REPEAT + 1)
        );
        let text = "Mirror(axis: Z, shape: Repeat(count: 3, step: (0, 0.5, 0), \
            shape: Cone(height: 1, bottom: 0.3)))";
        let read: Shape3 = ron::from_str(text).unwrap();
        assert!(matches!(read, Shape3::Mirror { axis: Axis::Z, .. }));
    }

    #[test]
    fn chains_need_no_stack() {
        let sphere = || Shape3::Sphere { radius: 0.5 };
        let hollow = Shape3::Subtract {
            base: Box::new(Shape3::Box {
                size: [1.2, 1.2, 1.2],
                round: 0.04,
            }),
            cut: vec![sphere(), sphere()],
            smooth: 0.05,
        };
        let list = Shape3::Union {
            smooth: 0.4,
            shapes: vec![sphere(), sphere(), sphere(), sphere()],
        };
        assert!(compile(&hollow).unwrap().chain(), "вырезание — цепочка");
        assert!(
            compile(&list).unwrap().chain(),
            "объединение списком — цепочка"
        );
        assert!(
            !compile(&sphere()).unwrap().chain(),
            "одна форма считается напрямую"
        );
        // Вырезать объединение: пока считается вырезаемое, основа ждёт на стеке.
        let nested = Shape3::Subtract {
            base: Box::new(sphere()),
            cut: vec![list.clone()],
            smooth: 0.0,
        };
        assert!(!compile(&nested).unwrap().chain());
        assert!(!compile(&sample()).unwrap().chain());
    }

    fn sample() -> Shape3 {
        Shape3::Union {
            smooth: 0.0,
            shapes: vec![
                Shape3::Plane {
                    normal: [0.0, 1.0, 0.0],
                    offset: 0.0,
                },
                Shape3::Union {
                    smooth: 0.4,
                    shapes: vec![
                        Shape3::Sphere { radius: 0.6 }.at([-0.5, 1.0, 0.0]),
                        Shape3::Sphere { radius: 0.5 }.at([0.6, 1.1, 0.0]),
                    ],
                }
                .paint(1),
                Shape3::Subtract {
                    base: Box::new(Shape3::Box {
                        size: [1.2, 1.2, 1.2],
                        round: 0.05,
                    }),
                    cut: vec![Shape3::Sphere { radius: 0.75 }],
                    smooth: 0.0,
                }
                .placed([2.0, 0.6, -1.0], [0.0, 30.0, 0.0])
                .paint(2),
                // Выдавленная звезда и тело вращения: у них после примитива идут данные
                // поля, следующие команды должны читаться с правильного места.
                Shape3::Extrude {
                    profile: Profile::new(
                        "M 0 1 L 0.22 0.3 L 0.95 0.31 L 0.36 -0.12 L 0.59 -0.81 \
                         L 0 -0.38 L -0.59 -0.81 L -0.36 -0.12 L -0.95 0.31 L -0.22 0.3 Z",
                    )
                    .unwrap(),
                    depth: 0.3,
                    round: 0.05,
                }
                .placed([-2.5, 1.0, -2.0], [0.0, 40.0, 0.0])
                .paint(4),
                Shape3::Revolve {
                    profile: Profile::new("M 0 0 L 0.4 0 Q 0.5 0.4 0.1 0.8 Z").unwrap(),
                    offset: 0.2,
                }
                .at([2.5, 0.0, 2.5]),
                Shape3::Move {
                    at: [-2.0, 1.0, 1.0],
                    turn: [90.0, 0.0, 20.0],
                    scale: 1.5,
                    shape: Box::new(Shape3::Torus {
                        radius: 0.5,
                        thickness: 0.15,
                    }),
                }
                .paint(3),
                Shape3::Intersect {
                    smooth: 0.1,
                    shapes: vec![
                        Shape3::Cylinder {
                            height: 1.0,
                            radius: 0.4,
                            round: 0.05,
                        },
                        Shape3::Capsule {
                            height: 0.6,
                            radius: 0.45,
                        },
                    ],
                }
                .at([0.0, 0.5, 2.5]),
            ],
        }
    }

    #[test]
    fn primitives_measure_the_right_distance() {
        assert!((sd_sphere(Vec3::new(3.0, 0.0, 0.0), 1.0) - 2.0).abs() < 1e-6);
        assert!((sd_box(Vec3::new(2.0, 0.0, 0.0), Vec3::ONE, 0.0) - 1.0).abs() < 1e-6);
        assert!(sd_box(Vec3::ZERO, Vec3::ONE, 0.0) < 0.0);
        // Скругление не меняет середину грани, а угол отодвигает.
        assert!((sd_box(Vec3::new(2.0, 0.0, 0.0), Vec3::ONE, 0.3) - 1.0).abs() < 1e-6);
        assert!(sd_box(Vec3::splat(1.0), Vec3::ONE, 0.3) > 0.0);
        assert!((sd_capsule(Vec3::new(0.0, 3.0, 0.0), 1.0, 0.5) - 1.5).abs() < 1e-6);
        assert!((sd_cylinder(Vec3::new(2.0, 0.0, 0.0), 1.0, 1.0, 0.0) - 1.0).abs() < 1e-6);
        assert!((sd_cylinder(Vec3::new(0.0, 3.0, 0.0), 1.0, 1.0, 0.0) - 2.0).abs() < 1e-6);
        assert!(sd_torus(Vec3::new(1.0, 0.0, 0.0), 1.0, 0.2) < 0.0);
        assert!((sd_torus(Vec3::ZERO, 1.0, 0.2) - 0.8).abs() < 1e-6);
        assert!((sd_plane(Vec3::new(0.0, 2.0, 0.0), Vec3::Y, 0.5) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn smooth_operations_join_softly_and_never_overshoot() {
        // Плавное объединение не дальше резкого (поле «снизу»), а вдали от шва совпадает.
        for (a, b) in [(0.3, 0.35), (1.0, -0.2), (2.0, 5.0)] {
            assert!(op_union(a, b, 0.5) <= a.min(b) + 1e-6);
            assert!(op_intersect(a, b, 0.5) >= a.max(b) - 1e-6 - 0.5);
        }
        assert!((op_union(1.0, 5.0, 0.5) - 1.0).abs() < 1e-6);
        assert_eq!(op_subtract(1.0, -0.5, 0.0), 1.0);
        assert_eq!(op_subtract(-1.0, -0.5, 0.0), 0.5);
    }

    #[test]
    fn program_measures_the_same_as_the_tree() {
        let shape = sample();
        let program = compile(&shape).unwrap();
        for p in points() {
            let tree = shape.distance(p);
            let (code, _) = program.distance(p);
            assert!(
                (tree - code).abs() < 1e-4,
                "{p}: дерево {tree}, программа {code}"
            );
        }
    }

    #[test]
    fn program_reports_the_material_of_the_nearest_part() {
        let program = compile(&sample()).unwrap();
        // Над полом между шарами — шары (1); у коробки — 2; у тора — 3; у пола — 0.
        assert_eq!(program.distance(Vec3::new(0.0, 1.2, 0.0)).1, 1);
        // Угол коробки (середины граней вырезаны шаром).
        let corner = rotation([0.0, 30.0, 0.0]) * Vec3::splat(0.62) + Vec3::new(2.0, 0.6, -1.0);
        assert_eq!(program.distance(corner).1, 2);
        // Точка на кольце тора.
        let ring =
            rotation([90.0, 0.0, 20.0]) * Vec3::new(0.0, 0.0, -0.75) + Vec3::new(-2.0, 1.0, 1.0);
        assert_eq!(program.distance(ring).1, 3);
        assert_eq!(program.distance(Vec3::new(5.0, 0.01, 5.0)).1, 0);
    }

    #[test]
    fn scaled_and_turned_shapes_keep_true_distances() {
        // Шар радиусом 1, увеличенный вдвое и сдвинутый: расстояние в метрах мира.
        let s = Shape3::Move {
            at: [1.0, 2.0, 3.0],
            turn: [10.0, 20.0, 30.0],
            scale: 2.0,
            shape: Box::new(Shape3::Sphere { radius: 1.0 }),
        };
        let p = compile(&s).unwrap();
        let d = p.distance(Vec3::new(1.0, 2.0, 8.0)).0;
        assert!((d - 3.0).abs() < 1e-4, "{d}");
        // Поворот: коробка 2×1×1, повёрнутая на 90° вокруг Z, стоит вертикально.
        let tall = Shape3::Box {
            size: [2.0, 1.0, 1.0],
            round: 0.0,
        }
        .placed([0.0; 3], [0.0, 0.0, 90.0]);
        assert!(tall.distance(Vec3::new(0.0, 0.9, 0.0)) < 0.0);
        assert!(tall.distance(Vec3::new(0.9, 0.0, 0.0)) > 0.0);
    }

    #[test]
    fn deep_nesting_is_refused_and_empty_unions_too() {
        let mut s = Shape3::Sphere { radius: 1.0 };
        for _ in 0..STACK + 1 {
            s = Shape3::Union {
                smooth: 0.0,
                shapes: vec![Shape3::Sphere { radius: 1.0 }, s],
            };
        }
        assert_eq!(compile(&s), Err(CompileError::TooDeep));
        let empty = Shape3::Union {
            shapes: vec![],
            smooth: 0.0,
        };
        assert_eq!(compile(&empty), Err(CompileError::Empty));
    }

    #[test]
    fn shapes_read_from_ron_like_the_scene_file_will() {
        let text = "Union(smooth: 0.3, shapes: [\
            Paint(material: 1, shape: Move(at: (0, 1, 0), shape: Sphere(radius: 1))),\
            Move(at: (2, 0.5, 0), turn: (0, 45, 0), shape: Box(size: (1, 1, 1))),\
        ])";
        let s: Shape3 = ron::from_str(text).unwrap();
        assert!(compile(&s).is_ok());
        assert!(s.distance(Vec3::new(0.0, 1.0, 0.0)) < 0.0);
    }

    #[test]
    fn scene_pack_lays_out_objects_materials_and_code() {
        let a = compile(&Shape3::Sphere { radius: 1.0 }).unwrap();
        let b = compile(&sample()).unwrap();
        let materials = [Material3 {
            color: [0.5, 0.5, 0.5],
            glow: 0.0,
            grid: true,
            texture: 0,
        }];
        let lamps = [Lamp3 {
            at: [1.0, 2.0, 3.0],
            color: [1.0, 0.5, 0.25],
            power: 4.0,
            range: 6.0,
        }];
        let floor = compile(
            &Shape3::Plane {
                normal: [0.0, 1.0, 0.0],
                offset: 0.5,
            }
            .placed([0.0, 1.0, 0.0], [0.0, 0.0, 30.0]),
        )
        .unwrap();
        let w = pack_scene(&[a.clone(), b.clone(), floor.clone()], &materials, &lamps);
        let o = |i: usize| HEADER_WORDS + 8 * i;
        assert_eq!(&w[..6], &[3, 14, 38, 1, 46, 1]);
        assert_eq!(&w[12..14], &[0, 0], "поля уровня нет");
        let (start_b, len_b) = (w[o(1)] as usize, w[o(1) + 1] as usize);
        assert_eq!(len_b, b.words.len());
        assert_eq!(&w[start_b..start_b + len_b], &b.words[..]);
        assert_eq!(f32::from_bits(w[o(0) + 7]), 1.0, "граница шара");
        assert_eq!(
            f32::from_bits(w[o(1) + 7]),
            -1.0,
            "у формы с полом границы нет"
        );
        assert_eq!(w[w[2] as usize + 3], MATERIAL_GRID);
        assert_eq!(f32::from_bits(w[w[4] as usize + 3]), 6.0, "дальность лампы");
        assert_eq!(f32::from_bits(w[w[4] as usize + 7]), 4.0, "яркость лампы");
        assert_eq!(w[w[o(0)] as usize..][..a.words.len()], a.words[..]);
        // Плоскость (7.6и): только третий объект — у второго пол внутри объединения.
        assert_eq!(w[o(1) + 2] & OBJECT_PLANE, 0);
        assert_eq!(w[o(2) + 2] & OBJECT_PLANE, OBJECT_PLANE);
        let planes_at = w[8] as usize;
        assert_eq!(planes_at, w[o(2)] as usize + floor.words.len());
        assert_eq!(&w[planes_at..planes_at + w[9] as usize], &[2]);
        assert_eq!(w[6] as usize, planes_at + 1, "сетка сразу за плоскостями");
        // Плоскость в мире совпадает с расстоянием по программе.
        let f = |i: usize| f32::from_bits(w[o(2) + i]);
        let (n, c) = (Vec3::new(f(4), f(5), f(6)), f(3));
        for p in points() {
            let want = floor.distance(p).0;
            assert!(
                (n.dot(p) + c - want).abs() < 1e-4,
                "{p}: {} вместо {want}",
                n.dot(p) + c
            );
        }
    }

    /// Граница честная: снаружи шара расстояние до формы не меньше расстояния до шара.
    #[test]
    fn bounds_hold_the_whole_shape() {
        let Shape3::Union { shapes, .. } = sample() else {
            unreachable!()
        };
        let mut tested = 0;
        for shape in shapes.iter().skip(1) {
            let (c, r) = shape.bounds().expect("конечная форма");
            for p in points() {
                let p = p * 2.5;
                let outside = (p - c).length() - r;
                if outside > 0.0 {
                    assert!(
                        shape.distance(p) >= outside - 1e-4,
                        "{shape:?} в {p}: до формы {}, до границы {outside}",
                        shape.distance(p)
                    );
                    tested += 1;
                }
            }
            // И граница не слишком велика: точка на шаре недалеко от формы.
            assert!(r < 3.0, "{shape:?}: радиус {r}");
        }
        assert!(tested > 1000);
        assert_eq!(sample().bounds(), None, "с полом граница бесконечна");
        let two = Shape3::Union {
            smooth: 0.0,
            shapes: vec![
                Shape3::Sphere { radius: 1.0 }.at([-2.0, 0.0, 0.0]),
                Shape3::Sphere { radius: 1.0 }.at([2.0, 0.0, 0.0]),
            ],
        };
        let (c, r) = two.bounds().unwrap();
        assert!(c.length() < 1e-6 && (r - 3.0).abs() < 1e-6);
    }

    /// Коробка тоже честная (8.8а): снаружи неё расстояние до формы не меньше расстояния
    /// до коробки; у длинной стены коробка много плотнее шара.
    #[test]
    fn bounds_box_holds_the_whole_shape() {
        let Shape3::Union { shapes, .. } = sample() else {
            unreachable!()
        };
        let turned = |s: Shape3| s.placed([0.3, -0.2, 0.5], [20.0, 35.0, -15.0]);
        let mut tested = 0;
        for shape in shapes
            .iter()
            .skip(1)
            .cloned()
            .chain(shapes.iter().skip(1).cloned().map(turned))
        {
            let program = compile(&shape).unwrap();
            let (lo, hi) = program.bounds_box.expect("конечная форма");
            for p in points() {
                let p = p * 2.5;
                let outside = (p - p.clamp(lo, hi)).length();
                if outside > 0.0 {
                    assert!(
                        shape.distance(p) >= outside - 1e-4,
                        "{shape:?} в {p}: до формы {}, до коробки {outside}",
                        shape.distance(p)
                    );
                    tested += 1;
                }
            }
        }
        assert!(tested > 1000);
        assert_eq!(sample().bounds_box(), None, "с полом граница бесконечна");
        let wall = compile(&Shape3::Box {
            size: [60.0, 1.5, 0.3],
            round: 0.0,
        })
        .unwrap();
        let (lo, hi) = wall.bounds_box.unwrap();
        assert!((hi - lo - Vec3::new(60.0, 1.5, 0.3)).abs().max_element() < 1e-4);
    }

    const STAR: &str = "M 0 1 L 0.22 0.3 L 0.95 0.31 L 0.36 -0.12 L 0.59 -0.81 \
                        L 0 -0.38 L -0.59 -0.81 L -0.36 -0.12 L -0.95 0.31 L -0.22 0.3 Z";

    /// Выдавленная звезда и она же через поле слоя (7.18б), одинаково поставленные.
    fn extrude_and_field() -> (Shape3, Shape3) {
        let profile = Profile::new(STAR).unwrap();
        let field = FieldRef(Arc::new(LayerField::from_profile(&profile, 1.0)));
        let place = |s: Shape3| s.placed([0.5, 1.0, -0.3], [10.0, 25.0, 0.0]).paint(3);
        (
            place(Shape3::Extrude {
                profile,
                depth: 0.3,
                round: 0.02,
            }),
            place(Shape3::Field {
                field,
                depth: 0.3,
                round: 0.02,
            }),
        )
    }

    /// Поле слоя считается как выдавливание (с точностью half), шар-граница у него та же,
    /// в сцену ложится обычным объектом без флагов плоскости и цепочки.
    #[test]
    fn field_primitive_mirrors_extrude() {
        let (extrude, field) = extrude_and_field();
        let a = compile(&extrude).unwrap();
        let b = compile(&field).unwrap();
        assert_eq!(b.fields.len(), 1);
        assert_eq!(b.words.len(), PRIM_WORDS + FIELD_PRIM_WORDS);
        assert_eq!(
            b.words[0],
            op::PRIM | (prim::FIELD << 8) | ((FIELD_PRIM_WORDS as u32) << 16)
        );
        // Материал, матрица, масштаб, половина толщины и скругление — как у выдавливания.
        assert_eq!(&a.words[1..17], &b.words[1..17]);
        assert_eq!(f32::from_bits(b.words[18]), 1.0, "полоса B");
        // Шар-граница по прямоугольнику фигуры — тот же, что у выдавливания: отсев по
        // взгляду, сетка отсева и пересчёт поля 7.10 видят их одинаково.
        assert!(field.bounds().is_some());
        assert_eq!(field.bounds(), extrude.bounds());
        assert_eq!(b.bounds, a.bounds);
        assert!(!b.chain() && b.plane().is_none());
        for p in points() {
            let (da, _) = a.distance(p);
            let (db, m) = b.distance(p);
            let dt = field.distance(p);
            assert!(
                (da - db).abs() <= da.abs() * 2f32.powi(-10) + 1e-4,
                "{p}: выдавливание {da}, поле {db}"
            );
            assert!((dt - db).abs() < 1e-5, "{p}: дерево {dt}, программа {db}");
            assert_eq!(m, 3, "{p}: на собственной сетке поле всегда знает материал");
        }
        let w = pack_scene(std::slice::from_ref(&b), &[], &[]);
        assert_eq!(
            f32::from_bits(w[HEADER_WORDS + 7]),
            a.bounds.unwrap().1,
            "радиус границы в сцене"
        );
        // Из файла сцены поле не читается, его создаёт только код.
        assert!(ron::from_str::<Shape3>("Field(depth: 1.0)").is_err());
        // Программы равны, пока ссылаются на одно поле; другое поле с теми же словами —
        // другая программа (поле 7.10 тогда пересчитается, 7.18б).
        assert_eq!(b.clone(), b);
        let other = compile(&extrude_and_field().1).unwrap();
        assert_eq!(other.words, b.words);
        assert_ne!(other, b);
    }

    /// Поле без прямоугольника фигуры (так будет у поля уровня, 7.18в) бесконечно: в сцене
    /// радиус границы −1, не плоскость и не цепочка.
    #[test]
    fn field_without_a_rectangle_is_unbounded() {
        let profile = Profile::new(STAR).unwrap();
        let field = FieldRef(Arc::new(
            LayerField::from_profile(&profile, 1.0).without_rect(),
        ));
        let field = Shape3::Field {
            field,
            depth: 0.3,
            round: 0.02,
        }
        .placed([0.5, 1.0, -0.3], [10.0, 25.0, 0.0])
        .paint(3);
        let b = compile(&field).unwrap();
        assert_eq!(field.bounds(), None);
        assert_eq!(b.bounds, None);
        let union = Shape3::Union {
            smooth: 0.0,
            shapes: vec![field.clone(), Shape3::Sphere { radius: 1.0 }],
        };
        assert_eq!(union.bounds(), None);
        let w = pack_scene(std::slice::from_ref(&b), &[], &[]);
        assert_eq!(f32::from_bits(w[HEADER_WORDS + 7]), -1.0);
        assert_eq!(w[HEADER_WORDS + 2] & (OBJECT_PLANE | OBJECT_CHAIN), 0);
        assert_eq!(w[9], 0, "плоскостей нет");
    }

    /// Поле уровня (7.18д3): бесконечное поле слоя со сдвигом — отдельный объект, слова 12 и
    /// 13 заголовка указывают на него и начало его программы; с поворотом, масштабом,
    /// прямоугольником, у заменителя или светящегося — обычный объект.
    #[test]
    fn level_field_is_found_and_packed() {
        let profile = Profile::new(STAR).unwrap();
        let field = |rect: bool| Shape3::Field {
            field: FieldRef(Arc::new(if rect {
                LayerField::from_profile(&profile, 1.0)
            } else {
                LayerField::from_profile(&profile, 1.0).without_rect()
            })),
            depth: 0.3,
            round: 0.02,
        };
        let level = compile(&field(false).at([0.5, 1.0, -0.3]).paint(3)).unwrap();
        assert!(level.level_field());
        let mut refused = vec![
            compile(&field(false).placed([0.5, 1.0, -0.3], [0.0, 0.0, 10.0])).unwrap(),
            compile(&Shape3::Move {
                at: [0.0; 3],
                turn: [0.0; 3],
                scale: 2.0,
                shape: Box::new(field(false)),
            })
            .unwrap(),
            compile(&field(true)).unwrap(),
        ];
        let mut shadow_only = level.clone();
        shadow_only.shadow_only = true;
        let mut no_shadow = level.clone();
        no_shadow.no_shadow = true;
        refused.extend([shadow_only, no_shadow]);
        for (k, p) in refused.iter().enumerate() {
            assert!(!p.level_field(), "{k}");
        }
        let sphere = compile(&Shape3::Sphere { radius: 1.0 }.at([3.0, 0.0, 0.0])).unwrap();
        let objects = [sphere.clone(), level.clone()];
        assert_eq!(level_field_object(&objects), Some(1));
        let w = pack_scene(&objects, &[], &[]);
        assert_eq!(w[12], 2, "номер объекта + 1");
        let start = w[13] as usize;
        assert_eq!(start, w[HEADER_WORDS + OBJECT_WORDS] as usize);
        assert_eq!(&w[start..start + level.words.len()], &level.words[..]);
        let w = pack_scene_with(&objects, &[], &[], false, GridOptions::default());
        assert_eq!(&w[12..14], &[0, 0], "выключено — как до 7.18д3");
        let w = pack_scene(&[sphere], &[], &[]);
        assert_eq!(&w[12..14], &[0, 0], "поля нет");
    }

    /// Шар-граница поля фигуры держит тело: всё, что внутри, — в шаре, а снаружи шара
    /// расстояние не меньше расстояния до шара (или полосы B, дальше которой поле не
    /// знает) с точностью клетки.
    #[test]
    fn field_bounds_hold_the_whole_shape() {
        let (_, field) = extrude_and_field();
        let Shape3::Paint { shape: moved, .. } = &field else {
            unreachable!()
        };
        let Shape3::Move { shape: inner, .. } = moved.as_ref() else {
            unreachable!()
        };
        let Shape3::Field { field: f, .. } = inner.as_ref() else {
            unreachable!()
        };
        let cell = f.0.cell();
        let (c, r) = field.bounds().unwrap();
        let (mut inside, mut outside_tested) = (0, 0);
        let n = 24;
        for i in 0..=n {
            for j in 0..=n {
                for k in 0..=n {
                    let t = Vec3::new(i as f32, j as f32, k as f32) / n as f32 * 2.0 - 1.0;
                    let p = c + t * r * 1.6;
                    let d = field.distance(p);
                    let outside = (p - c).length() - r;
                    if d <= 0.0 {
                        assert!(outside <= 1e-4, "{p}: тело за шаром на {outside}");
                        inside += 1;
                    }
                    if outside > 0.0 {
                        assert!(
                            d >= outside.min(1.0) - cell,
                            "{p}: до формы {d}, до границы {outside}"
                        );
                        outside_tested += 1;
                    }
                }
            }
        }
        assert!(
            inside > 20 && outside_tested > 1000,
            "{inside} внутри, {outside_tested} снаружи"
        );
    }

    /// За полосой поле слоя — только оценка: материал-сторож, расстояние обрезано.
    #[test]
    fn field_estimate_beyond_the_band_is_marked() {
        let profile = Profile::new("M -1 -1 L 1 -1 L 1 1 L -1 1 Z").unwrap();
        let field = FieldRef(Arc::new(LayerField::from_profile(&profile, 0.05)));
        let shape = Shape3::Field {
            field,
            depth: 0.4,
            round: 0.0,
        }
        .paint(2);
        let program = compile(&shape).unwrap();
        let (d, m) = program.distance(Vec3::new(1.5, 0.0, 0.0));
        assert_eq!(m, ESTIMATE_MATERIAL);
        assert!((d - 0.5).abs() < 1e-3, "{d}: по прямоугольнику фигуры");
        let (d, m) = program.distance(Vec3::ZERO);
        assert_eq!(m, 2);
        assert!((d + 0.05).abs() < 1e-3, "{d}");
    }

    #[test]
    fn lamp_light_fades_to_zero_at_its_range() {
        let lamp = Lamp3 {
            at: [0.0; 3],
            color: [1.0; 3],
            power: 10.0,
            range: 5.0,
        };
        assert!(lamp_falloff(&lamp, 1.0) > lamp_falloff(&lamp, 2.0));
        assert!(lamp_falloff(&lamp, 4.9) > 0.0);
        assert_eq!(lamp_falloff(&lamp, 5.0), 0.0);
        assert_eq!(lamp_falloff(&lamp, 9.0), 0.0);
        // Вблизи не бесконечность.
        assert!(lamp_falloff(&lamp, 0.0) <= 10.0);
    }
}
