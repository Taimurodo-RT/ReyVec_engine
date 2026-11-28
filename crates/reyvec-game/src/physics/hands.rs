//! Взять и бросить (9.6в): руки персонажа.
//!
//! Персонаж нажимает «взять» рядом с телом. Лёгкое (не тяжелее его самого с запасом)
//! он поднимает над головой и несёт; второе нажатие бросает его вперёд, со стрелкой
//! вниз — кладёт перед собой. Тяжёлое он берёт за край и тянет волоком, сам идёт
//! медленнее; второе нажатие или прыжок отпускает. Совсем неподъёмное не берётся.
//!
//! Несомое не сталкивается с несущим (из маски тела убирается его слой), иначе ящик
//! над головой толкал бы героя, а герой стоял бы на нём. Брошенное ещё немного
//! пропускает бросившего, чтобы не задеть его на вылете. Тело движется скоростью, а
//! не переносом: несомое не проходит сквозь стены и толкает соседей.

use super::*;

/// Лёгкое — не тяжелее столько масс персонажа: его несут над головой.
pub const CARRY_MASS: f64 = 1.25;
/// Тяжёлое, но не тяжелее столько масс персонажа, тянут волоком.
pub const DRAG_MASS: f64 = 10.0;
/// Как далеко от края персонажа достают руки, единиц.
const REACH: f64 = 0.1;
/// Скорость броска вперёд и вверх, единиц в секунду; к ней — половина своей скорости.
/// Чуть вниз: брошенное из-над головы попадает во врага ростом с героя, а не
/// пролетает над ним; падает на пол шагах в четырёх.
const THROW: [f64; 2] = [5.0, -0.3];
/// Сколько брошенное ещё не сталкивается с бросившим, с.
const THROW_PASS: f64 = 0.3;
/// Несомое дальше этого от рук (застряло) падает.
const LOST: f64 = 0.35;
/// Быстрее этого несомое к рукам не летит, единиц в секунду.
const HOLD_SPEED: f32 = 8.0;

/// Персонаж с руками: где он, куда смотрит, как идёт, его слой столкновений и масса.
#[derive(Clone, Copy, Debug)]
pub struct Holder {
    /// Середина и половины размера.
    pub pos: [f64; 2],
    pub half: [f64; 2],
    /// 1 — вправо, −1 — влево.
    pub facing: f64,
    pub vel: [f64; 2],
    pub layer: u32,
    pub mass: f64,
}

impl Holder {
    fn bounds(&self) -> Aabb {
        Aabb {
            min: [self.pos[0] - self.half[0], self.pos[1] - self.half[1]],
            max: [self.pos[0] + self.half[0], self.pos[1] + self.half[1]],
        }
    }
}

/// Как держит.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hold {
    /// Над головой.
    Carry,
    /// Волоком: с какой стороны тело (знак) и на каком расстоянии от середины
    /// персонажа его взяли; на какой высоте от ног было его дно.
    Drag { side: f64, gap: f64, floor: f64 },
}

/// Что сделали руки за нажатие.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HandsEvent {
    /// Подняли (`Carry`) или взялись тянуть (`Drag`).
    Took { body: BodyId, hold: Hold },
    /// Бросили со скоростью.
    Threw { body: BodyId, vel: [f64; 2] },
    /// Положили или отпустили.
    Let { body: BodyId },
    /// Рядом только неподъёмное.
    TooHeavy { body: BodyId },
}

/// Руки одного персонажа.
#[derive(Clone, Debug, Default)]
pub struct Hands {
    /// Что держат, как, и какие биты слоя персонажа убраны из маски тела.
    held: Option<(BodyId, Hold, u32)>,
    /// Брошенные, которые ещё пропускают бросившего: тело, биты, сколько осталось.
    thrown: Vec<(BodyId, u32, f64)>,
}

impl Hands {
    /// Что держат.
    pub fn held(&self) -> Option<(BodyId, Hold)> {
        self.held.map(|(b, h, _)| (b, h))
    }

    /// Брошенные, которые ещё летят от бросившего.
    pub fn flying(&self) -> impl Iterator<Item = BodyId> + '_ {
        self.thrown.iter().map(|&(b, _, _)| b)
    }

    /// Нажали «взять»: взять ближайшее тело перед собой, бросить или положить несомое,
    /// отпустить тянущееся. `down` — со стрелкой вниз (положить, а не бросить).
    pub fn press(&mut self, w: &mut Physics, who: &Holder, down: bool) -> Option<HandsEvent> {
        if let Some((body, hold, bits)) = self.held.take() {
            let event = match hold {
                Hold::Carry if !down => {
                    let vel = [
                        who.facing * THROW[0] + who.vel[0] * 0.5,
                        THROW[1] + who.vel[1] * 0.5,
                    ];
                    w.set_velocity(body, vel, -who.facing * 4.0);
                    HandsEvent::Threw { body, vel }
                }
                Hold::Carry => {
                    w.set_velocity(body, [who.facing * 1.2 + who.vel[0], 0.4], 0.0);
                    HandsEvent::Let { body }
                }
                Hold::Drag { .. } => HandsEvent::Let { body },
            };
            if matches!(hold, Hold::Carry) {
                self.thrown.push((body, bits, THROW_PASS));
            } else {
                w.restore_mask(body, bits);
            }
            return Some(event);
        }
        let body = w.reachable(who)?;
        let mass = w.mass(body)?;
        let hold = if mass <= who.mass * CARRY_MASS {
            Hold::Carry
        } else if mass <= who.mass * DRAG_MASS {
            let b = w.bounds(body)?;
            let dx = b.center()[0] - who.pos[0];
            Hold::Drag {
                side: dx.signum(),
                gap: dx.abs(),
                floor: b.min[1] - (who.pos[1] - who.half[1]),
            }
        } else {
            return Some(HandsEvent::TooHeavy { body });
        };
        // Уже брошенное снова пропускает так же: биты не теряются.
        let mut bits = 0;
        self.thrown.retain(|&(b, k, _)| {
            if b == body {
                bits |= k;
            }
            b != body
        });
        if hold == Hold::Carry {
            bits |= w.ignore_layer(body, who.layer);
        }
        self.held = Some((body, hold, bits));
        Some(HandsEvent::Took { body, hold })
    }

    /// Шаг рук перед шагом физики: несомое — к рукам, тянущееся — за персонажем.
    /// Застрявшее несомое падает, тянущееся отпускается, когда персонаж прыгнул или
    /// ушёл слишком далеко.
    pub fn step(&mut self, w: &mut Physics, who: &Holder, dt: f64) -> Option<HandsEvent> {
        self.thrown.retain_mut(|(b, bits, left)| {
            *left -= dt;
            if *left > 0.0 && w.bounds(*b).is_some() {
                return true;
            }
            // Ещё внутри бросившего — подождём, иначе его вытолкнет.
            if *left > -1.0 && w.bounds(*b).is_some_and(|x| x.overlaps(&who.bounds())) {
                return true;
            }
            w.restore_mask(*b, *bits);
            false
        });
        let (body, hold, bits) = self.held?;
        let Some(b) = w.bounds(body) else {
            self.held = None;
            return None;
        };
        let lost = match hold {
            Hold::Carry => {
                let h = (b.max[1] - b.min[1]) * 0.5;
                let to = [
                    who.pos[0] + who.facing * who.half[0] * 0.3,
                    who.pos[1] + who.half[1] + h + 0.01,
                ];
                let c = b.center();
                let off = ((c[0] - to[0]).powi(2) + (c[1] - to[1]).powi(2)).sqrt();
                // Сразу после подъёма тело ещё летит к рукам.
                w.hold(body, to, dt);
                off > LOST + 2.0 * h && off > LOST * 2.0
            }
            Hold::Drag { side, gap, floor } => {
                let dx = (b.center()[0] - who.pos[0]) * side;
                let rise = (b.min[1] - (who.pos[1] - who.half[1])) - floor;
                // Персонаж уходит от тела — тело за ним, вдогонку до прежнего расстояния.
                if dx > gap + 0.005 {
                    let catch = ((dx - gap) / dt.max(1e-3)).min(2.0);
                    w.pull(body, who.vel[0] - side * catch * 0.5);
                }
                dx > gap + LOST || dx < 0.0 || rise.abs() > 0.15
            }
        };
        if lost {
            self.held = None;
            if hold == Hold::Carry {
                self.thrown.push((body, bits, 0.0));
            } else {
                w.restore_mask(body, bits);
            }
            return Some(HandsEvent::Let { body });
        }
        None
    }

    /// Во сколько раз медленнее идёт персонаж: тянуть тяжёлое тяжело (тело в десять
    /// раз тяжелее — втрое медленнее).
    pub fn slow(&self, w: &Physics, who_mass: f64) -> f64 {
        match self.held {
            Some((body, Hold::Drag { .. }, _)) => {
                let m = w.mass(body).unwrap_or(who_mass);
                (who_mass * 2.0 / m).sqrt().clamp(0.32, 1.0)
            }
            _ => 1.0,
        }
    }

    /// Всё отпустить (персонаж исчез или разбит).
    pub fn drop_all(&mut self, w: &mut Physics) {
        if let Some((body, _, bits)) = self.held.take() {
            w.restore_mask(body, bits);
        }
        for (b, bits, _) in self.thrown.drain(..) {
            w.restore_mask(b, bits);
        }
    }
}

impl Physics {
    /// Ближайшее подвижное тело, до которого достают руки: перед персонажем, на высоте
    /// от ног до макушки (то, на чём он стоит, не в счёт).
    fn reachable(&self, who: &Holder) -> Option<BodyId> {
        let me = who.bounds();
        let front = Aabb {
            min: [me.min[0] - REACH * 0.4, me.min[1] + 0.015],
            max: [me.max[0] + REACH * 0.4, me.max[1]],
        };
        let ahead = if who.facing >= 0.0 {
            Aabb {
                max: [me.max[0] + REACH, me.max[1]],
                ..front
            }
        } else {
            Aabb {
                min: [me.min[0] - REACH, front.min[1]],
                ..front
            }
        };
        let mut best: Option<(f64, BodyId)> = None;
        for h in self.last.keys() {
            let id = BodyId(*h);
            let Some(b) = self.bounds(id) else {
                continue;
            };
            if !b.overlaps(&ahead) || !self.bodies.get(*h).is_some_and(|b| b.is_dynamic()) {
                continue;
            }
            // Сзади — только вплотную; спереди — ближе к рукам лучше.
            let c = b.center();
            let d = (c[0] - who.pos[0]) * who.facing.signum();
            let score = d.abs() + if d < 0.0 { 1.0 } else { 0.0 };
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Убрать слой `layer` из маски тела: оно перестаёт сталкиваться с этим слоем.
    /// Вернёт, какие биты убраны.
    fn ignore_layer(&mut self, id: BodyId, layer: u32) -> u32 {
        let mut removed = 0;
        let Some(b) = self.bodies.get(id.0) else {
            return 0;
        };
        for &c in b.colliders() {
            if let Some(col) = self.colliders.get_mut(c) {
                let mut g = col.collision_groups();
                removed |= g.filter.bits() & layer;
                g.filter = Group::from_bits_retain(g.filter.bits() & !layer);
                col.set_collision_groups(g);
            }
        }
        removed
    }

    /// Вернуть в маску тела биты, убранные [`Self::ignore_layer`].
    fn restore_mask(&mut self, id: BodyId, bits: u32) {
        if bits == 0 {
            return;
        }
        let Some(b) = self.bodies.get(id.0) else {
            return;
        };
        for &c in b.colliders() {
            if let Some(col) = self.colliders.get_mut(c) {
                let mut g = col.collision_groups();
                g.filter = Group::from_bits_retain(g.filter.bits() | bits);
                col.set_collision_groups(g);
            }
        }
    }

    /// Задать скорость и вращение тела (градусы не нужны: радианы в секунду).
    pub fn set_velocity(&mut self, id: BodyId, vel: [f64; 2], spin: f64) {
        if let Some(b) = self.bodies.get_mut(id.0) {
            b.set_linvel(v(vel), true);
            b.set_angvel(spin as f32, true);
        }
    }

    /// Держать тело у точки `to` (несут в руках): скорость середины — к точке за шаг,
    /// с поправкой на тяжесть этого шага; поворот выпрямляется.
    pub fn hold(&mut self, id: BodyId, to: [f64; 2], dt: f64) {
        let g = self.gravity;
        let Some(b) = self.bodies.get_mut(id.0) else {
            return;
        };
        let dt = dt.max(1e-3) as f32;
        let at = b.center_of_mass();
        let want = ((v(to) - at) / dt).clamp_length_max(HOLD_SPEED) - g * dt;
        b.set_linvel(want, true);
        let angle = b.rotation().angle();
        b.set_angvel((-angle / dt * 0.3).clamp(-12.0, 12.0), true);
    }

    /// Тянуть тело вбок со скоростью `speed` (волоком): вертикальная скорость своя.
    pub fn pull(&mut self, id: BodyId, speed: f64) {
        if let Some(b) = self.bodies.get_mut(id.0) {
            let mut vel = b.linvel();
            vel.x = speed as f32;
            b.set_linvel(vel, true);
        }
    }
}
