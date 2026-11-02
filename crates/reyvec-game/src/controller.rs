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
    /// Скорость и длительность рывка (0 — рывка нет).
    pub dash_speed: f64,
    pub dash_time: f64,
    /// Скорость скольжения по стене вниз, когда прижимаешься к ней.
    pub wall_slide: f64,
    /// Прыжок от стены: во сколько раз скорости бега отталкивает вбок (0 — нельзя).
    pub wall_jump: f64,
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
            dash_speed: 4.2,
            dash_time: 0.16,
            wall_slide: 0.5,
            wall_jump: 1.1,
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
    /// Зажато «вниз»: вместе с прыжком — спрыгнуть с платформы вниз.
    pub down: bool,
    /// Рывок нажат на этом шаге.
    pub dash_pressed: bool,
}

/// Твёрдый прямоугольник уровня.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Collider {
    pub aabb: Aabb,
    /// Платформа: держит только сверху, снизу и сбоку сквозь неё проходишь.
    pub oneway: bool,
    /// Номер, по которому персонаж узнаёт «свою» опору на следующем шаге.
    pub id: u64,
    /// На сколько сдвинулся за этот шаг (движущаяся платформа везёт стоящего на ней).
    pub moved: [f64; 2],
}

impl Collider {
    /// Неподвижная твёрдая стена или пол.
    pub fn solid(aabb: Aabb) -> Self {
        Self {
            aabb,
            oneway: false,
            id: 0,
            moved: [0.0, 0.0],
        }
    }

    /// Неподвижная платформа, которая держит только сверху.
    pub fn oneway(aabb: Aabb) -> Self {
        Self {
            oneway: true,
            ..Self::solid(aabb)
        }
    }
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
    /// Прижат к стене в воздухе: -1 — стена слева, 1 — справа, 0 — нет.
    pub wall: f64,
    /// Идёт рывок.
    pub dashing: bool,
    coyote_left: f64,
    buffer_left: f64,
    /// На чём стоит (номер [`Collider::id`]) и платформа ли это.
    ground: Option<u64>,
    on_platform: bool,
    dash_left: f64,
    dash_dir: f64,
    /// Рывок можно сделать (один в воздухе, снова — после касания земли или стены).
    dash_ready: bool,
    /// После прыжка от стены управление по горизонтали ненадолго выключено: иначе
    /// игрок, зажавший кнопку к стене, тут же к ней вернётся.
    wall_lock: f64,
    /// Спрыгивает сквозь платформу: платформы не держат, пока не истечёт.
    drop_left: f64,
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
    /// Оттолкнулся от стены.
    pub wall_jumped: bool,
    /// Начал рывок.
    pub dashed: bool,
    /// Спрыгнул сквозь платформу.
    pub dropped: bool,
}

/// Насколько близко к стене считается «прижат».
const WALL_TOUCH: f64 = 0.002;
/// Сколько после прыжка от стены не слушается управление по горизонтали, секунд.
const WALL_LOCK: f64 = 0.14;
/// Сколько спрыгивание не замечает платформ, секунд.
const DROP_TIME: f64 = 0.2;

impl Body {
    /// Персонаж стоит в `pos` без движения.
    pub fn new(pos: [f64; 2]) -> Self {
        Self {
            pos,
            vel: [0.0, 0.0],
            on_ground: false,
            facing: 1.0,
            wall: 0.0,
            dashing: false,
            coyote_left: 0.0,
            buffer_left: 0.0,
            ground: None,
            on_platform: false,
            dash_left: 0.0,
            dash_dir: 1.0,
            dash_ready: true,
            wall_lock: 0.0,
            drop_left: 0.0,
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
        solids: &[Collider],
        dt: f64,
    ) -> StepEvents {
        let mut ev = StepEvents::default();
        if dt <= 0.0 {
            return ev;
        }
        let half = tuning.half();
        // Опора поехала — едем с ней (сначала, чтобы шаг считался уже от нового места).
        if self.on_ground
            && let Some(g) = self
                .ground
                .and_then(|id| solids.iter().find(|c| c.id == id))
            && g.moved != [0.0, 0.0]
        {
            let others: Vec<Collider> = solids.iter().filter(|c| c.id != g.id).copied().collect();
            self.pos[0] += sweep(self.bounds(tuning), 0, g.moved[0], &others, false).0;
            self.pos[1] += g.moved[1];
        }
        self.wall_lock -= dt;
        self.drop_left -= dt;
        let x = intent.x.clamp(-1.0, 1.0);
        // Прижат ли к стене (в воздухе, платформы не в счёт).
        self.wall = 0.0;
        if !self.on_ground {
            for side in [-1.0, 1.0] {
                if sweep(self.bounds(tuning), 0, side * WALL_TOUCH, solids, false)
                    .0
                    .abs()
                    < WALL_TOUCH * 0.5
                {
                    self.wall = side;
                }
            }
        }
        if self.on_ground || self.wall != 0.0 {
            self.dash_ready = true;
        }

        // Рывок: короткий быстрый бросок по горизонтали без тяжести.
        if intent.dash_pressed && self.dash_ready && self.dash_left <= 0.0 && tuning.dash_time > 0.0
        {
            self.dash_left = tuning.dash_time;
            self.dash_dir = if x != 0.0 { x.signum() } else { self.facing };
            self.facing = self.dash_dir;
            self.dash_ready = self.on_ground;
            ev.dashed = true;
        }
        self.dashing = self.dash_left > 0.0;
        if self.dashing {
            self.dash_left -= dt;
            self.vel = [self.dash_dir * tuning.dash_speed, 0.0];
            if self.dash_left <= 0.0 {
                // После рывка — обычная полная скорость, без рывка назад.
                self.vel[0] = self.dash_dir * tuning.speed;
            }
        } else {
            // Бег: скорость тянется к желаемой.
            let target = x * tuning.speed;
            let rate = if !self.on_ground {
                tuning.air_accel
            } else if x != 0.0 && target * self.vel[0] >= 0.0 {
                tuning.accel
            } else {
                tuning.decel
            };
            if self.wall_lock <= 0.0 {
                self.vel[0] = approach(self.vel[0], target, rate * dt);
                if x != 0.0 {
                    self.facing = x.signum();
                }
            }
        }

        // Прыжок: время койота, запомненное нажатие, от стены, вниз сквозь платформу.
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
        let on_platform = self.on_ground && self.on_platform;
        if self.buffer_left > 0.0 && intent.down && on_platform {
            self.drop_left = DROP_TIME;
            self.buffer_left = 0.0;
            self.coyote_left = 0.0;
            self.on_ground = false;
            ev.dropped = true;
        } else if self.buffer_left > 0.0 && self.coyote_left > 0.0 {
            self.vel[1] = tuning.jump_speed();
            self.buffer_left = 0.0;
            self.coyote_left = 0.0;
            self.on_ground = false;
            self.dash_left = 0.0;
            ev.jumped = true;
        } else if self.buffer_left > 0.0 && self.wall != 0.0 && tuning.wall_jump > 0.0 {
            self.vel = [
                -self.wall * tuning.speed * tuning.wall_jump,
                tuning.jump_speed() * 0.9,
            ];
            self.facing = -self.wall;
            self.wall_lock = WALL_LOCK;
            self.buffer_left = 0.0;
            self.dash_left = 0.0;
            ev.wall_jumped = true;
        }

        if !self.dashing {
            // Тяжесть: вверх с зажатой кнопкой слабее, при падении сильнее.
            let rising = self.vel[1] > 0.0 && intent.jump_held;
            let g = tuning.gravity() * if rising { 1.0 } else { tuning.fall };
            self.vel[1] = (self.vel[1] - g * dt).max(-tuning.max_fall);
            // Прижатый к стене скользит по ней медленно.
            if self.wall != 0.0 && x == self.wall && self.vel[1] < -tuning.wall_slide {
                self.vel[1] = -tuning.wall_slide;
            }
        }

        // Столкновения: сначала вбок, потом по вертикали.
        let dx = self.vel[0] * dt;
        let (moved, _) = sweep(self.bounds(tuning), 0, dx, solids, false);
        if moved != dx {
            self.vel[0] = 0.0;
            ev.hit_wall = true;
            self.dash_left = 0.0;
        }
        self.pos[0] += moved;
        let dy = self.vel[1] * dt;
        let platforms = self.drop_left <= 0.0;
        let (moved, by) = sweep(Aabb::around(self.pos, half), 1, dy, solids, platforms);
        let was_on_ground = self.on_ground;
        self.on_ground = false;
        self.ground = None;
        self.on_platform = false;
        if moved != dy {
            if dy < 0.0 {
                self.on_ground = true;
                self.ground = by.map(|i| solids[i].id);
                self.on_platform = by.is_some_and(|i| solids[i].oneway);
                ev.landed = !was_on_ground;
            } else {
                ev.hit_ceiling = true;
            }
            self.vel[1] = 0.0;
        }
        self.pos[1] += moved;
        self.dashing = self.dash_left > 0.0;
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

/// Насколько можно сдвинуть `b` по оси `axis` на `d`, не войдя ни в один из `solids`, и
/// какой из них остановил. Прямоугольники, в которых `b` уже стоит (например, персонажа
/// поставили внутрь стены), не держат: из них можно выйти. Платформы держат, только если
/// `platforms` и движение вниз начинается не ниже их верха.
fn sweep(
    b: Aabb,
    axis: usize,
    d: f64,
    solids: &[Collider],
    platforms: bool,
) -> (f64, Option<usize>) {
    let other = 1 - axis;
    let mut d = d;
    let mut by = None;
    for (i, c) in solids.iter().enumerate() {
        let s = &c.aabb;
        if c.oneway && !(platforms && axis == 1 && d < 0.0) {
            continue;
        }
        // Мимо по другой оси.
        if s.min[other] >= b.max[other] - EPS || s.max[other] <= b.min[other] + EPS {
            continue;
        }
        // Где был край до того, как предмет сдвинулся в этом шаге: едущая навстречу
        // платформа не должна проскочить сквозь персонажа.
        let was_min = s.min[axis] - c.moved[axis].min(0.0);
        let was_max = s.max[axis] - c.moved[axis].max(0.0);
        if d > 0.0 && was_min >= b.max[axis] - EPS {
            let limit = s.min[axis] - b.max[axis];
            if limit < d {
                d = limit;
                by = Some(i);
            }
        } else if d < 0.0 && was_max <= b.min[axis] + EPS {
            let limit = s.max[axis] - b.min[axis];
            if limit > d {
                d = limit;
                by = Some(i);
            }
        }
    }
    (d, by)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1.0 / 60.0;

    fn floor() -> Vec<Collider> {
        vec![Collider::solid(Aabb {
            min: [-10.0, -1.0],
            max: [10.0, 0.0],
        })]
    }

    fn run(body: &mut Body, t: &Tuning, solids: &[Collider], steps: usize, intent: Intent) {
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
        let ledge = vec![Collider::solid(Aabb {
            min: [-1.0, -1.0],
            max: [0.0, 0.0],
        })];
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
                ..Intent::default()
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
        solids.push(Collider::solid(Aabb {
            min: [0.5, 0.0],
            max: [0.501, 1.0],
        }));
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
        solids.push(Collider::solid(Aabb {
            min: [-1.0, 0.4],
            max: [1.0, 0.5],
        }));
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
        solids.push(Collider::solid(Aabb {
            min: [-0.05, 0.0],
            max: [0.05, 1.0],
        }));
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

    fn jump_now() -> Intent {
        Intent {
            jump_pressed: true,
            jump_held: true,
            ..Intent::default()
        }
    }

    fn held_jump() -> Intent {
        Intent {
            jump_held: true,
            ..Intent::default()
        }
    }

    #[test]
    fn platform_lets_through_from_below_and_holds_from_above() {
        let t = Tuning::default();
        let mut solids = floor();
        // Платформа на высоте 0,35 над полом: снизу персонаж (0,28) пролетает сквозь неё.
        solids.push(Collider::oneway(Aabb {
            min: [-1.0, 0.33],
            max: [1.0, 0.35],
        }));
        let mut b = standing(&t);
        b.step(&t, jump_now(), &solids, DT);
        let mut hit = false;
        for _ in 0..90 {
            hit |= b.step(&t, held_jump(), &solids, DT).hit_ceiling;
        }
        assert!(!hit, "головой о платформу биться не должен");
        assert!(b.on_ground);
        assert!((b.bounds(&t).min[1] - 0.35).abs() < 1e-9, "{:?}", b.pos);
        // Вниз + прыжок — спрыгивает сквозь неё на пол.
        let ev = b.step(
            &t,
            Intent {
                down: true,
                ..jump_now()
            },
            &solids,
            DT,
        );
        assert!(ev.dropped && !ev.jumped);
        run(&mut b, &t, &solids, 60, Intent::default());
        assert!(b.on_ground);
        assert!(b.bounds(&t).min[1].abs() < 1e-9, "{:?}", b.pos);
        // Вбок сквозь платформу тоже проходишь.
        assert!(
            !b.step(
                &t,
                Intent {
                    x: 1.0,
                    ..Intent::default()
                },
                &solids,
                DT
            )
            .hit_wall
        );
    }

    #[test]
    fn moving_platform_carries_the_body() {
        let t = Tuning::default();
        let mut lift = Collider {
            id: 7,
            ..Collider::solid(Aabb {
                min: [-0.5, -0.1],
                max: [0.5, 0.0],
            })
        };
        let mut b = Body::new([0.0, t.size[1] * 0.5 + 0.05]);
        run(&mut b, &t, &[lift], 30, Intent::default());
        assert!(b.on_ground);
        // Платформа едет вправо и вверх 30 шагов по 0,01.
        for _ in 0..30 {
            lift.aabb.min[0] += 0.01;
            lift.aabb.max[0] += 0.01;
            lift.aabb.min[1] += 0.005;
            lift.aabb.max[1] += 0.005;
            lift.moved = [0.01, 0.005];
            b.step(&t, Intent::default(), &[lift], DT);
            assert!(b.on_ground);
        }
        assert!((b.pos[0] - 0.3).abs() < 1e-9, "{:?}", b.pos);
        assert!((b.bounds(&t).min[1] - lift.aabb.max[1]).abs() < 1e-9);
    }

    #[test]
    fn dash_is_fast_short_and_once_in_the_air() {
        let t = Tuning::default();
        let mut b = standing(&t);
        let dash = Intent {
            dash_pressed: true,
            x: 1.0,
            ..Intent::default()
        };
        assert!(b.step(&t, dash, &floor(), DT).dashed);
        assert!(b.dashing);
        let x0 = b.pos[0];
        run(&mut b, &t, &floor(), 20, Intent::default());
        let dist = b.pos[0] - x0;
        // Рывок проходит примерно dash_speed × dash_time (плюс торможение после).
        assert!(dist > t.dash_speed * t.dash_time * 0.8, "{dist}");
        assert!(!b.dashing);
        // В воздухе — один рывок до касания земли.
        b.step(&t, jump_now(), &floor(), DT);
        run(&mut b, &t, &floor(), 5, held_jump());
        assert!(b.step(&t, dash, &floor(), DT).dashed);
        run(&mut b, &t, &floor(), 12, Intent::default());
        assert!(!b.on_ground);
        assert!(
            !b.step(&t, dash, &floor(), DT).dashed,
            "второй рывок в воздухе"
        );
        run(&mut b, &t, &floor(), 120, Intent::default());
        assert!(b.on_ground);
        assert!(b.step(&t, dash, &floor(), DT).dashed);
    }

    #[test]
    fn wall_slide_and_wall_jump() {
        let t = Tuning::default();
        let mut solids = floor();
        solids.push(Collider::solid(Aabb {
            min: [0.3, 0.0],
            max: [0.4, 3.0],
        }));
        let mut b = Body::new([0.3 - t.size[0] * 0.5, 1.5]);
        let toward = Intent {
            x: 1.0,
            ..Intent::default()
        };
        run(&mut b, &t, &solids, 40, toward);
        assert_eq!(b.wall, 1.0);
        assert!(
            b.vel[1] >= -t.wall_slide - 1e-9,
            "скользит медленно: {:?}",
            b.vel
        );
        let ev = b.step(
            &t,
            Intent {
                x: 1.0,
                ..jump_now()
            },
            &solids,
            DT,
        );
        assert!(ev.wall_jumped);
        assert!(b.vel[0] < 0.0 && b.vel[1] > 0.0, "{:?}", b.vel);
        assert_eq!(b.facing, -1.0);
        // Кнопка к стене ещё зажата, но персонаж отлетает от неё.
        let x0 = b.pos[0];
        run(&mut b, &t, &solids, 6, toward);
        assert!(b.pos[0] < x0 - 0.05, "{} {x0}", b.pos[0]);
    }
}
