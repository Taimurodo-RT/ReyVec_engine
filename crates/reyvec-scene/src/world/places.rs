//! Места предметов: движение по времени, позы костей, твёрдые прямоугольники и итоговые места от корней к листьям.

use super::*;

/// Движение: место предметов с [`Motion`] в текущий момент.
pub(super) fn animate(time: Res<SceneTime>, mut moving: Query<(&Motion, &mut Placement)>) {
    for (motion, mut placement) in &mut moving {
        *placement = motion.at(time.0);
    }
}

/// Кости героя в позе анимации: к месту и повороту из файла добавляется поза.
pub(super) fn pose_bones(
    players: Query<&Player>,
    mut bones: Query<(&BoneOf, &mut Placement), Without<Player>>,
) {
    let Some(p) = players.iter().next() else {
        return;
    };
    let pose = p.anim.pose();
    for (b, mut place) in &mut bones {
        let bp = pose.bone(b.bone);
        place.at = [b.base.at[0] + bp.offset[0], b.base.at[1] + bp.offset[1]];
        place.angle = b.base.angle + bp.angle;
    }
}

/// Твёрдое в координатах сцены и на сколько каждое сдвинулось и повернулось за шаг.
pub(super) fn gather_solids(
    mut q: Query<(Entity, &mut Solid, &GlobalPlacement)>,
    mut solids: ResMut<Solids>,
) {
    solids.0.clear();
    solids.1.clear();
    solids.2.clear();
    for (e, mut s, g) in &mut q {
        let c = g.affine.translation();
        let center = [c.x, c.y];
        // Поворот и растяжение места: первый столбец даёт поворот и растяжение по x,
        // второй (повёрнутый обратно) — растяжение по y со знаком (отражение).
        let [a, b, cc, d, _, _] = g.affine.as_coeffs();
        let angle = b.atan2(a);
        let sx = a.hypot(b);
        let sy = -cc * angle.sin() + d * angle.cos();
        let (moved, turned) = s.last.map_or(([0.0, 0.0], 0.0), |(l, la)| {
            ([center[0] - l[0], center[1] - l[1]], wrap_angle(angle - la))
        });
        s.last = Some((center, angle));
        let id = e.to_bits();
        let mut legacy = None;
        let surface = match s.form.clone() {
            Some(form) => {
                let scale = [sx, sy];
                let fresh = s
                    .built
                    .as_ref()
                    .is_some_and(|(k, _)| (k[0] - sx).abs() < 1e-9 && (k[1] - sy).abs() < 1e-9);
                if !fresh {
                    s.built = form.build(sx, sy).map(|shape| (scale, shape));
                }
                let Some((_, shape)) = &s.built else {
                    continue;
                };
                let mut f = Surface::placed(id, shape.clone(), center, angle);
                f.oneway = s.oneway;
                f.moved = moved;
                f.turned = turned;
                f
            }
            None => {
                let [hx, hy] = s.half;
                let corners = [[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]].map(|[x, y]| {
                    let p = g.affine * reyvec_vector::Point::new(x, y);
                    [p.x, p.y]
                });
                let Some(aabb) = Aabb::of_points(corners) else {
                    continue;
                };
                if s.slope == 0.0 && angle.abs() > 1e-9 {
                    // Повёрнутый прямоугольник — настоящей формой (9.3).
                    let shape = reyvec_game::Form::Box {
                        half: s.half,
                        corner: 0.0,
                    }
                    .build(sx, sy);
                    let Some(shape) = shape else { continue };
                    let mut f = Surface::placed(id, shape, center, angle);
                    f.oneway = s.oneway;
                    f.moved = moved;
                    f.turned = turned;
                    f
                } else {
                    // Склон: подъём растягивается вместе с предметом, отражённый — в
                    // другую сторону.
                    let slope = if s.slope == 0.0 {
                        0.0
                    } else {
                        let flip = (a * d - b * cc).signum();
                        s.slope * (aabb.max[1] - aabb.min[1]) / (2.0 * hy).max(1e-12) * flip
                    };
                    let c = Collider {
                        aabb,
                        oneway: s.oneway,
                        id,
                        moved,
                        slope,
                    };
                    legacy = Some(c);
                    Surface::of_collider(&c)
                }
            }
        };
        // Частицам и показу — описанный прямоугольник (настоящая форма для них — 9.5).
        solids.0.push(legacy.unwrap_or(Collider {
            aabb: surface.aabb,
            oneway: surface.oneway,
            id,
            moved,
            slope: 0.0,
        }));
        solids.1.push(g.dynamic);
        let (layer, mask) = s
            .layer
            .unwrap_or((reyvec_game::layer::LEVEL, reyvec_game::layer::ALL));
        solids.2.push(surface.in_layer(layer, mask));
    }
}

fn wrap_angle(a: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}

/// Второй расчёт мест: после шага персонажа.
pub(super) fn propagate_again(roots: Roots, nodes: Nodes, out: Query<&mut GlobalPlacement>) {
    propagate(roots, nodes, out);
}

pub(super) type Roots<'w, 's> = Query<'w, 's, Entity, (With<Placement>, Without<ChildOf>)>;
pub(super) type Nodes<'w, 's> =
    Query<'w, 's, (&'static Placement, Option<&'static Children>, Has<Dynamic>)>;

/// Итоговые места: от корней к листьям.
pub(super) fn propagate(roots: Roots, nodes: Nodes, mut out: Query<&mut GlobalPlacement>) {
    for root in &roots {
        visit(root, &GlobalPlacement::ROOT, &nodes, &mut out);
    }
}

pub(super) fn visit(
    e: Entity,
    parent: &GlobalPlacement,
    nodes: &Nodes,
    out: &mut Query<&mut GlobalPlacement>,
) {
    let Ok((p, children, dynamic)) = nodes.get(e) else {
        return;
    };
    let g = parent.then(p, e, dynamic);
    if let Ok(mut slot) = out.get_mut(e) {
        *slot = g;
    }
    if let Some(children) = children {
        for child in children.iter() {
            visit(child, &g, nodes, out);
        }
    }
}
