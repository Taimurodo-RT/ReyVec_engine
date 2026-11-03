//! Физика твёрдых тел на `rapier2d` (6.10): ящики, мяч, обломки разбитых врагов.
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

use rapier2d::prelude::*;

use crate::{Aabb, Collider};

/// Масштаб длины для допусков `rapier`: типичный размер тела в единицах сцены.
pub const LENGTH_UNIT: f32 = 0.2;

/// Тело из файла сцены.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyDesc {
    /// Ширина и высота (у круглого — диаметр по ширине).
    pub size: [f64; 2],
    pub round: bool,
    /// Плотность: тяжелее — труднее толкнуть.
    pub density: f64,
    /// Упругость: 0 — не отскакивает, 1 — отскакивает без потерь.
    pub bounce: f64,
    pub friction: f64,
}

impl Default for BodyDesc {
    fn default() -> Self {
        Self {
            size: [0.15, 0.15],
            round: false,
            density: 1.0,
            bounce: 0.2,
            friction: 0.6,
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
        }
    }

    /// Новое тело в `at` с поворотом `angle` (градусы) и скоростью `vel`.
    pub fn add(&mut self, at: [f64; 2], angle: f64, vel: [f64; 2], d: &BodyDesc) -> BodyId {
        let body = RigidBodyBuilder::dynamic()
            .translation(v(at))
            .rotation(angle.to_radians() as f32)
            .linvel(v(vel))
            // Быстрые обломки не пролетают сквозь тонкие стены.
            .ccd_enabled(true)
            .build();
        let h = self.bodies.insert(body);
        let shape = if d.round {
            ColliderBuilder::ball((d.size[0] * 0.5) as f32)
        } else {
            ColliderBuilder::cuboid((d.size[0] * 0.5) as f32, (d.size[1] * 0.5) as f32)
        };
        let c = shape
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
        self.bodies.remove(
            id.0,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.soft_bodies,
            true,
        );
    }

    /// Сколько тел (без уровня и героя).
    pub fn len(&self) -> usize {
        self.last.len()
    }

    pub fn is_empty(&self) -> bool {
        self.last.is_empty()
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
mod tests {
    use super::*;

    const DT: f64 = 1.0 / 60.0;

    fn floor() -> (u64, Collider) {
        (
            1,
            Collider::solid(Aabb {
                min: [-5.0, -1.0],
                max: [5.0, -0.8],
            }),
        )
    }

    #[test]
    fn box_falls_lands_and_rests_on_the_floor() {
        let mut w = Physics::new(9.3);
        let b = w.add([0.0, 0.0], 0.0, [0.0, 0.0], &BodyDesc::default());
        for _ in 0..180 {
            w.set_solids([floor()]);
            w.step(DT);
        }
        let (at, angle) = w.pose(b).unwrap();
        assert!((at[1] - (-0.8 + 0.075)).abs() < 0.01, "{at:?}");
        assert!(angle.abs() < 1.0 && w.velocity(b).unwrap()[1].abs() < 0.01);
    }

    #[test]
    fn hero_pushes_a_box_and_can_stand_on_it() {
        let mut w = Physics::new(9.3);
        let b = w.add([0.3, -0.725], 0.0, [0.0, 0.0], &BodyDesc::default());
        let hero_half = [0.07, 0.14];
        // Герой идёт вправо и упирается в ящик: ящик едет.
        let mut x = 0.0;
        for _ in 0..90 {
            x += 1.0 * DT;
            let hero = Collider::solid(Aabb::around([x, -0.66], hero_half));
            w.set_solids([floor(), (2, hero)]);
            w.step(DT);
        }
        let (at, _) = w.pose(b).unwrap();
        assert!(
            at[0] > x + 0.07 + 0.07,
            "ящик впереди героя: {at:?}, герой {x}"
        );
        assert!(at[0] > 0.6, "ящик сдвинулся: {at:?}");
        // Для героя ящик — платформа сверху.
        let plat = w.platforms(&[b], 100);
        assert!(
            plat[0].oneway && (plat[0].aabb.max[1] - -0.65).abs() < 0.01,
            "{plat:?}"
        );
        // Героя больше нет — его тело убрано, ящик стоит.
        w.set_solids([floor()]);
        w.step(DT);
        assert_eq!(w.movers.len(), 1);
    }

    #[test]
    fn kicked_debris_flies_spins_and_does_not_pass_walls() {
        let mut w = Physics::new(9.3);
        let d = BodyDesc {
            size: [0.04, 0.04],
            ..BodyDesc::default()
        };
        let wall = (
            3,
            Collider::solid(Aabb {
                min: [1.0, -0.8],
                max: [1.02, 1.0],
            }),
        );
        let bits: Vec<BodyId> = (0..10)
            .map(|i| w.add([0.0, -0.5 + 0.05 * i as f64], 0.0, [0.0, 0.0], &d))
            .collect();
        for (i, b) in bits.iter().enumerate() {
            w.kick(*b, [12.0, 1.0], 20.0 + i as f64);
        }
        for _ in 0..120 {
            w.set_solids([floor(), wall]);
            w.step(DT);
        }
        for b in &bits {
            let (at, _) = w.pose(*b).unwrap();
            assert!(at[0] < 1.0 && at[1] > -0.81, "сквозь стену или пол: {at:?}");
        }
        w.remove(bits[0]);
        assert_eq!(w.len(), 9);
    }

    #[test]
    fn slope_rolls_a_ball_down() {
        let mut w = Physics::new(9.3);
        let ball = w.add(
            [0.3, 0.0],
            0.0,
            [0.0, 0.0],
            &BodyDesc {
                size: [0.1, 0.1],
                round: true,
                ..BodyDesc::default()
            },
        );
        let slope = Collider::slope(
            Aabb {
                min: [0.0, -0.8],
                max: [0.5, -0.3],
            },
            0.5,
        );
        for _ in 0..90 {
            w.set_solids([floor(), (4, slope)]);
            w.step(DT);
        }
        let (at, _) = w.pose(ball).unwrap();
        assert!(at[0] < 0.0, "мяч скатился влево: {at:?}");
    }
}
