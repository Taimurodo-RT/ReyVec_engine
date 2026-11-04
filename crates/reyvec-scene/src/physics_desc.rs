//! Физика в файле сцены (6.12): сочленения (`joints`), мягкие тела (`soft`), повтор
//! предмета (`repeat`) и выброс тел (`spawn`).

use serde::Deserialize;

use reyvec_game::physics::{JointKind, Motor, Pin, SoftKind};

use crate::{Hex, P, check_point, check_size};

/// С чем соединено сочленение.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub enum JointTo {
    /// С неподвижным миром (точка в воздухе).
    #[default]
    World,
    /// С предыдущей копией повтора (`repeat`); у первой копии — с миром.
    Prev,
    /// С предметом по имени (у предмета должно быть `body`).
    Node(String),
}

/// Какие копии повтора получают это сочленение.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum JointOn {
    #[default]
    All,
    First,
    Last,
}

/// Вид сочленения в файле.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum JointKindDesc {
    /// Шарнир: вращение вокруг точки `at`.
    Hinge,
    /// Сварка намертво.
    Weld,
    /// Ползун вдоль `axis`.
    Slider,
    /// Верёвка: не дальше `length` (без него — сколько было при создании).
    Rope,
    /// Пружина длиной `length` (без него — сколько было при создании).
    Spring,
}

/// Мотор шарнира или ползуна.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotorDesc {
    /// Градусов в секунду у шарнира, единиц в секунду у ползуна; минус — в другую сторону.
    pub speed: f64,
    /// Наибольшая сила мотора.
    #[serde(default = "default_motor_force")]
    pub force: f64,
    /// Через сколько секунд менять направление (0 — никогда).
    #[serde(default)]
    pub flip: f64,
}

fn default_motor_force() -> f64 {
    1.0
}

/// Сочленение предмета с телом (`body`): `at` — точка на этом предмете (в его
/// координатах), `to_at` — точка на том, с чем соединено (в его координатах; у мира —
/// тоже в координатах этого предмета: так у копий повтора свои точки подвеса). Без
/// `to_at` — та же точка, что `at`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointSpec {
    pub kind: JointKindDesc,
    #[serde(default)]
    pub to: JointTo,
    #[serde(default)]
    pub at: P,
    #[serde(default)]
    pub to_at: Option<P>,
    /// Шарнир: на сколько градусов можно повернуть (от, до). Ползун: от и до вдоль оси.
    #[serde(default)]
    pub limits: Option<P>,
    /// Ползун: направление.
    #[serde(default)]
    pub axis: Option<P>,
    /// Верёвка, пружина: длина.
    #[serde(default)]
    pub length: Option<f64>,
    /// Пружина: жёсткость и гашение.
    #[serde(default)]
    pub stiffness: Option<f64>,
    #[serde(default)]
    pub damping: Option<f64>,
    #[serde(default)]
    pub motor: Option<MotorDesc>,
    /// С какой силой рвётся.
    #[serde(default)]
    pub breaks: Option<f64>,
    /// Касаются ли соединённые тела друг друга.
    #[serde(default)]
    pub collide: bool,
    /// Рисовать как нитку (верёвку, пружину): толщина и цвет.
    #[serde(default)]
    pub line: Option<LineDesc>,
    /// У повтора: каким копиям.
    #[serde(default)]
    pub on: JointOn,
}

/// Нитка сочленения на экране.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineDesc {
    pub width: f64,
    pub color: Hex,
}

impl JointSpec {
    pub(crate) fn check(&self) -> Result<(), String> {
        check_point("at", self.at)?;
        if let Some(p) = self.to_at {
            check_point("to_at", p)?;
        }
        if let Some((lo, hi)) = self.limits
            && !(lo.is_finite() && hi.is_finite() && lo <= hi)
        {
            return Err(format!("limits ({lo}, {hi}) — нужно «от» не больше «до»"));
        }
        if let Some(a) = self.axis {
            check_point("axis", a)?;
            if a.0 == 0.0 && a.1 == 0.0 {
                return Err("axis (0, 0) — у оси нужно направление".into());
            }
        }
        if let Some(l) = self.length {
            check_size("length", l)?;
        }
        for (what, v) in [("stiffness", self.stiffness), ("damping", self.damping)] {
            if let Some(v) = v
                && !(v.is_finite() && (0.0..=1.0e6).contains(&v))
            {
                return Err(format!("{what} {v} — нужно от 0 до 1000000"));
            }
        }
        if let Some(m) = self.motor {
            if !matches!(self.kind, JointKindDesc::Hinge | JointKindDesc::Slider) {
                return Err("motor бывает только у Hinge и Slider".into());
            }
            if !(m.speed.is_finite() && m.speed.abs() <= 1.0e5) {
                return Err(format!("motor: speed {} — нужно до 100000", m.speed));
            }
            if !(m.force.is_finite() && m.force > 0.0 && m.force <= 1.0e6) {
                return Err(format!("motor: force {} — нужно больше 0", m.force));
            }
            if !(m.flip.is_finite() && (0.0..=3600.0).contains(&m.flip)) {
                return Err(format!(
                    "motor: flip {} — нужно от 0 до 3600 секунд",
                    m.flip
                ));
            }
        }
        if let Some(b) = self.breaks
            && !(b.is_finite() && b > 0.0)
        {
            return Err(format!("breaks {b} — нужно больше 0"));
        }
        if let Some(l) = self.line {
            check_size("line: width", l.width)?;
        }
        if let JointTo::Node(name) = &self.to
            && name.is_empty()
        {
            return Err("to: Node(\"\") — нужно имя предмета".into());
        }
        Ok(())
    }

    /// Вид для физики. `start` — расстояние между точками при создании (длина верёвки и
    /// пружины по умолчанию).
    pub(crate) fn kind(&self, start: f64) -> JointKind {
        let length = self.length.unwrap_or(start.max(1e-3));
        match self.kind {
            JointKindDesc::Hinge => JointKind::Hinge {
                limits: self.limits.map(|(a, b)| [a, b]),
            },
            JointKindDesc::Weld => JointKind::Weld,
            JointKindDesc::Slider => JointKind::Slider {
                axis: self.axis.map_or([1.0, 0.0], |a| [a.0, a.1]),
                limits: self.limits.map(|(a, b)| [a, b]),
            },
            JointKindDesc::Rope => JointKind::Rope { length },
            JointKindDesc::Spring => JointKind::Spring {
                length,
                stiffness: self.stiffness.unwrap_or(1.0),
                damping: self.damping.unwrap_or(0.05),
            },
        }
    }

    pub(crate) fn motor(&self) -> Option<Motor> {
        self.motor.map(|m| Motor {
            speed: m.speed,
            force: m.force,
            flip: m.flip,
        })
    }
}

/// Форма мягкого тела в файле.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum SoftShapeDesc {
    /// Верёвка от места предмета до `to` (в координатах предмета), `count` частиц.
    Rope { to: P, count: usize },
    /// Желе радиусом `r` из `count` частиц по кругу.
    Jelly { r: f64, count: usize },
    /// Сетка `size` из `cols` × `rows` частиц, середина — место предмета.
    Net { size: P, cols: usize, rows: usize },
}

/// Мягкое тело (6.12): частицы на пружинках. Сталкивается с телами и уровнем, мышь
/// тащит любую частицу.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoftSpec {
    pub shape: SoftShapeDesc,
    /// Жёсткость: частота дрожания, раз в секунду.
    #[serde(default = "default_soft_stiffness")]
    pub stiffness: f64,
    /// Гашение дрожания от 0 до 1 и больше.
    #[serde(default = "default_soft_damping")]
    pub damping: f64,
    #[serde(default = "default_soft_mass")]
    pub mass: f64,
    /// Что закреплено: None, Start, Ends (верёвка), Sides, Top (сетка).
    #[serde(default)]
    pub pin: PinDesc,
    /// Рвётся, если растянуть на эту долю длины.
    #[serde(default)]
    pub tear: Option<f64>,
    /// Привязать начало и конец к предметам с `body` по имени.
    #[serde(default)]
    pub attach_start: Option<String>,
    #[serde(default)]
    pub attach_end: Option<String>,
    pub color: Hex,
    /// Толщина нитей (верёвка, сетка).
    #[serde(default = "default_soft_width")]
    pub width: f64,
}

fn default_soft_stiffness() -> f64 {
    30.0
}
fn default_soft_damping() -> f64 {
    0.5
}
fn default_soft_mass() -> f64 {
    0.05
}
fn default_soft_width() -> f64 {
    0.012
}

/// Закреплённые частицы мягкого тела.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum PinDesc {
    #[default]
    None,
    Start,
    Ends,
    Sides,
    Top,
}

impl SoftSpec {
    pub(crate) fn check(&self) -> Result<(), String> {
        match self.shape {
            SoftShapeDesc::Rope { to, count } => {
                check_point("to", to)?;
                if !(2..=400).contains(&count) {
                    return Err(format!("count {count} — у верёвки от 2 до 400 частиц"));
                }
            }
            SoftShapeDesc::Jelly { r, count } => {
                check_size("r", r)?;
                if !(3..=64).contains(&count) {
                    return Err(format!("count {count} — у желе от 3 до 64 частиц"));
                }
            }
            SoftShapeDesc::Net { size, cols, rows } => {
                check_size("ширина size", size.0)?;
                check_size("высота size", size.1)?;
                if !(2..=64).contains(&cols) || !(2..=64).contains(&rows) {
                    return Err(format!("cols {cols}, rows {rows} — у сетки от 2 до 64"));
                }
            }
        }
        let rope = matches!(self.shape, SoftShapeDesc::Rope { .. });
        let net = matches!(self.shape, SoftShapeDesc::Net { .. });
        match self.pin {
            PinDesc::Start | PinDesc::Ends if !rope => {
                return Err("pin: Start и Ends бывают только у верёвки".into());
            }
            PinDesc::Sides | PinDesc::Top if !net => {
                return Err("pin: Sides и Top бывают только у сетки".into());
            }
            _ => {}
        }
        if !(self.stiffness.is_finite() && (0.5..=1000.0).contains(&self.stiffness)) {
            return Err(format!(
                "stiffness {} — нужно от 0,5 до 1000",
                self.stiffness
            ));
        }
        if !(self.damping.is_finite() && (0.0..=100.0).contains(&self.damping)) {
            return Err(format!("damping {} — нужно от 0 до 100", self.damping));
        }
        check_size("mass", self.mass)?;
        if let Some(t) = self.tear
            && !(t.is_finite() && t > 0.0 && t <= 10.0)
        {
            return Err(format!("tear {t} — нужно больше 0 и до 10"));
        }
        if (self.attach_start.is_some() || self.attach_end.is_some()) && !rope {
            return Err("attach_start и attach_end бывают только у верёвки".into());
        }
        check_size("width", self.width)
    }

    /// Форма для физики: точки — от места предмета `at`.
    pub(crate) fn kind(&self, at: [f64; 2]) -> SoftKind {
        match self.shape {
            SoftShapeDesc::Rope { to, count } => SoftKind::Rope {
                from: at,
                to: [at[0] + to.0, at[1] + to.1],
                count,
            },
            SoftShapeDesc::Jelly { r, count } => SoftKind::Jelly {
                center: at,
                radius: r,
                count,
            },
            SoftShapeDesc::Net { size, cols, rows } => SoftKind::Net {
                center: at,
                size: [size.0, size.1],
                cols,
                rows,
            },
        }
    }

    pub(crate) fn pin(&self) -> Pin {
        match self.pin {
            PinDesc::None => Pin::None,
            PinDesc::Start => Pin::Start,
            PinDesc::Ends => Pin::Ends,
            PinDesc::Sides => Pin::Sides,
            PinDesc::Top => Pin::Top,
        }
    }
}

/// Повтор предмета (6.12): `count` копий через `step`, рядами. В ряду `r` копий на
/// `shrink * r` меньше, и ряд сдвинут на половину шага за каждую убранную копию — с
/// `shrink: 1` выходит пирамида. Копии называются «имя 1», «имя 2»… (в рядах —
/// «имя ряд.номер»). `colors` — цвета первой фигуры по очереди.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepeatDesc {
    pub count: usize,
    pub step: P,
    #[serde(default = "one_usize")]
    pub rows: usize,
    #[serde(default)]
    pub row_step: Option<P>,
    #[serde(default)]
    pub shrink: usize,
    #[serde(default)]
    pub colors: Vec<Hex>,
}

fn one_usize() -> usize {
    1
}

/// Копия повтора: где, номер, сколько всего в её ряду, имя и цвет.
pub(crate) struct RepeatCopy {
    pub at: P,
    pub index: usize,
    pub first: bool,
    pub last: bool,
    pub name: String,
    pub color: Option<Hex>,
}

/// Наибольшее число копий одного повтора.
pub const MAX_COPIES: usize = 4096;

impl RepeatDesc {
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.count == 0 || self.rows == 0 {
            return Err("count и rows — нужно больше 0".into());
        }
        check_point("step", self.step)?;
        if let Some(s) = self.row_step {
            check_point("row_step", s)?;
        }
        let total = self.copies((0.0, 0.0), "").len();
        if total > MAX_COPIES || self.count > MAX_COPIES || self.rows > MAX_COPIES {
            return Err(format!("копий {total} — можно не больше {MAX_COPIES}"));
        }
        Ok(())
    }

    /// Копии от места `at`: ряд за рядом.
    pub(crate) fn copies(&self, at: P, name: &str) -> Vec<RepeatCopy> {
        let row_step = self
            .row_step
            .unwrap_or((0.0, self.step.1.abs().max(self.step.0.abs())));
        let mut out = Vec::new();
        let mut index = 0;
        for r in 0..self.rows.min(MAX_COPIES) {
            let n = self.count.saturating_sub(self.shrink * r);
            if n == 0 {
                break;
            }
            let shift = self.shrink as f64 * r as f64 * 0.5;
            for i in 0..n.min(MAX_COPIES) {
                let k = i as f64 + shift;
                let copy_name = if self.rows > 1 {
                    format!("{name} {}.{}", r + 1, i + 1)
                } else {
                    format!("{name} {}", i + 1)
                };
                out.push(RepeatCopy {
                    at: (
                        at.0 + self.step.0 * k + row_step.0 * r as f64,
                        at.1 + self.step.1 * k + row_step.1 * r as f64,
                    ),
                    index,
                    first: index == 0,
                    last: false,
                    name: copy_name,
                    color: (!self.colors.is_empty())
                        .then(|| self.colors[index % self.colors.len()]),
                });
                index += 1;
                if out.len() > MAX_COPIES {
                    return out;
                }
            }
        }
        if let Some(l) = out.last_mut() {
            l.last = true;
        }
        out
    }
}

/// Выброс тел (6.12): предмет с `body` — образец; раз в `every` секунд такое тело
/// вылетает из места предмета со скоростью `speed` (± `spread` вбок) и живёт `life`
/// секунд. Одновременно — не больше `max`.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnDesc {
    pub every: f64,
    #[serde(default)]
    pub speed: P,
    #[serde(default)]
    pub spread: f64,
    #[serde(default = "default_life")]
    pub life: f64,
    #[serde(default = "default_max")]
    pub max: usize,
    /// Разброс места вылета по ширине (дождь).
    #[serde(default)]
    pub width: f64,
}

fn default_life() -> f64 {
    6.0
}
fn default_max() -> usize {
    20
}

impl SpawnDesc {
    pub(crate) fn check(&self) -> Result<(), String> {
        if !(self.every.is_finite() && (0.02..=3600.0).contains(&self.every)) {
            return Err(format!(
                "every {} — нужно от 0,02 до 3600 секунд",
                self.every
            ));
        }
        check_point("speed", self.speed)?;
        if !(self.life.is_finite() && (0.1..=3600.0).contains(&self.life)) {
            return Err(format!("life {} — нужно от 0,1 до 3600 секунд", self.life));
        }
        if !(1..=512).contains(&self.max) {
            return Err(format!("max {} — нужно от 1 до 512", self.max));
        }
        for (what, v) in [("spread", self.spread), ("width", self.width)] {
            if !(v.is_finite() && (0.0..=1000.0).contains(&v)) {
                return Err(format!("{what} {v} — нужно от 0 до 1000"));
            }
        }
        Ok(())
    }
}
