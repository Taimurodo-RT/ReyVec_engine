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

/// Твёрдые прямоугольники в координатах сцены и на сколько каждый сдвинулся за шаг.
pub(super) fn gather_solids(
    mut q: Query<(Entity, &mut Solid, &GlobalPlacement)>,
    mut solids: ResMut<Solids>,
) {
    solids.0.clear();
    solids.1.clear();
    for (e, mut s, g) in &mut q {
        let [hx, hy] = s.half;
        let corners = [[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]].map(|[x, y]| {
            let p = g.affine * reyvec_vector::Point::new(x, y);
            [p.x, p.y]
        });
        let Some(aabb) = Aabb::of_points(corners) else {
            continue;
        };
        let c = g.affine.translation();
        let center = [c.x, c.y];
        let moved = s
            .last
            .map_or([0.0, 0.0], |l| [center[0] - l[0], center[1] - l[1]]);
        s.last = Some(center);
        // Склон: подъём растягивается вместе с предметом, отражённый — в другую сторону.
        let slope = if s.slope == 0.0 {
            0.0
        } else {
            let a = g.affine.as_coeffs();
            let flip = (a[0] * a[3] - a[1] * a[2]).signum();
            s.slope * (aabb.max[1] - aabb.min[1]) / (2.0 * hy).max(1e-12) * flip
        };
        solids.0.push(Collider {
            aabb,
            oneway: s.oneway,
            id: e.to_bits(),
            moved,
            slope,
        });
        solids.1.push(g.dynamic);
    }
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
