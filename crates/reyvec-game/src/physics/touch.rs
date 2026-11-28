//! Касания и материалы (9.6).
//!
//! Rapier сообщает, когда два тела начали и перестали касаться. Мы добавляем к этому
//! силу удара: с какой скоростью они сближались по направлению касания до шага (после
//! шага решатель уже погасил удар). Из силы удара сцена делает звук, искры и урон.
//!
//! Материал тела — это его трение, упругость и плотность по умолчанию и то, как оно
//! звучит и что из него летит при ударе.

use std::sync::Mutex;

use rapier2d::prelude::*;

use super::BodyId;

/// Материал тела: что звучит и что летит при ударе; трение, упругость и плотность по
/// умолчанию.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Material {
    /// Дерево: глухой стук, щепки. Свойства — как у тела без материала.
    #[default]
    Wood,
    /// Металл: звон, искры. Тяжёлый, скользкий.
    Metal,
    /// Камень: тяжёлый удар, пыль и редкие искры. Не отскакивает.
    Stone,
    /// Резина: упругий шлепок, ничего не летит. Сильно отскакивает и не скользит.
    Rubber,
}

impl Material {
    pub const ALL: [Material; 4] = [
        Material::Wood,
        Material::Metal,
        Material::Stone,
        Material::Rubber,
    ];

    /// Плотность по умолчанию (дерево — 1).
    pub fn density(self) -> f64 {
        match self {
            Material::Wood => 1.0,
            Material::Metal => 4.0,
            Material::Stone => 2.5,
            Material::Rubber => 0.9,
        }
    }

    /// Упругость по умолчанию: 0 — не отскакивает, 1 — без потерь.
    pub fn bounce(self) -> f64 {
        match self {
            Material::Wood => 0.2,
            Material::Metal => 0.15,
            Material::Stone => 0.05,
            Material::Rubber => 0.8,
        }
    }

    /// Трение по умолчанию.
    pub fn friction(self) -> f64 {
        match self {
            Material::Wood => 0.6,
            Material::Metal => 0.35,
            Material::Stone => 0.8,
            Material::Rubber => 1.2,
        }
    }

    /// Насколько звонкий: удар двух тел звучит материалом звонче (металл о камень —
    /// звон, а не стук).
    pub fn ring(self) -> u8 {
        match self {
            Material::Rubber => 0,
            Material::Wood => 1,
            Material::Stone => 2,
            Material::Metal => 3,
        }
    }

    pub(super) fn to_bits(self) -> u128 {
        self as u128
    }

    pub(super) fn from_bits(b: u128) -> Self {
        Self::ALL
            .get((b & 0xff) as usize)
            .copied()
            .unwrap_or_default()
    }
}

/// Кто касается: тело физики или твёрдое уровня, герой, враг (номер из
/// [`super::Physics::set_surfaces`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Toucher {
    Body(BodyId),
    Solid(u64),
}

/// Касание двух тел (9.6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Touch {
    pub a: Toucher,
    pub b: Toucher,
    /// Начали касаться (`true`) или перестали.
    pub begin: bool,
    /// Место касания и направление от `a` к `b` (у конца касания — середина `a` и 0).
    pub at: [f64; 2],
    pub normal: [f64; 2],
    /// С какой скоростью сближались по направлению касания до шага, единиц в секунду:
    /// сила удара.
    pub speed: f64,
    /// Своя скорость каждого к другому (`a` к `b`, `b` к `a`): кто в кого влетел. Ящик,
    /// упавший на врага, бьёт его; враг, который сам вбежал в лежащий ящик, — нет.
    pub toward: [f64; 2],
    pub materials: [Material; 2],
}

impl Touch {
    /// Материал звонче из двух.
    pub fn louder(&self) -> Material {
        let [a, b] = self.materials;
        if a.ring() >= b.ring() { a } else { b }
    }

    /// То же касание, где `who` — первый (`None` — его тут нет).
    pub fn seen_by(&self, who: Toucher) -> Option<Touch> {
        if self.a == who {
            Some(*self)
        } else if self.b == who {
            Some(Touch {
                a: self.b,
                b: self.a,
                normal: [-self.normal[0], -self.normal[1]],
                toward: [self.toward[1], self.toward[0]],
                materials: [self.materials[1], self.materials[0]],
                ..*self
            })
        } else {
            None
        }
    }
}

/// Сырое событие Rapier: пара, началось ли, место и направление касания (в мире).
pub(super) struct Raw {
    pub c1: ColliderHandle,
    pub c2: ColliderHandle,
    pub begin: bool,
    pub contact: Option<(Vector, Vector)>,
}

/// Собирает события касаний во время шага Rapier.
#[derive(Default)]
pub(super) struct Collector(pub Mutex<Vec<Raw>>);

impl EventHandler for Collector {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        colliders: &ColliderSet,
        event: CollisionEvent,
        pair: Option<&ContactPair>,
    ) {
        if event.sensor() {
            return;
        }
        let (c1, c2) = (event.collider1(), event.collider2());
        // Место касания: самая глубокая точка пары (в координатах первого), направление —
        // общее у её группы точек.
        let contact = pair.and_then(|pair| {
            let (m, c) = pair.find_deepest_contact()?;
            let first = colliders.get(pair.collider1)?;
            let at = first.position().transform_point(c.local_p1);
            // Пара могла сохраниться в обратном порядке.
            let flip = pair.collider1 != c1;
            let n = if flip { -m.data.normal } else { m.data.normal };
            Some((at, n))
        });
        if let Ok(mut list) = self.0.lock() {
            list.push(Raw {
                c1,
                c2,
                begin: event.started(),
                contact,
            });
        }
    }

    fn handle_contact_force_event(
        &self,
        _dt: Real,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _pair: &ContactPair,
        _force: Real,
    ) {
    }

    fn handle_soft_body_tear_event(&self, _soft: &SoftBodySet, _event: &SoftBodyTearEvent) {}
}

/// Скорость точки `at` тела со скоростью `v`, вращением `w` и серединой масс `com`.
pub(super) fn point_velocity(v: Vector, w: f32, com: Vector, at: Vector) -> Vector {
    let r = at - com;
    v + Vector::new(-w * r.y, w * r.x)
}
