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
//! Столкновения (9.2): персонаж — прямоугольник, который спрашивает мир столкновений
//! ([`Space`]): на сколько можно сдвинуться, что под ногами, занято ли место. В мире —
//! настоящие формы: уровень, склоны, ящики физики любой формы и поворота. Сначала
//! движение по горизонтали, потом по вертикали; путь обрезается у ближайшей поверхности,
//! поэтому персонаж не проскакивает стену при любой скорости и встаёт вплотную (в
//! [`crate::space::SKIN`] от неё).
//!
//! Наклоны (6.4, 9.2): пологая поверхность (склон, повёрнутый ящик, круглый бок)
//! держит середину низа персонажа, а не весь прямоугольник: так он не висит над
//! склоном краем. Бегущий вниз по склону прижимается к нему, а не слетает
//! ступеньками; маленькую ступеньку (до половины ширины) на земле перешагивает сам.
//!
//! Цепляние за край (6.4): в падении, прижавшись к стене, персонаж хватается за её верх,
//! если тот на уровне головы и сверху есть место встать. Вверх — забраться, прыжок —
//! прыгнуть вверх, вниз или от стены — отпустить.

use crate::space::{self, Filter, Space};
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

    /// Настройки в области, где тяжесть в `gravity` раз другая (9.6б): толчок прыжка тот
    /// же, поэтому при слабой тяжести прыжок выше и дольше, падение медленнее. Тянуть
    /// вверх персонажа нельзя: меньше 0,05 не бывает.
    pub fn in_gravity(&self, gravity: f64) -> Self {
        let k = gravity.max(0.05);
        if (k - 1.0).abs() < 1e-9 {
            return *self;
        }
        Self {
            jump: self.jump / k,
            jump_time: self.jump_time / k,
            max_fall: self.max_fall * k.sqrt(),
            ..*self
        }
    }

    pub fn half(&self) -> [f64; 2] {
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
    /// Край, за который держится.
    ledge: Option<Ledge>,
    /// Забирается на край.
    climb: Option<Climb>,
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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
    /// Упёрся сбоку в предмет, который можно толкать ([`crate::Surface::pushable`]):
    /// его номер и с какой скоростью персонаж хотел идти (со знаком). Толкает мир сцены:
    /// он знает массу предмета.
    pub pushed: Option<(u64, f64)>,
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
/// Сколько забирается на край, секунд.
const CLIMB_TIME: f64 = 0.2;

/// Край, за который держится персонаж: верх стены и где у неё бок.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ledge {
    top: f64,
    edge: f64,
}

/// Забирание на край: откуда, куда и сколько уже прошло.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Climb {
    from: [f64; 2],
    to: [f64; 2],
    t: f64,
}

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
            climb: None,
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

    /// Один шаг длиной `dt` секунд в мире столкновений `space`.
    pub fn step(&mut self, tuning: &Tuning, intent: Intent, space: &Space, dt: f64) -> StepEvents {
        let mut ev = StepEvents::default();
        if dt <= 0.0 {
            return ev;
        }
        let half = tuning.half();
        // Опора поехала или повернулась — едем с ней (сначала, чтобы шаг считался уже
        // от нового места). Точка опоры — середина низа.
        if self.on_ground
            && let Some(id) = self.ground
            && space.get(id).is_some_and(|g| g.moving())
        {
            let foot = [self.pos[0], self.pos[1] - half[1]];
            let mut d = space.carry(id, foot);
            if space.get(id).is_some_and(|g| g.pushable) {
                // Тело физики под ногами (ящик, шар) несёт не быстрее бега: на ящике,
                // который отбросило ударом, не удержаться — ноги проскальзывают.
                let most = tuning.speed * dt;
                let len = d[0].hypot(d[1]);
                if len > most {
                    d = [d[0] * most / len, d[1] * most / len];
                }
            }
            let skip = [id];
            let f = Filter::default().skip(&skip);
            self.pos[0] += space.sweep(self.bounds(tuning), 0, d[0], &f).0;
            // Вверх — не сквозь потолок (ящик под ногами подбросило к полке).
            self.pos[1] += if d[1] > 0.0 {
                space.sweep(self.bounds(tuning), 1, d[1], &f).0
            } else {
                d[1]
            };
        }
        self.push_out(tuning, space);
        self.wall_lock -= dt;
        self.drop_left -= dt;
        self.grab_cooldown -= dt;
        let x = intent.x.clamp(-1.0, 1.0);

        // Забирается на край: движение по ходу, управление не слушается.
        if let Some(mut climb) = self.climb {
            climb.t += dt;
            let k = (climb.t / CLIMB_TIME).min(1.0);
            // Первые 60% времени — вверх, остальное — вбок на край.
            let up = (k / 0.6).min(1.0);
            let over = ((k - 0.6) / 0.4).clamp(0.0, 1.0);
            self.pos = [
                climb.from[0] + (climb.to[0] - climb.from[0]) * over,
                climb.from[1] + (climb.to[1] - climb.from[1]) * up,
            ];
            self.vel = [0.0, 0.0];
            if k >= 1.0 {
                self.climb = None;
                self.let_go();
                self.on_ground = true;
                self.coyote_left = tuning.coyote;
                ev.climbed = true;
            } else {
                self.climb = Some(climb);
            }
            return ev;
        }

        // Висит на краю: стоит на месте, пока не нажмут.
        if let Some(ledge) = self.ledge {
            let side = self.hanging;
            if intent.jump_pressed {
                self.let_go();
                self.vel = [0.0, tuning.jump_speed()];
                self.grab_cooldown = GRAB_COOLDOWN;
                ev.jumped = true;
            } else if intent.up {
                // Встаёт на край у самой стены. Забирается за CLIMB_TIME: сначала вверх
                // вдоль стены, потом на край.
                let stand = ledge.edge + side * (half[0] + 0.005);
                self.climb = Some(Climb {
                    from: self.pos,
                    to: [stand, ledge.top + half[1] + space::SKIN],
                    t: 0.0,
                });
                self.ledge = None;
                self.vel = [0.0, 0.0];
                // Дальше в этом шаге ничего: иначе тяжесть чуть опустит его, и
                // зажатая к стене стрелка снова схватит тот же край. Забравшись, он
                // «висел» бы на нём с нулевой стороной и с зажатой стрелкой вверх
                // забирался ещё раз — проезжал до дальнего конца края.
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
            let walls = Filter::default().normals(space::wall);
            for side in [-1.0, 1.0] {
                if space
                    .sweep(self.bounds(tuning), 0, side * WALL_TOUCH, &walls)
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
        // ступеньки ниже `step_up` и встаёт на неё; пологий склон фигуры (повёрнутый
        // ящик, холм) — та же ступенька. Склоны тайлов держат середину ног (ниже,
        // `stand_on_slopes`), остальное — сам прямоугольник.
        let walking = self.on_ground && self.vel[1] <= 0.0;
        let foot_before = [self.pos[0], self.pos[1] - half[1]];
        let dx = self.vel[0] * dt;
        let mut probe = self.bounds(tuning);
        if walking {
            probe.min[1] += tuning.step_up();
        }
        let across = Filter::default();
        let (mut moved_x, mut by_x) = space.sweep(probe, 0, dx, &across);
        if walking && by_x.is_some_and(|h| space::walkable(h.normal)) {
            // Вверх по пологому: шаг целиком, а `climb_step` поднимает на склон.
            moved_x = dx;
            by_x = None;
        }
        self.pos[0] += moved_x;
        if walking && !self.climb_step(tuning, space) {
            // На ступеньку не встать (над ней низко): упёрся, как в стену.
            self.pos[0] -= moved_x;
            (moved_x, by_x) = space.sweep(self.bounds(tuning), 0, dx, &across);
            self.pos[0] += moved_x;
        }
        let pushing = by_x.filter(|h| space.surfaces()[h.index].pushable);
        if let Some(h) = pushing
            && x != 0.0
        {
            // Толкает: стоит вплотную и идёт со скоростью предмета (тяжёлый или
            // упёршийся в стену не сдвинется — персонаж тоже стоит). Желаемая скорость
            // уходит миру сцены, он разгоняет предмет.
            let s = &space.surfaces()[h.index];
            let along = (s.moved[0] / dt) * x.signum();
            self.vel[0] = x.signum() * along.clamp(0.0, self.vel[0].abs());
            ev.pushed = Some((h.id, x * tuning.speed));
        } else if (moved_x - dx).abs() > EPS {
            self.vel[0] = 0.0;
            ev.hit_wall = true;
            self.dash_left = 0.0;
        }
        let dy = self.vel[1] * dt;
        let platforms = self.drop_left <= 0.0;
        let vertical = Filter::default().platforms(platforms && dy < 0.0);
        let (moved, by) = space.sweep(Aabb::around(self.pos, half), 1, dy, &vertical);
        let was_on_ground = self.on_ground;
        self.on_ground = false;
        self.ground = None;
        self.on_platform = false;
        self.pos[1] += moved;
        self.take_vertical_hit(by, dy, moved, half, space, &vertical, &mut ev);

        // Пологие поверхности держат середину ног; бегущего по земле прижимают, а не
        // дают слететь.
        let snap = if walking { moved_x.abs() + 0.005 } else { 0.0 };
        self.stand_on_slopes(tuning, space, foot_before, snap, platforms);
        if walking && !self.on_ground && snap > 0.0 {
            // Сошёл со склона на ровное или с ровного чуть ниже — тоже прижимается.
            let f = Filter::default().platforms(platforms);
            let (down, by) = space.sweep(self.bounds(tuning), 1, -snap, &f);
            if let Some(hit) = by.filter(|h| space::walkable(h.normal)) {
                self.pos[1] += down;
                self.on_ground = true;
                self.ground = Some(hit.id);
                self.on_platform = hit.oneway;
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
            && let Some(ledge) = self.find_ledge(tuning, space, x.signum(), -moved)
        {
            self.hanging = x.signum();
            self.ledge = Some(ledge);
            self.pos[1] = ledge.top + HANG_ABOVE - half[1];
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

    /// Чем кончился сдвиг по вертикали: встал на ровное, соскальзывает по крутому или
    /// ударился головой.
    #[allow(clippy::too_many_arguments)]
    fn take_vertical_hit(
        &mut self,
        by: Option<space::Hit>,
        dy: f64,
        moved: f64,
        half: [f64; 2],
        space: &Space,
        vertical: &Filter,
        ev: &mut StepEvents,
    ) {
        if let Some(hit) = by {
            if dy < 0.0 && space::walkable(hit.normal) {
                self.on_ground = true;
                self.ground = Some(hit.id);
                self.on_platform = hit.oneway;
                self.vel[1] = 0.0;
            } else if dy < 0.0 {
                // Крутая поверхность (круче, чем можно идти): не опора, по ней
                // соскальзывают вниз вдоль неё.
                let n = hit.normal;
                let t = if n[0] > 0.0 {
                    [n[1], -n[0]]
                } else {
                    [-n[1], n[0]]
                };
                let rest = dy - moved;
                let along = rest * t[1];
                let (k, _) = space.cast(
                    Aabb::around(self.pos, half),
                    [t[0] * along, t[1] * along],
                    vertical,
                );
                self.pos[0] += t[0] * along * k;
                self.pos[1] += t[1] * along * k;
                let apart = space.keep_apart(Aabb::around(self.pos, half), hit.index);
                self.pos[0] += apart[0];
                self.pos[1] += apart[1];
                self.vel[1] *= t[1] * t[1];
            } else {
                ev.hit_ceiling = true;
                self.vel[1] = 0.0;
            }
        }
    }

    /// Движущийся предмет въехал в персонажа (подъёмник опустился на голову, стена
    /// поехала вбок, ящик качнулся): вытолкнуть его от предмета, а если там тесно —
    /// вдоль предмета к ближнему краю. Иначе персонаж оказался бы внутри и проходил бы
    /// сквозь предмет.
    fn push_out(&mut self, tuning: &Tuning, space: &Space) {
        for (i, s) in space.surfaces().iter().enumerate() {
            if !s.moving() || s.oneway {
                continue;
            }
            let b = self.bounds(tuning);
            let Some((n, depth)) = space.contact(b, i) else {
                continue;
            };
            if space.overlapped_before(b, i) {
                // Был внутри и раньше (поставили так): выйдет сам, как из стены.
                continue;
            }
            let skip = [s.id];
            let others = Filter::default().skip(&skip);
            let push = depth + space::SKIN;
            let way = [n[0] * push, n[1] * push];
            let (t, _) = space.cast(b, way, &others);
            if t >= 1.0 - 1e-6 {
                self.pos[0] += way[0];
                self.pos[1] += way[1];
                if n[1] > 0.7 {
                    self.on_ground = true;
                    self.ground = Some(s.id);
                    self.vel[1] = self.vel[1].max(0.0);
                } else if n[1] < -0.7 {
                    self.vel[1] = self.vel[1].min(0.0);
                } else {
                    self.vel[0] = 0.0;
                }
                continue;
            }
            // Зажало: вбок к ближнему краю предмета, если там свободно.
            let axis = if n[1].abs() > n[0].abs() { 0 } else { 1 };
            let mut ways = [
                s.aabb.min[axis] - b.max[axis] - space::SKIN,
                s.aabb.max[axis] - b.min[axis] + space::SKIN,
            ];
            ways.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
            for w in ways {
                if (space.sweep(b, axis, w, &others).0 - w).abs() < EPS {
                    self.pos[axis] += w;
                    break;
                }
            }
        }
    }

    fn let_go(&mut self) {
        self.hanging = 0.0;
        self.ledge = None;
    }

    /// Забирается на край (6.8: своя поза).
    pub fn climbing(&self) -> bool {
        self.climb.is_some()
    }

    /// Висит на краю или забирается на него.
    pub fn on_ledge(&self) -> bool {
        self.hanging != 0.0
    }

    /// На чём стоит: номер поверхности ([`crate::Surface::id`]).
    pub fn ground(&self) -> Option<u64> {
        self.ground.filter(|_| self.on_ground)
    }

    /// Шагнул на низкую ступеньку: встать на её верх, если над головой есть место
    /// (`false` — места нет).
    fn climb_step(&mut self, tuning: &Tuning, space: &Space) -> bool {
        let b = self.bounds(tuning);
        let up = tuning.step_up();
        // Сверху вниз от поднятого на высоту ступеньки места: где ровный верх.
        let raised = Aabb {
            min: [b.min[0], b.min[1] + up],
            max: [b.max[0], b.max[1] + up],
        };
        let (down, hit) = space.sweep(raised, 1, -up, &Filter::default());
        let lift = up + down;
        if !hit.is_some_and(|h| space::walkable(h.normal)) {
            // Опоры нет (крутой край). Поднятый прямоугольник прошёл
            // над препятствием, но сам персонаж мог упереться в его бок под скруглённым
            // краем (банка со скруглённой крышкой): вошёл в стену — не прошёл. В склон
            // (нормаль пологая) входить можно: его держит середина ног.
            let walled = space.surfaces().iter().enumerate().any(|(i, s)| {
                !s.oneway && !s.slope && space.contact(b, i).is_some_and(|(n, _)| space::wall(n))
            });
            return !walled;
        }
        if lift <= 0.0 {
            return true;
        }
        // На ступеньке персонаж ни во что не входит: ни головой, ни боком (выступ
        // над ступенькой, например крышка банки шире самой банки).
        let lifted = Aabb {
            min: [b.min[0], b.min[1] + lift],
            max: [b.max[0], b.max[1] + lift],
        };
        if space.overlaps(lifted, &Filter::default()) {
            return false;
        }
        self.pos[1] += lift;
        true
    }

    /// Встать на пологую поверхность под серединой низа. `snap` — насколько можно
    /// опуститься к ней внизу (бег вниз по склону), 0 — только если провалился в неё.
    fn stand_on_slopes(
        &mut self,
        tuning: &Tuning,
        space: &Space,
        foot_before: [f64; 2],
        snap: f64,
        platforms: bool,
    ) {
        // Взлетающий не встаёт на склон, но и не входит в него, если вбок движется
        // быстрее, чем вверх (рывок в прыжке вверх по склону).
        let rising = self.vel[1] > 0.0;
        let snap = if rising { 0.0 } else { snap };
        let half = tuning.half();
        let foot = [self.pos[0], self.pos[1] - half[1]];
        // Сверху: луч из точки чуть выше ног до шага (с запасом на подъём по склону за
        // шаг), только по пологим поверхностям.
        let climb = (foot[0] - foot_before[0]).abs() * 2.0 + 0.01;
        let from = foot_before[1].max(foot[1]) + climb;
        let reach_max = (snap + half[0]) * 2.0 + 0.005;
        let f = Filter::default()
            .platforms(platforms)
            // Ровное тоже: на стыке двух склонов луч попадает в угол, а у угла нормаль
            // смотрит прямо вверх.
            .normals(|n| n[1] > 0.5);
        let Some((h, hit)) = space.ray_down([foot[0], from], from - foot[1] + reach_max, &f) else {
            return;
        };
        // Середину ног держат склоны тайлов и ровное; пологие фигуры держат сам
        // прямоугольник (выше, при сдвиге по вертикали).
        if !space.surfaces()[hit.index].slope && !space::floor(hit.normal) {
            return;
        }
        // Край прямоугольника мог стоять на углу соседнего блока выше склона.
        let steep = (hit.normal[0] / hit.normal[1]).abs();
        let reach = if snap > 0.0 {
            (snap + half[0]) * steep + 0.005
        } else {
            0.0
        };
        let sink = foot[1] < h + EPS;
        let near = foot[1] - h <= reach;
        if !(sink || near) {
            return;
        }
        // Опуститься к склону можно, только если по пути нет ничего твёрдого.
        if foot[1] > h {
            let solid = Filter::default();
            let (down, by) = space.sweep(self.bounds(tuning), 1, h - foot[1], &solid);
            if by.is_some() && down > h - foot[1] {
                return;
            }
        }
        if h > foot[1] {
            // Подняться на склон можно, только если голове есть место (склон тела
            // физики под полкой). Сам склон не в счёт: в него угол прямоугольника входит.
            let skip = [hit.id];
            let f = Filter::default().skip(&skip);
            let (up, by) = space.sweep(self.bounds(tuning), 1, h - foot[1], &f);
            if by.is_some() && up < h - foot[1] {
                return;
            }
        }
        self.pos[1] += h - foot[1];
        if rising {
            return;
        }
        self.vel[1] = 0.0;
        self.on_ground = true;
        self.ground = Some(hit.id);
        self.on_platform = hit.oneway;
    }

    /// Край, за который можно схватиться со стороны `side`: верх неподвижной стены на
    /// уровне головы (за этот шаг голова опустилась на `fell`), сверху есть место встать.
    fn find_ledge(&self, tuning: &Tuning, space: &Space, side: f64, fell: f64) -> Option<Ledge> {
        let b = self.bounds(tuning);
        let band = (tuning.size[1] * 0.3).max(fell + 0.01);
        // Стена вплотную сбоку.
        let walls = Filter::default().normals(space::wall);
        let (d, hit) = space.sweep(b, 0, side * WALL_TOUCH * 2.0, &walls);
        let hit = hit?;
        let wall = &space.surfaces()[hit.index];
        if wall.moving() || wall.oneway {
            return None;
        }
        let edge = if side > 0.0 {
            b.max[0] + d
        } else {
            b.min[0] + d
        };
        // Верх стены: луч сверху чуть за её боком, в полосе у головы.
        let inset = (tuning.size[0] * 0.25).min(0.02);
        let x = edge + side * inset;
        let flat = Filter::default().normals(|n| n[1] > 0.9);
        let (top, top_hit) = space.ray_down([x, b.max[1] + EPS * 10.0], band, &flat)?;
        if top_hit.id != hit.id && space.surfaces()[top_hit.index].moving() {
            return None;
        }
        // Наверху есть место встать.
        let x0 = if side > 0.0 {
            edge
        } else {
            edge - tuning.size[0]
        };
        let stand = Aabb {
            min: [x0 + EPS * 10.0, top + EPS * 10.0],
            max: [x0 + tuning.size[0] - EPS * 10.0, top + tuning.size[1]],
        };
        // Путь забирания: вверх вдоль стены до роста над краем.
        let rise = Aabb {
            min: [b.min[0], b.max[1]],
            max: [b.max[0], top + tuning.size[1]],
        };
        // Висящему самому есть место: край ниже головы опустит его, а снизу может быть
        // пол (низкий ящик — на него просто встают).
        let shift = top + HANG_ABOVE - b.max[1];
        let hang = Aabb {
            min: [b.min[0], b.min[1] + shift],
            max: [b.max[0], b.max[1] + shift],
        };
        let free = Filter::default().slopes(true);
        if space.overlaps(rise, &free)
            || space.overlaps(stand, &free)
            || space.overlaps(hang, &free)
        {
            return None;
        }
        Some(Ledge { top, edge })
    }
}

fn approach(v: f64, target: f64, by: f64) -> f64 {
    if v < target {
        (v + by).min(target)
    } else {
        (v - by).max(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Допуск на место: мир столкновений считает формы в f32 и останавливает в зазоре.
    const TOL: f64 = 3.0 * crate::space::SKIN;

    const DT: f64 = 1.0 / 60.0;

    fn floor() -> Vec<Collider> {
        vec![Collider::solid(Aabb {
            min: [-10.0, -1.0],
            max: [10.0, 0.0],
        })]
    }

    fn run(body: &mut Body, t: &Tuning, solids: &[Collider], steps: usize, intent: Intent) {
        for _ in 0..steps {
            body.step(t, intent, &Space::of(solids), DT);
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
        assert!((b.bounds(&t).min[1]).abs() < TOL, "{:?}", b.pos);
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
            &Space::of(&floor()),
            DT,
        );
        for _ in 0..120 {
            b.step(&t, held, &Space::of(&floor()), DT);
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
            &Space::of(&floor()),
            DT,
        );
        let mut top: f64 = ground;
        for _ in 0..120 {
            b.step(&t, Intent::default(), &Space::of(&floor()), DT);
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
            b.step(&t, right, &Space::of(&ledge), DT);
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
            &Space::of(&ledge),
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
            &Space::of(&ledge),
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
            &Space::of(&floor()),
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
                &Space::of(&floor()),
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
            &Space::of(&solids),
            DT,
        );
        assert!(ev.hit_wall);
        assert!((b.bounds(&t).max[0] - 0.5).abs() < TOL);
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
            &Space::of(&solids),
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
                    &Space::of(&solids),
                    DT,
                )
                .hit_ceiling;
        }
        assert!(bumped);
        assert!(b.bounds(&t).max[1] <= 0.4 + TOL);
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
        b.step(&t, jump_now(), &Space::of(&solids), DT);
        let mut hit = false;
        for _ in 0..90 {
            hit |= b.step(&t, held_jump(), &Space::of(&solids), DT).hit_ceiling;
        }
        assert!(!hit, "головой о платформу биться не должен");
        assert!(b.on_ground);
        assert!((b.bounds(&t).min[1] - 0.35).abs() < TOL, "{:?}", b.pos);
        // Вниз + прыжок — спрыгивает сквозь неё на пол.
        let ev = b.step(
            &t,
            Intent {
                down: true,
                ..jump_now()
            },
            &Space::of(&solids),
            DT,
        );
        assert!(ev.dropped && !ev.jumped);
        run(&mut b, &t, &solids, 60, Intent::default());
        assert!(b.on_ground);
        assert!(b.bounds(&t).min[1].abs() < TOL, "{:?}", b.pos);
        // Вбок сквозь платформу тоже проходишь.
        assert!(
            !b.step(
                &t,
                Intent {
                    x: 1.0,
                    ..Intent::default()
                },
                &Space::of(&solids),
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
            b.step(&t, Intent::default(), &Space::of(&[lift]), DT);
            assert!(b.on_ground);
        }
        assert!((b.pos[0] - 0.3).abs() < TOL, "{:?}", b.pos);
        assert!((b.bounds(&t).min[1] - lift.aabb.max[1]).abs() < TOL);
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
        assert!(b.step(&t, dash, &Space::of(&floor()), DT).dashed);
        assert!(b.dashing);
        let x0 = b.pos[0];
        run(&mut b, &t, &floor(), 20, Intent::default());
        let dist = b.pos[0] - x0;
        // Рывок проходит примерно dash_speed × dash_time (плюс торможение после).
        assert!(dist > t.dash_speed * t.dash_time * 0.8, "{dist}");
        assert!(!b.dashing);
        // В воздухе — один рывок до касания земли.
        b.step(&t, jump_now(), &Space::of(&floor()), DT);
        run(&mut b, &t, &floor(), 5, held_jump());
        assert!(b.step(&t, dash, &Space::of(&floor()), DT).dashed);
        run(&mut b, &t, &floor(), 12, Intent::default());
        assert!(!b.on_ground);
        assert!(
            !b.step(&t, dash, &Space::of(&floor()), DT).dashed,
            "второй рывок в воздухе"
        );
        run(&mut b, &t, &floor(), 120, Intent::default());
        assert!(b.on_ground);
        assert!(b.step(&t, dash, &Space::of(&floor()), DT).dashed);
    }

    #[test]
    fn weaker_gravity_keeps_the_jump_push_and_jumps_higher() {
        let t = Tuning::default();
        let light = t.in_gravity(0.5);
        assert!((light.jump_speed() - t.jump_speed()).abs() < 1e-9);
        assert!((light.gravity() - 0.5 * t.gravity()).abs() < 1e-9);
        assert!((light.jump - 2.0 * t.jump).abs() < 1e-9);
        assert_eq!(t.in_gravity(1.0), t);
        assert!(t.in_gravity(-3.0).gravity() > 0.0);
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
            &Space::of(&solids),
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
            let ev = b.step(&t, walk(1.0), &Space::of(&solids), DT);
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
                foot >= ground - TOL && foot <= ground + t.step_up(),
                "шаг {i}: x {:.3} низ {foot:.3}, земля {ground:.3}",
                b.pos[0]
            );
            top = top.max(foot);
            if b.pos[0] > 5.0 {
                break;
            }
        }
        assert!(b.pos[0] > 5.0, "не дошёл: {:?}", b.pos);
        assert!((top - 1.0).abs() < TOL, "площадка: {top}");
        // И обратно.
        for _ in 0..400 {
            b.step(&t, walk(-1.0), &Space::of(&solids), DT);
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
        assert!((foot - 0.5).abs() < TOL, "{foot}");
        let x = b.pos[0];
        run(&mut b, &t, &solids, 120, Intent::default());
        assert!((b.pos[0] - x).abs() < TOL, "сполз: {:?}", b.pos);
        assert!((b.pos[1] - t.size[1] * 0.5 - 0.5).abs() < TOL);
        // Прыжок со склона.
        b.step(&t, jump_now(), &Space::of(&solids), DT);
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
            (b.bounds(&t).max[0] - 2.0).abs() < TOL,
            "прошёл сквозь: {:?}",
            b.pos
        );
        assert!((b.bounds(&t).min[1]).abs() < TOL);
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
            &Space::of(solids),
            DT,
        );
        let mut grabbed = false;
        for _ in 0..120 {
            if b.step(t, toward, &Space::of(solids), DT).grabbed {
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
        assert!((top - (0.7 + HANG_ABOVE)).abs() < TOL, "{top}");
        assert!((b.bounds(&t).max[0] - 1.0).abs() < TOL);
        // Висит, сколько угодно.
        let hold = walk(1.0);
        run(&mut b, &t, &solids, 120, hold);
        assert_eq!(b.hanging, 1.0);
        assert!((b.bounds(&t).max[1] - top).abs() < TOL);
        // Вверх — забирается (за 0,2 с, без скачков и сквозь блок) и стоит на краю.
        let mut climbed = false;
        for _ in 0..30 {
            let before = b.pos;
            let ev = b.step(&t, Intent { up: true, ..hold }, &Space::of(&solids), DT);
            let moved = (b.pos[0] - before[0]).hypot(b.pos[1] - before[1]);
            assert!(moved < 0.06, "скачок на {moved}");
            for c in &solids {
                assert!(!b.bounds(&t).overlaps(&c.aabb), "сквозь блок: {:?}", b.pos);
            }
            if ev.climbed {
                climbed = true;
                break;
            }
        }
        assert!(climbed && b.on_ground && b.hanging == 0.0);
        run(&mut b, &t, &solids, 30, Intent::default());
        assert!(b.on_ground);
        assert!((b.bounds(&t).min[1] - 0.7).abs() < TOL, "{:?}", b.pos);
        assert!(b.pos[0] > 1.0);
    }

    #[test]
    fn climbing_with_toward_and_up_held_does_not_climb_again() {
        // Повис со стрелкой к стене, нажал вверх и держит обе. Раньше герой во время
        // забирания снова хватался за тот же край и, встав, забирался ещё раз —
        // проезжал по краю без скорости до его дальнего конца. Теперь он встаёт у
        // стены и дальше просто идёт: скорость растёт как при обычном беге.
        let t = Tuning::default();
        let solids = cliff();
        let mut b = hang_on_cliff(&t, &solids);
        let both = Intent {
            up: true,
            ..walk(1.0)
        };
        let mut climbs = 0;
        for _ in 0..120 {
            let before = b.pos;
            let ev = b.step(&t, both, &Space::of(&solids), DT);
            climbs += usize::from(ev.climbed);
            if climbs > 0 {
                // На краю: каждый шаг не дальше, чем позволяет скорость бега.
                let dx = b.pos[0] - before[0];
                assert!(
                    dx <= b.vel[0].max(0.0) * DT + 1e-9,
                    "проехал {dx} при {:?}",
                    b.vel
                );
                assert!(b.on_ground || b.pos[0] > 2.0, "{:?}", b.pos);
            }
        }
        assert_eq!(climbs, 1);
        assert!(!b.climbing() && !b.on_ledge());
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
                &Space::of(&solids),
                DT
            )
            .jumped
        );
        for _ in 0..90 {
            if b.step(&t, toward, &Space::of(&solids), DT).landed {
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
            &Space::of(&solids),
            DT,
        );
        assert_eq!(b.hanging, 0.0);
        let mut grabbed = false;
        for _ in 0..90 {
            grabbed |= b.step(&t, walk(1.0), &Space::of(&solids), DT).grabbed;
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
            &Space::of(&solids),
            DT,
        );
        for _ in 0..120 {
            assert!(
                !b.step(&t, toward, &Space::of(&solids), DT).grabbed,
                "{:?}",
                b.pos
            );
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
            b.step(&t, Intent::default(), &Space::of(&all), DT);
            assert!(
                !b.bounds(&t).overlaps(&lift.aabb),
                "шаг {i}: внутри подъёмника {:?}",
                b.pos
            );
        }
        // Выдавило вбок, стоит на полу.
        assert!(
            b.on_ground && b.bounds(&t).min[1].abs() < TOL,
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
            b.step(&t, Intent::default(), &Space::of(&all), DT);
            assert!(!b.bounds(&t).overlaps(&wall.aabb), "{:?}", b.pos);
        }
        assert!(
            (b.bounds(&t).max[0] - wall.aabb.min[0]).abs() < TOL,
            "{} {}",
            b.bounds(&t).max[0],
            wall.aabb.min[0]
        );
        assert!(b.on_ground);
    }

    #[test]
    fn no_grab_on_a_low_crate_next_to_the_floor() {
        // Рывок вдоль пола в низкий ящик: висеть на нём значило бы уйти в пол.
        let t = Tuning::default();
        let mut solids = floor();
        solids.push(Collider::solid(Aabb {
            min: [1.0, 0.0],
            max: [1.25, 0.25],
        }));
        let mut b = Body::new([0.0, t.size[1] * 0.5 + 0.005]);
        b.step(
            &t,
            Intent {
                x: 1.0,
                dash_pressed: true,
                ..Intent::default()
            },
            &Space::of(&solids),
            DT,
        );
        for _ in 0..40 {
            let ev = b.step(&t, walk(1.0), &Space::of(&solids), DT);
            assert!(!ev.grabbed, "схватился у пола: {:?}", b.pos);
            assert!(b.bounds(&t).min[1] > -TOL, "в полу: {:?}", b.pos);
        }
    }

    // --- 9.2: мир столкновений с настоящими формами ---

    use crate::space::{Surface, cuboid};
    use rapier2d::parry::math::{Pose, Vector};
    use rapier2d::parry::shape::SharedShape;

    fn floor_and(extra: Vec<Surface>) -> Space {
        let mut s = Space::of(&floor());
        s.extend(extra);
        s
    }

    #[test]
    fn stands_on_a_turned_box_with_the_corner_of_the_feet() {
        let t = Tuning::default();
        // Ящик 0,6 × 0,6, повёрнут на 20°: верх — пологий склон вверх влево.
        let angle = 20f64.to_radians();
        let crate_ = Surface::of_shape(
            5,
            cuboid([0.3, 0.3]),
            Pose::new(Vector::new(0.0, 0.3), angle as f32),
        );
        let space = floor_and(vec![crate_.clone()]);
        let mut b = Body::new([0.0, 1.5]);
        for _ in 0..120 {
            b.step(&t, Intent::default(), &space, DT);
        }
        assert!(b.on_ground && b.ground() == Some(5), "{:?}", b.pos);
        // Прямоугольник стоит на грани левым нижним углом: середина ног выше грани на
        // половину ширины × наклон (склоны тайлов держат середину, фигуры — угол).
        let top = 0.3 + 0.3 / angle.cos();
        let foot = b.pos[1] - t.size[1] * 0.5;
        let lift = t.size[0] * 0.5 * angle.tan();
        assert!(
            (foot - top - lift).abs() < 0.01,
            "ноги {foot}, грань {top} + {lift}"
        );
        assert!(!space.overlaps(b.bounds(&t), &Filter::default()));
        // Стоит и не сползает.
        let x = b.pos[0];
        for _ in 0..60 {
            b.step(&t, Intent::default(), &space, DT);
        }
        assert!((b.pos[0] - x).abs() < TOL && b.on_ground, "{:?}", b.pos);
    }

    #[test]
    fn walks_up_and_down_a_leaning_plank() {
        // Доска 1,2 м под 25° лежит на полу: по ней заходят наверх и спускаются, не
        // отрываясь от неё и не входя внутрь.
        let t = Tuning::default();
        let angle = 25f64.to_radians();
        let (hl, ht) = (0.6, 0.02);
        let c = [0.5 + hl * angle.cos(), hl * angle.sin()];
        let plank = Surface::of_shape(
            8,
            cuboid([hl, ht]),
            Pose::new(Vector::new(c[0] as f32, c[1] as f32), angle as f32),
        );
        let space = floor_and(vec![plank]);
        let mut b = standing(&t);
        let mut top: f64 = 0.0;
        for i in 0..45 {
            b.step(&t, walk(1.0), &space, DT);
            assert!(
                !space.overlaps(b.bounds(&t), &Filter::default()),
                "шаг {i}: внутри доски {:?}",
                b.pos
            );
            top = top.max(b.pos[1] - t.size[1] * 0.5);
        }
        assert!(top > 0.2 && b.on_ground, "не поднялся по доске: {top}");
        for i in 0..80 {
            b.step(&t, walk(-1.0), &space, DT);
            assert!(
                !space.overlaps(b.bounds(&t), &Filter::default()),
                "назад, шаг {i}: внутри доски {:?}",
                b.pos
            );
        }
        assert!(b.on_ground && b.pos[0] < 0.5, "{:?}", b.pos);
    }

    #[test]
    fn stands_on_top_of_a_ball() {
        let t = Tuning::default();
        let ball = Surface::of_shape(
            6,
            SharedShape::ball(0.25),
            Pose::from_translation(Vector::new(0.0, 0.25)),
        );
        let space = floor_and(vec![ball]);
        let mut b = Body::new([0.0, 1.2]);
        for _ in 0..90 {
            b.step(&t, Intent::default(), &space, DT);
        }
        assert!(b.on_ground && b.ground() == Some(6), "{:?}", b.pos);
        let foot = b.pos[1] - t.size[1] * 0.5;
        assert!((foot - 0.5).abs() < 0.005, "{foot}");
    }

    #[test]
    fn turning_plank_carries_the_body_around() {
        let t = Tuning::default();
        // Доска вокруг (0, 1): за шаг поворачивается на 0,5°.
        let turn = 0.5f64.to_radians();
        let plank = |angle: f64| {
            let mut s = Surface::of_shape(
                9,
                cuboid([1.0, 0.05]),
                Pose::new(Vector::new(0.0, 1.0), angle as f32),
            );
            s.turned = turn;
            s.center = [0.0, 1.0];
            s
        };
        let mut b = Body::new([0.5, 1.05 + t.size[1] * 0.5 + 0.01]);
        let mut angle = 0.0;
        for _ in 0..10 {
            b.step(&t, Intent::default(), &floor_and(vec![plank(angle)]), DT);
        }
        assert!(b.on_ground && b.ground() == Some(9), "{:?}", b.pos);
        let x0 = b.pos[0];
        for _ in 0..20 {
            angle += turn;
            b.step(&t, Intent::default(), &floor_and(vec![plank(angle)]), DT);
        }
        // Доска поднимает правый конец: стоящего справа везёт вверх и чуть к середине.
        assert!(b.on_ground, "{:?}", b.pos);
        assert!(b.pos[0] < x0, "{x0} → {:?}", b.pos);
        let foot = b.pos[1] - t.size[1] * 0.5;
        assert!(foot > 1.05 + 0.03, "{foot}");
    }

    #[test]
    fn pushes_a_box_and_goes_with_its_speed() {
        let t = Tuning::default();
        let mut box_ = Surface::of_shape(
            3,
            cuboid([0.1, 0.1]),
            Pose::from_translation(Vector::new(0.5, 0.1)),
        );
        box_.pushable = true;
        let mut b = standing(&t);
        let mut pushed = None;
        for _ in 0..60 {
            let ev = b.step(&t, walk(1.0), &floor_and(vec![box_.clone()]), DT);
            pushed = pushed.or(ev.pushed);
        }
        // Ящик стоит (его никто не двигает): персонаж упёрся и стоит, а толкнуть хотел.
        assert_eq!(pushed.map(|p| p.0), Some(3));
        assert!(b.vel[0].abs() < 1e-9, "{:?}", b.vel);
        assert!((b.bounds(&t).max[0] - 0.4).abs() < TOL, "{:?}", b.pos);
        // Ящик поехал со скоростью 1: персонаж разгоняется (не рывком), догоняет и
        // идёт за ним с его скоростью.
        box_.moved = [1.0 * DT, 0.0];
        for _ in 0..30 {
            box_.pose.translation.x += DT as f32;
            box_.aabb.min[0] += DT;
            box_.aabb.max[0] += DT;
            b.step(&t, walk(1.0), &floor_and(vec![box_.clone()]), DT);
        }
        assert!((b.vel[0] - 1.0).abs() < 1e-6, "{:?}", b.vel);
        assert!(
            (b.bounds(&t).max[0] - box_.aabb.min[0]).abs() < TOL,
            "{:?}",
            b.pos
        );
    }

    #[test]
    fn slides_down_a_steep_face_without_standing_on_it() {
        let t = Tuning::default();
        // Клин с гранью 70°.
        let k = 70f64.to_radians().tan() as f32;
        let wedge = Surface::of_shape(
            4,
            SharedShape::convex_hull(&[
                Vector::new(0.0, 0.0),
                Vector::new(1.0 / k, 0.0),
                Vector::new(0.0, 1.0),
            ])
            .unwrap(),
            Pose::IDENTITY,
        );
        let space = floor_and(vec![wedge]);
        let mut b = Body::new([0.2, 1.3]);
        let mut stood_on_wedge = false;
        for _ in 0..120 {
            b.step(&t, Intent::default(), &space, DT);
            stood_on_wedge |= b.ground() == Some(4);
        }
        assert!(!stood_on_wedge, "стоял на крутой грани");
        assert!(b.on_ground && b.ground() == Some(0), "{:?}", b.pos);
        assert!(b.pos[0] > 1.0 / f64::from(k), "не съехал: {:?}", b.pos);
    }
    #[test]
    fn does_not_walk_into_a_jar_with_a_round_top_just_under_the_step() {
        // Банка со скруглённой крышкой чуть ниже высоты ступеньки: подняться на неё
        // нельзя (ровного верха под ногами нет), войти в её бок — тоже.
        let t = Tuning::default();
        let top = t.step_up() - 0.003;
        let jar = Surface::of_shape(
            7,
            crate::Form::Box {
                half: [0.05, 0.125],
                corner: 0.03,
            }
            .build(1.0, 1.0)
            .unwrap(),
            Pose::from_translation(Vector::new(0.5, (top - 0.125) as f32)),
        );
        let space = floor_and(vec![jar]);
        let mut b = standing(&t);
        for i in 0..90 {
            b.step(&t, walk(1.0), &space, DT);
            let bb = b.bounds(&t);
            assert!(
                !space.overlaps(bb, &Filter::default()),
                "шаг {i}: {:?} {:?}",
                b.pos,
                b.vel
            );
        }
    }
}
