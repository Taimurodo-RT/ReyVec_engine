//! Шаг физики (6.10): тела предметов, удар по телам, обломки разбитых врагов.

use super::*;

/// Обломки: место и цвет квадрата (не тела предметов и не герой).
pub(super) type PieceShapes<'w, 's> = Query<
    'w,
    's,
    (&'static mut Placement, &'static mut VectorShape),
    (Without<PhysBody>, Without<Player>),
>;

/// Физика (6.10): уровень, герой и враги — кинематические тела, предметы с `body` и
/// обломки — настоящие. Удар героя отбрасывает тела; разбитый враг разлетается на
/// обломки, которые через несколько секунд гаснут.
#[allow(clippy::too_many_arguments)]
pub(super) fn physics_step(
    solids: Res<Solids>,
    mut phys: ResMut<PhysicsRes>,
    mut events: ResMut<Events>,
    mut players: Query<(Entity, &mut Player)>,
    targets: Query<(Entity, &Target)>,
    mut bodies: Query<(Entity, &PhysBody, &mut Placement), Without<Player>>,
    mut shapes: PieceShapes,
    mut slots: SlotPlaces,
) {
    let phys = &mut *phys;
    let dt = phys.dt;
    if dt <= 0.0 || (!phys.physics.has_work() && phys.pieces.is_empty() && !phys.extra.has_work()) {
        return;
    }
    let step = events.step;
    // Кинематические тела: уровень, герои, целые враги.
    let level = solids.2.iter().cloned();
    let boxed = |id: u64, b: Aabb| {
        let mut s = Surface::of_collider(&Collider::solid(b));
        s.id = id;
        s
    };
    let heroes: Vec<Surface> = players
        .iter()
        .map(|(e, p)| boxed(e.to_bits(), p.body.bounds(&p.tuning)))
        .collect();
    let enemies: Vec<Surface> = targets
        .iter()
        .filter(|(_, t)| t.dummy.alive())
        .map(|(e, t)| boxed(e.to_bits(), t.dummy.bounds()))
        .collect();
    phys.physics
        .set_surfaces(level.chain(heroes).chain(enemies));
    // Герои толкают тела сбоку (9.2): тело разгоняется тем медленнее, чем тяжелее.
    // Мягкое тело толкается отрезком (9.4).
    for (id, speed) in std::mem::take(&mut phys.pushes) {
        if let Some(key) = id.checked_sub(SOFT_IDS) {
            phys.physics.push_soft(key, speed, HERO_MASS, dt);
            continue;
        }
        let Some(&body) = id
            .checked_sub(PLATFORM_IDS)
            .and_then(|i| phys.bodies.get(i as usize))
        else {
            continue;
        };
        phys.physics.push(body, speed, HERO_MASS, dt);
    }
    // Стоящие давят весом на то, что под ногами (9.4): качели наклоняются, желе
    // сминается, гамак прогибается.
    for press in std::mem::take(&mut phys.presses) {
        if press.ground >= SOFT_IDS {
            phys.physics
                .press_soft(press.feet, HERO_MASS, press.fall, dt);
        } else if let Some(&body) = phys.bodies.get((press.ground - PLATFORM_IDS) as usize) {
            let at = [press.feet.center()[0], press.feet.center()[1]];
            phys.physics.press(body, at, HERO_MASS, press.fall, dt);
        }
    }
    // Удар героя отбрасывает тела.
    for (_, mut player) in &mut players {
        let p = &mut *player;
        let Some(hitbox) = p.fighter.hitbox(&p.attack, p.body.pos, p.tuning.half()) else {
            continue;
        };
        for (e, b, _) in &bodies {
            let Some(bounds) = phys.physics.bounds(b.id) else {
                continue;
            };
            if hitbox.overlaps(&bounds) && p.fighter.first_hit(e.to_bits()) {
                let dir = p.fighter.dir();
                let k = p.attack.knockback;
                phys.physics
                    .kick(b.id, [dir * k[0] * 1.4, k[1] * 1.2], -dir * 6.0);
                let x = if dir > 0.0 {
                    bounds.min[0]
                } else {
                    bounds.max[0]
                };
                let y = (bounds.min[1] + bounds.max[1]) * 0.5;
                events.list.push_back((
                    step,
                    GameEvent::Hit {
                        at: [x as f32, y as f32],
                        dir: dir as f32,
                    },
                ));
            }
        }
    }
    // Обломки разбитых врагов.
    for brk in std::mem::take(&mut phys.breaks) {
        for i in 0..PIECES_PER_BREAK {
            phys.seed = phys
                .seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let r = |k: u32| ((phys.seed >> (16 + 8 * k)) & 0xff) as f64 / 255.0;
            let (rx, ry, rs, rv) = (r(0), r(1), r(2), r(3));
            // Свободный обломок или тот, что дольше всех лежит.
            let Some(slot) = (0..phys.pieces.len()).min_by(|&a, &b| {
                phys.pieces[a]
                    .left
                    .partial_cmp(&phys.pieces[b].left)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) else {
                break;
            };
            if let Some(old) = phys.pieces[slot].body.take() {
                phys.physics.remove(old);
            }
            let side = 0.18 + 0.12 * rs;
            let piece = brk.size[0].min(brk.size[1]) * side;
            let at = [
                brk.at[0] + (rx - 0.5) * brk.size[0] * 0.7,
                brk.at[1] + (ry - 0.5) * brk.size[1] * 0.7,
            ];
            let vel = [
                brk.dir * (0.6 + 1.6 * rv) + (rx - 0.5) * 1.2,
                1.0 + 1.8 * ry,
            ];
            let desc = reyvec_game::physics::BodyDesc {
                size: [piece, piece],
                density: 0.5,
                bounce: 0.35,
                bullet: true,
                ..Default::default()
            };
            let id = phys.physics.add(at, 0.0, vel, &desc);
            phys.physics
                .kick(id, [0.0, 0.0], (rs - 0.5) * 30.0 + i as f64);
            let p = &mut phys.pieces[slot];
            p.body = Some(id);
            p.left = PIECE_LIFE;
            if let Ok((mut place, mut shape)) = shapes.get_mut(p.entity) {
                place.scale = piece;
                shape.tint = brk.color;
            }
        }
    }
    phys.extra.before_step(&mut phys.physics, dt, &mut slots);
    phys.physics.step(dt);
    phys.extra
        .after_step(&mut phys.physics, dt, step, &mut events.list, &mut slots);
    // Места тел и обломков после шага (тело создано уже повёрнутым, поворот — его).
    for (_, b, mut place) in &mut bodies {
        if let Some((at, angle)) = phys.physics.pose(b.id) {
            place.at = at;
            place.angle = angle;
        }
    }
    for i in 0..phys.pieces.len() {
        let p = &mut phys.pieces[i];
        let Some(id) = p.body else {
            continue;
        };
        p.left -= dt;
        let Ok((mut place, _)) = shapes.get_mut(p.entity) else {
            continue;
        };
        if p.left <= 0.0 {
            p.body = None;
            p.left = 0.0;
            place.opacity = 0.0;
            phys.physics.remove(id);
            continue;
        }
        if let Some((at, angle)) = phys.physics.pose(id) {
            place.at = at;
            place.angle = angle;
            place.opacity = (p.left / PIECE_FADE).min(1.0) as f32;
        }
    }
    let ids = phys.bodies.clone();
    phys.surfaces = phys.physics.surfaces(&ids, PLATFORM_IDS);
    let soft = phys.physics.soft_surfaces(SOFT_IDS);
    phys.surfaces.extend(soft);
}
