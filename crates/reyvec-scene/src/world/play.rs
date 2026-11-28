//! Герой и враги (6.1–6.5): шаги контроллера, удар, попадания и как выглядит бой.

use super::*;

/// Персонаж игрока и манекены: шаги контроллера за время с прошлого шага логики, удар
/// и попадания (6.5). Во время стоп-кадра тела стоят, а свечение и тряска идут.
#[allow(clippy::too_many_arguments)]
pub(super) fn play(
    time: Res<SceneTime>,
    input: Res<GameInput>,
    solids: Res<Solids>,
    mut last: ResMut<LastStep>,
    mut combat: ResMut<Combat>,
    mut pending: ResMut<Pending>,
    mut events: ResMut<Events>,
    mut phys: ResMut<PhysicsRes>,
    mut players: Query<(Entity, &mut Player, &mut Placement)>,
    mut targets: Query<(Entity, &mut Target), Without<Player>>,
    scripts: Res<ScriptRes>,
    mut zones: Query<(&mut Zone, &GlobalPlacement)>,
) {
    phys.dt = 0.0;
    phys.presses.clear();
    let dt = (time.0 - last.0).clamp(0.0, 0.25);
    phys.step_dt = dt;
    last.0 = time.0;
    events.step += 1;
    let step = events.step;
    while events
        .list
        .front()
        .is_some_and(|&(s, _)| s + EVENT_STEPS <= step)
    {
        events.list.pop_front();
    }
    // Идёт разговор (Сю.7): герой стоит, клавиши достаются окну диалога.
    let talking = scripts.scripts.story().is_some_and(reyvec_script::talking);
    let input = if talking {
        InputFrame::default()
    } else {
        input.0
    };
    pending.jump |= input.pressed(Action::Jump);
    pending.dash |= input.pressed(Action::Dash);
    pending.attack |= input.pressed(Action::Attack);
    if input.pressed(Action::Grab) {
        pending.grab = true;
        pending.grab_down = input.held(Action::Down);
    }
    if input.aimed(Action::Attack) {
        pending.attack_at = input.aim();
    } else if input.pressed(Action::Attack) {
        pending.attack_at = None;
    }

    // Таймеры, которые идут и в стоп-кадре.
    combat.shake_left = (combat.shake_left - dt).max(0.0);
    let heroes: Vec<Aabb> = players
        .iter()
        .map(|(_, p, _)| p.body.bounds(&p.tuning))
        .collect();
    for (_, mut t) in &mut targets {
        t.dummy
            .tick(dt, |at| heroes.iter().any(|h| h.overlaps(&at)));
    }
    let frozen = combat.hit_stop.min(dt);
    combat.hit_stop -= frozen;
    let dt = dt - frozen;
    if dt <= 1e-9 {
        for (_, mut t) in &mut targets {
            t.dummy.moved = [0.0, 0.0];
        }
        return;
    }
    let presses = std::mem::take(&mut *pending);
    phys.dt = dt;
    phys.grab = presses.grab.then_some(presses.grab_down);

    // Запас на неточность дробей: ровно один шаг логики — один шаг персонажа.
    let steps = (dt / PLAYER_STEP - 1e-6).ceil().max(1.0) as usize;
    let sdt = dt / steps as f64;
    // Для героя манекены — тоже твёрдые тела (на них можно встать, их не пройти).
    let bodies: Vec<Surface> = targets
        .iter()
        .filter(|(_, t)| t.dummy.alive())
        .map(|(e, t)| {
            Surface::of_collider(&t.dummy.collider(e.to_bits())).in_layer(t.layer.0, t.layer.1)
        })
        .collect();
    // Мир столкновений героя (9.2): уровень, манекены и настоящие формы тел физики. На
    // тела встают, сбоку их толкают. Опоры сдвинулись один раз за шаг логики: везут
    // только в первом из шагов. Каждый видит только слои, с которыми сталкивается (9.6а).
    // Тела физики — только рядом с героями (9.7): в сцене на 2 000 тел контроллер не
    // перебирает дальние.
    let near_heroes: Vec<Aabb> = players
        .iter()
        .map(|(_, p, _)| near(p.body.bounds(&p.tuning), p.body.vel, dt))
        .collect();
    let mut world_space = solid_space(&solids);
    world_space.extend(bodies.iter().cloned());
    world_space.extend(
        phys.surfaces
            .iter()
            .filter(|s| near_heroes.iter().any(|a| a.overlaps(&s.aabb)))
            .cloned(),
    );

    let mut stop = false;
    for (hero, (entity, mut player, mut place)) in players.iter_mut().enumerate() {
        let p = &mut *player;
        let space = world_space.meeting(p.layer.0, p.layer.1);
        let space_still = space.still();
        // Зоны (9.6б): другая тяжесть меняет прыжок, ветер сдувает.
        let field = field_at(zones.iter(), p.body.pos);
        let mut tuning = p.tuning.in_gravity(field.gravity);
        // Тянет тяжёлое волоком (9.6в) — идёт медленнее.
        if let Some(h) = phys.hands.get(&entity.to_bits()) {
            tuning.speed *= h.slow(&phys.physics, HERO_MASS);
        }
        p.body.vel[0] += field.wind[0] * WIND_PUSH * dt;
        p.body.vel[1] += field.wind[1] * WIND_PUSH * dt;
        // С какой скоростью приземлился за этот шаг логики (удар по опоре, 9.4).
        let mut landing = 0.0f64;
        for i in 0..steps {
            // Нажатие — только на первом из нескольких шагов.
            let first = i == 0;
            let intent = Intent {
                x: input.x(),
                jump_pressed: first && presses.jump,
                jump_held: input.held(Action::Jump),
                down: input.held(Action::Down),
                dash_pressed: first && presses.dash,
                up: input.held(Action::Up),
            };
            let here = if first { &space } else { &space_still };
            let fall = -p.body.vel[1];
            let ev = p.body.step(&tuning, intent, here, sdt);
            if let Some(push) = ev.pushed {
                phys.pushes.push(push);
            }
            let feet = [
                p.body.pos[0] as f32,
                (p.body.pos[1] - p.tuning.size[1] * 0.5) as f32,
            ];
            if ev.landed {
                landing = landing.max(fall);
                let speed = fall.max(0.0) as f32;
                events
                    .list
                    .push_back((step, GameEvent::Landed { at: feet, speed }));
            }
            if ev.jumped || ev.wall_jumped {
                events
                    .list
                    .push_back((step, GameEvent::Jumped { at: feet }));
            }
            if ev.dashed {
                let dir = p.body.facing as f32;
                let at = [feet[0], p.body.pos[1] as f32];
                events.list.push_back((step, GameEvent::Dashed { at, dir }));
            }
            let can = !p.body.on_ledge();
            // Удар мышью — в сторону курсора (если так выбрано в настройках управления).
            if first
                && presses.attack
                && can
                && let Some(at) = presses.attack_at
                && (at[0] - p.body.pos[0]).abs() > 1e-6
            {
                p.body.facing = (at[0] - p.body.pos[0]).signum();
            }
            let swings = p.fighter.swings;
            p.fighter
                .step(&p.attack, first && presses.attack, can, p.body.facing, sdt);
            if p.fighter.swings > swings {
                let at = [p.body.pos[0] as f32, p.body.pos[1] as f32];
                let dir = p.body.facing as f32;
                events.list.push_back((step, GameEvent::Swing { at, dir }));
            }
            // Анимация (6.8): поза по телу и удару; нога встала на землю — шаг (звук и
            // пыль из-под ног).
            let anim = AnimInput {
                landed: ev.landed,
                ..AnimInput::of(&p.body, &p.tuning, &p.fighter, &p.attack)
            };
            if p.anim.step(&anim, sdt).step {
                let dir = p.body.vel[0].signum() as f32;
                events
                    .list
                    .push_back((step, GameEvent::Run { at: feet, dir }));
            }
            if let Some(hitbox) = p.fighter.hitbox(&p.attack, p.body.pos, p.tuning.half()) {
                for (e, mut t) in &mut targets {
                    if t.dummy.alive()
                        && hitbox.overlaps(&t.dummy.bounds())
                        && p.fighter.first_hit(e.to_bits())
                    {
                        let dir = p.fighter.dir();
                        // Место попадания: край цели со стороны героя, на высоте удара.
                        let b = t.dummy.bounds();
                        let x = if dir > 0.0 { b.min[0] } else { b.max[0] };
                        let y =
                            hitbox.min[1].max(b.min[1]) * 0.5 + hitbox.max[1].min(b.max[1]) * 0.5;
                        events.list.push_back((
                            step,
                            GameEvent::Hit {
                                at: [x as f32, y as f32],
                                dir: dir as f32,
                            },
                        ));
                        let k = p.attack.knockback;
                        let broken = t.dummy.hurt(p.attack.damage, [dir * k[0], k[1]]);
                        let top = [b.center()[0] as f32, b.max[1] as f32 + 0.04];
                        events.list.push_back((
                            step,
                            GameEvent::Damage {
                                at: top,
                                amount: p.attack.damage,
                                left: t.dummy.hp,
                            },
                        ));
                        if broken {
                            // Разбит: обломки разлетаются (6.10).
                            phys.breaks.push(Break::of(&t, dir));
                        }
                        p.hits += 1;
                        combat.hit_stop = combat.hit_stop.max(p.attack.hit_stop);
                        if p.attack.shake > 0.0 && p.attack.shake_time > 0.0 {
                            combat.shake = p.attack.shake;
                            combat.shake_time = p.attack.shake_time;
                            combat.shake_left = p.attack.shake_time;
                        }
                    }
                }
            }
            if combat.hit_stop > 0.0 {
                // Попал: остаток шага уже в стоп-кадре.
                stop = true;
                break;
            }
        }
        if p.body.pos[1] < p.spawn[1] - FALL_OUT {
            p.body = Body::new(p.spawn);
        }
        phys.presses.extend(Press::of(&p.body, &p.tuning, landing));
        // Вошёл в зону или вышел.
        let me_key = p.body.pos;
        for (mut z, g) in &mut zones {
            let now = z.area(g).contains(me_key);
            let id = hero as u64;
            let was = z.inside.contains(&id);
            if now != was {
                if now {
                    z.inside.push(id);
                } else {
                    z.inside.retain(|k| *k != id);
                }
                events.list.push_back((
                    step,
                    GameEvent::Zone {
                        at: [me_key[0] as f32, me_key[1] as f32],
                        zone: z.index,
                        enter: now,
                    },
                ));
            }
        }
        // Враги ранят касанием (6.10б). Враг для героя твёрдый, поэтому вплотную они
        // только касаются: прямоугольник героя чуть шире.
        p.health.tick(dt);
        let me = p.body.bounds(&p.tuning).grow(CONTACT_REACH);
        let touch = targets
            .iter()
            .filter(|_| !p.god)
            .find(|(_, t)| t.contact > 0 && t.dummy.alive() && me.overlaps(&t.dummy.bounds()));
        if let Some((_, t)) = touch {
            hurt_hero(
                p,
                t.contact,
                t.dummy.body.pos[0],
                step,
                &mut events,
                &mut combat,
            );
        }
        place.at = p.body.pos;
        place.flip = p.body.facing < 0.0;
        // Неуязвимый мигает.
        let inv = p.health.invulnerable;
        place.opacity = if inv > 0.0 && (inv * 12.0) as i64 % 2 == 0 {
            0.35
        } else {
            1.0
        };
    }
    // Где герой теперь, после своего шага: в него упираются манекены.
    let heroes_now: Vec<Surface> = players
        .iter()
        .map(|(_, p, _)| {
            Surface::of_collider(&Collider::solid(p.body.bounds(&p.tuning)))
                .in_layer(p.layer.0, p.layer.1)
        })
        .collect();
    // Брошенное героем (9.6в) для врага не опора и не стена: оно в него влетает и ранит
    // (касание в физике), а не подставляется под ноги.
    let flying: Vec<u64> = phys
        .hands
        .values()
        .flat_map(|h| h.flying())
        .filter_map(|b| phys.bodies.iter().position(|&x| x == b))
        .map(|i| PLATFORM_IDS + i as u64)
        .collect();
    for (e, mut t) in &mut targets {
        let layer = t.layer;
        let d = &mut t.dummy;
        if stop {
            d.moved = [0.0, 0.0];
            continue;
        }
        // Манекены не проходят друг сквозь друга: отброшенный упирается в соседа. Герой
        // для них тоже твёрдый (6.9): враг со скриптом упирается в героя, а не входит в
        // него.
        let me = e.to_bits();
        let mut mine = solid_space(&solids);
        mine.extend(bodies.iter().filter(|s| s.id != me).cloned());
        mine.extend(heroes_now.iter().cloned());
        let area = near(d.bounds(), d.body.vel, dt);
        mine.extend(
            phys.surfaces
                .iter()
                .filter(|s| area.overlaps(&s.aabb) && !flying.contains(&s.id))
                .cloned(),
        );
        let mine = mine.meeting(layer.0, layer.1);
        let mine_still = mine.still();
        let field = field_at(zones.iter(), d.body.pos);
        let saved = d.tuning;
        d.tuning = saved.in_gravity(field.gravity);
        d.body.vel[0] += field.wind[0] * WIND_PUSH * dt;
        d.body.vel[1] += field.wind[1] * WIND_PUSH * dt;
        let mut moved = [0.0, 0.0];
        for i in 0..steps {
            d.step(if i == 0 { &mine } else { &mine_still }, sdt);
            moved[0] += d.moved[0];
            moved[1] += d.moved[1];
        }
        d.tuning = saved;
        d.moved = moved;
        phys.presses.extend(Press::of(&d.body, &d.tuning, 0.0));
        if d.body.pos[1] < d.spawn[1] - FALL_OUT {
            d.body = Body::new(d.spawn);
            d.moved = [0.0, 0.0];
        }
    }
}

/// Где персонаж может оказаться за шаг логики `dt` (с запасом): тела физики дальше
/// этого его шагу не нужны.
fn near(bounds: Aabb, vel: [f64; 2], dt: f64) -> Aabb {
    let reach = [
        0.3 + (vel[0].abs() + 1.0) * dt * 2.0,
        0.3 + (vel[1].abs() + 1.0) * dt * 2.0,
    ];
    Aabb {
        min: [bounds.min[0] - reach[0], bounds.min[1] - reach[1]],
        max: [bounds.max[0] + reach[0], bounds.max[1] + reach[1]],
    }
}

/// Как сильно ветер зоны толкает героя и врагов: доля скорости ветра за секунду.
const WIND_PUSH: f64 = 3.0;

/// Героя ранили на `amount` (касание врага, упавший ящик): отбрасывает от `from` по
/// горизонтали и чуть вверх, тряска; кончилось здоровье — появляется снова в начале.
pub(super) fn hurt_hero(
    p: &mut Player,
    amount: u32,
    from: f64,
    step: u64,
    events: &mut Events,
    combat: &mut Combat,
) {
    if p.god {
        return;
    }
    let at = [p.body.pos[0] as f32, p.body.pos[1] as f32];
    match p.health.hurt(amount) {
        Hurt::Ignored => {}
        hurt => {
            let dir = if p.body.pos[0] >= from { 1.0 } else { -1.0 };
            p.body.vel = [dir * HURT_PUSH[0], HURT_PUSH[1]];
            p.body.on_ground = false;
            events.list.push_back((
                step,
                GameEvent::Hurt {
                    at,
                    amount,
                    dir: dir as f32,
                },
            ));
            combat.shake = 0.03;
            combat.shake_time = 0.25;
            combat.shake_left = 0.25;
            if hurt == Hurt::Died {
                events.list.push_back((step, GameEvent::Died { at }));
                p.body = Body::new(p.spawn);
                p.health.restore();
            }
        }
    }
}

/// Как выглядит бой: манекены на своих местах, белеют от попадания, разбитые не видны;
/// взмах удара виден только в фазе удара.
pub(super) fn combat_look(
    players: Query<&Player>,
    mut targets: Query<(&Target, &mut Placement), Without<Strike>>,
    mut strikes: Query<&mut Placement, With<Strike>>,
) {
    let striking = players.iter().any(|p| p.fighter.striking());
    for mut place in &mut strikes {
        place.opacity = if striking { 1.0 } else { 0.0 };
    }
    for (t, mut place) in &mut targets {
        place.at = t.dummy.body.pos;
        place.flip = t.dummy.body.facing < 0.0;
        place.flash = (t.dummy.flash / reyvec_game::combat::FLASH_TIME) as f32;
        place.opacity = if t.dummy.alive() { t.opacity } else { 0.0 };
    }
}

/// Мир столкновений из твёрдого сцены (без героев, врагов и тел).
fn solid_space(solids: &Solids) -> Space {
    let mut s = Space::default();
    s.extend(solids.2.iter().cloned());
    s
}
