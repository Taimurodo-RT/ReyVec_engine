//! Тряпичная кукла (9.6г): разбитый враг падает телом на шарнирах, а не исчезает.
//!
//! Кукла собирается из фигур самого врага. Крупные фигуры (не меньше доли самой
//! большой) становятся телами физики, самая большая — туловищем; остальные крупные
//! держатся за туловище шарнирами с пределами. Мелочь (глаз, полоска) не тело: она
//! едет за ближайшим телом, как приклеенная. Для каждой фигуры при сборке сцены
//! заводится невидимая копия; разбитый враг гаснет, а копии встают на его место и
//! падают. Кукла лежит, пока враг не появится снова, и последние доли секунды гаснет.

use super::*;
use reyvec_game::physics::{BodyDesc, BodyShape, JointDesc, JointKind};

/// Фигура меньше этой доли самой большой (по описанному прямоугольнику) — не тело.
const BODY_SHARE: f64 = 0.15;
/// Тел в кукле не больше.
const MAX_BODIES: usize = 8;
/// Тоньше этого тело не делается (тонкое в Rapier дрожит), единиц.
const MIN_SIZE: f64 = 0.02;
/// Насколько гнутся шарниры, градусы в обе стороны.
const BEND: f64 = 50.0;
/// Последние столько секунд кукла гаснет.
const FADE: f64 = 0.6;

/// Часть куклы в координатах врага (до отражения): где середина её тела, поворот,
/// размер тела, круг ли, где начало фигуры относительно середины тела (в осях тела),
/// и за каким телом едет мелочь.
#[derive(Clone, Debug, PartialEq)]
pub struct RagPart {
    pub at: [f64; 2],
    pub angle: f64,
    pub size: [f64; 2],
    pub round: bool,
    pub offset: [f64; 2],
    /// Мелочь: номер части-тела, за которой едет (`None` — сама тело).
    pub follow: Option<usize>,
}

/// Кукла врага: части и их невидимые копии.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ragdoll {
    pub parts: Vec<RagPart>,
    pub entities: Vec<Entity>,
    /// Растяжение врага.
    pub scale: f64,
    /// Какая часть — туловище: к нему крепятся шарниры.
    pub torso: usize,
}

/// Кукла, которая сейчас лежит в мире.
pub(super) struct Fallen {
    parts: Vec<RagPart>,
    entities: Vec<Entity>,
    bodies: Vec<Option<BodyId>>,
    left: f64,
    life: f64,
    flip: bool,
    scale: f64,
}

fn rot(a: [f64; 2], deg: f64) -> [f64; 2] {
    let (s, c) = deg.to_radians().sin_cos();
    [a[0] * c - a[1] * s, a[0] * s + a[1] * c]
}

/// Части куклы из фигур врага с растяжением `scale` (по порядку фигур) и какая из них
/// туловище.
pub(super) fn parts_of(shapes: &[ShapeDesc], scale: f64) -> (Vec<RagPart>, usize) {
    let mut parts: Vec<RagPart> = Vec::new();
    for shape in shapes {
        let (at, angle, _) = shape.placement();
        let Some(form) = shape.form().build(scale, scale) else {
            continue;
        };
        let b = form.compute_local_aabb();
        let c = [
            f64::from(b.mins.x + b.maxs.x) * 0.5,
            f64::from(b.mins.y + b.maxs.y) * 0.5,
        ];
        let size = [
            f64::from(b.maxs.x - b.mins.x),
            f64::from(b.maxs.y - b.mins.y),
        ];
        let v = [at.0 * scale, at.1 * scale];
        let r = rot(c, angle);
        parts.push(RagPart {
            at: [v[0] + r[0], v[1] + r[1]],
            angle,
            size,
            round: matches!(shape, ShapeDesc::Circle { .. }),
            offset: [-c[0], -c[1]],
            follow: None,
        });
    }
    let area = |p: &RagPart| p.size[0] * p.size[1];
    let biggest = parts.iter().map(area).fold(0.0, f64::max);
    // Тела — крупные, не больше MAX_BODIES самых крупных; первое по размеру — туловище.
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by(|&a, &b| area(&parts[b]).total_cmp(&area(&parts[a])));
    let bodies: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| area(&parts[i]) >= biggest * BODY_SHARE)
        .take(MAX_BODIES)
        .collect();
    for i in 0..parts.len() {
        if bodies.contains(&i) {
            continue;
        }
        let me = parts[i].at;
        let near = bodies.iter().copied().min_by(|&a, &b| {
            let d = |j: usize| {
                let q = parts[j].at;
                (q[0] - me[0]).powi(2) + (q[1] - me[1]).powi(2)
            };
            d(a).total_cmp(&d(b))
        });
        parts[i].follow = near;
    }
    for p in &mut parts {
        if p.follow.is_none() {
            p.size = [p.size[0].max(MIN_SIZE), p.size[1].max(MIN_SIZE)];
        }
    }
    (parts, bodies.first().copied().unwrap_or(0))
}

/// Точка шарнира между телами `a` и `b`: середина между ближайшими точками их
/// прямоугольников (повороты не в счёт).
fn hinge_point(a: &RagPart, b: &RagPart) -> [f64; 2] {
    let clamp = |p: [f64; 2], r: &RagPart| {
        [
            p[0].clamp(r.at[0] - r.size[0] * 0.5, r.at[0] + r.size[0] * 0.5),
            p[1].clamp(r.at[1] - r.size[1] * 0.5, r.at[1] + r.size[1] * 0.5),
        ]
    };
    let on_b = clamp(a.at, b);
    let on_a = clamp(on_b, a);
    [(on_a[0] + on_b[0]) * 0.5, (on_a[1] + on_b[1]) * 0.5]
}

impl Fallen {
    /// Кукла падает: тела на месте врага в `at` (отражённого, если смотрел влево),
    /// толчок в сторону удара `dir`. Лежит `life` секунд.
    pub(super) fn start(
        physics: &mut Physics,
        doll: &Ragdoll,
        at: [f64; 2],
        flip: bool,
        dir: f64,
        life: f64,
    ) -> Self {
        let parts: Vec<RagPart> = doll
            .parts
            .iter()
            .map(|p| {
                if flip {
                    RagPart {
                        at: [-p.at[0], p.at[1]],
                        angle: -p.angle,
                        offset: [-p.offset[0], p.offset[1]],
                        ..p.clone()
                    }
                } else {
                    p.clone()
                }
            })
            .collect();
        let mask = reyvec_game::layer::ALL & !reyvec_game::layer::ENEMY;
        let mut bodies = Vec::with_capacity(parts.len());
        for (i, p) in parts.iter().enumerate() {
            if p.follow.is_some() {
                bodies.push(None);
                continue;
            }
            let shape = if p.round {
                BodyShape::Ball
            } else if p.size[0].max(p.size[1]) > 2.0 * p.size[0].min(p.size[1]) {
                BodyShape::Capsule
            } else {
                BodyShape::Box
            };
            let desc = BodyDesc {
                size: if p.round {
                    let d = p.size[0].max(p.size[1]);
                    [d, d]
                } else {
                    p.size
                },
                shape,
                friction: 0.8,
                bounce: 0.05,
                mask,
                ..BodyDesc::default()
            };
            // Верх летит сильнее низа: кукла заваливается от удара.
            let up = (p.at[1] / 0.3).clamp(-0.5, 1.0);
            let vel = [dir * (0.5 + 0.6 * up.max(0.0)), 0.6 + 0.2 * up];
            let world = [at[0] + p.at[0], at[1] + p.at[1]];
            let id = physics.add(world, p.angle, vel, &desc);
            physics.kick(id, [0.0, 0.0], -dir * (1.0 + i as f64));
            bodies.push(Some(id));
        }
        let t = doll.torso;
        if let Some(Some(torso)) = bodies.get(t).copied() {
            for i in 0..parts.len() {
                let Some(b) = bodies[i].filter(|_| i != t) else {
                    continue;
                };
                let h = hinge_point(&parts[t], &parts[i]);
                let joint = JointDesc::new(
                    JointKind::Hinge {
                        limits: Some([-BEND, BEND]),
                    },
                    [at[0] + h[0], at[1] + h[1]],
                );
                physics.join(b, Some(torso), &joint);
            }
        }
        Self {
            parts,
            entities: doll.entities.clone(),
            bodies,
            left: life,
            life,
            flip,
            scale: doll.scale,
        }
    }

    /// Шаг после физики: копии фигур — на места тел. `false` — кукла погасла, тела
    /// убраны.
    pub(super) fn step(
        &mut self,
        physics: &mut Physics,
        places: &mut PieceShapes,
        dt: f64,
    ) -> bool {
        self.left -= dt;
        let gone = self.left <= 0.0;
        let opacity = (self.left / FADE.min(self.life * 0.5)).clamp(0.0, 1.0) as f32;
        let poses: Vec<Option<([f64; 2], f64)>> = self
            .bodies
            .iter()
            .map(|b| b.and_then(|b| physics.pose(b)))
            .collect();
        for (i, p) in self.parts.iter().enumerate() {
            let Ok((mut place, _)) = places.get_mut(self.entities[i]) else {
                continue;
            };
            if gone {
                place.opacity = 0.0;
                continue;
            }
            let (at, angle) = match p.follow {
                None => {
                    let Some((c, a)) = poses[i] else {
                        continue;
                    };
                    let o = rot(p.offset, a);
                    ([c[0] + o[0], c[1] + o[1]], a)
                }
                Some(l) => {
                    let Some((c, a)) = poses[l] else {
                        continue;
                    };
                    let lead = &self.parts[l];
                    // Где мелочь относительно тела-вожака в его осях при сборке.
                    let mine = rot(p.offset, p.angle);
                    let vis = [p.at[0] + mine[0], p.at[1] + mine[1]];
                    let rel = rot([vis[0] - lead.at[0], vis[1] - lead.at[1]], -lead.angle);
                    let r = rot(rel, a);
                    ([c[0] + r[0], c[1] + r[1]], a + p.angle - lead.angle)
                }
            };
            place.at = at;
            place.angle = angle;
            place.flip = self.flip;
            place.scale = self.scale;
            place.opacity = opacity;
        }
        if gone {
            self.clear(physics);
        }
        !gone
    }

    /// Убрать тела (кукла погасла или враг разбит снова).
    pub(super) fn clear(&mut self, physics: &mut Physics) {
        for b in self.bodies.iter_mut() {
            if let Some(b) = b.take() {
                physics.remove(b);
            }
        }
    }

    pub(super) fn uses(&self, entities: &[Entity]) -> bool {
        self.entities.first() == entities.first()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mannequin_shapes_become_three_bodies_and_followers() {
        let shapes: Vec<ShapeDesc> = ron::from_str(
            r##"[
                Rect(at: (0.0, -0.135), size: (0.14, 0.03), corner: 0.01, color: "#3a2a1c"),
                Rect(at: (0.0, -0.07), size: (0.025, 0.12), color: "#5a4028"),
                Rect(at: (0.0, 0.0), size: (0.15, 0.17), corner: 0.05, color: "#b08850"),
                Rect(at: (0.0, 0.035), size: (0.15, 0.015), color: "#6e5232"),
                Circle(at: (0.0, 0.11), r: 0.045, color: "#c8a068"),
            ]"##,
        )
        .unwrap();
        let (parts, torso) = parts_of(&shapes, 1.0);
        assert_eq!(parts.len(), 5);
        // Туловище — мешок, подставка и голова — тела, шест и полоска едут.
        assert_eq!(torso, 2);
        assert!((parts[2].size[0] - 0.15).abs() < 1e-3 && parts[2].follow.is_none());
        let bodies: Vec<_> = parts.iter().filter(|p| p.follow.is_none()).collect();
        assert_eq!(bodies.len(), 3);
        assert!(parts.iter().any(|p| p.round && p.follow.is_none()));
        let stripe = parts.iter().find(|p| p.size[1] < 0.02).unwrap();
        assert_eq!(stripe.follow, Some(2), "полоска едет на мешке");
        // Шарнир головы — между мешком и головой.
        let head = parts.iter().find(|p| p.round).unwrap();
        let h = hinge_point(&parts[2], head);
        assert!(h[1] > 0.06 && h[1] < 0.09, "{h:?}");
    }
}
