//! Шаг физики (6.10): тела предметов, удар по телам, обломки разбитых врагов.

use super::*;

/// Обломки: место и цвет квадрата (не тела предметов и не герой).
pub(super) type PieceShapes<'w, 's> = Query<
    'w,
    's,
    (&'static mut Placement, &'static mut VectorShape),
    (Without<PhysBody>, Without<Player>),
>;

/// Тише этого удар не слышен, единиц в секунду.
const KNOCK_SPEED: f64 = 0.6;
/// Масса, у которой громкость удара — по скорости: ящик 0,15 × 0,15 плотности 1.
/// Легче — тише, тяжелее — громче (до полутора раз).
const KNOCK_MASS: f64 = 0.0225;
/// Одно тело стучит не чаще раза за столько шагов: дребезг не звучит очередью.
const KNOCK_GAP: u64 = 5;
/// Ударов за шаг не больше: обвал пирамиды — несколько громких, а не сотня.
const KNOCKS_PER_STEP: usize = 8;
/// Тело, влетевшее в героя или врага быстрее этого, ранит (единиц в секунду).
const CRUSH_SPEED: f64 = 2.5;
/// Сколько «массы на скорость» в одной единице урона: ящик 0,2 × 0,2 плотности 2,
/// упавший с высоты героя, отнимает 2–3.
const CRUSH_UNIT: f64 = 0.08;

/// Касания за шаг физики (9.6): удары звучат и искрят материалом, тело, влетевшее в
/// героя или врага, ранит его.
fn touches(
    phys: &mut PhysicsRes,
    step: u64,
    events: &mut Events,
    combat: &mut Combat,
    players: &mut Query<(Entity, &mut Player)>,
    targets: &mut Query<(Entity, &mut Target), Without<Player>>,
) {
    use reyvec_game::physics::Toucher;
    let heroes: Vec<u64> = players.iter().map(|(e, _)| e.to_bits()).collect();
    let enemies: Vec<u64> = targets.iter().map(|(e, _)| e.to_bits()).collect();
    let character =
        |t: Toucher| matches!(t, Toucher::Solid(k) if heroes.contains(&k) || enemies.contains(&k));
    let mut knocks: Vec<(f64, GameEvent, Toucher)> = Vec::new();
    for touch in phys.physics.take_touches() {
        if !touch.begin {
            continue;
        }
        // Персонаж сам звучит шагами и приземлением: слышно только тело, которое в него
        // влетело. О пол и стены звучит само тело (резиновый мяч о камень — шлепок);
        // два тела — звонкий из двух.
        let (speed, material, who) = match (touch.a, touch.b) {
            (a, _) if character(a) => (touch.toward[1], touch.materials[1], touch.b),
            (_, b) if character(b) => (touch.toward[0], touch.materials[0], touch.a),
            (Toucher::Body(_), Toucher::Body(_)) => (touch.speed, touch.louder(), touch.a),
            (Toucher::Body(_), _) => (touch.speed, touch.materials[0], touch.a),
            _ => (touch.speed, touch.materials[1], touch.b),
        };
        // Громкость — от силы удара: лёгкое (капля дождя, песчинка) стучит тише.
        let speed = match who {
            Toucher::Body(b) => {
                let m = phys.physics.mass(b).unwrap_or(KNOCK_MASS);
                speed * (m / KNOCK_MASS).sqrt().clamp(0.25, 1.5)
            }
            Toucher::Solid(_) => speed,
        };
        if speed >= KNOCK_SPEED && phys.knocks.get(&who).is_none_or(|&s| s + KNOCK_GAP <= step) {
            knocks.push((
                speed,
                GameEvent::Impact {
                    at: [touch.at[0] as f32, touch.at[1] as f32],
                    speed: speed as f32,
                    material: crate::Material::of_physics(material),
                },
                who,
            ));
        }
        // Влетевшее тело ранит героя или врага.
        for (victim, body, toward) in [
            (touch.a, touch.b, touch.toward[1]),
            (touch.b, touch.a, touch.toward[0]),
        ] {
            let (Toucher::Solid(key), Toucher::Body(body)) = (victim, body) else {
                continue;
            };
            if toward < CRUSH_SPEED {
                continue;
            }
            let mass = phys.physics.mass(body).unwrap_or(0.0);
            let amount = (mass * toward / CRUSH_UNIT).floor() as u32;
            if amount == 0 {
                continue;
            }
            let from = phys.physics.pose(body).map_or(touch.at[0], |(at, _)| at[0]);
            if let Some((_, mut p)) = players.iter_mut().find(|(e, _)| e.to_bits() == key) {
                hurt_hero(&mut p, amount, from, step, events, combat);
            } else if let Some((_, mut t)) = targets
                .iter_mut()
                .find(|(e, t)| e.to_bits() == key && t.dummy.alive())
            {
                let dir = if t.dummy.body.pos[0] >= from {
                    1.0
                } else {
                    -1.0
                };
                let broken = t.dummy.hurt(amount, [dir * 0.8, 1.0]);
                let b = t.dummy.bounds();
                events.list.push_back((
                    step,
                    GameEvent::Damage {
                        at: [b.center()[0] as f32, b.max[1] as f32 + 0.04],
                        amount,
                        left: t.dummy.hp,
                    },
                ));
                if broken {
                    phys.breaks.push(Break {
                        at: t.dummy.body.pos,
                        size: t.dummy.tuning.size,
                        color: t.color,
                        dir,
                    });
                }
            }
        }
    }
    // Самые сильные удары шага.
    knocks.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, event, who) in knocks.into_iter().take(KNOCKS_PER_STEP) {
        phys.knocks.insert(who, step);
        events.list.push_back((step, event));
    }
    // Старые отметки не копятся.
    if step.is_multiple_of(600) {
        phys.knocks.retain(|_, s| *s + KNOCK_GAP > step);
    }
}

/// Физика (6.10): уровень, герой и враги — кинематические тела, предметы с `body` и
/// обломки — настоящие. Удар героя отбрасывает тела; разбитый враг разлетается на
/// обломки, которые через несколько секунд гаснут.
#[allow(clippy::too_many_arguments)]
pub(super) fn physics_step(
    solids: Res<Solids>,
    mut phys: ResMut<PhysicsRes>,
    mut events: ResMut<Events>,
    mut combat: ResMut<Combat>,
    mut players: Query<(Entity, &mut Player)>,
    mut targets: Query<(Entity, &mut Target), Without<Player>>,
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
    let boxed = |id: u64, b: Aabb, (layer, mask): (u32, u32)| {
        let mut s = Surface::of_collider(&Collider::solid(b)).in_layer(layer, mask);
        s.id = id;
        s
    };
    let heroes: Vec<Surface> = players
        .iter()
        .map(|(e, p)| boxed(e.to_bits(), p.body.bounds(&p.tuning), p.layer))
        .collect();
    let enemies: Vec<Surface> = targets
        .iter()
        .filter(|(_, t)| t.dummy.alive())
        .map(|(e, t)| boxed(e.to_bits(), t.dummy.bounds(), t.layer))
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
        if let Some(key) = press.ground.checked_sub(SOFT_IDS) {
            phys.physics
                .press_soft(key, press.feet, HERO_MASS, press.fall, dt);
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
    touches(
        phys,
        step,
        &mut events,
        &mut combat,
        &mut players,
        &mut targets,
    );
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
