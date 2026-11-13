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
}

fn one() -> f32 {
    1.0
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
/// Глубина стека машины в шейдере: столько форм может ждать операции одновременно.
pub const STACK: usize = 8;

/// Программа одной формы: слова для видеокарты.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    pub words: Vec<u32>,
    /// Шар, в котором форма целиком (середина, радиус); `None` — форма бесконечна.
    pub bounds: Option<(Vec3, f32)>,
    /// Невидимый заменитель полигональной модели (7.6ж): только тени и затенение в
    /// углах, лучи глаза его не видят.
    pub shadow_only: bool,
    /// Светящийся предмет (огонёк, шар-светильник; 7.9б): виден, но теней и затенения
    /// в углах не даёт. Иначе он у самой лампы и бросает от неё тень на всё вокруг.
    pub no_shadow: bool,
    /// Поля слоя (7.18б) в порядке примитивов `PRIM_FIELD` в словах: по ним считает
    /// зеркало на процессоре, видеокарта читает картинки по номерам из слов.
    pub fields: Vec<FieldRef>,
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
    let mut out = Emit::default();
    emit(shape, Placement::IDENTITY, 0, 0, &mut out)?;
    if out.words.len() > MAX_PROGRAM_WORDS {
        return Err(CompileError::TooLong);
    }
    Ok(Program {
        words: out.words,
        bounds: shape.bounds(),
        shadow_only: false,
        no_shadow: false,
        fields: out.fields,
    })
}

/// Куда пишется программа: слова и поля слоя по порядку их примитивов.
#[derive(Default)]
struct Emit {
    words: Vec<u32>,
    fields: Vec<FieldRef>,
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
            let mut need = emit(b, at, material, base, out)?;
            for c in cut {
                need = need.max(1 + emit(c, at, material, base + 1, out)?);
                out.words.extend([op::SUBTRACT, smooth.to_bits()]);
            }
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
    let mut need = emit(first, at, material, base, out)?;
    for s in rest {
        need = need.max(1 + emit(s, at, material, base + 1, out)?);
        out.words.extend([code, smooth.to_bits()]);
    }
    Ok(need)
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
            if w[i] & 0xff == op::PRIM {
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
        let w = &self.words;
        let f = |i: usize| f32::from_bits(w[i]);
        let mut stack: Vec<(f32, u32)> = Vec::with_capacity(STACK);
        let mut i = 0;
        let mut fields = self.fields.iter();
        while i < w.len() {
            let code = w[i] & 0xff;
            if code == op::PRIM {
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
        stack.pop().unwrap_or((f32::MAX, 0))
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
}

/// Флаг материала: сетка на поверхности.
pub const MATERIAL_GRID: u32 = 1;

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
/// обрезается до него на процессоре заранее, с предупреждением (7.18б).
pub const MAX_OBJECTS: usize = 1024;
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
    pack_scene_with(objects, materials, lamps, true, true)
}

/// То же; `level_field: false` — поле уровня считается в общем переборе, как до 7.18д3
/// (для сравнения в тестах); `free: false` — у клеток сетки нет свободного расстояния,
/// шаг в пустоте не длиннее клетки, как до 7.18д4 (для сравнения на панели).
pub fn pack_scene_with(
    objects: &[Program],
    materials: &[Material3],
    lamps: &[Lamp3],
    level_field: bool,
    free: bool,
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
        out.push(u32::from(m.grid) * MATERIAL_GRID);
        out.extend([m.glow.to_bits(), 0, 0, 0]);
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
    let grid = if free {
        crate::grid3d::Grid::build_with(objects, field)
    } else {
        crate::grid3d::Grid::build_lists(objects, field)
    };
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
        let w = pack_scene_with(&objects, &[], &[], false, true);
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
