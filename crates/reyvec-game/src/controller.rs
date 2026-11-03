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
//!
//! Наклоны (6.4): наклонный прямоугольник держит только сверху, по линии из угла в
//! угол. На нём стоит середина низа персонажа, а не весь прямоугольник: так он не висит
//! над склоном краем. Бегущий вниз по склону прижимается к нему, а не слетает
//! ступеньками; маленькую ступеньку (до половины ширины) на земле перешагивает сам.
//!
//! Цепляние за край (6.4): в падении, прижавшись к стене, персонаж хватается за её верх,
//! если тот на уровне головы и сверху есть место встать. Вверх — забраться, прыжок —
//! прыгнуть вверх, вниз или от стены — отпустить.

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
    /// Можно цепляться за край (6.4).
    pub ledge: bool,
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
            ledge: true,
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

    /// Какую ступеньку персонаж на земле перешагивает сам: на 45° склоне край
    /// прямоугольника упирается в верх соседнего блока как раз на половине ширины ниже.
    fn step_up(&self) -> f64 {
        self.size[0] * 0.5 + 0.01
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
    /// Зажато «вверх»: висящий на краю забирается наверх (6.4).
    pub up: bool,
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
    /// Наклон (6.4): 0 — обычный прямоугольник. Иначе это склон, и число — на сколько
    /// он поднимается слева направо (меньше 0 — опускается): верх склона идёт по прямой
    /// от одного верхнего угла прямоугольника вниз на эту высоту у другого края. Ниже
    /// склона прямоугольник сплошной (столбик под склоном у уровня из тайлов). Склон
    /// держит только сверху.
    pub slope: f64,
}

impl Collider {
    /// Неподвижная твёрдая стена или пол.
    pub fn solid(aabb: Aabb) -> Self {
        Self {
            aabb,
            oneway: false,
            id: 0,
            moved: [0.0, 0.0],
            slope: 0.0,
        }
    }

    /// Неподвижный склон, поднимающийся слева направо на `rise` (меньше 0 —
    /// опускающийся).
    pub fn slope(aabb: Aabb, rise: f64) -> Self {
        Self {
            slope: rise,
            ..Self::solid(aabb)
        }
    }

    /// Высота склона над точкой `x` (за краями — высота ближнего края).
    fn surface(&self, x: f64) -> f64 {
        let b = &self.aabb;
        let w = b.max[0] - b.min[0];
        let t = if w > 0.0 {
            ((x - b.min[0]) / w).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let t = if self.slope > 0.0 { t } else { 1.0 - t };
        b.max[1] - self.slope.abs() * (1.0 - t)
    }

    /// Крутизна склона: подъём на единицу пути.
    fn steepness(&self) -> f64 {
        let b = &self.aabb;
        self.slope.abs() / (b.max[0] - b.min[0]).max(EPS)
    }

    fn is_slope(&self) -> bool {
        self.slope != 0.0
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
    /// Висит на краю (6.4): -1 — край слева, 1 — справа, 0 — не висит.
    pub hanging: f64,
    /// Верх края, за который держится, и прямоугольник под ним.
    ledge: Option<Aabb>,
    /// Сколько ещё нельзя снова схватиться за край (только что отпустил).
    grab_cooldown: f64,
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
    /// Схватился за край.
    pub grabbed: bool,
    /// Забрался на край.
    pub climbed: bool,
}

/// Насколько близко к стене считается «прижат».
const WALL_TOUCH: f64 = 0.002;
/// Сколько после прыжка от стены не слушается управление по горизонтали, секунд.
const WALL_LOCK: f64 = 0.14;
/// Сколько спрыгивание не замечает платформ, секунд.
const DROP_TIME: f64 = 0.2;
/// Насколько верх висящего выше края: руки лежат на краю.
const HANG_ABOVE: f64 = 0.02;
/// Сколько после того, как отпустил край, за него нельзя схватиться, секунд.
const GRAB_COOLDOWN: f64 = 0.25;

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
            hanging: 0.0,
            ledge: None,
            grab_cooldown: 0.0,
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
        self.push_out(tuning, solids);
        self.wall_lock -= dt;
        self.drop_left -= dt;
        self.grab_cooldown -= dt;
        let x = intent.x.clamp(-1.0, 1.0);

        // Висит на краю: стоит на месте, пока не нажмут.
        if let Some(ledge) = self.ledge {
            let side = self.hanging;
            if intent.jump_pressed {
                self.let_go();
                self.vel = [0.0, tuning.jump_speed()];
                self.grab_cooldown = GRAB_COOLDOWN;
                ev.jumped = true;
            } else if intent.up {
                // Встаёт на край у самой стены.
                let edge = if side > 0.0 {
                    ledge.min[0]
                } else {
                    ledge.max[0]
                };
                let mut stand = edge + side * (half[0] + 0.005);
                if ledge.max[0] - ledge.min[0] >= tuning.size[0] {
                    stand = stand.clamp(ledge.min[0] + half[0], ledge.max[0] - half[0]);
                }
                self.let_go();
                self.pos = [stand, ledge.max[1] + half[1]];
                self.vel = [0.0, 0.0];
                self.on_ground = true;
                self.coyote_left = tuning.coyote;
                ev.climbed = true;
                return ev;
            } else if intent.down || x * side < 0.0 {
                self.let_go();
                self.grab_cooldown = GRAB_COOLDOWN;
            } else {
                self.vel = [0.0, 0.0];
                self.facing = side;
                self.wall = side;
                self.dash_ready = true;
                self.buffer_left = 0.0;
                return ev;
            }
        }

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
        if intent.jump_pressed && !ev.jumped {
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

        // Столкновения: сначала вбок, потом по вертикали. Идущий по земле не замечает
        // ступеньки ниже `step_up` и встаёт на неё.
        let walking = self.on_ground && self.vel[1] <= 0.0;
        let foot_before = [self.pos[0], self.pos[1] - half[1]];
        let dx = self.vel[0] * dt;
        let mut probe = self.bounds(tuning);
        if walking {
            probe.min[1] += tuning.step_up();
        }
        let (mut moved_x, _) = sweep(probe, 0, dx, solids, false);
        self.pos[0] += moved_x;
        if walking && !self.climb_step(tuning, solids) {
            // На ступеньку не встать (над ней низко): упёрся, как в стену.
            self.pos[0] -= moved_x;
            moved_x = sweep(self.bounds(tuning), 0, dx, solids, false).0;
            self.pos[0] += moved_x;
        }
        if moved_x != dx {
            self.vel[0] = 0.0;
            ev.hit_wall = true;
            self.dash_left = 0.0;
        }
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
            } else {
                ev.hit_ceiling = true;
            }
            self.vel[1] = 0.0;
        }
        self.pos[1] += moved;

        // Склоны держат сверху; бегущего по земле прижимают, а не дают слететь.
        let snap = if walking { moved_x.abs() + 0.005 } else { 0.0 };
        self.stand_on_slopes(tuning, solids, foot_before, snap);
        if walking && !self.on_ground && snap > 0.0 {
            // Сошёл со склона на ровное или с ровного чуть ниже — тоже прижимается.
            let (down, by) = sweep(self.bounds(tuning), 1, -snap, solids, platforms);
            if down > -snap {
                self.pos[1] += down;
                self.on_ground = true;
                self.ground = by.map(|i| solids[i].id);
                self.on_platform = by.is_some_and(|i| solids[i].oneway);
                self.vel[1] = 0.0;
            }
        }
        ev.landed = self.on_ground && !was_on_ground;
        self.dashing = self.dash_left > 0.0;

        // Схватиться за край: в падении, прижавшись к стене, край на уровне головы.
        if tuning.ledge
            && !self.on_ground
            && !self.dashing
            && self.vel[1] <= 0.0
            && self.grab_cooldown <= 0.0
            && x != 0.0
            && let Some(ledge) = self.find_ledge(tuning, solids, x.signum(), -moved)
        {
            self.hanging = x.signum();
            self.ledge = Some(ledge);
            self.pos[1] = ledge.max[1] + HANG_ABOVE - half[1];
            self.vel = [0.0, 0.0];
            self.wall = self.hanging;
            self.facing = self.hanging;
            self.dash_ready = true;
            self.dash_left = 0.0;
            self.dashing = false;
            ev.grabbed = true;
        }
        ev
    }

    /// Движущийся предмет въехал в персонажа (подъёмник опустился на голову, стена
    /// поехала вбок): вытолкнуть его по ходу движения, а если там тесно — вбок из-под
    /// предмета. Иначе персонаж оказался бы внутри и проходил бы сквозь предмет.
    fn push_out(&mut self, tuning: &Tuning, solids: &[Collider]) {
        for (i, c) in solids.iter().enumerate() {
            if c.moved == [0.0, 0.0] || c.oneway || c.is_slope() {
                continue;
            }
            let b = self.bounds(tuning);
            if !c.aabb.overlaps(&b) {
                continue;
            }
            let s = &c.aabb;
            let before = Aabb {
                min: [s.min[0] - c.moved[0], s.min[1] - c.moved[1]],
                max: [s.max[0] - c.moved[0], s.max[1] - c.moved[1]],
            };
            if before.overlaps(&b) {
                // Был внутри и раньше (поставили так): выйдет сам, как из стены.
                continue;
            }
            let others: Vec<Collider> = solids
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, c)| *c)
                .collect();
            // По какой оси предмет подъехал: по ней и толкаем.
            let apart_y = before.min[1] >= b.max[1] - EPS || before.max[1] <= b.min[1] + EPS;
            let axis = if apart_y { 1 } else { 0 };
            let push = if c.moved[axis] < 0.0 {
                s.min[axis] - b.max[axis]
            } else {
                s.max[axis] - b.min[axis]
            };
            let done = sweep(b, axis, push, &others, false).0;
            if (done - push).abs() < EPS {
                self.pos[axis] += push;
                if axis == 1 && push > 0.0 {
                    self.on_ground = true;
                    self.ground = Some(c.id);
                    self.vel[1] = self.vel[1].max(0.0);
                } else if axis == 1 {
                    self.vel[1] = self.vel[1].min(0.0);
                } else {
                    self.vel[0] = 0.0;
                }
                continue;
            }
            // Зажало: вбок к ближнему краю предмета, если там свободно.
            let other = 1 - axis;
            let mut ways = [s.min[other] - b.max[other], s.max[other] - b.min[other]];
            ways.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
            for way in ways {
                if (sweep(b, other, way, &others, false).0 - way).abs() < EPS {
                    self.pos[other] += way;
                    break;
                }
            }
        }
    }

    fn let_go(&mut self) {
        self.hanging = 0.0;
        self.ledge = None;
    }

    /// Шагнул на низкую ступеньку: встать на её верх, если над головой есть место
    /// (`false` — места нет).
    fn climb_step(&mut self, tuning: &Tuning, solids: &[Collider]) -> bool {
        let b = self.bounds(tuning);
        let mut lift: f64 = 0.0;
        for c in solids {
            if c.oneway || c.is_slope() {
                continue;
            }
            let s = &c.aabb;
            let across = s.min[0] < b.max[0] - EPS && s.max[0] > b.min[0] + EPS;
            let rise = s.max[1] - b.min[1];
            if across && rise > EPS && rise <= tuning.step_up() && s.min[1] < b.min[1] + EPS {
                lift = lift.max(rise);
            }
        }
        if lift > 0.0 {
            let free = solids.iter().all(|c| {
                c.oneway || c.is_slope() || {
                    let s = &c.aabb;
                    // Над ступенькой ничего не мешает голове.
                    !(s.min[0] < b.max[0] - EPS
                        && s.max[0] > b.min[0] + EPS
                        && s.min[1] >= b.max[1] - EPS
                        && s.min[1] < b.max[1] + lift - EPS)
                }
            });
            if !free {
                return false;
            }
            self.pos[1] += lift;
        }
        true
    }

    /// Встать на склон под серединой низа. `snap` — насколько можно опуститься к склону
    /// внизу (бег вниз по склону), 0 — только если провалился в него.
    fn stand_on_slopes(
        &mut self,
        tuning: &Tuning,
        solids: &[Collider],
        foot_before: [f64; 2],
        snap: f64,
    ) {
        if self.vel[1] > 0.0 {
            return;
        }
        let foot = [self.pos[0], self.pos[1] - tuning.half()[1]];
        let mut best: Option<(f64, &Collider)> = None;
        for c in solids.iter().filter(|c| c.is_slope()) {
            let s = &c.aabb;
            if foot[0] < s.min[0] - EPS || foot[0] > s.max[0] + EPS {
                continue;
            }
            let h = c.surface(foot[0]);
            // Сверху: до шага стоял не ниже склона (с запасом на подъём по нему).
            let from_above = foot_before[1] >= c.surface(foot_before[0]) - 0.01;
            // Край прямоугольника мог стоять на углу соседнего блока выше склона.
            let reach = if snap > 0.0 {
                (snap + tuning.half()[0]) * c.steepness() + 0.005
            } else {
                0.0
            };
            let sink = foot[1] < h + EPS;
            let near = foot[1] - h <= reach;
            if from_above && (sink || near) && best.is_none_or(|(b, _)| h > b) {
                best = Some((h, c));
            }
        }
        if let Some((h, c)) = best {
            // Опуститься к склону можно, только если по пути нет ничего твёрдого.
            if foot[1] > h {
                let (down, _) = sweep(self.bounds(tuning), 1, h - foot[1], solids, false);
                if down > h - foot[1] {
                    return;
                }
            }
            self.pos[1] += h - foot[1];
            self.vel[1] = 0.0;
            self.on_ground = true;
            self.ground = Some(c.id);
            self.on_platform = false;
        }
    }

    /// Край, за который можно схватиться со стороны `side`: верх стены на уровне головы
    /// (за этот шаг голова опустилась на `fell`), сверху есть место встать.
    fn find_ledge(
        &self,
        tuning: &Tuning,
        solids: &[Collider],
        side: f64,
        fell: f64,
    ) -> Option<Aabb> {
        let b = self.bounds(tuning);
        let band = (tuning.size[1] * 0.3).max(fell + 0.01);
        let blocked = |area: Aabb| solids.iter().any(|c| !c.oneway && c.aabb.overlaps(&area));
        solids
            .iter()
            .filter(|c| !c.oneway && !c.is_slope() && c.moved == [0.0, 0.0])
            .filter(|c| {
                let s = &c.aabb;
                let face = if side > 0.0 {
                    s.min[0] - b.max[0]
                } else {
                    b.min[0] - s.max[0]
                };
                face.abs() <= WALL_TOUCH
                    && s.max[1] <= b.max[1] + EPS
                    && s.max[1] >= b.max[1] - band
                    && s.min[1] < s.max[1] - EPS
            })
            .map(|c| c.aabb)
            .find(|s| {
                let top = s.max[1];
                // Голове есть куда подняться до виса.
                let head = Aabb {
                    min: [b.min[0], b.max[1]],
                    max: [b.max[0], top + HANG_ABOVE + EPS * 10.0],
                };
                // Наверху есть место встать.
                let edge = if side > 0.0 { s.min[0] } else { s.max[0] };
                let x0 = if side > 0.0 {
                    edge
                } else {
                    edge - tuning.size[0]
                };
                let stand = Aabb {
                    min: [x0 + EPS * 10.0, top + EPS * 10.0],
                    max: [x0 + tuning.size[0] - EPS * 10.0, top + tuning.size[1]],
                };
                (head.max[1] <= head.min[1] || !blocked(head)) && !blocked(stand)
            })
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
/// какой из них остановил. Склоны здесь не держат: их проверяет [`Body::stand_on_slopes`]. Прямоугольники, в которых `b` уже стоит (например, персонажа
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
        if c.is_slope() || (c.oneway && !(platforms && axis == 1 && d < 0.0)) {
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

    /// Пол, холм из двух склонов по 45° с площадкой между ними, как из тайлов: под
    /// склонами сплошные столбики.
    fn hill() -> Vec<Collider> {
        let r = |x0: f64, y0: f64, x1: f64, y1: f64| Aabb {
            min: [x0, y0],
            max: [x1, y1],
        };
        vec![
            Collider::solid(r(-10.0, -1.0, 1.0, 0.0)),
            Collider::slope(r(1.0, -1.0, 1.5, 0.5), 0.5),
            Collider::slope(r(1.5, -1.0, 2.0, 1.0), 0.5),
            Collider::solid(r(2.0, -1.0, 3.0, 1.0)),
            Collider::slope(r(3.0, -1.0, 4.0, 1.0), -1.0),
            Collider::solid(r(4.0, -1.0, 10.0, 0.0)),
        ]
    }

    fn walk(x: f64) -> Intent {
        Intent {
            x,
            ..Intent::default()
        }
    }

    #[test]
    fn walks_over_a_hill_without_leaving_the_ground() {
        let t = Tuning::default();
        let solids = hill();
        let mut b = Body::new([0.0, t.size[1] * 0.5 + 0.05]);
        run(&mut b, &t, &solids, 30, Intent::default());
        assert!(b.on_ground);
        let mut top: f64 = 0.0;
        for i in 0..400 {
            let ev = b.step(&t, walk(1.0), &solids, DT);
            assert!(b.on_ground, "шаг {i}: оторвался от земли в {:?}", b.pos);
            assert!(!ev.landed, "шаг {i}: «приземлился» на ровном месте");
            let foot = b.pos[1] - t.size[1] * 0.5;
            // Середина низа на поверхности (ступенька у края площадки — до step_up).
            let ground = match b.pos[0] {
                x if x < 1.0 => 0.0,
                x if x < 2.0 => x - 1.0,
                x if x < 3.0 => 1.0,
                x if x < 4.0 => 4.0 - x,
                _ => 0.0,
            };
            assert!(
                foot >= ground - 1e-6 && foot <= ground + t.step_up(),
                "шаг {i}: x {:.3} низ {foot:.3}, земля {ground:.3}",
                b.pos[0]
            );
            top = top.max(foot);
            if b.pos[0] > 5.0 {
                break;
            }
        }
        assert!(b.pos[0] > 5.0, "не дошёл: {:?}", b.pos);
        assert!((top - 1.0).abs() < 1e-6, "площадка: {top}");
        // И обратно.
        for _ in 0..400 {
            b.step(&t, walk(-1.0), &solids, DT);
            assert!(b.on_ground, "обратно: оторвался в {:?}", b.pos);
            if b.pos[0] < 0.0 {
                break;
            }
        }
        assert!(b.pos[0] < 0.0);
    }

    #[test]
    fn stands_still_on_a_slope_and_lands_on_it() {
        let t = Tuning::default();
        let solids = hill();
        let mut b = Body::new([1.5, 1.2]);
        run(&mut b, &t, &solids, 90, Intent::default());
        assert!(b.on_ground);
        let foot = b.pos[1] - t.size[1] * 0.5;
        assert!((foot - 0.5).abs() < 1e-6, "{foot}");
        let x = b.pos[0];
        run(&mut b, &t, &solids, 120, Intent::default());
        assert!((b.pos[0] - x).abs() < 1e-9, "сполз: {:?}", b.pos);
        assert!((b.pos[1] - t.size[1] * 0.5 - 0.5).abs() < 1e-6);
        // Прыжок со склона.
        b.step(&t, jump_now(), &solids, DT);
        assert!(!b.on_ground && b.vel[1] > 0.0);
    }

    #[test]
    fn steps_up_low_ledges_but_not_high_ones() {
        let t = Tuning::default();
        let mut solids = floor();
        solids.push(Collider::solid(Aabb {
            min: [0.5, -0.5],
            max: [1.0, 0.05],
        }));
        solids.push(Collider::solid(Aabb {
            min: [2.0, -0.5],
            max: [2.5, 0.2],
        }));
        let mut b = standing(&t);
        run(&mut b, &t, &solids, 50, walk(1.0));
        assert!(b.pos[0] > 1.0 && b.on_ground, "{:?}", b.pos);
        run(&mut b, &t, &solids, 120, walk(1.0));
        // Высокий блок — стена.
        assert!(
            (b.bounds(&t).max[0] - 2.0).abs() < 1e-9,
            "прошёл сквозь: {:?}",
            b.pos
        );
        assert!((b.bounds(&t).min[1]).abs() < 1e-9);
    }

    /// Пол и высокий блок справа: верх на 0.7, выше прыжка.
    fn cliff() -> Vec<Collider> {
        let mut s = floor();
        s.push(Collider::solid(Aabb {
            min: [1.0, -1.0],
            max: [2.0, 0.7],
        }));
        s
    }

    fn hang_on_cliff(t: &Tuning, solids: &[Collider]) -> Body {
        let mut b = standing(t);
        b.pos[0] = 0.7;
        let toward = Intent {
            x: 1.0,
            jump_held: true,
            ..Intent::default()
        };
        b.step(
            t,
            Intent {
                jump_pressed: true,
                ..toward
            },
            solids,
            DT,
        );
        let mut grabbed = false;
        for _ in 0..120 {
            if b.step(t, toward, solids, DT).grabbed {
                grabbed = true;
                break;
            }
        }
        assert!(grabbed, "не схватился: {:?}", b.pos);
        b
    }

    #[test]
    fn grabs_a_ledge_hangs_and_climbs() {
        let t = Tuning::default();
        let solids = cliff();
        let mut b = hang_on_cliff(&t, &solids);
        assert_eq!(b.hanging, 1.0);
        let top = b.bounds(&t).max[1];
        assert!((top - (0.7 + HANG_ABOVE)).abs() < 1e-9, "{top}");
        assert!((b.bounds(&t).max[0] - 1.0).abs() < 1e-9);
        // Висит, сколько угодно.
        let hold = walk(1.0);
        run(&mut b, &t, &solids, 120, hold);
        assert_eq!(b.hanging, 1.0);
        assert!((b.bounds(&t).max[1] - top).abs() < 1e-12);
        // Вверх — стоит на краю.
        let ev = b.step(&t, Intent { up: true, ..hold }, &solids, DT);
        assert!(ev.climbed && b.on_ground && b.hanging == 0.0);
        run(&mut b, &t, &solids, 30, Intent::default());
        assert!(b.on_ground);
        assert!((b.bounds(&t).min[1] - 0.7).abs() < 1e-9, "{:?}", b.pos);
        assert!(b.pos[0] > 1.0);
    }

    #[test]
    fn jumps_from_a_ledge_or_lets_go() {
        let t = Tuning::default();
        let solids = cliff();
        let mut b = hang_on_cliff(&t, &solids);
        // Прыжок с края к стене — оказывается наверху.
        let toward = Intent {
            x: 1.0,
            jump_held: true,
            ..Intent::default()
        };
        assert!(
            b.step(
                &t,
                Intent {
                    jump_pressed: true,
                    ..toward
                },
                &solids,
                DT
            )
            .jumped
        );
        for _ in 0..90 {
            if b.step(&t, toward, &solids, DT).landed {
                break;
            }
        }
        assert!(b.on_ground && b.bounds(&t).min[1] > 0.69, "{:?}", b.pos);

        // Вниз — отпускает и падает на пол, сразу снова не хватается.
        let mut b = hang_on_cliff(&t, &solids);
        b.step(
            &t,
            Intent {
                down: true,
                x: 1.0,
                ..Intent::default()
            },
            &solids,
            DT,
        );
        assert_eq!(b.hanging, 0.0);
        let mut grabbed = false;
        for _ in 0..90 {
            grabbed |= b.step(&t, walk(1.0), &solids, DT).grabbed;
        }
        assert!(!grabbed && b.on_ground, "{:?}", b.pos);
    }

    #[test]
    fn no_grab_without_room_on_top() {
        let t = Tuning::default();
        let mut solids = cliff();
        // Над краем низкий потолок: встать некуда.
        solids.push(Collider::solid(Aabb {
            min: [1.0, 0.8],
            max: [2.0, 1.0],
        }));
        let mut b = standing(&t);
        b.pos[0] = 0.7;
        let toward = Intent {
            x: 1.0,
            jump_held: true,
            ..Intent::default()
        };
        b.step(
            &t,
            Intent {
                jump_pressed: true,
                ..toward
            },
            &solids,
            DT,
        );
        for _ in 0..120 {
            assert!(!b.step(&t, toward, &solids, DT).grabbed, "{:?}", b.pos);
        }
    }

    #[test]
    fn lift_coming_down_does_not_swallow_the_body() {
        let t = Tuning::default();
        let solids = floor();
        let mut lift = Collider {
            id: 9,
            ..Collider::solid(Aabb {
                min: [-0.3, 0.5],
                max: [0.3, 0.56],
            })
        };
        let mut b = standing(&t);
        for i in 0..50 {
            lift.aabb.min[1] -= 0.01;
            lift.aabb.max[1] -= 0.01;
            lift.moved = [0.0, -0.01];
            let mut all = solids.clone();
            all.push(lift);
            b.step(&t, Intent::default(), &all, DT);
            assert!(
                !b.bounds(&t).overlaps(&lift.aabb),
                "шаг {i}: внутри подъёмника {:?}",
                b.pos
            );
        }
        // Выдавило вбок, стоит на полу.
        assert!(
            b.on_ground && b.bounds(&t).min[1].abs() < 1e-9,
            "{:?}",
            b.pos
        );
        assert!(b.pos[0].abs() > 0.3, "{:?}", b.pos);
    }

    #[test]
    fn wall_moving_sideways_pushes_the_body() {
        let t = Tuning::default();
        let solids = floor();
        let mut wall = Collider {
            id: 5,
            ..Collider::solid(Aabb {
                min: [0.2, 0.0],
                max: [0.4, 1.0],
            })
        };
        let mut b = standing(&t);
        for _ in 0..30 {
            wall.aabb.min[0] -= 0.01;
            wall.aabb.max[0] -= 0.01;
            wall.moved = [-0.01, 0.0];
            let mut all = solids.clone();
            all.push(wall);
            b.step(&t, Intent::default(), &all, DT);
            assert!(!b.bounds(&t).overlaps(&wall.aabb), "{:?}", b.pos);
        }
        assert!((b.bounds(&t).max[0] - wall.aabb.min[0]).abs() < 1e-9);
        assert!(b.on_ground);
    }
}
