//! Физика на `rapier2d` (6.10, полигон физики 6.12): твёрдые тела разных форм,
//! сочленения (шарнир, сварка, ползун, верёвка, пружина) с моторами и разрывом, мягкие
//! тела (верёвка, желе, полотно), захват мышью и взрыв.
//!
//! Rapier собирается из своей копии в `third_party` (решение 0015). Остальной движок
//! говорит с ним только через этот модуль: свои типы, свои единицы и градусы, номера
//! вместо ручек Rapier. Поэтому наши надстройки (разрыв сочленений, моторы туда-обратно,
//! захват, взрыв) живут здесь, и замена версии Rapier трогает только этот модуль.
//!
//! Герой и враги движутся своим контроллером (точно и предсказуемо), а не физикой.
//! Здесь они — «кинематические» тела: физика знает, где они, и они расталкивают
//! ящики, но сами от ящиков не отлетают. Уровень (стены, пол, платформы, склоны) тоже
//! кинематический: платформы ездят, и физика двигает их вместе с тем, что на них лежит.
//!
//! Обратно в контроллер героя ящики идут как платформы ([`Physics::platforms`]): на
//! ящик можно встать, а сбоку герой его толкает.
//!
//! Единицы — единицы сцены (герой ростом 0,28). Чтобы допуски `rapier` подходили к
//! таким мелким телам, задан масштаб длины [`LENGTH_UNIT`].

use std::collections::HashMap;
use std::time::Instant;

use rapier2d::prelude::*;

use crate::{Aabb, Collider};

mod joints;
mod soft;

pub use joints::{JointDesc, JointId, JointKind, Motor};
pub use soft::{Pin, SoftDesc, SoftId, SoftKind, SoftView};

/// Масштаб длины для допусков `rapier`: типичный размер тела в единицах сцены.
pub const LENGTH_UNIT: f32 = 0.2;

/// Форма тела.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum BodyShape {
    /// Прямоугольник `size`.
    #[default]
    Box,
    /// Круг диаметром `size[0]`.
    Ball,
    /// Капсула (прямоугольник со скруглёнными концами) в прямоугольнике `size`: вдоль
    /// длинной стороны.
    Capsule,
    /// Выпуклый многоугольник по точкам вокруг середины тела (лишние точки внутри
    /// отбрасываются). `size` для него не нужен.
    Polygon(Vec<[f64; 2]>),
}

/// Тело из файла сцены.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyDesc {
    /// Ширина и высота (у круглого — диаметр по ширине).
    pub size: [f64; 2],
    pub shape: BodyShape,
    /// Плотность: тяжелее — труднее толкнуть.
    pub density: f64,
    /// Упругость: 0 — не отскакивает, 1 — отскакивает без потерь.
    pub bounce: f64,
    pub friction: f64,
    /// Неподвижное тело: опора для сочленений, стойка, ось. Двигается, только если его
    /// тащить мышью — тогда нет, оно стоит.
    pub fixed: bool,
    /// Сопротивление воздуха: 0 — нет, 1 — заметно тормозит.
    pub damping: f64,
    /// Во сколько раз на тело действует тяжесть (0 — парит).
    pub gravity: f64,
    /// Быстрое тело («пуля»: ядро, обломок, выброшенное тело): Rapier проверяет весь его
    /// путь за шаг, и оно не пролетает сквозь тонкое. Пули друг друга так не проверяют,
    /// поэтому обычным телам этого не включаем: иначе кубик падает сквозь тонкую полку.
    pub bullet: bool,
}

impl Default for BodyDesc {
    fn default() -> Self {
        Self {
            size: [0.15, 0.15],
            shape: BodyShape::Box,
            density: 1.0,
            bounce: 0.2,
            friction: 0.6,
            fixed: false,
            damping: 0.0,
            gravity: 1.0,
            bullet: false,
        }
    }
}

impl BodyDesc {
    /// Форма для `rapier` (без плотности и трения).
    fn collider(&self) -> ColliderBuilder {
        let (w, h) = ((self.size[0] * 0.5) as f32, (self.size[1] * 0.5) as f32);
        match &self.shape {
            BodyShape::Box => ColliderBuilder::cuboid(w, h),
            BodyShape::Ball => ColliderBuilder::ball(w),
            BodyShape::Capsule if h >= w => ColliderBuilder::capsule_y(h - w, w),
            BodyShape::Capsule => ColliderBuilder::capsule_x(w - h, h),
            BodyShape::Polygon(points) => {
                let pts: Vec<Vector> = points.iter().map(|q| v(*q)).collect();
                ColliderBuilder::convex_hull(&pts).unwrap_or_else(|| ColliderBuilder::cuboid(w, h))
            }
        }
    }
}

/// Номер тела в физике.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BodyId(RigidBodyHandle);

/// Кинематическое тело уровня или героя: его прямоугольник (и склон) в прошлый раз.
struct Mover {
    body: RigidBodyHandle,
    collider: ColliderHandle,
    half: [f64; 2],
    slope: f64,
    /// Отмечено на этом шаге (неотмеченные убираются).
    seen: bool,
}

/// Что держит мышь: тело (точка на нём в его собственных координатах) или частицу
/// мягкого тела.
#[derive(Clone, Copy, Debug)]
enum Grip {
    Body {
        body: RigidBodyHandle,
        local: Vector,
    },
    /// `keep` — частица была закреплена и остаётся закреплённой там, где её отпустили.
    Particle {
        soft: SoftBodyHandle,
        index: usize,
        keep: bool,
    },
}

/// Счётчики для панели.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicsStats {
    /// Тела, которые двигает физика (без уровня и героя).
    pub bodies: usize,
    /// Из них спят: стоят и не считаются, пока их не тронут.
    pub sleeping: usize,
    pub joints: usize,
    /// Сколько сочленений порвалось с начала сцены.
    pub broken: usize,
    /// Мягкие тела (с кусками порванных) и их частицы.
    pub soft: usize,
    pub particles: usize,
    /// Время последнего шага физики, мс.
    pub step_ms: f64,
}

/// Мир физики.
pub struct Physics {
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    soft_bodies: SoftBodySet,
    pipeline: PhysicsPipeline,
    broad: BroadPhaseBvh,
    narrow: NarrowPhase,
    islands: IslandManager,
    ccd: CCDSolver,
    params: IntegrationParameters,
    gravity: Vector,
    /// Кинематические тела по номеру (`Collider::id` или номер героя/врага).
    movers: HashMap<u64, Mover>,
    /// Где были тела на прошлом шаге (для платформ: на сколько сдвинулись).
    last: HashMap<RigidBodyHandle, [f64; 2]>,
    /// Неподвижное тело в начале координат: к нему крепятся сочленения «к миру».
    ground: Option<RigidBodyHandle>,
    /// Наши надстройки над сочленениями: моторы туда-обратно, разрыв.
    joint_extras: Vec<joints::Extra>,
    /// Где порвались сочленения на последнем шаге.
    breaks: Vec<[f64; 2]>,
    broken: usize,
    /// Как рисовать мягкие тела: номер вида по номеру мягкого тела.
    soft_styles: HashMap<SoftBodyHandle, u32>,
    grip: Option<(Grip, Vector)>,
    /// Время мира, с: для моторов туда-обратно.
    time: f64,
    step_ms: f64,
}

impl Physics {
    /// Мир с тяжестью `gravity` (единиц в секунду за секунду, вниз).
    pub fn new(gravity: f64) -> Self {
        let params = IntegrationParameters {
            length_unit: LENGTH_UNIT,
            ..IntegrationParameters::default()
        };
        Self {
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            soft_bodies: SoftBodySet::new(),
            pipeline: PhysicsPipeline::new(),
            broad: BroadPhaseBvh::new(),
            narrow: NarrowPhase::new(),
            islands: IslandManager::new(),
            ccd: CCDSolver::new(),
            params,
            gravity: Vector::new(0.0, -gravity as f32),
            movers: HashMap::new(),
            last: HashMap::new(),
            ground: None,
            joint_extras: Vec::new(),
            breaks: Vec::new(),
            broken: 0,
            soft_styles: HashMap::new(),
            grip: None,
            time: 0.0,
            step_ms: 0.0,
        }
    }

    /// Новое тело в `at` с поворотом `angle` (градусы) и скоростью `vel`.
    pub fn add(&mut self, at: [f64; 2], angle: f64, vel: [f64; 2], d: &BodyDesc) -> BodyId {
        let builder = if d.fixed {
            RigidBodyBuilder::fixed()
        } else {
            RigidBodyBuilder::dynamic()
                .linvel(v(vel))
                .linear_damping(d.damping.max(0.0) as f32)
                .angular_damping(d.damping.max(0.0) as f32)
                .gravity_scale(d.gravity as f32)
                // Быстрые тела не пролетают сквозь тонкие стены. Без этого Rapier всё равно
                // проверяет путь быстрых тел до неподвижных; «пуля» проверяет ещё и до
                // подвижных. (Мягкое предсказание касаний пробовали: роняет пирамиду.)
                .ccd_enabled(d.bullet)
        };
        let body = builder
            .translation(v(at))
            .rotation(angle.to_radians() as f32)
            .build();
        let h = self.bodies.insert(body);
        let c = d
            .collider()
            .density(d.density.max(0.01) as f32)
            .restitution(d.bounce.clamp(0.0, 1.0) as f32)
            .friction(d.friction.max(0.0) as f32)
            .build();
        self.colliders.insert_with_parent(c, h, &mut self.bodies);
        self.last.insert(h, at);
        BodyId(h)
    }

    /// Убирает тело.
    pub fn remove(&mut self, id: BodyId) {
        self.last.remove(&id.0);
        if matches!(self.grip, Some((Grip::Body { body, .. }, _)) if body == id.0) {
            self.grip = None;
        }
        self.drop_body(id.0);
    }

    /// Сколько тел (без уровня и героя).
    pub fn len(&self) -> usize {
        self.last.len()
    }

    pub fn is_empty(&self) -> bool {
        self.last.is_empty()
    }

    /// Есть ли что считать: тела или мягкие тела.
    pub fn has_work(&self) -> bool {
        !self.last.is_empty() || !self.soft_bodies.is_empty()
    }

    /// Счётчики для панели.
    pub fn stats(&self) -> PhysicsStats {
        let sleeping = self
            .last
            .keys()
            .filter(|h| self.bodies.get(**h).is_some_and(|b| b.is_sleeping()))
            .count();
        PhysicsStats {
            bodies: self.last.len(),
            sleeping,
            joints: self.impulse_joints.len(),
            broken: self.broken,
            soft: self.soft_bodies.len(),
            particles: self
                .soft_bodies
                .iter()
                .map(|(_, s)| s.num_particles())
                .sum(),
            step_ms: self.step_ms,
        }
    }

    /// Где порвались сочленения за последний шаг (для искр и звука).
    pub fn take_breaks(&mut self) -> Vec<[f64; 2]> {
        std::mem::take(&mut self.breaks)
    }

    /// Твёрдое уровня, герой и враги на этот шаг: кинематические тела. `key` отличает
    /// их номера от номеров друг друга. Чего нет в списке с прошлого раза — убирается
    /// в [`Self::step`].
    pub fn set_solids(&mut self, solids: impl IntoIterator<Item = (u64, Collider)>) {
        for (key, c) in solids {
            let half = [
                (c.aabb.max[0] - c.aabb.min[0]) * 0.5,
                (c.aabb.max[1] - c.aabb.min[1]) * 0.5,
            ];
            let center = [
                (c.aabb.max[0] + c.aabb.min[0]) * 0.5,
                (c.aabb.max[1] + c.aabb.min[1]) * 0.5,
            ];
            let same = self.movers.get(&key).is_some_and(|m| {
                (m.half[0] - half[0]).abs() < 1e-9
                    && (m.half[1] - half[1]).abs() < 1e-9
                    && m.slope == c.slope
            });
            if !same {
                if let Some(old) = self.movers.remove(&key) {
                    self.drop_body(old.body);
                }
                let body = self.bodies.insert(
                    RigidBodyBuilder::kinematic_position_based()
                        .translation(v(center))
                        .build(),
                );
                let collider = self.colliders.insert_with_parent(
                    mover_shape(half, c.slope).friction(0.7).build(),
                    body,
                    &mut self.bodies,
                );
                self.movers.insert(
                    key,
                    Mover {
                        body,
                        collider,
                        half,
                        slope: c.slope,
                        seen: true,
                    },
                );
            } else if let Some(m) = self.movers.get_mut(&key) {
                m.seen = true;
                if let Some(b) = self.bodies.get_mut(m.body) {
                    b.set_next_kinematic_translation(v(center));
                }
            }
        }
    }

    fn drop_body(&mut self, h: RigidBodyHandle) {
        self.bodies.remove(
            h,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.soft_bodies,
            true,
        );
    }

    /// Шаг физики длиной `dt`.
    pub fn step(&mut self, dt: f64) {
        let started = Instant::now();
        let gone: Vec<u64> = self
            .movers
            .iter()
            .filter(|(_, m)| !m.seen)
            .map(|(k, _)| *k)
            .collect();
        for k in gone {
            if let Some(m) = self.movers.remove(&k) {
                let _ = m.collider;
                self.drop_body(m.body);
            }
        }
        for m in self.movers.values_mut() {
            m.seen = false;
        }
        for (h, at) in self.last.iter_mut() {
            if let Some(b) = self.bodies.get(*h) {
                *at = p(b.translation());
            }
        }
        self.params.dt = dt.clamp(1e-4, 0.05) as f32;
        let dt = self.params.dt;
        self.time += f64::from(dt);
        self.drive_motors();
        self.pull_grip(dt);
        self.pipeline.step(
            self.gravity,
            &self.params,
            &mut self.islands,
            &mut self.broad,
            &mut self.narrow,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.soft_bodies,
            &mut self.ccd,
            &(),
            &(),
        );
        self.check_breaks(dt);
        self.step_ms = started.elapsed().as_secs_f64() * 1000.0;
    }

    /// Мышь хватает то, что под точкой `at`: тело или частицу мягкого тела рядом.
    /// Неподвижные тела не хватаются. `true` — схватила.
    pub fn grab(&mut self, at: [f64; 2]) -> bool {
        self.release();
        let point = v(at);
        let hit = self.colliders.iter().find_map(|(_, c)| {
            let body = c.parent()?;
            let b = self.bodies.get(body)?;
            if !b.is_dynamic() || !self.last.contains_key(&body) {
                return None;
            }
            c.shape()
                .contains_point(c.position(), point)
                .then(|| (body, b.position().inverse_transform_point(point)))
        });
        if let Some((body, local)) = hit {
            self.grip = Some((Grip::Body { body, local }, point));
            return true;
        }
        // Частица мягкого тела ближе 0,06 (или своего радиуса).
        let mut best: Option<(f32, SoftBodyHandle, usize)> = None;
        for (h, s) in self.soft_bodies.iter() {
            let reach = (s.particle_radius() * 1.5).max(0.06);
            for (i, q) in s.particle_positions().enumerate() {
                let d = (q - point).length();
                if d < reach && best.is_none_or(|(bd, _, _)| d < bd) {
                    best = Some((d, h, i));
                }
            }
        }
        let Some((_, soft, index)) = best else {
            return false;
        };
        let mut keep = false;
        if let Some(s) = self.soft_bodies.get_mut(soft) {
            // Закреплённую частицу тащим и оставляем закреплённой на новом месте.
            keep = s.particles()[index].is_pinned();
            s.set_particle_pinned(index, true);
            s.wake_up();
        }
        self.grip = Some((Grip::Particle { soft, index, keep }, point));
        true
    }

    /// Куда тянуть схваченное.
    pub fn drag(&mut self, to: [f64; 2]) {
        if let Some((_, target)) = self.grip.as_mut() {
            *target = v(to);
        }
    }

    /// Мышь отпустила.
    pub fn release(&mut self) {
        let Some((grip, _)) = self.grip.take() else {
            return;
        };
        if let Grip::Particle {
            soft,
            index,
            keep: false,
        } = grip
            && let Some(s) = self.soft_bodies.get_mut(soft)
            && index < s.num_particles()
        {
            s.set_particle_pinned(index, false);
        }
    }

    /// Что держит мышь: точка на теле и куда её тянут (для рисования нитки).
    pub fn grip_line(&self) -> Option<([f64; 2], [f64; 2])> {
        let (grip, target) = self.grip?;
        let from = match grip {
            Grip::Body { body, local } => self.bodies.get(body)?.position().transform_point(local),
            Grip::Particle { soft, index, .. } => {
                let s = self.soft_bodies.get(soft)?;
                (index < s.num_particles()).then(|| s.particle_position(index))?
            }
        };
        Some((p(from), p(target)))
    }

    /// Тянет схваченное к цели: скорость точки захвата — к цели, без рывков (не
    /// телепорт, поэтому тело не проходит сквозь стены и толкает соседей).
    fn pull_grip(&mut self, dt: f32) {
        let Some((grip, target)) = self.grip else {
            return;
        };
        match grip {
            Grip::Body { body, local } => {
                let Some(b) = self.bodies.get_mut(body) else {
                    self.grip = None;
                    return;
                };
                let at = b.position().transform_point(local);
                let want = ((target - at) * 14.0).clamp_length_max(30.0);
                let now = b.velocity_at_point(at);
                let mass = b.mass();
                b.apply_impulse_at_point((want - now) * mass * 0.6, at, true);
                // Гасим кручение, иначе схваченное за край раскачивается без конца.
                let w = b.angvel() * 0.9;
                b.set_angvel(w, true);
            }
            Grip::Particle { soft, index, .. } => {
                let Some(s) = self.soft_bodies.get_mut(soft) else {
                    self.grip = None;
                    return;
                };
                if index >= s.num_particles() {
                    self.grip = None;
                    return;
                }
                let at = s.particle_position(index);
                let step = (target - at).clamp_length_max(30.0 * dt);
                s.set_particle_kinematic_target(index, at + step);
                s.wake_up();
            }
        }
    }

    /// Взрыв в `at`: всё ближе `radius` отлетает от середины, тем сильнее, чем ближе.
    /// `speed` — скорость, которую получает тело в самой середине.
    pub fn blast(&mut self, at: [f64; 2], radius: f64, speed: f64) {
        let center = v(at);
        let r = radius as f32;
        for h in self.last.keys() {
            let Some(b) = self.bodies.get_mut(*h) else {
                continue;
            };
            if !b.is_dynamic() {
                continue;
            }
            let d = b.translation() - center;
            let len = d.length();
            if len >= r {
                continue;
            }
            let dir = if len > 1e-4 { d / len } else { Vector::Y };
            let k = (1.0 - len / r) * speed as f32;
            let vel = b.linvel() + dir * k;
            b.set_linvel(vel, true);
            let spin = b.angvel() + (dir.x - dir.y) * k * 4.0;
            b.set_angvel(spin, true);
        }
        for (_, s) in self.soft_bodies.iter_mut() {
            let mut woke = false;
            for i in 0..s.num_particles() {
                if s.particles()[i].is_pinned() {
                    continue;
                }
                let d = s.particle_position(i) - center;
                let len = d.length();
                if len >= r {
                    continue;
                }
                let dir = if len > 1e-4 { d / len } else { Vector::Y };
                let vel = s.particle_velocity(i) + dir * (1.0 - len / r) * speed as f32;
                s.set_particle_velocity(i, vel);
                woke = true;
            }
            if woke {
                s.wake_up();
            }
        }
    }

    /// Где тело и его поворот (градусы).
    pub fn pose(&self, id: BodyId) -> Option<([f64; 2], f64)> {
        let b = self.bodies.get(id.0)?;
        Some((
            p(b.translation()),
            f64::from(b.rotation().angle()).to_degrees(),
        ))
    }

    pub fn velocity(&self, id: BodyId) -> Option<[f64; 2]> {
        self.bodies.get(id.0).map(|b| p(b.linvel()))
    }

    /// Прямоугольник вокруг тела (у повёрнутого — описанный).
    pub fn bounds(&self, id: BodyId) -> Option<Aabb> {
        let b = self.bodies.get(id.0)?;
        let mut out: Option<Aabb> = None;
        for &c in b.colliders() {
            let a = self.colliders.get(c)?.compute_aabb();
            let box_ = Aabb {
                min: [f64::from(a.mins.x), f64::from(a.mins.y)],
                max: [f64::from(a.maxs.x), f64::from(a.maxs.y)],
            };
            out = Some(match out {
                None => box_,
                Some(o) => Aabb::of_points([o.min, o.max, box_.min, box_.max]).unwrap(),
            });
        }
        out
    }

    /// Толчок: мгновенно добавить скорость `dv` (не зависит от массы — лёгкое и тяжёлое
    /// отлетают одинаково быстро; для удара так понятнее).
    pub fn kick(&mut self, id: BodyId, dv: [f64; 2], spin: f64) {
        if let Some(b) = self.bodies.get_mut(id.0) {
            let vel = b.linvel() + v(dv);
            b.set_linvel(vel, true);
            let w = b.angvel() + spin as f32;
            b.set_angvel(w, true);
        }
    }

    /// Тела как платформы для контроллера героя: на них встают сверху, сдвиг за шаг
    /// везёт стоящего. Номер платформы — `base` плюс номер тела по порядку.
    pub fn platforms(&self, ids: &[BodyId], base: u64) -> Vec<Collider> {
        ids.iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let aabb = self.bounds(*id)?;
                let now = p(self.bodies.get(id.0)?.translation());
                let was = self.last.get(&id.0).copied().unwrap_or(now);
                Some(Collider {
                    id: base + i as u64,
                    moved: [now[0] - was[0], now[1] - was[1]],
                    ..Collider::oneway(aabb)
                })
            })
            .collect()
    }
}

fn mover_shape(half: [f64; 2], slope: f64) -> ColliderBuilder {
    if slope == 0.0 {
        return ColliderBuilder::cuboid(half[0] as f32, half[1] as f32);
    }
    // Склон: треугольник в прямоугольнике, подъём в сторону знака `slope`.
    let (hx, hy) = (half[0] as f32, half[1] as f32);
    let (a, b, c) = if slope > 0.0 {
        (
            Vector::new(-hx, -hy),
            Vector::new(hx, -hy),
            Vector::new(hx, hy),
        )
    } else {
        (
            Vector::new(-hx, -hy),
            Vector::new(hx, -hy),
            Vector::new(-hx, hy),
        )
    };
    ColliderBuilder::triangle(a, b, c)
}

fn v(a: [f64; 2]) -> Vector {
    Vector::new(a[0] as f32, a[1] as f32)
}

fn p(a: Vector) -> [f64; 2] {
    [f64::from(a.x), f64::from(a.y)]
}

#[cfg(test)]
mod tests;
