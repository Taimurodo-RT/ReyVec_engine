//! Физика на `rapier2d` (6.10, полигон физики 6.13): твёрдые тела разных форм,
//! сочленения (шарнир, сварка, ползун, верёвка, пружина) с моторами и разрывом, мягкие
//! тела (верёвка, желе, полотно), захват мышью и взрыв.
//!
//! Rapier собирается из своей копии в `third_party` (решение 0016). Остальной движок
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

use crate::space::Surface;
use crate::{Aabb, Collider};

mod hands;
mod joints;
mod soft;
mod touch;

pub use hands::{CARRY_MASS, DRAG_MASS, Hands, HandsEvent, Hold, Holder};
pub use joints::{JointDesc, JointId, JointKind, Motor};
pub use soft::{Pin, SoftDesc, SoftId, SoftKind, SoftView};
pub use touch::{Material, Touch, Toucher};

/// Масштаб длины для допусков `rapier`: типичный размер тела в единицах сцены.
pub const LENGTH_UNIT: f32 = 0.2;

/// Сдвиг кинематического тела за шаг, больше которого это перенос, а не движение: быстрее
/// рывка героя (0,3 за шаг — 18 единиц в секунду).
const TELEPORT: f32 = 0.3;

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
    /// Материал (9.6): звук и искры при ударе.
    pub material: Material,
    /// Слой и маска столкновений (9.6а, [`crate::layer`]).
    pub layer: u32,
    pub mask: u32,
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
            material: Material::Wood,
            layer: crate::layer::BODY,
            mask: crate::layer::ALL,
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

/// Поле сил в области (9.6б): ветер или течение, другая тяжесть, магнит.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    /// Скорость ветра, единиц в секунду: тела разгоняются к ней вдоль ветра, лёгкие
    /// быстрее тяжёлых.
    pub wind: [f64; 2],
    /// Во сколько раз тяжесть в области (1 — как везде, 0 — невесомость, меньше нуля —
    /// тянет вверх).
    pub gravity: f64,
    /// Магнит: тянет металлические тела к середине области с ускорением `magnet`
    /// и гасит их раскачку.
    pub magnet: f64,
}

impl Default for Field {
    fn default() -> Self {
        Self {
            wind: [0.0, 0.0],
            gravity: 1.0,
            magnet: 0.0,
        }
    }
}

/// Масса, которую ветер разгоняет почти до своей скорости за шестую долю секунды: ящик
/// 0,15 (тяжелее — медленнее, по корню из массы).
const WIND_MASS: f32 = 0.0225;

/// Номер тела в физике.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BodyId(RigidBodyHandle);

/// Кинематическое тело уровня или героя: его прямоугольник (и склон) в прошлый раз.
struct Mover {
    body: RigidBodyHandle,
    collider: ColliderHandle,
    shape: SharedShape,
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
    /// Поворот тел на прошлом шаге, радианы (вращающаяся опора везёт стоящего).
    last_angle: HashMap<RigidBodyHandle, f64>,
    /// Неподвижное тело в начале координат: к нему крепятся сочленения «к миру».
    ground: Option<RigidBodyHandle>,
    /// Наши надстройки над сочленениями: моторы туда-обратно, разрыв.
    joint_extras: Vec<joints::Extra>,
    /// Где порвались сочленения на последнем шаге.
    breaks: Vec<[f64; 2]>,
    broken: usize,
    /// Как рисовать мягкие тела: номер вида по номеру мягкого тела.
    soft_styles: HashMap<SoftBodyHandle, u32>,
    /// Где были частицы мягких тел до последнего шага (на сколько сдвинулся отрезок).
    soft_last: HashMap<SoftBodyHandle, Vec<Vector>>,
    grip: Option<(Grip, Vector)>,
    /// Время мира, с: для моторов туда-обратно.
    time: f64,
    step_ms: f64,
    /// События касаний во время шага Rapier (9.6).
    collector: touch::Collector,
    /// Скорость тел до шага: скорость, вращение, середина масс (сила удара).
    vel_before: HashMap<RigidBodyHandle, (Vector, f32, Vector)>,
    /// Номер кинематического тела по его ручке.
    mover_keys: HashMap<RigidBodyHandle, u64>,
    /// Касания с прошлого [`Self::take_touches`].
    touches: Vec<Touch>,
    /// Поля сил дали телам силы на этот шаг (сбросить после шага).
    forced: bool,
}

/// Больше касаний не копим, если их никто не забирает.
const MAX_TOUCHES: usize = 4096;

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
            last_angle: HashMap::new(),
            ground: None,
            joint_extras: Vec::new(),
            breaks: Vec::new(),
            broken: 0,
            soft_styles: HashMap::new(),
            soft_last: HashMap::new(),
            grip: None,
            time: 0.0,
            step_ms: 0.0,
            collector: touch::Collector::default(),
            vel_before: HashMap::new(),
            mover_keys: HashMap::new(),
            touches: Vec::new(),
            forced: false,
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
                // Правила CCD (9.7), проверки пути — чтобы быстрое не проскочило тонкое:
                // - до неподвижного уровня Rapier проверяет путь быстрых тел сам, всегда;
                // - до едущего (лифт, вращающаяся звезда, герой) и до других тел — только
                //   у «пули» (`bullet: true`: обломки, выброшенное).
                // Включать всем нельзя: пробовали (6.12) — кубики проваливались сквозь
                // полку; включать быстрым на шаг (9.7) — резиновые шары в куче не
                // засыпают, а героя дёргает. Мягкое предсказание касаний роняет пирамиду.
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
            .user_data(d.material.to_bits())
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .collision_groups(groups(d.layer, d.mask))
            .build();
        self.colliders.insert_with_parent(c, h, &mut self.bodies);
        self.last.insert(h, at);
        self.last_angle.insert(h, angle.to_radians());
        BodyId(h)
    }

    /// Убирает тело.
    pub fn remove(&mut self, id: BodyId) {
        self.last.remove(&id.0);
        self.last_angle.remove(&id.0);
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

    /// Твёрдое уровня, герой и враги на этот шаг прямоугольниками и склонами (см.
    /// [`Self::set_surfaces`]); `key` — номер.
    pub fn set_solids(&mut self, solids: impl IntoIterator<Item = (u64, Collider)>) {
        self.set_surfaces(solids.into_iter().map(|(key, c)| {
            let mut s = Surface::of_collider(&c);
            s.id = key;
            s
        }));
    }

    /// Твёрдое уровня, герой и враги на этот шаг: неподвижные, а сдвинувшиеся — кинематические тела настоящей формы
    /// (9.3). Номер — [`Surface::id`]. Чего нет в списке с прошлого раза — убирается в
    /// [`Self::step`]. Форма меняется, только если стала другой.
    pub fn set_surfaces(&mut self, solids: impl IntoIterator<Item = Surface>) {
        for s in solids {
            let key = s.id;
            if let Some(m) = self.movers.get_mut(&key) {
                m.seen = true;
                if !same_shape(&m.shape, &s.shape) {
                    if let Some(c) = self.colliders.get_mut(m.collider) {
                        c.set_shape(s.shape.clone());
                    }
                    m.shape = s.shape;
                }
                if let Some(b) = self.bodies.get_mut(m.body) {
                    // Неподвижное до сих пор (пол, стена) — неподвижное тело Rapier: через
                    // него не связываются острова, и куча ящиков засыпает, пока в другом
                    // конце уровня что-то движется. Сдвинулось — становится кинематическим.
                    if b.is_fixed() {
                        if *b.position() == s.pose {
                            continue;
                        }
                        b.set_body_type(RigidBodyType::KinematicPositionBased, true);
                    }
                    // Перенос (герой появился заново, отладка поставила его в другое
                    // место) — сразу на новое место. Иначе Rapier счёл бы перенос очень
                    // быстрым движением и отшвырнул всё, чего тело касалось.
                    if (b.translation() - s.pose.translation).length() > TELEPORT {
                        b.set_position(s.pose, true);
                    }
                    b.set_next_kinematic_position(s.pose);
                }
                continue;
            }
            let body = self
                .bodies
                .insert(RigidBodyBuilder::fixed().pose(s.pose).build());
            let collider = self.colliders.insert_with_parent(
                ColliderBuilder::new(s.shape.clone())
                    .friction(0.7)
                    .user_data(Material::Stone.to_bits())
                    .collision_groups(groups(s.layer, s.mask))
                    .build(),
                body,
                &mut self.bodies,
            );
            self.mover_keys.insert(body, key);
            self.movers.insert(
                key,
                Mover {
                    body,
                    collider,
                    shape: s.shape,
                    seen: true,
                },
            );
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
                self.mover_keys.remove(&m.body);
                self.drop_body(m.body);
            }
        }
        for m in self.movers.values_mut() {
            m.seen = false;
        }
        for (h, at) in self.last.iter_mut() {
            // Спящее не двигалось: прошлое место то же (9.7: 2 000 спящих тел — даром).
            if let Some(b) = self.bodies.get(*h).filter(|b| !b.is_sleeping()) {
                *at = p(b.translation());
                self.last_angle.insert(*h, f64::from(b.rotation().angle()));
            }
        }
        self.soft_last.clear();
        for (h, s) in self.soft_bodies.iter() {
            self.soft_last.insert(h, s.particle_positions().collect());
        }
        self.params.dt = dt.clamp(1e-4, 0.05) as f32;
        let dt = self.params.dt;
        self.time += f64::from(dt);
        self.drive_motors();
        self.pull_grip(dt);
        self.vel_before.clear();
        for (h, b) in self.bodies.iter() {
            if !b.is_fixed() && !b.is_sleeping() {
                self.vel_before
                    .insert(h, (b.linvel(), b.angvel(), b.center_of_mass()));
            }
        }
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
            &self.collector,
        );
        self.collect_touches();
        if std::mem::take(&mut self.forced) {
            for h in self.last.keys() {
                if let Some(b) = self.bodies.get_mut(*h) {
                    b.reset_forces(false);
                }
            }
        }
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

    /// Поле сил `f` в прямоугольнике `area` на шаг `dt` (9.6б): действует на тела, чья
    /// середина внутри, и на частицы мягких тел. Звать перед [`Self::step`].
    pub fn field(&mut self, area: Aabb, f: &Field, dt: f64) {
        let dt = dt as f32;
        let g = self.gravity;
        let center = v(area.center());
        let inside = |q: Vector| area.contains([f64::from(q.x), f64::from(q.y)]);
        let wind = v(f.wind);
        let wind_len = wind.length();
        let wind_dir = if wind_len > 1e-6 {
            wind / wind_len
        } else {
            Vector::ZERO
        };
        // Скорость `vel` тела массой `mass` после шага в поле.
        let push = |vel: Vector, mass: f32, at: Vector, metal: bool| {
            let mut vel = vel;
            if wind_len > 1e-6 {
                let along = vel.dot(wind_dir);
                if along < wind_len {
                    let k = (6.0 * (WIND_MASS / mass.max(1e-6)).sqrt()).min(20.0) * dt;
                    vel += wind_dir * (wind_len - along) * k.min(1.0);
                }
            }
            if metal && f.magnet != 0.0 {
                let to = center - at;
                vel += to.normalize_or_zero() * f.magnet as f32 * dt;
                vel -= vel * (3.0 * dt).min(1.0);
            }
            vel
        };
        for h in self.last.keys() {
            let Some(b) = self.bodies.get_mut(*h) else {
                continue;
            };
            if !b.is_dynamic() || !inside(b.translation()) {
                continue;
            }
            let metal = b
                .colliders()
                .first()
                .and_then(|c| self.colliders.get(*c))
                .is_some_and(|c| Material::from_bits(c.user_data) == Material::Metal);
            let vel = push(b.linvel(), b.mass(), b.translation(), metal);
            b.set_linvel(vel, true);
            // Другая тяжесть — силой: Rapier сложит её с тяжестью мира тем же способом,
            // и в невесомости тело стоит точно. Силы сбрасываются после шага.
            let extra = g * (f.gravity as f32 - 1.0) * b.gravity_scale() * b.mass();
            b.add_force(extra, true);
            self.forced = true;
        }
        for (_, s) in self.soft_bodies.iter_mut() {
            let n = s.num_particles();
            let mass = s.mass() / n.max(1) as f32;
            let mut woke = false;
            for i in 0..n {
                let at = s.particle_position(i);
                if s.particles()[i].is_pinned() || !inside(at) {
                    continue;
                }
                let vel = push(s.particle_velocity(i), mass, at, false)
                    + g * (f.gravity as f32 - 1.0) * dt;
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

    /// Тела как поверхности мира столкновений героя и врагов (9.2): настоящие формы на
    /// своих местах, со сдвигом и поворотом за прошлый шаг физики (опора везёт стоящего).
    /// Подвижные тела можно толкать. Номер поверхности — `base` плюс номер тела по
    /// порядку в `ids`.
    pub fn surfaces(&self, ids: &[BodyId], base: u64) -> Vec<Surface> {
        let mut out = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let Some(b) = self.bodies.get(id.0) else {
                continue;
            };
            let now = p(b.translation());
            let was = self.last.get(&id.0).copied().unwrap_or(now);
            let angle = f64::from(b.rotation().angle());
            let was_angle = self.last_angle.get(&id.0).copied().unwrap_or(angle);
            let mut turned = angle - was_angle;
            // Через ±180° — короткой дорогой.
            if turned > std::f64::consts::PI {
                turned -= std::f64::consts::TAU;
            } else if turned < -std::f64::consts::PI {
                turned += std::f64::consts::TAU;
            }
            for &c in b.colliders() {
                let Some(col) = self.colliders.get(c) else {
                    continue;
                };
                let mut s =
                    Surface::of_shape(base + i as u64, col.shared_shape().clone(), *col.position());
                s.moved = [now[0] - was[0], now[1] - was[1]];
                s.turned = turned;
                s.center = now;
                s.pushable = b.is_dynamic();
                let g = col.collision_groups();
                s.layer = g.memberships.bits();
                s.mask = g.filter.bits();
                out.push(s);
            }
        }
        out
    }

    /// Персонаж толкает тело сбоку, желая идти со скоростью `speed` (со знаком): тело
    /// разгоняется к ней тем медленнее, чем оно тяжелее персонажа массой `pusher`.
    pub fn push(&mut self, id: BodyId, speed: f64, pusher: f64, dt: f64) {
        let Some(b) = self.bodies.get_mut(id.0) else {
            return;
        };
        if !b.is_dynamic() {
            return;
        }
        let mut vel = b.linvel();
        if let Some(x) = push_speed(f64::from(vel.x), speed, pusher, f64::from(b.mass()), dt) {
            vel.x = x as f32;
            b.set_linvel(vel, true);
        }
    }

    /// Касания с прошлого вызова (9.6): кто кого, где и с какой силой.
    pub fn take_touches(&mut self) -> Vec<Touch> {
        std::mem::take(&mut self.touches)
    }

    /// Тело спит: стоит и не считается, пока его не тронут.
    pub fn is_sleeping(&self, id: BodyId) -> bool {
        self.bodies.get(id.0).is_some_and(|b| b.is_sleeping())
    }

    /// Масса тела.
    pub fn mass(&self, id: BodyId) -> Option<f64> {
        self.bodies.get(id.0).map(|b| f64::from(b.mass()))
    }

    /// Кто это: тело физики или кинематическое тело уровня, героя, врага.
    fn toucher(&self, c: ColliderHandle) -> Option<(Toucher, Material, RigidBodyHandle)> {
        let col = self.colliders.get(c)?;
        let body = col.parent()?;
        let material = Material::from_bits(col.user_data);
        if self.last.contains_key(&body) {
            return Some((Toucher::Body(BodyId(body)), material, body));
        }
        let key = self.mover_keys.get(&body)?;
        Some((Toucher::Solid(*key), material, body))
    }

    /// События шага Rapier — в касания: сила удара по скоростям до шага.
    fn collect_touches(&mut self) {
        let raw = match self.collector.0.lock() {
            Ok(mut list) => std::mem::take(&mut *list),
            Err(_) => return,
        };
        for r in raw {
            let (Some((a, ma, ha)), Some((b, mb, hb))) = (self.toucher(r.c1), self.toucher(r.c2))
            else {
                continue;
            };
            let (at, normal) = r.contact.unwrap_or_else(|| {
                let at = self
                    .bodies
                    .get(ha)
                    .map(|b| b.translation())
                    .unwrap_or(Vector::ZERO);
                (at, Vector::ZERO)
            });
            let vel = |h: RigidBodyHandle| {
                self.vel_before
                    .get(&h)
                    .map(|&(v, w, com)| touch::point_velocity(v, w, com, at))
                    .unwrap_or(Vector::ZERO)
            };
            let (va, vb) = (vel(ha), vel(hb));
            let toward = [
                f64::from(va.dot(normal)).max(0.0),
                f64::from(-vb.dot(normal)).max(0.0),
            ];
            let speed = f64::from((va - vb).dot(normal)).max(0.0);
            if self.touches.len() < MAX_TOUCHES {
                self.touches.push(Touch {
                    a,
                    b,
                    begin: r.begin,
                    at: p(at),
                    normal: p(normal),
                    speed: if r.begin { speed } else { 0.0 },
                    toward: if r.begin { toward } else { [0.0, 0.0] },
                    materials: [ma, mb],
                });
            }
        }
    }

    /// Тяжесть мира, единиц в секунду за секунду (вниз).
    pub fn gravity(&self) -> f64 {
        -f64::from(self.gravity.y)
    }

    /// Персонаж массой `mass` стоит на теле в точке `at` и давит на него (9.4): своим
    /// весом за шаг `dt` и, если только что упал со скоростью `fall`, ударом. Качели
    /// наклоняются под героем, ящик на желе проминает его.
    pub fn press(&mut self, id: BodyId, at: [f64; 2], mass: f64, fall: f64, dt: f64) {
        let g = self.gravity();
        let Some(b) = self.bodies.get_mut(id.0) else {
            return;
        };
        if !b.is_dynamic() {
            return;
        }
        let m = f64::from(b.mass()).max(1e-6);
        // Удар неупругий: тело получает долю скорости упавшего.
        let impulse = mass * g * dt + fall.max(0.0) * mass * m / (mass + m);
        b.apply_impulse_at_point(Vector::new(0.0, -impulse as f32), v(at), true);
    }
}

/// Новая скорость вбок толкаемого массой `mass` (тело или частицы мягкого тела) при
/// скорости `cur`, если персонаж массой `pusher` хочет идти со скоростью `speed`; `None` —
/// оно уже идёт не медленнее. Лёгкое идёт почти со скоростью персонажа, тяжёлое —
/// медленно.
fn push_speed(cur: f64, speed: f64, pusher: f64, mass: f64, dt: f64) -> Option<f64> {
    let mass = mass.max(1e-6);
    let target = speed * (pusher / (pusher + mass)).clamp(0.05, 0.95) * 1.6;
    let target = target.clamp(-speed.abs(), speed.abs());
    let accel = 12.0 * pusher / (pusher + mass);
    let dv = (target - cur).clamp(-accel * dt, accel * dt);
    ((target > 0.0 && cur < target) || (target < 0.0 && cur > target)).then_some(cur + dv)
}

/// Та же форма: тот же объект или прямоугольник того же размера (у героя и врагов
/// форма собирается заново каждый шаг).
fn same_shape(a: &SharedShape, b: &SharedShape) -> bool {
    if std::sync::Arc::ptr_eq(&a.0, &b.0) {
        return true;
    }
    if let (Some(x), Some(y)) = (a.as_cuboid(), b.as_cuboid()) {
        return (x.half_extents - y.half_extents).length() < 1e-7;
    }
    // Склоны тайлов собираются заново каждый шаг (`Surface::of_collider`).
    if let (Some(x), Some(y)) = (a.as_convex_polygon(), b.as_convex_polygon()) {
        return x.points().len() == y.points().len()
            && x.points()
                .iter()
                .zip(y.points())
                .all(|(p, q)| (*p - *q).length() < 1e-7);
    }
    false
}

/// Группы Rapier из слоя и маски (9.6а): сталкиваются, если каждый в маске другого.
fn groups(layer: u32, mask: u32) -> InteractionGroups {
    InteractionGroups::new(
        Group::from_bits_retain(layer),
        Group::from_bits_retain(mask),
        InteractionTestMode::And,
    )
}

fn v(a: [f64; 2]) -> Vector {
    Vector::new(a[0] as f32, a[1] as f32)
}

fn p(a: Vector) -> [f64; 2] {
    [f64::from(a.x), f64::from(a.y)]
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_soft;

#[cfg(test)]
mod tests_hands;
