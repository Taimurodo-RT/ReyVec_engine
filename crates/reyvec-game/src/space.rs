//! Мир столкновений для контроллера персонажа (9.2).
//!
//! Раньше герой и враги видели уровень как список прямоугольников со сторонами вдоль осей,
//! а ящики физики — как описанные вокруг них прямоугольники. Теперь всё твёрдое, о
//! что персонаж может удариться, — это [`Surface`]: настоящая форма (прямоугольник,
//! склон, круг, капсула, многоугольник, повёрнутые как угодно) из той же библиотеки
//! форм, что считает физику (`parry`, часть Rapier).
//!
//! Контроллер задаёт миру вопросы ([`Space`]):
//!
//! - [`Space::cast`]: на сколько можно сдвинуть прямоугольник персонажа по пути, пока он
//!   ни во что не упрётся, и во что упрётся (номер, нормаль поверхности);
//! - [`Space::overlaps`]: занято ли место;
//! - [`Space::ray_down`]: что под точкой (поверхность под серединой ног на склоне);
//! - [`Space::carry`]: на сколько поверхность сдвинула точку на себе за шаг (едущая или
//!   вращающаяся опора везёт стоящего);
//! - [`Space::contact`]: насколько въехавший предмет вошёл в персонажа и куда вытолкнуть.
//!
//! Мир собирается заново на каждом шаге логики из уровня и тел физики. Поиск — перебор
//! с отсевом по описанным прямоугольникам: на сценах до тысячи поверхностей это
//! микросекунды; дерево поиска — в 9.7 вместе со сценой-нагрузкой.

use rapier2d::parry::math::{Pose, Vector};
use rapier2d::parry::query::{self, Ray, ShapeCastOptions};
use rapier2d::parry::shape::{Cuboid, Shape, SharedShape};

use crate::{Aabb, Collider, EPS};

/// Зазор, на котором персонаж останавливается у поверхности. Формы считаются в `f32`, и
/// без зазора персонаж мог бы войти в стену на долю миллиметра и застрять.
pub const SKIN: f64 = 1e-4;

/// Слои столкновений (9.6а): у каждого тела и поверхности есть слой (один бит) и маска
/// «с какими слоями сталкиваюсь». Двое сталкиваются, только если каждый есть в маске
/// другого. Первые четыре слоя — встроенные, остальные сцена называет сама.
pub mod layer {
    /// Уровень: стены, пол, фигуры сцены.
    pub const LEVEL: u32 = 1;
    pub const HERO: u32 = 1 << 1;
    pub const ENEMY: u32 = 1 << 2;
    /// Тела физики и мягкие тела.
    pub const BODY: u32 = 1 << 3;
    /// Со всеми.
    pub const ALL: u32 = u32::MAX;
    /// Первый слой, который называет сцена.
    pub const FIRST_CUSTOM: u32 = 4;

    /// Сталкиваются ли двое (слой и маска каждого).
    pub fn meet(a: (u32, u32), b: (u32, u32)) -> bool {
        a.0 & b.1 != 0 && b.0 & a.1 != 0
    }
}

/// Поверхность мира столкновений: форма на месте и что о ней нужно контроллеру.
#[derive(Clone)]
pub struct Surface {
    /// Номер: по нему персонаж узнаёт «свою» опору на следующем шаге.
    pub id: u64,
    pub shape: SharedShape,
    pub pose: Pose,
    /// Описанный прямоугольник.
    pub aabb: Aabb,
    /// Держит только сверху (доска): снизу и сбоку сквозь неё проходишь.
    pub oneway: bool,
    /// Склон уровня: держит только середину ног (луч [`Space::ray_down`]), а не весь
    /// прямоугольник. Иначе персонаж цеплялся бы за углы на стыках склонов.
    pub slope: bool,
    /// Можно толкать (тело физики): упёршийся сбоку персонаж не останавливается, а
    /// толкает ([`crate::StepEvents::pushed`]).
    pub pushable: bool,
    /// На сколько сдвинулась за шаг и на сколько повернулась (радианы против часовой)
    /// вокруг `center`.
    pub moved: [f64; 2],
    pub turned: f64,
    pub center: [f64; 2],
    /// Слой и маска (9.6а, [`layer`]): уровень и со всеми по умолчанию.
    pub layer: u32,
    pub mask: u32,
}

impl Surface {
    /// Поверхность из прямоугольника или склона уровня.
    pub fn of_collider(c: &Collider) -> Self {
        let center = c.aabb.center();
        let hx = ((c.aabb.max[0] - c.aabb.min[0]) * 0.5) as f32;
        let hy = ((c.aabb.max[1] - c.aabb.min[1]) * 0.5) as f32;
        let shape = if c.slope == 0.0 {
            SharedShape::cuboid(hx.max(1e-6), hy.max(1e-6))
        } else {
            // Склон: верх по прямой от одного верхнего угла вниз на `slope` у другого
            // края, ниже — сплошной (столбик под склоном у уровня из тайлов).
            let drop = (c.slope.abs() as f32).min(2.0 * hy);
            let (left, right) = if c.slope > 0.0 {
                (hy - drop, hy)
            } else {
                (hy, hy - drop)
            };
            let pts = [
                Vector::new(-hx, -hy),
                Vector::new(hx, -hy),
                Vector::new(hx, right),
                Vector::new(-hx, left),
            ];
            SharedShape::convex_hull(&pts).unwrap_or_else(|| SharedShape::cuboid(hx, hy))
        };
        Self {
            id: c.id,
            shape,
            pose: Pose::from_translation(v(center)),
            aabb: c.aabb,
            oneway: c.oneway,
            slope: c.slope != 0.0,
            pushable: false,
            moved: c.moved,
            turned: 0.0,
            center,
            layer: layer::LEVEL,
            mask: layer::ALL,
        }
    }

    /// Поверхность любой формы на месте `pose`.
    pub fn of_shape(id: u64, shape: SharedShape, pose: Pose) -> Self {
        let a = shape.compute_aabb(&pose);
        Self {
            id,
            aabb: Aabb {
                min: p(a.mins),
                max: p(a.maxs),
            },
            center: p(pose.translation),
            shape,
            pose,
            oneway: false,
            slope: false,
            pushable: false,
            moved: [0.0, 0.0],
            turned: 0.0,
            layer: layer::LEVEL,
            mask: layer::ALL,
        }
    }

    /// Та же поверхность в слое `layer` с маской `mask`.
    pub fn in_layer(mut self, layer: u32, mask: u32) -> Self {
        self.layer = layer;
        self.mask = mask;
        self
    }

    /// Поверхность формы `shape` с серединой `at`, повёрнутая на `angle` (радианы).
    pub fn placed(id: u64, shape: SharedShape, at: [f64; 2], angle: f64) -> Self {
        Self::of_shape(id, shape, Pose::new(v(at), angle as f32))
    }

    /// Двигалась ли за шаг.
    pub fn moving(&self) -> bool {
        self.moved != [0.0, 0.0] || self.turned != 0.0
    }

    /// Та же поверхность, но стоящая (шаги персонажа после первого в одном шаге логики).
    pub fn still(&self) -> Self {
        Self {
            moved: [0.0, 0.0],
            turned: 0.0,
            ..self.clone()
        }
    }
}

/// Во что упёрся прямоугольник.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// Номер поверхности ([`Surface::id`]) и её место в списке.
    pub id: u64,
    pub index: usize,
    /// Нормаль поверхности в точке касания: смотрит от поверхности к персонажу.
    pub normal: [f64; 2],
    pub oneway: bool,
}

/// Какие поверхности учитывать в вопросе.
#[derive(Clone, Copy, Default)]
pub struct Filter<'a> {
    /// Доски держат (только при движении вниз и если персонаж был не ниже их верха).
    pub platforms: bool,
    /// Эти номера не в счёт (своя опора, сам персонаж).
    pub skip: &'a [u64],
    /// Склоны уровня ([`Surface::slope`]) в счёт. Без этого их видит только луч.
    pub slopes: bool,
    /// Учитывать касание только с такой нормалью поверхности (например, только ровный
    /// пол или только стены). `None` — любые.
    pub normals: Option<fn([f64; 2]) -> bool>,
}

impl<'a> Filter<'a> {
    pub fn platforms(mut self, on: bool) -> Self {
        self.platforms = on;
        self
    }

    pub fn slopes(mut self, on: bool) -> Self {
        self.slopes = on;
        self
    }

    pub fn skip(mut self, ids: &'a [u64]) -> Self {
        self.skip = ids;
        self
    }

    pub fn normals(mut self, f: fn([f64; 2]) -> bool) -> Self {
        self.normals = Some(f);
        self
    }
}

/// Нормаль «пол»: почти вверх.
pub fn floor(n: [f64; 2]) -> bool {
    n[1] > 0.98
}

/// Нормаль «можно стоять»: не круче 60°.
pub fn walkable(n: [f64; 2]) -> bool {
    n[1] > 0.5
}

/// Нормаль «стена»: почти вбок.
pub fn wall(n: [f64; 2]) -> bool {
    n[0].abs() > 0.7
}

/// Нормаль «не пологая»: всё, кроме склонов, по которым можно идти (их держит середина
/// ног, а не весь прямоугольник).
pub fn not_walkable_slope(n: [f64; 2]) -> bool {
    !(n[1] > 0.5 && n[1] <= 0.98)
}

/// Нормаль «не склон»: пол, потолок, стены, но не пологие склоны вниз и вверх.
pub fn not_slope(n: [f64; 2]) -> bool {
    n[1] > 0.98 || n[1] <= 0.5
}

/// Мир столкновений на один шаг.
#[derive(Clone, Default)]
pub struct Space {
    surfaces: Vec<Surface>,
}

impl Space {
    pub fn new() -> Self {
        Self::default()
    }

    /// Мир из прямоугольников и склонов уровня.
    pub fn of(colliders: &[Collider]) -> Self {
        let mut s = Self::new();
        s.extend_colliders(colliders);
        s
    }

    pub fn extend_colliders(&mut self, colliders: &[Collider]) {
        self.surfaces
            .extend(colliders.iter().map(Surface::of_collider));
    }

    pub fn push(&mut self, s: Surface) {
        self.surfaces.push(s);
    }

    pub fn extend(&mut self, s: impl IntoIterator<Item = Surface>) {
        self.surfaces.extend(s);
    }

    pub fn surfaces(&self) -> &[Surface] {
        &self.surfaces
    }

    pub fn len(&self) -> usize {
        self.surfaces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Surface> {
        self.surfaces.iter().find(|s| s.id == id)
    }

    /// Тот же мир, где ничего не движется.
    pub fn still(&self) -> Self {
        Self {
            surfaces: self.surfaces.iter().map(Surface::still).collect(),
        }
    }

    /// Только то, с чем сталкивается слой `layer` с маской `mask` (9.6а): мир
    /// столкновений персонажа.
    pub fn meeting(&self, layer: u32, mask: u32) -> Self {
        Self {
            surfaces: self
                .surfaces
                .iter()
                .filter(|s| layer::meet((s.layer, s.mask), (layer, mask)))
                .cloned()
                .collect(),
        }
    }

    /// Без поверхностей с этими номерами.
    pub fn without(&self, ids: &[u64]) -> Self {
        Self {
            surfaces: self
                .surfaces
                .iter()
                .filter(|s| !ids.contains(&s.id))
                .cloned()
                .collect(),
        }
    }

    /// Поверхность подходит под фильтр (без проверки нормали).
    fn takes(&self, s: &Surface, b: &Aabb, delta: [f64; 2], f: &Filter) -> bool {
        if f.skip.contains(&s.id) || (s.slope && !f.slopes) {
            return false;
        }
        if s.oneway {
            // Доска держит только того, кто падает на неё сверху.
            return f.platforms && delta[1] < 0.0 && b.min[1] >= s.aabb.max[1] - 1e-5;
        }
        true
    }

    /// На сколько можно сдвинуть прямоугольник `b` на `delta`: доля пути от 0 до 1 и во
    /// что упёрся. Поверхности, в которых прямоугольник уже стоит (поставили внутрь
    /// стены), не держат: из них можно выйти. Останавливается в [`SKIN`] от поверхности.
    pub fn cast(&self, b: Aabb, delta: [f64; 2], f: &Filter) -> (f64, Option<Hit>) {
        let len = delta[0].hypot(delta[1]);
        if len < EPS {
            return (1.0, None);
        }
        let center = b.center();
        let half = half_of(&b);
        let me = Cuboid::new(Vector::new(half[0] as f32, half[1] as f32));
        let inner = Cuboid::new(Vector::new(
            (half[0] - SKIN * 0.5).max(1e-6) as f32,
            (half[1] - SKIN * 0.5).max(1e-6) as f32,
        ));
        let pose = Pose::from_translation(v(center));
        let swept = Aabb::of_points([
            b.min,
            b.max,
            [b.min[0] + delta[0], b.min[1] + delta[1]],
            [b.max[0] + delta[0], b.max[1] + delta[1]],
        ])
        .unwrap()
        .grow(SKIN * 2.0);
        // Доля пути считается в длинах `delta`: `vel` = `delta`, время 0..1. Касание ищется
        // точно (нулевое расстояние), а потом персонаж отступает на `SKIN` назад по пути:
        // поиск «до расстояния SKIN» ненадёжен, когда персонаж уже стоит ближе.
        let back = SKIN / len;
        let opts = ShapeCastOptions {
            max_time_of_impact: (1.0 + back * 8.0) as f32,
            target_distance: 0.0,
            stop_at_penetration: false,
            compute_impact_geometry_on_penetration: true,
        };
        let mut best: (f64, Option<Hit>) = (1.0, None);
        for (index, s) in self.surfaces.iter().enumerate() {
            if !aabb_touch(&swept, &s.aabb) || !self.takes(s, &b, delta, f) {
                continue;
            }
            // Уже внутри — не держит.
            if inside(&pose, &inner, &s.pose, s.shape.as_ref()) {
                continue;
            }
            let Ok(Some(hit)) = query::cast_shapes(
                &pose,
                v(delta),
                &me,
                &s.pose,
                Vector::ZERO,
                s.shape.as_ref(),
                opts,
            ) else {
                continue;
            };
            let n = p(-hit.normal1);
            let n = normalize(n);
            // Касание вскользь (движение вдоль поверхности) не держит.
            let into = -(n[0] * delta[0] + n[1] * delta[1]);
            if into <= EPS * len {
                continue;
            }
            let at = f64::from(hit.time_of_impact);
            // Почти вдоль поверхности (нормаль в f32 у касания плавает на доли градуса):
            // держит, только если без этого вошёл бы заметно глубже допуска.
            if into < 0.1 * len && into * (1.0 - at) < SKIN * 0.25 {
                continue;
            }
            // Отступ `SKIN` — по нормали, а не по пути: у косой грани отступ по пути
            // оставил бы зазор меньше `SKIN`, и скольжение вдоль неё уводило бы внутрь.
            let back_n = (SKIN / into).min(back * 8.0);
            let toi = (at - back_n).max(0.0);
            if toi >= best.0 {
                continue;
            }
            if let Some(pass) = f.normals
                && !pass(n)
            {
                continue;
            }
            best = (
                toi,
                Some(Hit {
                    id: s.id,
                    index,
                    normal: n,
                    oneway: s.oneway,
                }),
            );
        }
        best
    }

    /// Сдвиг вдоль одной оси: сколько удалось пройти и во что упёрся.
    pub fn sweep(&self, b: Aabb, axis: usize, d: f64, f: &Filter) -> (f64, Option<Hit>) {
        let mut delta = [0.0, 0.0];
        delta[axis] = d;
        let (t, hit) = self.cast(b, delta, f);
        (d * t, hit)
    }

    /// Занято ли место `b` чем-то твёрдым (доски и касание вплотную не в счёт).
    pub fn overlaps(&self, b: Aabb, f: &Filter) -> bool {
        let half = half_of(&b);
        if half[0] <= 0.0 || half[1] <= 0.0 {
            return false;
        }
        let me = Cuboid::new(Vector::new(
            (half[0] - EPS).max(1e-7) as f32,
            (half[1] - EPS).max(1e-7) as f32,
        ));
        let pose = Pose::from_translation(v(b.center()));
        self.surfaces.iter().any(|s| {
            !s.oneway
                && !f.skip.contains(&s.id)
                && (!s.slope || f.slopes)
                && b.overlaps(&s.aabb)
                && inside(&pose, &me, &s.pose, s.shape.as_ref())
        })
    }

    /// Первая поверхность под точкой `at` не дальше `max` вниз: высота и что за
    /// поверхность. Доски — по [`Filter::platforms`].
    pub fn ray_down(&self, at: [f64; 2], max: f64, f: &Filter) -> Option<(f64, Hit)> {
        let ray = Ray::new(v(at), Vector::new(0.0, -1.0));
        let column = Aabb {
            min: [at[0] - EPS, at[1] - max],
            max: [at[0] + EPS, at[1]],
        };
        let mut best: Option<(f64, Hit)> = None;
        for (index, s) in self.surfaces.iter().enumerate() {
            if !aabb_touch(&column, &s.aabb) || f.skip.contains(&s.id) {
                continue;
            }
            if s.oneway && !(f.platforms && at[1] >= s.aabb.max[1] - 1e-5) {
                continue;
            }
            let Some(hit) = s
                .shape
                .cast_ray_and_get_normal(&s.pose, &ray, max as f32, true)
            else {
                continue;
            };
            let toi = f64::from(hit.time_of_impact);
            if toi <= 0.0 {
                // Точка внутри формы: не «сверху».
                continue;
            }
            let n = normalize(p(hit.normal));
            if let Some(pass) = f.normals
                && !pass(n)
            {
                continue;
            }
            let h = at[1] - toi;
            if best.is_none_or(|(b, _)| h > b) {
                best = Some((
                    h,
                    Hit {
                        id: s.id,
                        index,
                        normal: n,
                        oneway: s.oneway,
                    },
                ));
            }
        }
        best
    }

    /// На сколько поверхность `id` за шаг сдвинула свою точку `point` (где она была до
    /// шага).
    pub fn carry(&self, id: u64, point: [f64; 2]) -> [f64; 2] {
        let Some(s) = self.get(id) else {
            return [0.0, 0.0];
        };
        let mut d = s.moved;
        // Круг, вращаясь, своей поверхности не сдвигает: катящийся шар не швыряет
        // стоящего на нём со скоростью вращения.
        if s.turned != 0.0 && s.shape.as_ball().is_none() {
            // Точка вращается вокруг середины поверхности (где та была до шага).
            let c = [s.center[0] - s.moved[0], s.center[1] - s.moved[1]];
            let r = [point[0] - c[0], point[1] - c[1]];
            let (sn, cs) = s.turned.sin_cos();
            d[0] += r[0] * cs - r[1] * sn - r[0];
            d[1] += r[0] * sn + r[1] * cs - r[1];
        }
        d
    }

    /// Насколько поверхность `index` вошла в прямоугольник `b`: куда выталкивать
    /// (единичный вектор от поверхности к персонажу) и на сколько. `None` — не касаются.
    pub fn contact(&self, b: Aabb, index: usize) -> Option<([f64; 2], f64)> {
        let s = self.surfaces.get(index)?;
        if !b.overlaps(&s.aabb) {
            return None;
        }
        let half = half_of(&b);
        let me = Cuboid::new(Vector::new(half[0] as f32, half[1] as f32));
        let pose = Pose::from_translation(v(b.center()));
        let c = query::contact(&pose, &me, &s.pose, s.shape.as_ref(), 0.0).ok()??;
        let depth = -f64::from(c.dist);
        if depth <= EPS {
            return None;
        }
        Some((normalize(p(-c.normal1)), depth))
    }

    /// Сдвиг, который держит `b` на зазоре `SKIN` от поверхности `index` (нулевой, если
    /// зазор уже есть). Нужен после скольжения вдоль косой грани: нормаль касания
    /// посчитана в f32, и шаг вдоль неё понемногу уводит внутрь.
    pub fn keep_apart(&self, b: Aabb, index: usize) -> [f64; 2] {
        let Some(s) = self.surfaces.get(index) else {
            return [0.0, 0.0];
        };
        let half = half_of(&b);
        let me = Cuboid::new(Vector::new(half[0] as f32, half[1] as f32));
        let pose = Pose::from_translation(v(b.center()));
        let Ok(Some(c)) =
            query::contact(&pose, &me, &s.pose, s.shape.as_ref(), (SKIN * 2.0) as f32)
        else {
            return [0.0, 0.0];
        };
        let gap = f64::from(c.dist);
        if gap >= SKIN {
            return [0.0, 0.0];
        }
        let n = normalize(p(-c.normal1));
        [n[0] * (SKIN - gap), n[1] * (SKIN - gap)]
    }

    /// Пересекался ли `b` с поверхностью `index` до того, как она сдвинулась за шаг.
    pub fn overlapped_before(&self, b: Aabb, index: usize) -> bool {
        let Some(s) = self.surfaces.get(index) else {
            return false;
        };
        let before = Pose::new(
            s.pose.translation - v(s.moved),
            s.pose.rotation.angle() - s.turned as f32,
        );
        let half = half_of(&b);
        let me = Cuboid::new(Vector::new(
            (half[0] - EPS).max(1e-7) as f32,
            (half[1] - EPS).max(1e-7) as f32,
        ));
        let pose = Pose::from_translation(v(b.center()));
        inside(&pose, &me, &before, s.shape.as_ref())
    }
}

/// Общая форма прямоугольника (для физики и рисования форм столкновений).
pub fn cuboid(half: [f64; 2]) -> SharedShape {
    SharedShape::cuboid(half[0] as f32, half[1] as f32)
}

/// Пересекаются ли формы. `query::intersection_test` в f32 отвечает «да» уже на
/// расстоянии около 2·10⁻⁴ (больше `SKIN`), поэтому — по точному расстоянию.
fn inside(p1: &Pose, s1: &dyn Shape, p2: &Pose, s2: &dyn Shape) -> bool {
    query::distance(p1, s1, p2, s2).is_ok_and(|d| d.distance <= 0.0)
}

fn half_of(b: &Aabb) -> [f64; 2] {
    [(b.max[0] - b.min[0]) * 0.5, (b.max[1] - b.min[1]) * 0.5]
}

fn aabb_touch(a: &Aabb, b: &Aabb) -> bool {
    (0..2).all(|i| a.min[i] <= b.max[i] && a.max[i] >= b.min[i])
}

fn normalize(n: [f64; 2]) -> [f64; 2] {
    let l = n[0].hypot(n[1]);
    if l < 1e-12 {
        [0.0, 1.0]
    } else {
        [n[0] / l, n[1] / l]
    }
}

pub(crate) fn v(a: [f64; 2]) -> Vector {
    Vector::new(a[0] as f32, a[1] as f32)
}

pub(crate) fn p(a: Vector) -> [f64; 2] {
    [f64::from(a.x), f64::from(a.y)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor_space() -> Space {
        Space::of(&[Collider::solid(Aabb {
            min: [-10.0, -1.0],
            max: [10.0, 0.0],
        })])
    }

    #[test]
    fn cast_stops_at_skin_above_floor() {
        let s = floor_space();
        let b = Aabb::around([0.0, 1.0], [0.1, 0.1]);
        let (moved, hit) = s.sweep(b, 1, -2.0, &Filter::default());
        let bottom = 0.9 + moved;
        assert!(bottom > 0.0 && bottom <= 2.0 * SKIN, "{bottom}");
        let hit = hit.unwrap();
        assert!(hit.normal[1] > 0.99);
    }

    #[test]
    fn inside_does_not_hold() {
        let s = floor_space();
        let b = Aabb::around([0.0, -0.5], [0.1, 0.1]);
        let (moved, hit) = s.sweep(b, 0, 1.0, &Filter::default());
        assert_eq!(moved, 1.0);
        assert!(hit.is_none());
    }

    #[test]
    fn platform_holds_only_from_above() {
        let s = Space::of(&[Collider::oneway(Aabb {
            min: [-1.0, -0.1],
            max: [1.0, 0.0],
        })]);
        let f = Filter::default().platforms(true);
        let above = Aabb::around([0.0, 0.2], [0.1, 0.1]);
        assert!(s.sweep(above, 1, -1.0, &f).1.is_some());
        let below = Aabb::around([0.0, -0.5], [0.1, 0.1]);
        assert!(s.sweep(below, 1, 1.0, &f).1.is_none());
        assert!(s.sweep(above, 1, -1.0, &Filter::default()).1.is_none());
    }

    #[test]
    fn ray_finds_slope_height() {
        let s = Space::of(&[Collider::slope(
            Aabb {
                min: [0.0, 0.0],
                max: [1.0, 1.0],
            },
            1.0,
        )]);
        let (h, hit) = s.ray_down([0.25, 2.0], 5.0, &Filter::default()).unwrap();
        assert!((h - 0.25).abs() < 1e-4, "{h}");
        assert!((hit.normal[1] - 0.5f64.sqrt()).abs() < 1e-3);
    }

    #[test]
    fn rotating_surface_carries_point_around() {
        let mut sf = Surface::of_shape(7, cuboid([1.0, 0.1]), Pose::IDENTITY);
        sf.turned = std::f64::consts::FRAC_PI_2;
        let s = Space { surfaces: vec![sf] };
        let d = s.carry(7, [1.0, 0.0]);
        assert!(
            (d[0] + 1.0).abs() < 1e-9 && (d[1] - 1.0).abs() < 1e-9,
            "{d:?}"
        );
    }
}
