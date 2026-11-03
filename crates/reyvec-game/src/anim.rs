//! Анимация героя из костей (6.8).
//!
//! Герой собран из частей-костей ([`Bone`]): тело, голова, руки, ноги, плащ. Каждая кость
//! поворачивается вокруг своей точки крепления (плечо, бедро, шея) и чуть сдвигается.
//! Поза ([`Pose`]) — поворот и сдвиг каждой кости. Позы считаются по состоянию тела
//! (стоит, бежит, прыгает, падает, рывок, стена, край) и удара: это «клипы» ([`Clip`]),
//! между ними поза переходит плавно, а удар накладывается поверх (бежит и бьёт — ноги
//! бегут, руки бьют).
//!
//! События анимации: шаг ноги — в тот миг, когда нога ставится на землю. Фаза бега
//! идёт от пройденного пути, а не от времени, поэтому ноги не скользят по полу: быстрее
//! бежит — чаще шаги. Шаг даёт звук и пыль. Удар берёт фазы удара ([`Fighter::progress`]),
//! поэтому взмах руки и хитбокс совпадают по времени.
//!
//! Углы в градусах против часовой стрелки, для героя, который смотрит вправо (влево он
//! отражается целиком). Рука и нога висят вниз при угле 0; больше 0 — вперёд.
//!
//! [`Fighter::progress`]: crate::Fighter::progress

use crate::{AttackTuning, Body, Fighter, Tuning};

/// Кость героя. В файле сцены — предмет внутри героя с `bone: "имя"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bone {
    Body,
    Head,
    ArmFront,
    ArmBack,
    LegFront,
    LegBack,
    Cape,
}

impl Bone {
    pub const ALL: [Bone; 7] = [
        Bone::Body,
        Bone::Head,
        Bone::ArmFront,
        Bone::ArmBack,
        Bone::LegFront,
        Bone::LegBack,
        Bone::Cape,
    ];

    /// Имя в файле сцены.
    pub fn name(self) -> &'static str {
        match self {
            Bone::Body => "тело",
            Bone::Head => "голова",
            Bone::ArmFront => "рука",
            Bone::ArmBack => "рука_сзади",
            Bone::LegFront => "нога",
            Bone::LegBack => "нога_сзади",
            Bone::Cape => "плащ",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.name() == name)
    }

    /// Все имена через запятую (для сообщения об ошибке).
    pub fn names() -> String {
        Self::ALL.map(Bone::name).join(", ")
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Поворот (градусы) и сдвиг (единицы сцены) кости от её места в файле.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BonePose {
    pub angle: f64,
    pub offset: [f64; 2],
}

/// Поза: поворот и сдвиг каждой кости.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose([BonePose; 7]);

impl Pose {
    pub fn bone(&self, b: Bone) -> BonePose {
        self.0[b.index()]
    }

    fn set(&mut self, b: Bone, angle: f64) -> &mut Self {
        self.0[b.index()].angle = angle;
        self
    }

    fn shift(&mut self, b: Bone, offset: [f64; 2]) -> &mut Self {
        let o = &mut self.0[b.index()].offset;
        o[0] += offset[0];
        o[1] += offset[1];
        self
    }

    /// Поза между `self` (k = 0) и `to` (k = 1).
    pub fn lerp(&self, to: &Pose, k: f64) -> Pose {
        let mut out = *self;
        for (o, t) in out.0.iter_mut().zip(&to.0) {
            o.angle += (t.angle - o.angle) * k;
            o.offset[0] += (t.offset[0] - o.offset[0]) * k;
            o.offset[1] += (t.offset[1] - o.offset[1]) * k;
        }
        out
    }
}

/// Что делает герой: от этого зависит поза.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clip {
    Idle,
    Run,
    Jump,
    Fall,
    Dash,
    Slide,
    Hang,
    Climb,
}

impl Clip {
    /// Как назвать на панели.
    pub fn label(self) -> &'static str {
        match self {
            Clip::Idle => "стоит",
            Clip::Run => "бежит",
            Clip::Jump => "прыгает",
            Clip::Fall => "падает",
            Clip::Dash => "рывок",
            Clip::Slide => "скользит по стене",
            Clip::Hang => "висит",
            Clip::Climb => "забирается",
        }
    }
}

/// Сколько пути на полный шаг двумя ногами (два удара ногой о землю).
pub const STRIDE: f64 = 0.32;
/// Сколько длится переход между позами, секунды.
const BLEND: f64 = 0.12;
/// Сколько длится «присед» после приземления.
const LAND: f64 = 0.12;

/// Что случилось в анимации за шаг.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnimEvents {
    /// Нога встала на землю (звук шага и пыль).
    pub step: bool,
}

/// Состояние тела и удара, от которого считается поза.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnimInput {
    pub on_ground: bool,
    pub vel: [f64; 2],
    /// Наибольшая скорость бега (для размаха ног).
    pub run_speed: f64,
    pub wall: bool,
    pub dashing: bool,
    pub hanging: bool,
    pub climbing: bool,
    /// Удар: 0–1 замах, 1–2 удар, 2–3 возврат ([`Fighter::progress`]).
    pub attack: Option<f64>,
    /// Только что приземлился.
    pub landed: bool,
}

impl AnimInput {
    pub fn of(body: &Body, tuning: &Tuning, fighter: &Fighter, attack: &AttackTuning) -> Self {
        Self {
            on_ground: body.on_ground,
            vel: body.vel,
            run_speed: tuning.speed,
            wall: body.wall != 0.0,
            dashing: body.dashing,
            hanging: body.on_ledge() && !body.climbing(),
            climbing: body.climbing(),
            attack: fighter.progress(attack),
            landed: false,
        }
    }

    fn clip(&self) -> Clip {
        if self.climbing {
            Clip::Climb
        } else if self.hanging {
            Clip::Hang
        } else if self.dashing {
            Clip::Dash
        } else if !self.on_ground && self.wall {
            Clip::Slide
        } else if !self.on_ground {
            if self.vel[1] > 0.0 {
                Clip::Jump
            } else {
                Clip::Fall
            }
        } else if self.vel[0].abs() > 0.15 {
            Clip::Run
        } else {
            Clip::Idle
        }
    }
}

/// Анимация одного героя: какой клип, переход, фаза бега.
#[derive(Clone, Debug, PartialEq)]
pub struct Animator {
    clip: Clip,
    /// Время в клипе, секунды.
    time: f64,
    /// Фаза бега: целая часть — сколько полных шагов, дробная — где ноги.
    phase: f64,
    /// Поза, из которой идёт переход, и сколько он уже идёт.
    from: Pose,
    blend: f64,
    land_left: f64,
    pose: Pose,
    attacking: bool,
}

impl Default for Animator {
    fn default() -> Self {
        Self {
            clip: Clip::Idle,
            time: 0.0,
            phase: 0.0,
            from: Pose::default(),
            blend: BLEND,
            land_left: 0.0,
            pose: idle(0.0),
            attacking: false,
        }
    }
}

impl Animator {
    /// Текущая поза.
    pub fn pose(&self) -> &Pose {
        &self.pose
    }

    pub fn clip(&self) -> Clip {
        self.clip
    }

    /// Бьёт ли герой (поза удара наложена).
    pub fn attacking(&self) -> bool {
        self.attacking
    }

    /// Шаг анимации длиной `dt`.
    pub fn step(&mut self, input: &AnimInput, dt: f64) -> AnimEvents {
        let mut events = AnimEvents::default();
        let clip = input.clip();
        if clip != self.clip {
            self.from = self.pose;
            self.blend = 0.0;
            self.clip = clip;
            self.time = 0.0;
            if clip == Clip::Run {
                // Первый шаг — почти сразу после начала бега.
                self.phase = 0.15;
            }
        }
        self.time += dt;
        self.blend = (self.blend + dt).min(BLEND);
        if input.landed {
            self.land_left = LAND;
        }
        self.land_left = (self.land_left - dt).max(0.0);

        let k = (input.vel[0].abs() / input.run_speed.max(1e-6)).min(1.0);
        if clip == Clip::Run {
            let before = self.phase;
            self.phase += input.vel[0].abs() * dt / STRIDE;
            // Нога ставится на землю на четверти и трёх четвертях шага.
            let contacts = |p: f64| ((p - 0.25) * 2.0).floor();
            events.step = contacts(self.phase) > contacts(before);
        }
        let mut target = match clip {
            Clip::Idle => idle(self.time),
            Clip::Run => run(self.phase, k),
            Clip::Jump => jump(),
            Clip::Fall => fall(self.time),
            Clip::Dash => dash(),
            Clip::Slide => slide(),
            Clip::Hang => hang(self.time),
            Clip::Climb => climb(),
        };
        if self.land_left > 0.0 {
            let s = self.land_left / LAND;
            target
                .shift(Bone::Body, [0.0, -0.014 * s])
                .shift(Bone::Head, [0.0, -0.014 * s]);
        }
        let mut pose = self.from.lerp(&target, smooth(self.blend / BLEND));
        self.attacking = input.attack.is_some();
        if let Some(p) = input.attack {
            pose = attack(&pose, p);
        }
        self.pose = pose;
        events
    }
}

fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn lerp(a: f64, b: f64, k: f64) -> f64 {
    a + (b - a) * k
}

fn idle(t: f64) -> Pose {
    let breathe = (std::f64::consts::TAU * t / 2.4).sin();
    let mut p = Pose::default();
    p.set(Bone::Head, 2.0 * breathe)
        .set(Bone::ArmFront, 5.0 + 3.0 * breathe)
        .set(Bone::ArmBack, -5.0 - 2.0 * breathe)
        .set(Bone::Cape, -6.0 - 3.0 * breathe)
        .shift(Bone::Body, [0.0, 0.002 * breathe])
        .shift(Bone::Head, [0.0, 0.003 * breathe]);
    p
}

fn run(phase: f64, k: f64) -> Pose {
    let a = std::f64::consts::TAU * phase;
    let (s, c) = (a.sin(), a.cos());
    let bounce = 0.007 * k * c.abs();
    let mut p = Pose::default();
    p.set(Bone::LegFront, 40.0 * k * s)
        .set(Bone::LegBack, -40.0 * k * s)
        .set(Bone::ArmFront, -34.0 * k * s)
        .set(Bone::ArmBack, 34.0 * k * s)
        .set(Bone::Body, -9.0 * k)
        .set(Bone::Head, 4.0 * k)
        .set(Bone::Cape, -(15.0 + 40.0 * k) + 6.0 * s)
        .shift(Bone::Body, [0.0, bounce])
        .shift(Bone::Head, [0.0, bounce])
        .shift(Bone::ArmFront, [0.0, bounce])
        .shift(Bone::ArmBack, [0.0, bounce])
        .shift(Bone::Cape, [0.0, bounce]);
    p
}

fn jump() -> Pose {
    let mut p = Pose::default();
    p.set(Bone::LegFront, 40.0)
        .set(Bone::LegBack, -18.0)
        .set(Bone::ArmFront, 140.0)
        .set(Bone::ArmBack, 115.0)
        .set(Bone::Body, 4.0)
        .set(Bone::Cape, -12.0);
    p
}

fn fall(t: f64) -> Pose {
    let flail = 12.0 * (t * 14.0).sin();
    let mut p = Pose::default();
    p.set(Bone::LegFront, 12.0)
        .set(Bone::LegBack, -25.0)
        .set(Bone::ArmFront, 125.0 + flail)
        .set(Bone::ArmBack, 150.0 - flail)
        .set(Bone::Head, 6.0)
        .set(Bone::Cape, -115.0);
    p
}

fn dash() -> Pose {
    let mut p = Pose::default();
    p.set(Bone::LegFront, 55.0)
        .set(Bone::LegBack, -45.0)
        .set(Bone::ArmFront, -70.0)
        .set(Bone::ArmBack, -85.0)
        .set(Bone::Body, -22.0)
        .set(Bone::Head, 8.0)
        .set(Bone::Cape, -95.0);
    p
}

fn slide() -> Pose {
    let mut p = Pose::default();
    p.set(Bone::LegFront, 30.0)
        .set(Bone::LegBack, 10.0)
        .set(Bone::ArmFront, 150.0)
        .set(Bone::ArmBack, 100.0)
        .set(Bone::Body, 6.0)
        .set(Bone::Cape, -30.0);
    p
}

fn hang(t: f64) -> Pose {
    let sway = 8.0 * (t * 3.0).sin();
    let mut p = Pose::default();
    p.set(Bone::ArmFront, 175.0)
        .set(Bone::ArmBack, 168.0)
        .set(Bone::LegFront, sway)
        .set(Bone::LegBack, -sway)
        .set(Bone::Cape, -5.0)
        .shift(Bone::Body, [0.0, -0.01]);
    p
}

fn climb() -> Pose {
    let mut p = Pose::default();
    p.set(Bone::ArmFront, 120.0)
        .set(Bone::ArmBack, 100.0)
        .set(Bone::LegFront, 70.0)
        .set(Bone::LegBack, -10.0)
        .set(Bone::Body, -15.0)
        .set(Bone::Cape, -40.0);
    p
}

/// Удар поверх позы `base`: замах рукой за голову, взмах вперёд-вниз, возврат.
fn attack(base: &Pose, p: f64) -> Pose {
    // Рука (градусы): поднята назад над головой → вперёд и вниз.
    const UP: f64 = 205.0;
    const DOWN: f64 = 45.0;
    let (arm, back, lean) = if p < 1.0 {
        let k = smooth(p);
        (
            lerp(base.bone(Bone::ArmFront).angle, UP, k),
            lerp(base.bone(Bone::ArmBack).angle, -35.0, k),
            lerp(base.bone(Bone::Body).angle, 7.0, k),
        )
    } else if p < 2.0 {
        // Взмах быстрый: рука проходит дугу за первую треть удара и опережает серп
        // взмаха, хитбокс в это время уже бьёт.
        let k = 1.0 - (1.0 - ((p - 1.0) / 0.35).min(1.0)).powi(3);
        (lerp(UP, DOWN, k), lerp(-35.0, 40.0, k), lerp(7.0, -14.0, k))
    } else {
        let k = smooth(p - 2.0);
        (
            lerp(DOWN, base.bone(Bone::ArmFront).angle, k),
            lerp(40.0, base.bone(Bone::ArmBack).angle, k),
            lerp(-14.0, base.bone(Bone::Body).angle, k),
        )
    };
    let mut out = *base;
    out.set(Bone::ArmFront, arm)
        .set(Bone::ArmBack, back)
        .set(Bone::Body, lean);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1.0 / 60.0;

    fn running(vx: f64) -> AnimInput {
        AnimInput {
            on_ground: true,
            vel: [vx, 0.0],
            run_speed: 1.6,
            ..AnimInput::default()
        }
    }

    #[test]
    fn steps_come_from_distance_not_time() {
        for vx in [0.8, 1.6] {
            let mut a = Animator::default();
            let steps = (0..120).filter(|_| a.step(&running(vx), DT).step).count() as f64;
            // За 2 с пройдено 2·vx, на шаг — половина STRIDE.
            let expected = 2.0 * vx / (STRIDE * 0.5);
            assert!(
                (steps - expected).abs() <= 1.0,
                "{vx}: {steps} / {expected}"
            );
            assert_eq!(a.clip(), Clip::Run);
        }
        let mut a = Animator::default();
        assert!(
            (0..120).all(|_| !a.step(&running(0.0), DT).step),
            "стоит — без шагов"
        );
    }

    #[test]
    fn legs_swing_opposite_and_wider_when_faster() {
        let swing = |vx: f64| {
            let mut a = Animator::default();
            let mut most: f64 = 0.0;
            for _ in 0..90 {
                a.step(&running(vx), DT);
                let p = a.pose();
                assert!(
                    (p.bone(Bone::LegFront).angle + p.bone(Bone::LegBack).angle).abs() < 1e-6
                        || a.blend < BLEND
                );
                most = most.max(p.bone(Bone::LegFront).angle.abs());
            }
            most
        };
        assert!(swing(1.6) > 35.0);
        assert!(swing(0.5) < swing(1.6) * 0.5);
    }

    #[test]
    fn attack_raises_then_swings_the_arm_and_keeps_legs_running() {
        let mut a = Animator::default();
        let t = AttackTuning::default();
        let mut input = running(1.6);
        for _ in 0..30 {
            a.step(&input, DT);
        }
        input.attack = Some(0.999);
        a.step(&input, DT);
        assert!(
            a.pose().bone(Bone::ArmFront).angle > 180.0,
            "замах за голову"
        );
        input.attack = Some(1.5);
        a.step(&input, DT);
        assert!(a.pose().bone(Bone::ArmFront).angle < 90.0, "взмах вперёд");
        assert!(
            a.pose().bone(Bone::LegFront).angle.abs() > 1.0,
            "ноги бегут"
        );
        assert!(a.attacking());
        // Фазы удара настоящего бойца.
        let mut f = Fighter::default();
        f.step(&t, true, true, 1.0, DT);
        let p = f.progress(&t).expect("удар начат");
        assert!((0.0..1.0).contains(&p));
        for _ in 0..3 {
            f.step(&t, false, true, 1.0, DT);
        }
        let p = f.progress(&t).unwrap();
        assert!((1.0..2.0).contains(&p) && f.striking(), "{p}");
    }

    #[test]
    fn switching_clips_blends_without_jumps() {
        let mut a = Animator::default();
        let states = [
            running(1.6),
            AnimInput {
                vel: [1.0, 2.0],
                run_speed: 1.6,
                ..AnimInput::default()
            },
            AnimInput {
                vel: [1.0, -2.0],
                run_speed: 1.6,
                ..AnimInput::default()
            },
            AnimInput {
                dashing: true,
                run_speed: 1.6,
                ..AnimInput::default()
            },
            AnimInput {
                hanging: true,
                run_speed: 1.6,
                ..AnimInput::default()
            },
            AnimInput {
                on_ground: true,
                landed: true,
                run_speed: 1.6,
                ..AnimInput::default()
            },
        ];
        let mut last = *a.pose();
        for s in &states {
            for i in 0..20 {
                let mut s = *s;
                s.landed &= i == 0;
                a.step(&s, DT);
                for b in Bone::ALL {
                    let d = (a.pose().bone(b).angle - last.bone(b).angle).abs();
                    // Резкие смены (падение → рывок, руки через голову) — до 60° за кадр.
                    assert!(d < 60.0, "{b:?} скачок {d}° в {:?}", a.clip());
                }
                last = *a.pose();
            }
        }
    }

    #[test]
    fn bone_names_round_trip() {
        for b in Bone::ALL {
            assert_eq!(Bone::parse(b.name()), Some(b));
        }
        assert_eq!(Bone::parse("хвост"), None);
        assert!(Bone::names().contains("рука_сзади"));
    }
}
