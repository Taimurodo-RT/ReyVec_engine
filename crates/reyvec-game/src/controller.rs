//! Контроллер персонажа платформера.
//!
//! Это не физический движок: персонаж — прямоугольник, который движется так, как удобно
//! игроку, а не как настоящее тело. Приёмы, без которых платформер кажется «деревянным»:
//!
//! - **Разгон и торможение**: скорость не прыгает сразу до полной, но и не тянется долго.
//!   На земле торможение сильнее разгона, в воздухе управление слабее.
//! - **Прыжок задан высотой и временем до верхней точки**, а не силой и тяжестью: так
//!   его легко подбирать под уровень (из этих двух чисел считается тяжесть).
//! - **Высота по длине нажатия**: отпустил прыжок раньше — вверх тянет сильнее, прыжок
//!   ниже. Падение тоже быстрее подъёма: прыжок не «плавает».
//! - **Время койота**: прыжок ещё срабатывает, если игрок нажал чуть позже, чем сошёл с
//!   края ([`Tuning::coyote`]).
//! - **Запоминание нажатия**: нажал прыжок чуть раньше приземления — прыжок случится в
//!   момент касания земли ([`Tuning::buffer`]).
//!
//! Столкновения: сначала по горизонтали, потом по вертикали. На каждой оси путь
//! обрезается у ближайшего края стены, поэтому персонаж не проскакивает стену при любой
//! скорости и встаёт ровно вплотную.

use crate::{Aabb, EPS};

/// Настройки движения. Все расстояния в единицах сцены (высота кадра — 2 единицы),
/// время в секундах.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// Ширина и высота прямоугольника столкновений.
    pub size: [f64; 2],
    /// Наибольшая скорость бега, единиц в секунду.
    pub speed: f64,
    /// Разгон на земле, единиц в секунду за секунду.
    pub accel: f64,
    /// Торможение на земле (отпустил или повернул).
    pub decel: f64,
    /// Разгон и торможение в воздухе.
    pub air_accel: f64,
    /// Высота прыжка при зажатой кнопке.
    pub jump: f64,
    /// Время от толчка до верхней точки при зажатой кнопке.
    pub jump_time: f64,
    /// Во сколько раз сильнее тянет вниз при падении или отпущенной кнопке прыжка.
    pub fall: f64,
    /// Наибольшая скорость падения.
    pub max_fall: f64,
    /// Время койота: сколько после схода с края ещё можно прыгнуть.
    pub coyote: f64,
    /// Сколько помнится нажатие прыжка до приземления.
    pub buffer: f64,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            size: [0.14, 0.28],
            speed: 1.6,
            accel: 14.0,
            decel: 20.0,
            air_accel: 9.0,
            jump: 0.55,
            jump_time: 0.36,
            fall: 1.8,
            max_fall: 4.0,
            coyote: 0.1,
            buffer: 0.12,
        }
    }
}

impl Tuning {
    /// Тяжесть: за `jump_time` скорость прыжка падает до нуля на высоте `jump`.
    pub fn gravity(&self) -> f64 {
        2.0 * self.jump / (self.jump_time * self.jump_time)
    }

    /// Скорость толчка вверх.
    pub fn jump_speed(&self) -> f64 {
        2.0 * self.jump / self.jump_time
    }

    fn half(&self) -> [f64; 2] {
        [self.size[0] * 0.5, self.size[1] * 0.5]
    }
}

/// Чего хочет игрок на этом шаге.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Intent {
    /// Направление бега от -1 до 1.
    pub x: f64,
    /// Прыжок нажат на этом шаге.
    pub jump_pressed: bool,
    /// Прыжок зажат.
    pub jump_held: bool,
}

/// Состояние персонажа.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// Середина прямоугольника столкновений.
    pub pos: [f64; 2],
    pub vel: [f64; 2],
    pub on_ground: bool,
    /// Куда смотрит: 1 — вправо, -1 — влево.
    pub facing: f64,
    coyote_left: f64,
    buffer_left: f64,
}

/// Что случилось за шаг (для звука, частиц и анимации в следующих задачах).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepEvents {
    pub jumped: bool,
    pub landed: bool,
    /// Упёрся в стену сбоку.
    pub hit_wall: bool,
    /// Ударился головой.
    pub hit_ceiling: bool,
}

impl Body {
    /// Персонаж стоит в `pos` без движения.
    pub fn new(pos: [f64; 2]) -> Self {
        Self {
            pos,
            vel: [0.0, 0.0],
            on_ground: false,
            facing: 1.0,
            coyote_left: 0.0,
            buffer_left: 0.0,
        }
    }

    /// Прямоугольник столкновений сейчас.
    pub fn bounds(&self, tuning: &Tuning) -> Aabb {
        Aabb::around(self.pos, tuning.half())
    }

    /// Один шаг длиной `dt` секунд среди твёрдых прямоугольников `solids`.
    pub fn step(
        &mut self,
        tuning: &Tuning,
        intent: Intent,
        solids: &[Aabb],
        dt: f64,
    ) -> StepEvents {
        let mut ev = StepEvents::default();
        if dt <= 0.0 {
            return ev;
        }
        // Бег: скорость тянется к желаемой.
        let x = intent.x.clamp(-1.0, 1.0);
        let target = x * tuning.speed;
        let rate = if !self.on_ground {
            tuning.air_accel
        } else if x != 0.0 && target * self.vel[0] >= 0.0 {
            tuning.accel
        } else {
            tuning.decel
        };
        self.vel[0] = approach(self.vel[0], target, rate * dt);
        if x != 0.0 {
            self.facing = x.signum();
        }

        // Прыжок: время койота и запомненное нажатие.
        if self.on_ground {
            self.coyote_left = tuning.coyote;
        } else {
            self.coyote_left -= dt;
        }
        if intent.jump_pressed {
            self.buffer_left = tuning.buffer;
        } else {
            self.buffer_left -= dt;
        }
        if self.buffer_left > 0.0 && self.coyote_left > 0.0 {
            self.vel[1] = tuning.jump_speed();
            self.buffer_left = 0.0;
            self.coyote_left = 0.0;
            self.on_ground = false;
            ev.jumped = true;
        }

        // Тяжесть: вверх с зажатой кнопкой слабее, при падении сильнее.
        let rising = self.vel[1] > 0.0 && intent.jump_held;
        let g = tuning.gravity() * if rising { 1.0 } else { tuning.fall };
        self.vel[1] = (self.vel[1] - g * dt).max(-tuning.max_fall);

        // Столкновения: сначала вбок, потом по вертикали.
        let half = tuning.half();
        let dx = self.vel[0] * dt;
        let moved = sweep(self.bounds(tuning), 0, dx, solids);
        if moved != dx {
            self.vel[0] = 0.0;
            ev.hit_wall = true;
        }
        self.pos[0] += moved;
        let dy = self.vel[1] * dt;
        let moved = sweep(Aabb::around(self.pos, half), 1, dy, solids);
        let was_on_ground = self.on_ground;
        self.on_ground = false;
        if moved != dy {
            if dy < 0.0 {
                self.on_ground = true;
                ev.landed = !was_on_ground;
            } else {
                ev.hit_ceiling = true;
            }
            self.vel[1] = 0.0;
        }
        self.pos[1] += moved;
        ev
    }
}

fn approach(v: f64, target: f64, by: f64) -> f64 {
    if v < target {
        (v + by).min(target)
    } else {
        (v - by).max(target)
    }
}

/// Насколько можно сдвинуть `b` по оси `axis` на `d`, не войдя ни в один из `solids`.
/// Прямоугольники, в которых `b` уже стоит (например, персонажа поставили внутрь стены),
/// не держат: из них можно выйти.
fn sweep(b: Aabb, axis: usize, d: f64, solids: &[Aabb]) -> f64 {
    let other = 1 - axis;
    let mut d = d;
    for s in solids {
        // Мимо по другой оси.
        if s.min[other] >= b.max[other] - EPS || s.max[other] <= b.min[other] + EPS {
            continue;
        }
        if d > 0.0 && s.min[axis] >= b.max[axis] - EPS {
            d = d.min((s.min[axis] - b.max[axis]).max(0.0));
        } else if d < 0.0 && s.max[axis] <= b.min[axis] + EPS {
            d = d.max((s.max[axis] - b.min[axis]).min(0.0));
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1.0 / 60.0;

    fn floor() -> Vec<Aabb> {
        vec![Aabb {
            min: [-10.0, -1.0],
            max: [10.0, 0.0],
        }]
    }

    fn run(body: &mut Body, t: &Tuning, solids: &[Aabb], steps: usize, intent: Intent) {
        for _ in 0..steps {
            body.step(t, intent, solids, DT);
        }
    }

    fn standing(t: &Tuning) -> Body {
        let mut b = Body::new([0.0, t.size[1] * 0.5 + 0.2]);
        run(&mut b, t, &floor(), 60, Intent::default());
        assert!(b.on_ground);
        b
    }

    #[test]
    fn falls_and_lands_exactly_on_floor() {
        let t = Tuning::default();
        let b = standing(&t);
        assert!((b.bounds(&t).min[1]).abs() < 1e-9, "{:?}", b.pos);
        assert_eq!(b.vel, [0.0, 0.0]);
    }

    #[test]
    fn jump_reaches_set_height() {
        let t = Tuning::default();
        let mut b = standing(&t);
        let ground = b.pos[1];
        let mut top: f64 = ground;
        let held = Intent {
            jump_held: true,
            ..Intent::default()
        };
        b.step(
            &t,
            Intent {
                jump_pressed: true,
                ..held
            },
            &floor(),
            DT,
        );
        for _ in 0..120 {
            b.step(&t, held, &floor(), DT);
            top = top.max(b.pos[1]);
        }
        let h = top - ground;
        assert!((h - t.jump).abs() < 0.03, "высота прыжка {h}");
        assert!(b.on_ground);
    }

    #[test]
    fn short_press_jumps_lower() {
        let t = Tuning::default();
        let mut b = standing(&t);
        let ground = b.pos[1];
        b.step(
            &t,
            Intent {
                jump_pressed: true,
                jump_held: true,
                ..Intent::default()
            },
            &floor(),
            DT,
        );
        let mut top: f64 = ground;
        for _ in 0..120 {
            b.step(&t, Intent::default(), &floor(), DT);
            top = top.max(b.pos[1]);
        }
        assert!(top - ground < t.jump * 0.7, "{}", top - ground);
    }

    #[test]
    fn coyote_time_allows_late_jump() {
        let t = Tuning::default();
        // Узкий уступ: персонаж сходит с него вправо.
        let ledge = vec![Aabb {
            min: [-1.0, -1.0],
            max: [0.0, 0.0],
        }];
        let mut b = Body::new([-0.2, t.size[1] * 0.5]);
        b.vel[0] = t.speed;
        let right = Intent {
            x: 1.0,
            ..Intent::default()
        };
        run(&mut b, &t, &ledge, 1, right);
        assert!(b.on_ground);
        // Бежим, пока не сойдём с края.
        let mut steps = 0;
        while b.on_ground && steps < 120 {
            b.step(&t, right, &ledge, DT);
            steps += 1;
        }
        assert!(!b.on_ground);
        // Нажали через 3 шага (50 мс) после схода — прыжок всё равно есть.
        run(&mut b, &t, &ledge, 3, right);
        let ev = b.step(
            &t,
            Intent {
                jump_pressed: true,
                jump_held: true,
                x: 1.0,
            },
            &ledge,
            DT,
        );
        assert!(ev.jumped);

        // А если ждать дольше времени койота — уже нет.
        let mut b = Body::new([-0.02, t.size[1] * 0.5 + 0.001]);
        run(&mut b, &t, &ledge, 2, right);
        run(&mut b, &t, &ledge, 20, right);
        let ev = b.step(
            &t,
            Intent {
                jump_pressed: true,
                ..right
            },
            &ledge,
            DT,
        );
        assert!(!ev.jumped);
    }

    #[test]
    fn early_press_jumps_on_landing() {
        let t = Tuning::default();
        let mut b = Body::new([0.0, t.size[1] * 0.5 + 0.03]);
        b.vel[1] = -2.0;
        // Нажали в воздухе за пару шагов до касания.
        b.step(
            &t,
            Intent {
                jump_pressed: true,
                jump_held: true,
                ..Intent::default()
            },
            &floor(),
            DT,
        );
        let mut jumped = false;
        for _ in 0..8 {
            let ev = b.step(
                &t,
                Intent {
                    jump_held: true,
                    ..Intent::default()
                },
                &floor(),
                DT,
            );
            jumped |= ev.jumped;
        }
        assert!(jumped);
        assert!(b.vel[1] > 0.0 || b.pos[1] > t.size[1] * 0.5 + 0.01);
    }

    #[test]
    fn runs_to_full_speed_and_stops() {
        let t = Tuning::default();
        let mut b = standing(&t);
        run(
            &mut b,
            &t,
            &floor(),
            30,
            Intent {
                x: 1.0,
                ..Intent::default()
            },
        );
        assert!((b.vel[0] - t.speed).abs() < 1e-9);
        run(&mut b, &t, &floor(), 30, Intent::default());
        assert_eq!(b.vel[0], 0.0);
        run(
            &mut b,
            &t,
            &floor(),
            2,
            Intent {
                x: -1.0,
                ..Intent::default()
            },
        );
        assert_eq!(b.facing, -1.0);
    }

    #[test]
    fn thin_wall_stops_fast_body() {
        let t = Tuning::default();
        let mut solids = floor();
        // Стена тоньше, чем путь за один шаг.
        solids.push(Aabb {
            min: [0.5, 0.0],
            max: [0.501, 1.0],
        });
        let mut b = standing(&t);
        b.vel[0] = 0.0;
        let fast = Tuning {
            speed: 100.0,
            accel: 1e6,
            ..t
        };
        let ev = b.step(
            &fast,
            Intent {
                x: 1.0,
                ..Intent::default()
            },
            &solids,
            DT,
        );
        assert!(ev.hit_wall);
        assert!((b.bounds(&t).max[0] - 0.5).abs() < 1e-9);
        assert_eq!(b.vel[0], 0.0);
    }

    #[test]
    fn head_bump_stops_rise() {
        let t = Tuning::default();
        let mut solids = floor();
        solids.push(Aabb {
            min: [-1.0, 0.4],
            max: [1.0, 0.5],
        });
        let mut b = standing(&t);
        b.step(
            &t,
            Intent {
                jump_pressed: true,
                jump_held: true,
                ..Intent::default()
            },
            &solids,
            DT,
        );
        let mut bumped = false;
        for _ in 0..30 {
            bumped |= b
                .step(
                    &t,
                    Intent {
                        jump_held: true,
                        ..Intent::default()
                    },
                    &solids,
                    DT,
                )
                .hit_ceiling;
        }
        assert!(bumped);
        assert!(b.bounds(&t).max[1] <= 0.4 + 1e-9);
    }

    #[test]
    fn walks_out_of_wall_it_was_placed_in() {
        let t = Tuning::default();
        let mut solids = floor();
        solids.push(Aabb {
            min: [-0.05, 0.0],
            max: [0.05, 1.0],
        });
        let mut b = standing(&t);
        run(
            &mut b,
            &t,
            &solids,
            60,
            Intent {
                x: 1.0,
                ..Intent::default()
            },
        );
        assert!(b.pos[0] > 0.5);
    }
}
