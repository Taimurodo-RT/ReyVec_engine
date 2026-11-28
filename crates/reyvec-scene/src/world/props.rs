//! Свойства и метки предметов (9.8, решение 0025): что лежит в файле сцены и что из
//! этого движок понимает сам.
//!
//! Свойства хранятся у сущности предмета компонентом [`Props`] как есть. Встроенные
//! действуют так:
//! - `damage: N` и `burns: true` — [`Hazard`]: герой, коснувшийся предмета, ранится (на
//!   `N`, горящее — хотя бы на 1). Горящее ещё и мерцает.
//! - `grab: false` — тело не берут руки (`BodyDesc::grab`, при сборке тела).
//! - `glow: true` — SDF-объекты предмета светятся (при сборке объектов).
//!
//! Скрипт предмета получает всё в `me.props` и `me.tags`.

use std::collections::BTreeMap;

use super::*;
use crate::Prop;

/// Свойства и метки предмета из файла.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct Props {
    pub values: BTreeMap<String, Prop>,
    pub tags: Vec<String>,
    /// Порядок сборки (по порядку в файле).
    pub(super) order: u32,
}

impl Props {
    pub fn get(&self, key: &str) -> Option<&Prop> {
        self.values.get(key)
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Для скрипта (`me.props`).
    pub(super) fn for_script(&self) -> Vec<(String, reyvec_script::PropValue)> {
        self.values
            .iter()
            .map(|(k, v)| (k.clone(), v.script()))
            .collect()
    }
}

/// Предмет ранит героя касанием (`damage`, `burns`).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(super) struct Hazard {
    pub(super) damage: u32,
    pub(super) burns: bool,
    /// Прямоугольник касания вокруг места предмета, в его координатах (до растяжения
    /// предками): середина и половины сторон.
    pub(super) center: [f64; 2],
    pub(super) half: [f64; 2],
}

/// Горящее мерцает: насколько ярче (доля вспышки) и как быстро.
const BURN_FLICKER: f32 = 0.25;
const BURN_RATE: f64 = 17.0;

impl Hazard {
    /// Из свойств предмета с фигурами `shapes` (прямоугольник касания — их описанный
    /// прямоугольник; без фигур — `touch` или 0,1 × 0,1).
    pub(super) fn of(n: &NodeDesc) -> Option<Self> {
        let damage = n
            .props
            .get("damage")
            .and_then(Prop::number)
            .map_or(0, |d| d.round() as u32);
        let burns = n.props.get("burns").and_then(Prop::flag) == Some(true);
        let damage = if burns { damage.max(1) } else { damage };
        if damage == 0 {
            return None;
        }
        let (center, half) = match (n.touch, shapes_box(&n.shapes)) {
            (Some(t), _) => ([0.0, 0.0], [t.0 * 0.5, t.1 * 0.5]),
            (None, Some(b)) => (
                b.center(),
                [(b.max[0] - b.min[0]) * 0.5, (b.max[1] - b.min[1]) * 0.5],
            ),
            (None, None) => ([0.0, 0.0], [0.05, 0.05]),
        };
        Some(Self {
            damage,
            burns,
            center,
            half,
        })
    }

    /// Прямоугольник касания в мире (поворот предмета не учитывается).
    pub(super) fn area(&self, g: &GlobalPlacement) -> Aabb {
        let c = g.affine * reyvec_vector::Point::new(self.center[0], self.center[1]);
        Aabb::around([c.x, c.y], [self.half[0] * g.scale, self.half[1] * g.scale])
    }
}

/// Описанный прямоугольник фигур предмета в его координатах.
fn shapes_box(shapes: &[ShapeDesc]) -> Option<Aabb> {
    let mut out: Option<Aabb> = None;
    for shape in shapes {
        let (at, angle, _) = shape.placement();
        let Some(form) = shape.form().build(1.0, 1.0) else {
            continue;
        };
        let (s, c) = angle.to_radians().sin_cos();
        let b = form.compute_local_aabb();
        for (x, y) in [
            (b.mins.x, b.mins.y),
            (b.maxs.x, b.mins.y),
            (b.mins.x, b.maxs.y),
            (b.maxs.x, b.maxs.y),
        ] {
            let (x, y) = (f64::from(x), f64::from(y));
            let p = [at.0 + x * c - y * s, at.1 + x * s + y * c];
            out = Some(match out {
                None => Aabb { min: p, max: p },
                Some(o) => Aabb {
                    min: [o.min[0].min(p[0]), o.min[1].min(p[1])],
                    max: [o.max[0].max(p[0]), o.max[1].max(p[1])],
                },
            });
        }
    }
    out
}

/// Опасные предметы ранят коснувшегося героя; горящие мерцают.
pub(super) fn hazards(
    time: Res<SceneTime>,
    mut events: ResMut<Events>,
    mut combat: ResMut<Combat>,
    mut players: Query<&mut Player>,
    mut items: Query<(&Hazard, &GlobalPlacement, &mut Placement), Without<Player>>,
) {
    let step = events.step;
    for (h, g, mut place) in &mut items {
        if h.burns {
            let wave = (time.0 * BURN_RATE).sin() * 0.5 + 0.5;
            place.flash = BURN_FLICKER * wave as f32;
        }
        if place.opacity <= 0.0 {
            continue;
        }
        let area = h.area(g);
        for mut p in &mut players {
            if p.body.bounds(&p.tuning).overlaps(&area) {
                let from = area.center()[0];
                hurt_hero(&mut p, h.damage, from, step, &mut events, &mut combat);
            }
        }
    }
}
