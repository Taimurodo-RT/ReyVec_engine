//! Проверки рук (9.6в): поднять, нести, бросить, тянуть волоком.

use super::*;
use crate::space::layer;

const DT: f64 = 1.0 / 60.0;
const HERO_MASS: f64 = 0.04;
const HALF: [f64; 2] = [0.07, 0.14];
/// Верх пола.
const FLOOR: f64 = -0.8;

fn world_with(at: f64, desc: &BodyDesc) -> (Physics, BodyId) {
    let mut w = Physics::new(8.5);
    let y = FLOOR + desc.size[1] * 0.5;
    let b = w.add([at, y], 0.0, [0.0, 0.0], desc);
    (w, b)
}

fn holder(x: f64, facing: f64, vel: f64) -> Holder {
    Holder {
        pos: [x, FLOOR + HALF[1]],
        half: HALF,
        facing,
        vel: [vel, 0.0],
        layer: layer::HERO,
        mass: HERO_MASS,
    }
}

/// Шаг мира: пол и герой (кинематическое тело слоя героя), потом руки.
fn step(w: &mut Physics, hands: &mut Hands, who: &Holder) -> Option<HandsEvent> {
    let floor = Surface::of_collider(&Collider::solid(Aabb {
        min: [-5.0, -1.0],
        max: [5.0, FLOOR],
    }));
    let mut hero = Surface::of_collider(&Collider::solid(Aabb::around(who.pos, who.half)))
        .in_layer(layer::HERO, layer::ALL);
    hero.id = 2;
    w.set_surfaces([floor, hero]);
    let ev = hands.step(w, who, DT);
    w.step(DT);
    ev
}

#[test]
fn hero_lifts_a_light_box_carries_it_and_throws_it() {
    let (mut w, b) = world_with(0.16, &BodyDesc::default());
    let mut hands = Hands::default();
    let mut who = holder(0.0, 1.0, 0.0);
    for _ in 0..10 {
        step(&mut w, &mut hands, &who);
    }
    let ev = hands.press(&mut w, &who, false);
    assert_eq!(
        ev,
        Some(HandsEvent::Took {
            body: b,
            hold: Hold::Carry
        })
    );
    // Ящик поднялся над головой и идёт за героем.
    for _ in 0..60 {
        who.pos[0] += 0.8 * DT;
        who.vel[0] = 0.8;
        assert_eq!(step(&mut w, &mut hands, &who), None);
    }
    let (at, angle) = w.pose(b).unwrap();
    let top = who.pos[1] + HALF[1];
    eprintln!("ящик {at:?} {angle:.1}°, герой {:?}", who.pos);
    assert!(at[1] > top + 0.06 && at[1] < top + 0.1, "{at:?}");
    assert!((at[0] - who.pos[0]).abs() < 0.05 && angle.abs() < 5.0);
    // Несомое героя не задевает: для его мира столкновений ящика нет.
    let s = &w.surfaces(&[b], 100)[0];
    assert!(!layer::meet((s.layer, s.mask), (layer::HERO, layer::ALL)));
    // Бросок: ящик летит вперёд и падает далеко.
    who.vel[0] = 0.0;
    let thrown = hands.press(&mut w, &who, false);
    assert!(matches!(thrown, Some(HandsEvent::Threw { body, .. }) if body == b));
    for _ in 0..90 {
        step(&mut w, &mut hands, &who);
    }
    let (at, _) = w.pose(b).unwrap();
    eprintln!("брошен: {at:?}");
    assert!(at[0] > who.pos[0] + 0.8, "{at:?}");
    assert!(at[1] < FLOOR + 0.15);
    // Улетевшее снова сталкивается с героем.
    let s = &w.surfaces(&[b], 100)[0];
    assert!(layer::meet((s.layer, s.mask), (layer::HERO, layer::ALL)));
}

#[test]
fn hero_puts_a_box_down_in_front() {
    let (mut w, b) = world_with(-0.16, &BodyDesc::default());
    let mut hands = Hands::default();
    let who = holder(0.0, -1.0, 0.0);
    step(&mut w, &mut hands, &who);
    assert!(hands.press(&mut w, &who, false).is_some());
    for _ in 0..40 {
        step(&mut w, &mut hands, &who);
    }
    assert_eq!(
        hands.press(&mut w, &who, true),
        Some(HandsEvent::Let { body: b })
    );
    for _ in 0..90 {
        step(&mut w, &mut hands, &who);
    }
    let (at, _) = w.pose(b).unwrap();
    eprintln!("положен: {at:?}");
    assert!(at[0] < -0.12 && at[0] > -0.6, "{at:?}");
    assert!((at[1] - (FLOOR + 0.075)).abs() < 0.01, "{at:?}");
}

#[test]
fn heavy_crate_is_dragged_slowly_and_the_heaviest_does_not_move() {
    // Железный ящик 0,2 × 0,2 — вчетверо тяжелее героя: его тянут.
    let iron = BodyDesc {
        size: [0.2, 0.2],
        material: Material::Metal,
        density: Material::Metal.density(),
        friction: Material::Metal.friction(),
        ..BodyDesc::default()
    };
    let (mut w, b) = world_with(0.18, &iron);
    let mut hands = Hands::default();
    let mut who = holder(0.0, 1.0, 0.0);
    step(&mut w, &mut hands, &who);
    let ev = hands.press(&mut w, &who, false);
    assert!(
        matches!(
            ev,
            Some(HandsEvent::Took {
                hold: Hold::Drag { .. },
                ..
            })
        ),
        "{ev:?}"
    );
    let slow = hands.slow(&w, HERO_MASS);
    eprintln!("медленнее в {slow:.2}");
    assert!(slow < 0.8 && slow > 0.3);
    // Герой пятится влево — ящик едет за ним, на прежнем расстоянии.
    let speed = 1.6 * slow;
    for _ in 0..90 {
        who.pos[0] -= speed * DT;
        who.vel[0] = -speed;
        assert_eq!(step(&mut w, &mut hands, &who), None);
    }
    let (at, _) = w.pose(b).unwrap();
    eprintln!("ящик {at:?}, герой {:?}", who.pos);
    assert!(at[0] - who.pos[0] < 0.18 + 0.05, "отстал: {at:?}");
    assert!(at[0] < 0.0, "не поехал: {at:?}");
    // Прыжок отпускает.
    who.pos[1] += 0.2;
    assert_eq!(
        step(&mut w, &mut hands, &who),
        Some(HandsEvent::Let { body: b })
    );
    // Каменная глыба 0,5 × 0,4 — неподъёмна.
    let rock = BodyDesc {
        size: [0.5, 0.4],
        density: 2.5,
        ..BodyDesc::default()
    };
    let (mut w, b) = world_with(0.33, &rock);
    let mut hands = Hands::default();
    let who = holder(0.0, 1.0, 0.0);
    step(&mut w, &mut hands, &who);
    assert_eq!(
        hands.press(&mut w, &who, false),
        Some(HandsEvent::TooHeavy { body: b })
    );
    assert!(hands.held().is_none());
}

#[test]
fn thrown_box_hits_an_enemy_in_front() {
    let (mut w, b) = world_with(0.16, &BodyDesc::default());
    let mut hands = Hands::default();
    let who = holder(0.0, 1.0, 0.0);
    let floor = || {
        Surface::of_collider(&Collider::solid(Aabb {
            min: [-5.0, -1.0],
            max: [5.0, FLOOR],
        }))
    };
    let mut enemy = Surface::of_collider(&Collider::solid(Aabb::around(
        [0.6, FLOOR + 0.15],
        [0.08, 0.15],
    )))
    .in_layer(layer::ENEMY, layer::ALL);
    enemy.id = 3;
    let mut hero = Surface::of_collider(&Collider::solid(Aabb::around(who.pos, who.half)))
        .in_layer(layer::HERO, layer::ALL);
    hero.id = 2;
    let tick = |w: &mut Physics, hands: &mut Hands| {
        w.set_surfaces([floor(), hero.clone(), enemy.clone()]);
        hands.step(w, &who, DT);
        w.step(DT);
    };
    tick(&mut w, &mut hands);
    hands.press(&mut w, &who, false);
    for _ in 0..40 {
        tick(&mut w, &mut hands);
    }
    hands.press(&mut w, &who, false);
    let mut hit = false;
    for _ in 0..40 {
        tick(&mut w, &mut hands);
        hit |= w
            .take_touches()
            .iter()
            .any(|t| t.begin && t.seen_by(Toucher::Solid(3)).is_some() && t.speed > 2.5);
    }
    let (at, _) = w.pose(b).unwrap();
    eprintln!("ящик {at:?}");
    assert!(at[0] < 0.6, "пролетел сквозь врага: {at:?}");
    assert!(hit);
}
