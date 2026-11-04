//! Сочленения: шарнир, сварка, ползун, верёвка, пружина. Наши надстройки над Rapier:
//! мотор, который сам меняет направление, и разрыв от силы (в Rapier такого нет:
//! он только сообщает, с какой силой держит сочленение).

use rapier2d::prelude::*;

use super::{BodyId, Physics, p, v};

/// Как быстро мотор набирает свою скорость (больше — быстрее; предел — его сила).
const MOTOR_GAIN: f32 = 40.0;

/// Вид сочленения. Углы — в градусах, расстояния — в единицах сцены.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JointKind {
    /// Шарнир: тела вращаются вокруг общей точки. `limits` — насколько можно
    /// повернуть второе тело относительно первого (от и до, градусы).
    Hinge { limits: Option<[f64; 2]> },
    /// Сварка: тела держатся друг за друга намертво.
    Weld,
    /// Ползун: второе тело скользит вдоль `axis` (направление в мире в момент создания),
    /// `limits` — от и до вдоль оси.
    Slider {
        axis: [f64; 2],
        limits: Option<[f64; 2]>,
    },
    /// Верёвка: точки не дальше `length` друг от друга, ближе — можно.
    Rope { length: f64 },
    /// Пружина длиной `length`: `stiffness` — жёсткость, `damping` — гашение качания.
    Spring {
        length: f64,
        stiffness: f64,
        damping: f64,
    },
}

/// Мотор шарнира или ползуна: `speed` — скорость (градусов в секунду у шарнира, единиц
/// в секунду у ползуна), `force` — наибольшая сила. `flip` — через сколько секунд мотор
/// меняет направление (0 — никогда).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motor {
    pub speed: f64,
    pub force: f64,
    pub flip: f64,
}

/// Сочленение из файла сцены. Точки — в координатах сцены в момент создания.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointDesc {
    pub kind: JointKind,
    /// Точка крепления к первому телу.
    pub at: [f64; 2],
    /// Точка крепления ко второму телу (у верёвки и пружины); без неё — та же `at`.
    pub to_at: Option<[f64; 2]>,
    pub motor: Option<Motor>,
    /// Сила, при которой сочленение рвётся (без неё — не рвётся).
    pub breaks: Option<f64>,
    /// Касаются ли друг друга соединённые тела (обычно нет: звенья цепи перекрываются).
    pub collide: bool,
}

impl JointDesc {
    pub fn new(kind: JointKind, at: [f64; 2]) -> Self {
        Self {
            kind,
            at,
            to_at: None,
            motor: None,
            breaks: None,
            collide: false,
        }
    }
}

/// Номер сочленения.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JointId(ImpulseJointHandle);

/// Наша надстройка над одним сочленением.
pub(super) struct Extra {
    handle: ImpulseJointHandle,
    motor: Option<Motor>,
    /// Шарнир (мотор вращает) или ползун (мотор двигает).
    angular: bool,
    breaks: Option<f64>,
    /// Направление мотора сейчас: 1 или −1.
    sign: f64,
}

impl Physics {
    /// Соединяет тело `a` с телом `b` или, без `b`, с неподвижным миром.
    pub fn join(&mut self, a: BodyId, b: Option<BodyId>, d: &JointDesc) -> Option<JointId> {
        let b = match b {
            Some(b) => b.0,
            None => self.ground_body(),
        };
        // `pa` — опора (первое тело Rapier), `pb` — само тело.
        let (pa, pb) = (
            *self.bodies.get(b)?.position(),
            *self.bodies.get(a.0)?.position(),
        );
        let at = v(d.at);
        let to = v(d.to_at.unwrap_or(d.at));
        let (la, lb) = (
            pa.inverse_transform_point(to),
            pb.inverse_transform_point(at),
        );
        let soft = |m: Option<Motor>| m.map(|m| (m.speed, m.force));
        let mut data: GenericJoint = match d.kind {
            JointKind::Hinge { limits } => {
                let mut j = RevoluteJointBuilder::new()
                    .local_anchor1(la)
                    .local_anchor2(lb)
                    .contacts_enabled(d.collide);
                if let Some([lo, hi]) = limits {
                    j = j.limits([lo.to_radians() as f32, hi.to_radians() as f32]);
                }
                if let Some((speed, force)) = soft(d.motor) {
                    j = j
                        .motor_velocity(speed.to_radians() as f32, MOTOR_GAIN)
                        .motor_max_force(force as f32);
                }
                j.build().into()
            }
            JointKind::Weld => {
                let frame1 = Pose::from_translation(la);
                let frame2 = pb.inverse() * pa * frame1;
                FixedJointBuilder::new()
                    .local_frame1(frame1)
                    .local_frame2(frame2)
                    .contacts_enabled(d.collide)
                    .build()
                    .into()
            }
            JointKind::Slider { axis, limits } => {
                let world_axis = v(axis).normalize_or(Vector::X);
                let axis1 = pa.inverse_transform_vector(world_axis);
                let axis2 = pb.inverse_transform_vector(world_axis);
                let mut j = PrismaticJointBuilder::new(axis1)
                    .local_axis2(axis2)
                    .local_anchor1(la)
                    .local_anchor2(lb)
                    .contacts_enabled(d.collide);
                if let Some([lo, hi]) = limits {
                    j = j.limits([lo as f32, hi as f32]);
                }
                if let Some((speed, force)) = soft(d.motor) {
                    j = j
                        .motor_velocity(speed as f32, MOTOR_GAIN)
                        .motor_max_force(force as f32);
                }
                j.build().into()
            }
            JointKind::Rope { length } => RopeJointBuilder::new(length as f32)
                .local_anchor1(la)
                .local_anchor2(lb)
                .contacts_enabled(d.collide)
                .build()
                .into(),
            JointKind::Spring {
                length,
                stiffness,
                damping,
            } => SpringJointBuilder::new(length as f32, stiffness as f32, damping as f32)
                .local_anchor1(la)
                .local_anchor2(lb)
                .contacts_enabled(d.collide)
                .build()
                .into(),
        };
        data.set_contacts_enabled(d.collide);
        // Первое тело Rapier — то, к чему крепят (`b` или мир), второе — само тело `a`:
        // так мотор и пределы описывают поворот тела относительно опоры.
        let handle = self.impulse_joints.insert(b, a.0, data, true);
        if d.motor.is_some() || d.breaks.is_some() {
            self.joint_extras.push(Extra {
                handle,
                motor: d.motor,
                angular: matches!(d.kind, JointKind::Hinge { .. }),
                breaks: d.breaks,
                sign: 1.0,
            });
        }
        Some(JointId(handle))
    }

    /// Неподвижное тело в начале координат для сочленений «к миру».
    fn ground_body(&mut self) -> RigidBodyHandle {
        if let Some(g) = self.ground {
            return g;
        }
        let g = self.bodies.insert(RigidBodyBuilder::fixed().build());
        self.ground = Some(g);
        g
    }

    /// Сочленение ещё держит (не порвалось и тела на месте).
    pub fn joint_alive(&self, id: JointId) -> bool {
        self.impulse_joints.get(id.0).is_some()
    }

    /// Точки крепления сочленения в сцене (для рисования верёвок и пружин).
    pub fn joint_ends(&self, id: JointId) -> Option<([f64; 2], [f64; 2])> {
        let j = self.impulse_joints.get(id.0)?;
        let a = self.bodies.get(j.body1())?.position();
        let b = self.bodies.get(j.body2())?.position();
        Some((
            p(a.transform_point(j.data.local_anchor1())),
            p(b.transform_point(j.data.local_anchor2())),
        ))
    }

    /// Моторы туда-обратно: меняют направление каждые `flip` секунд.
    pub(super) fn drive_motors(&mut self) {
        let time = self.time;
        for e in &mut self.joint_extras {
            let Some(m) = e.motor else {
                continue;
            };
            if m.flip <= 0.0 {
                continue;
            }
            let sign = if (time / m.flip).floor() as i64 % 2 == 0 {
                1.0
            } else {
                -1.0
            };
            if sign == e.sign {
                continue;
            }
            e.sign = sign;
            let Some(j) = self.impulse_joints.get_mut(e.handle, true) else {
                continue;
            };
            let speed = if e.angular {
                m.speed.to_radians()
            } else {
                m.speed
            } * sign;
            let axis = if e.angular {
                JointAxis::AngX
            } else {
                JointAxis::LinX
            };
            j.data.set_motor_velocity(axis, speed as f32, MOTOR_GAIN);
        }
    }

    /// Рвёт сочленения, которые держали силой больше своей `breaks`.
    pub(super) fn check_breaks(&mut self, dt: f32) {
        let mut gone = Vec::new();
        self.joint_extras.retain(|e| {
            let Some(j) = self.impulse_joints.get(e.handle) else {
                // Тело убрали, сочленения больше нет.
                return false;
            };
            let Some(limit) = e.breaks else {
                return true;
            };
            let force = f64::from(j.impulses.length() / dt.max(1e-6));
            if force > limit {
                gone.push(e.handle);
                return false;
            }
            true
        });
        for h in gone {
            if let Some((a, _)) = self.joint_ends(JointId(h)) {
                self.breaks.push(a);
            }
            self.impulse_joints.remove(h, true);
            self.broken += 1;
        }
    }
}
