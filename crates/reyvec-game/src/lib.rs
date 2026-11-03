//! Игровые системы 2D (этап 6).
//!
//! Здесь только расчёты без видеокарты и без окна, поэтому всё проверяется тестами:
//!
//! - [`input`]: действия игрока (бег, прыжок) от клавиатуры. Окно пишет нажатия, поток
//!   логики читает их на каждом шаге и не теряет короткие нажатия между шагами.
//! - [`controller`]: свой контроллер персонажа платформера (не физический движок):
//!   разгон и торможение, прыжок заданной высоты, «время койота», запоминание нажатия
//!   прыжка, высота прыжка по длине нажатия; платформы, сквозь которые прыгаешь снизу,
//!   движущиеся платформы, рывок, скольжение по стене и прыжок от неё (6.2), склоны,
//!   ступеньки и вис на краю (6.4).
//! - [`Aabb`]: прямоугольник столкновений. Персонаж движется по осям по очереди и
//!   останавливается ровно у ближайшей стены на пути, поэтому не проскакивает тонкие
//!   стены даже на большой скорости.

pub mod controller;
pub mod input;

pub use controller::{Body, Collider, Intent, StepEvents, Tuning};
pub use input::{Action, InputFrame, InputReader, InputShared};

/// Прямоугольник со сторонами вдоль осей: от `min` до `max`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f64; 2],
    pub max: [f64; 2],
}

impl Aabb {
    /// Прямоугольник с серединой `center` и половинами сторон `half`.
    pub fn around(center: [f64; 2], half: [f64; 2]) -> Self {
        Self {
            min: [center[0] - half[0], center[1] - half[1]],
            max: [center[0] + half[0], center[1] + half[1]],
        }
    }

    /// Наименьший прямоугольник, в который входят все точки.
    pub fn of_points(points: impl IntoIterator<Item = [f64; 2]>) -> Option<Self> {
        let mut it = points.into_iter();
        let first = it.next()?;
        let mut b = Self {
            min: first,
            max: first,
        };
        for p in it {
            for (i, v) in p.into_iter().enumerate() {
                b.min[i] = b.min[i].min(v);
                b.max[i] = b.max[i].max(v);
            }
        }
        Some(b)
    }

    /// Пересекается ли с `other` (касание не считается).
    pub fn overlaps(&self, other: &Self) -> bool {
        (0..2).all(|i| self.min[i] < other.max[i] - EPS && self.max[i] > other.min[i] + EPS)
    }
}

/// Запас при сравнении краёв: стоящий вплотную персонаж не считается вошедшим в стену.
pub(crate) const EPS: f64 = 1e-7;
