//! Проверки 9.4: герой и мягкие тела в одном мире столкновений.

use super::*;
use crate::{Body, Intent, Space, Tuning};

const DT: f64 = 1.0 / 60.0;
const FLOOR: f64 = -0.8;
const SOFT_IDS: u64 = 1 << 40;
const BODY_IDS: u64 = 1 << 30;
const HERO_MASS: f64 = 0.04;

fn floor() -> Collider {
    Collider {
        id: 1,
        ..Collider::solid(Aabb {
            min: [-5.0, -1.0],
            max: [5.0, FLOOR],
        })
    }
}

/// Мир как у сцены: физика с полом и героем, герой видит пол и мягкие тела.
struct World {
    w: Physics,
    hero: Body,
    t: Tuning,
    /// Скорость падения перед приземлением.
    fall: f64,
    landed: bool,
    /// Тела, на которые можно встать.
    bodies: Vec<BodyId>,
}

impl World {
    fn new(w: Physics, at: [f64; 2]) -> Self {
        Self {
            w,
            hero: Body::new(at),
            t: Tuning::default(),
            fall: 0.0,
            landed: false,
            bodies: Vec::new(),
        }
    }

    fn feet(&self) -> f64 {
        self.hero.pos[1] - self.t.half()[1]
    }

    fn frame(&mut self, intent: Intent) {
        let mut hero = Surface::of_collider(&Collider::solid(self.hero.bounds(&self.t)));
        hero.id = 2;
        self.w.set_surfaces([Surface::of_collider(&floor()), hero]);
        let fall = if self.landed { self.fall } else { 0.0 };
        let foot = self.feet();
        let ground = self.hero.ground().filter(|_| self.hero.on_ground);
        if let Some(i) = ground.and_then(|g| g.checked_sub(BODY_IDS))
            && let Some(&b) = self.bodies.get(i as usize)
        {
            self.w
                .press(b, [self.hero.pos[0], foot], HERO_MASS, fall, DT);
        }
        if let Some(key) = ground.and_then(|g| g.checked_sub(SOFT_IDS)) {
            let h = self.t.half();
            let strip = Aabb {
                min: [self.hero.pos[0] - h[0], foot - 0.005],
                max: [self.hero.pos[0] + h[0], foot + 0.005],
            };
            assert!(self.w.press_soft(key, strip, HERO_MASS, fall, DT));
        }
        self.w.step(DT);
        let mut space = Space::of(&[floor()]);
        space.extend(self.w.soft_surfaces(SOFT_IDS));
        space.extend(self.w.surfaces(&self.bodies, BODY_IDS));
        self.fall = -self.hero.vel[1];
        let ev = self.hero.step(&self.t, intent, &space, DT);
        self.landed = ev.landed;
        if let Some((id, speed)) = ev.pushed {
            self.w.push_soft(id - SOFT_IDS, speed, HERO_MASS, DT);
        }
    }

    fn run(&mut self, frames: usize, intent: Intent) {
        for _ in 0..frames {
            self.frame(intent);
        }
    }
}

fn jelly(stiffness: f64) -> Physics {
    let mut w = Physics::new(9.3);
    w.add_soft(&SoftDesc {
        stiffness,
        damping: 0.4,
        mass: 0.04,
        ..SoftDesc::new(SoftKind::Jelly {
            center: [0.0, FLOOR + 0.13],
            radius: 0.13,
            count: 16,
        })
    });
    w
}

/// Верх желе под героем: самая высокая частица под его ногами.
fn jelly_top(w: &Physics, x: f64, half: f64) -> f64 {
    w.soft_views()[0]
        .points
        .iter()
        .filter(|q| (q[0] - x).abs() < half)
        .map(|q| q[1])
        .fold(f64::MIN, f64::max)
}

#[test]
fn hero_stands_on_a_jelly_and_soft_jelly_sinks_deeper() {
    let mut sink = Vec::new();
    for stiffness in [6.0, 12.0] {
        let mut world = World::new(jelly(stiffness), [0.0, 0.6]);
        world.run(240, Intent::default());
        let top = jelly_top(&world.w, world.hero.pos[0], world.t.half()[0]);
        let feet = world.feet();
        assert!(world.hero.on_ground, "стоит на желе ({stiffness})");
        assert!(
            world.hero.ground().is_some_and(|g| g >= SOFT_IDS),
            "опора — желе ({stiffness})"
        );
        assert!(
            feet > FLOOR + 0.05,
            "не провалился до пола ({stiffness}): ноги {feet}"
        );
        assert!(
            feet - top > 0.0 && feet - top < 0.03,
            "ноги на верху желе ({stiffness}): ноги {feet}, верх {top}"
        );
        sink.push(FLOOR + 0.26 + 0.01 - feet);
    }
    assert!(
        sink[0] > sink[1] + 0.02,
        "мягкое желе проминается глубже: {sink:?}"
    );
}

#[test]
fn hero_sinks_into_a_hammock() {
    let mut w = Physics::new(9.3);
    let net = w.add_soft(&SoftDesc {
        stiffness: 40.0,
        damping: 0.5,
        mass: 0.08,
        pin: Pin::Sides,
        tear: Some(0.6),
        ..SoftDesc::new(SoftKind::Net {
            center: [0.0, 0.0],
            size: [1.0, 0.12],
            cols: 18,
            rows: 3,
        })
    });
    let mut world = World::new(w, [0.0, 0.6]);
    world.run(240, Intent::default());
    let feet = world.feet();
    let sag = world.w.soft_center(net).unwrap()[1];
    assert!(world.hero.on_ground, "стоит на гамаке");
    assert!(feet < 0.06 - 0.03, "гамак прогнулся: ноги {feet}");
    assert!(
        feet > FLOOR + 0.1,
        "не провалился сквозь гамак: ноги {feet}"
    );
    assert_eq!(world.w.soft_pieces(net), 1, "гамак цел");
    // Сходит с гамака вправо: гамак выпрямляется.
    let walk = Intent {
        x: 1.0,
        ..Intent::default()
    };
    world.run(120, walk);
    let mid = world.w.soft_center(net).unwrap();
    assert!(
        mid[1] > sag + 0.02,
        "гамак без героя выпрямился: {mid:?}, был {sag}"
    );
}

#[test]
fn hero_pushes_a_hanging_rope_aside() {
    let mut w = Physics::new(9.3);
    let rope = w.add_soft(&SoftDesc {
        stiffness: 40.0,
        pin: Pin::Start,
        ..SoftDesc::new(SoftKind::Rope {
            from: [0.4, 0.5],
            to: [0.4, FLOOR + 0.1],
            count: 24,
        })
    });
    let mut world = World::new(w, [0.0, FLOOR + 0.14 + 1e-4]);
    world.run(30, Intent::default());
    let walk = Intent {
        x: 1.0,
        ..Intent::default()
    };
    let mut most = 0.0f64;
    for _ in 0..60 {
        world.frame(walk);
        most = most.max(world.w.soft_center(rope).unwrap()[0]);
    }
    assert!(most > 0.45, "верёвку отодвинуло: середина {most}");
    assert!(
        world.hero.pos[0] > 0.4,
        "герой прошёл мимо верёвки: {:?}",
        world.hero.pos
    );
}

#[test]
fn seesaw_tips_under_the_hero() {
    let mut w = Physics::new(9.3);
    let plank = w.add(
        [0.0, 0.0],
        0.0,
        [0.0, 0.0],
        &BodyDesc {
            size: [1.0, 0.04],
            ..BodyDesc::default()
        },
    );
    w.join(
        plank,
        None,
        &JointDesc::new(
            JointKind::Hinge {
                limits: Some([-25.0, 25.0]),
            },
            [0.0, 0.0],
        ),
    )
    .unwrap();
    let mut world = World::new(w, [0.35, 0.3]);
    world.bodies.push(plank);
    world.run(120, Intent::default());
    let (_, angle) = world.w.pose(plank).unwrap();
    assert!(
        angle < -15.0,
        "качели наклонились под героем вправо: {angle}"
    );
    assert!(world.hero.on_ground, "герой стоит на качелях");
}
