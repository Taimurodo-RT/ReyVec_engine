//! Бой (6.5): удар героя, хитбоксы, манекены, стоп-кадр и тряска.
//!
//! Удар идёт по фазам: замах (`windup`), удар (`active`, только в нём хитбокс бьёт) и
//! возврат (`recover`, новый удар нельзя). Нажатие во время возврата запоминается и
//! начинает следующий удар сразу, как только можно. Одна цель получает от одного
//! удара только одно попадание.
//!
//! Манекен ([`Dummy`]) — тело с тем же контроллером, что у героя, но без управления:
//! от удара его отбрасывает, он падает, скользит по полу и упирается в стены. Когда
//! здоровье кончилось, он пропадает и через время появляется на своём месте снова.

use crate::Aabb;
use crate::controller::{Body, Collider, Intent, Tuning};

/// Настройки удара.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackTuning {
    /// Ширина и высота хитбокса: он стоит вплотную перед героем, посередине по высоте.
    pub reach: [f64; 2],
    /// Замах, удар и возврат, секунды.
    pub windup: f64,
    pub active: f64,
    pub recover: f64,
    /// Урон одного попадания.
    pub damage: u32,
    /// Скорость, с которой отбрасывает цель: вбок (от героя) и вверх.
    pub knockback: [f64; 2],
    /// Стоп-кадр при попадании: на столько секунд замирают герой и манекены.
    pub hit_stop: f64,
    /// Размах тряски камеры при попадании (единиц сцены) и сколько она длится.
    pub shake: f64,
    pub shake_time: f64,
}

impl Default for AttackTuning {
    fn default() -> Self {
        Self {
            reach: [0.2, 0.18],
            windup: 0.04,
            active: 0.1,
            recover: 0.14,
            damage: 1,
            knockback: [1.2, 1.0],
            hit_stop: 0.07,
            shake: 0.025,
            shake_time: 0.2,
        }
    }
}

/// Сколько помнится нажатие удара, пока удар нельзя начать.
const ATTACK_BUFFER: f64 = 0.12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Ready,
    Windup,
    Active,
    Recover,
}

/// Удар героя: фаза, направление и кого этот удар уже задел.
#[derive(Clone, Debug, PartialEq)]
pub struct Fighter {
    phase: Phase,
    /// Сколько осталось до конца фазы.
    left: f64,
    /// Куда бьёт (1 — вправо, -1 — влево): берётся в начале удара.
    dir: f64,
    /// Кого задел этот удар (номера целей).
    hit: Vec<u64>,
    queued: f64,
    /// Сколько ударов начато.
    pub swings: u32,
}

impl Default for Fighter {
    fn default() -> Self {
        Self {
            phase: Phase::Ready,
            left: 0.0,
            dir: 1.0,
            hit: Vec::new(),
            queued: 0.0,
            swings: 0,
        }
    }
}

impl Fighter {
    /// Шаг длиной `dt`: `pressed` — удар нажат, `can` — можно бить (не висит на краю),
    /// `facing` — куда смотрит герой. Возвращает `true`, если удар начался.
    pub fn step(
        &mut self,
        t: &AttackTuning,
        pressed: bool,
        can: bool,
        facing: f64,
        dt: f64,
    ) -> bool {
        if pressed {
            self.queued = ATTACK_BUFFER;
        }
        if self.phase != Phase::Ready {
            self.left -= dt;
            while self.left <= 0.0 && self.phase != Phase::Ready {
                let (next, time) = match self.phase {
                    Phase::Windup => (Phase::Active, t.active),
                    Phase::Active => (Phase::Recover, t.recover),
                    _ => (Phase::Ready, 0.0),
                };
                self.phase = next;
                self.left += time;
            }
        }
        if !can && self.phase != Phase::Ready {
            // Схватился за край посреди удара: удар прерывается.
            self.phase = Phase::Ready;
        }
        let started = self.phase == Phase::Ready && can && self.queued > 0.0;
        if started {
            self.phase = Phase::Windup;
            self.left = t.windup.max(1e-9);
            self.dir = if facing < 0.0 { -1.0 } else { 1.0 };
            self.hit.clear();
            self.queued = 0.0;
            self.swings += 1;
        }
        self.queued = (self.queued - dt).max(0.0);
        started
    }

    /// Хитбокс в координатах сцены, пока идёт фаза удара (иначе `None`). `body` и
    /// `half` — место и половины размера героя.
    pub fn hitbox(&self, t: &AttackTuning, pos: [f64; 2], half: [f64; 2]) -> Option<Aabb> {
        (self.phase == Phase::Active).then(|| {
            Aabb::around(
                [pos[0] + self.dir * (half[0] + t.reach[0] * 0.5), pos[1]],
                [t.reach[0] * 0.5, t.reach[1] * 0.5],
            )
        })
    }

    /// Цель `id` задета этим ударом впервые (повторно — `false`).
    pub fn first_hit(&mut self, id: u64) -> bool {
        if self.hit.contains(&id) {
            return false;
        }
        self.hit.push(id);
        true
    }

    /// Куда направлен текущий удар.
    pub fn dir(&self) -> f64 {
        self.dir
    }

    /// Идёт ли удар (любая фаза, кроме готовности).
    pub fn busy(&self) -> bool {
        self.phase != Phase::Ready
    }

    /// Идёт фаза удара (хитбокс бьёт; в ней видно взмах).
    pub fn striking(&self) -> bool {
        self.phase == Phase::Active
    }
}

/// Манекен: тело без управления со здоровьем.
#[derive(Clone, Debug, PartialEq)]
pub struct Dummy {
    pub body: Body,
    pub tuning: Tuning,
    pub hp: u32,
    pub max_hp: u32,
    /// Где появляется.
    pub spawn: [f64; 2],
    /// Сколько ещё светится белым после попадания, секунды.
    pub flash: f64,
    /// Разбит: сколько секунд до появления снова (0 — цел).
    pub broken: f64,
    /// Через сколько секунд разбитый появляется снова.
    pub respawn: f64,
    /// На сколько сдвинулся за последний шаг (для толчка героя).
    pub moved: [f64; 2],
}

/// Сколько светится манекен после попадания.
pub const FLASH_TIME: f64 = 0.12;

impl Dummy {
    /// Манекен размером `size` со здоровьем `hp` стоит в `spawn`.
    pub fn new(spawn: [f64; 2], size: [f64; 2], hp: u32, respawn: f64) -> Self {
        let tuning = Tuning {
            size,
            // Отброшенный скользит по полу и быстро встаёт; в воздухе почти не тормозит.
            decel: 7.0,
            air_accel: 1.0,
            // Без кнопки прыжка контроллер тянет вниз сильнее («низкий прыжок»); у
            // манекена кнопки нет, поэтому тяжесть обычная.
            fall: 1.0,
            dash_speed: 0.0,
            dash_time: 0.0,
            wall_jump: 0.0,
            ledge: false,
            ..Tuning::default()
        };
        Self {
            body: Body::new(spawn),
            tuning,
            hp: hp.max(1),
            max_hp: hp.max(1),
            spawn,
            flash: 0.0,
            broken: 0.0,
            respawn,
            moved: [0.0, 0.0],
        }
    }

    /// Цел (виден, его можно бить, о него можно опереться).
    pub fn alive(&self) -> bool {
        self.broken <= 0.0
    }

    pub fn bounds(&self) -> Aabb {
        self.body.bounds(&self.tuning)
    }

    /// Попадание: урон и отброс со скоростью `push`. `true` — манекен разбит.
    pub fn hurt(&mut self, damage: u32, push: [f64; 2]) -> bool {
        self.flash = FLASH_TIME;
        self.body.vel = push;
        if push[1] > 0.0 {
            self.body.on_ground = false;
        }
        self.hp = self.hp.saturating_sub(damage);
        if self.hp == 0 {
            self.broken = self.respawn.max(1e-3);
            return true;
        }
        false
    }

    /// Твёрдое тело манекена для героя: на него можно встать, сквозь него не пройти.
    pub fn collider(&self, id: u64) -> Collider {
        Collider {
            id,
            moved: self.moved,
            ..Collider::solid(self.bounds())
        }
    }

    /// Шаг тела длиной `dt` среди `solids` (без героя).
    pub fn step(&mut self, solids: &[Collider], dt: f64) {
        let before = self.body.pos;
        if self.alive() {
            self.body.step(&self.tuning, Intent::default(), solids, dt);
        }
        self.moved = [self.body.pos[0] - before[0], self.body.pos[1] - before[1]];
    }

    /// Таймеры, которые идут и во время стоп-кадра: свечение и появление снова.
    /// `blocked` — место появления занято (там герой): ждать, пока освободится.
    pub fn tick(&mut self, dt: f64, blocked: impl FnOnce(Aabb) -> bool) {
        self.flash = (self.flash - dt).max(0.0);
        if self.broken > 0.0 {
            self.broken -= dt;
            if self.broken <= 0.0 {
                let at = Aabb::around(self.spawn, self.tuning.half());
                if blocked(at) {
                    self.broken = 0.1;
                } else {
                    self.broken = 0.0;
                    self.body = Body::new(self.spawn);
                    self.hp = self.max_hp;
                    self.moved = [0.0, 0.0];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor() -> Vec<Collider> {
        vec![
            Collider::solid(Aabb {
                min: [-10.0, -1.0],
                max: [10.0, 0.0],
            }),
            Collider::solid(Aabb {
                min: [2.0, 0.0],
                max: [2.2, 2.0],
            }),
        ]
    }

    const DT: f64 = 1.0 / 60.0;

    #[test]
    fn swing_goes_through_phases_and_hits_only_in_strike() {
        let t = AttackTuning::default();
        let mut f = Fighter::default();
        let (pos, half) = ([0.0, 0.14], [0.07, 0.14]);
        assert!(f.step(&t, true, true, 1.0, DT));
        assert!(f.hitbox(&t, pos, half).is_none(), "в замахе не бьёт");
        let mut struck = 0;
        let mut steps = 0;
        while f.busy() {
            f.step(&t, false, true, -1.0, DT);
            if let Some(h) = f.hitbox(&t, pos, half) {
                struck += 1;
                // Повернулся во время удара — бьёт туда, куда начал.
                assert!(h.min[0] >= half[0] - 1e-9, "хитбокс справа: {h:?}");
            }
            steps += 1;
            assert!(steps < 60);
        }
        let expect = (t.active / DT).round() as i32;
        assert!(
            (struck - expect).abs() <= 1,
            "шагов удара {struck}, ждали {expect}"
        );
        assert_eq!(f.swings, 1);
    }

    #[test]
    fn press_during_recover_starts_next_swing() {
        let t = AttackTuning::default();
        let mut f = Fighter::default();
        f.step(&t, true, true, 1.0, DT);
        while !f.striking() {
            f.step(&t, false, true, 1.0, DT);
        }
        while f.striking() {
            f.step(&t, false, true, 1.0, DT);
        }
        // Возврат: нажатие запомнится.
        f.step(&t, true, true, 1.0, DT);
        for _ in 0..30 {
            f.step(&t, false, true, 1.0, DT);
        }
        assert_eq!(f.swings, 2, "второй удар начался после возврата");
    }

    #[test]
    fn one_swing_hits_a_target_once() {
        let mut f = Fighter::default();
        f.step(&AttackTuning::default(), true, true, 1.0, DT);
        assert!(f.first_hit(7));
        assert!(!f.first_hit(7));
        assert!(f.first_hit(8));
    }

    #[test]
    fn hanging_cancels_and_blocks_the_swing() {
        let t = AttackTuning::default();
        let mut f = Fighter::default();
        assert!(!f.step(&t, true, false, 1.0, DT));
        f.step(&t, false, true, 1.0, DT);
        assert!(
            f.busy(),
            "нажатие запомнилось и удар начался, когда стало можно"
        );
        f.step(&t, false, false, 1.0, DT);
        assert!(!f.busy());
    }

    #[test]
    fn dummy_flies_back_lands_and_stops_at_the_wall() {
        let mut d = Dummy::new([1.0, 0.15], [0.16, 0.3], 5, 2.0);
        let solids = floor();
        for _ in 0..30 {
            d.step(&solids, DT);
        }
        assert!(d.body.on_ground);
        let y0 = d.body.pos[1];
        d.hurt(1, [3.0, 1.2]);
        let mut top = y0;
        let mut landed = false;
        for _ in 0..120 {
            d.step(&solids, DT);
            top = top.max(d.body.pos[1]);
            let b = d.bounds();
            assert!(b.max[0] <= 2.0 + 1e-9, "манекен прошёл стену: {b:?}");
            assert!(b.min[1] >= -1e-9, "манекен в полу: {b:?}");
            landed |= d.body.on_ground && top > y0 + 0.02;
        }
        assert!(
            top > y0 + 0.05,
            "отброс вверх: поднялся до {top}, стоял на {y0}"
        );
        assert!(landed, "приземлился");
        assert!(
            (d.bounds().max[0] - 2.0).abs() < 1e-6,
            "доехал до стены и встал у неё"
        );
        assert!(d.body.vel[0].abs() < 1e-9);
    }

    #[test]
    fn dummy_breaks_and_comes_back_when_spawn_is_free() {
        let mut d = Dummy::new([0.0, 0.15], [0.16, 0.3], 2, 1.0);
        assert!(!d.hurt(1, [1.0, 0.0]));
        assert!(d.hurt(1, [1.0, 0.0]));
        assert!(!d.alive());
        // Место занято героем — ждёт.
        for _ in 0..90 {
            d.tick(DT, |_| true);
        }
        assert!(!d.alive());
        d.tick(DT, |_| false);
        d.tick(0.2, |_| false);
        assert!(d.alive());
        assert_eq!(d.hp, 2);
        assert_eq!(d.body.pos, [0.0, 0.15]);
    }

    #[test]
    fn hero_stands_on_a_dummy_and_rides_its_knockback() {
        // Манекен — опора: герой встаёт на него и едет вместе с ним.
        let solids = floor();
        let mut d = Dummy::new([0.0, 0.15], [0.16, 0.3], 9, 1.0);
        for _ in 0..10 {
            d.step(&solids, DT);
        }
        let t = Tuning::default();
        let mut hero = Body::new([0.0, 0.6]);
        let mut all = solids.clone();
        for _ in 0..60 {
            all.truncate(solids.len());
            all.push(d.collider(1));
            hero.step(&t, Intent::default(), &all, DT);
            d.step(&solids, DT);
        }
        assert!(hero.on_ground);
        assert!(
            (hero.bounds(&t).min[1] - d.bounds().max[1]).abs() < 1e-6,
            "стоит на манекене"
        );
        d.hurt(1, [1.5, 0.0]);
        let x0 = hero.pos[0];
        for _ in 0..30 {
            all.truncate(solids.len());
            all.push(d.collider(1));
            hero.step(&t, Intent::default(), &all, DT);
            d.step(&solids, DT);
        }
        assert!(
            hero.pos[0] > x0 + 0.05,
            "герой поехал с манекеном: {x0} → {}",
            hero.pos[0]
        );
        assert!(!hero.bounds(&t).overlaps(&d.bounds()));
    }
}
