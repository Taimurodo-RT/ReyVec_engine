//! Скрипты (6.9) и сюжет (Сю.7) на шаге логики.

use super::*;

/// Ведёт начатый разговор сюжета (Сю.7): ответы игрока, паузы, карточки «Скрипт».
pub(super) fn drive_story(
    time: Res<SceneTime>,
    mut clock: ResMut<StoryClock>,
    mut res: ResMut<ScriptRes>,
) {
    let dt = (time.0 - clock.0).clamp(0.0, 0.25) as f32;
    clock.0 = time.0;
    res.scripts.story_step(dt);
}

/// Скрипты (6.9): на каждом шаге логики каждый предмет со скриптом решает, что делать.
/// Раз в [`SCRIPT_CHECK_STEPS`] шагов изменённые файлы перечитываются.
pub(super) fn run_scripts(
    time: Res<SceneTime>,
    combat: Res<Combat>,
    mut res: ResMut<ScriptRes>,
    mut events: ResMut<Events>,
    players: Query<&Player>,
    mut actors: Query<(&mut Scripted, &mut Placement, Option<&mut Target>), Without<Player>>,
) {
    if actors.is_empty() {
        return;
    }
    let res = &mut *res;
    res.until_check = res.until_check.saturating_sub(1);
    if res.until_check == 0 {
        res.until_check = SCRIPT_CHECK_STEPS;
        res.scripts.reload_changed();
    }
    if combat.hit_stop > 0.0 {
        // Стоп-кадр: скрипты тоже ждут.
        return;
    }
    let (hero, hero_box) = match players.iter().next() {
        Some(p) => (
            reyvec_script::HeroIn {
                x: p.body.pos[0],
                y: p.body.pos[1],
                vx: p.body.vel[0],
                vy: p.body.vel[1],
                striking: p.fighter.striking(),
            },
            Some(p.body.bounds(&p.tuning)),
        ),
        None => (reyvec_script::HeroIn::default(), None),
    };
    let step = events.step;
    for (mut s, mut place, target) in &mut actors {
        let s = &mut *s;
        let (pos, vel, on_ground, wall, hp, alive) = match &target {
            Some(t) => {
                let b = &t.dummy.body;
                (
                    b.pos,
                    b.vel,
                    b.on_ground,
                    b.wall,
                    t.dummy.hp,
                    t.dummy.alive(),
                )
            }
            None => (place.at, [0.0, 0.0], false, 0.0, 0, true),
        };
        let bounds = Aabb::around(pos, [s.touch[0] * 0.5, s.touch[1] * 0.5]);
        let input = reyvec_script::ActorIn {
            x: pos[0],
            y: pos[1],
            vx: vel[0],
            vy: vel[1],
            on_ground,
            wall,
            hp,
            alive,
            touching: alive && hero_box.is_some_and(|h| h.overlaps(&bounds)),
            time: time.0,
        };
        let out = res
            .scripts
            .run(&s.actor, &input, &hero, crate::STEP.as_secs_f64());
        match target {
            Some(mut t) => {
                t.dummy.intent.x = out.move_x;
                t.dummy.intent.jump_pressed |= out.jump;
                // Прыжок врага всегда в полную высоту.
                t.dummy.intent.jump_held = true;
            }
            None => {
                if let (Some(x), Some(y)) = (out.x, out.y) {
                    place.at = [x, y];
                }
                place.opacity = if out.hidden { 0.0 } else { s.opacity };
            }
        }
        if out.collected && !s.collected {
            events.list.push_back((
                step,
                GameEvent::Pickup {
                    at: [pos[0] as f32, pos[1] as f32],
                },
            ));
        }
        s.collected = out.collected;
    }
}
