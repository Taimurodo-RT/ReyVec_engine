//! Проверки полигона физики (6.13): сочленения, мягкие тела, повторы и выброс тел из
//! файла сцены.

use super::*;

fn library() -> ShapeLibrary {
    ShapeLibrary::new(0.001)
}

fn polygon() -> SceneWorld {
    let path = std::path::Path::new(EXAMPLE_DIR).join("physics-polygon.ron");
    let text = std::fs::read_to_string(&path).unwrap();
    let file = SceneFile::parse_in(&text, Some(std::path::Path::new(EXAMPLE_DIR))).unwrap();
    SceneWorld::new(&file, &mut library(), &mut ShapeCache::new()).unwrap()
}

fn place(w: &mut SceneWorld, name: &str) -> Placement {
    let e = w
        .find(name)
        .unwrap_or_else(|| panic!("нет предмета «{name}»"));
    *w.world_mut().get::<Placement>(e).unwrap()
}

/// Прогоняет мир `seconds` секунд шагами логики, начиная с `from`.
fn run(w: &mut SceneWorld, from: f64, seconds: f64) -> f64 {
    let steps = (seconds * 60.0).round() as usize;
    let mut t = from;
    for _ in 0..steps {
        t += 1.0 / 60.0;
        w.update_at(t);
    }
    t
}

#[test]
fn polygon_scene_builds_and_runs() {
    let mut w = polygon();
    let t = run(&mut w, 0.0, 6.0);
    let scene = w.snapshot();
    let stats = scene.physics.expect("в полигоне есть физика");
    eprintln!("{stats:?}, сущностей {}", scene.entities);
    assert!(stats.bodies >= 300, "тел {}", stats.bodies);
    assert!(stats.joints >= 40, "сочленений {}", stats.joints);
    assert!(stats.soft >= 5, "мягких {}", stats.soft);
    // Желе — SDF-объекты, их три; всё в пределах видеокарты.
    assert!(scene.objects.len() <= MAX_OBJECTS);
    assert!(scene.objects.iter().map(|o| o.parts.len()).sum::<usize>() <= MAX_PARTS);
    // Маятник уронил домино: первые десять костяшек лежат одна на другой.
    for i in 1..=10 {
        let d = place(&mut w, &format!("домино {i}"));
        assert!(d.angle < -45.0, "домино {i} стоит: {}", d.angle);
    }
    // Пирамида стоит: верхний кубик почти на месте.
    let top = place(&mut w, "кубик 20.1");
    eprintln!("вершина пирамиды: {:?}", top.at);
    assert!((top.at[0] - (-0.4 + 0.0705 * 9.5)).abs() < 0.05);
    assert!(top.at[1] > -0.745 + 19.0 * 0.0702 - 0.06);
    // Мост висит: средняя доска ниже краёв, но не упала на пол.
    let mid = place(&mut w, "доска 7");
    eprintln!("доска 7: {:?}", mid.at);
    assert!(mid.at[1] < -0.35 && mid.at[1] > -0.6);
    // Машина ездит, кукла висит на верёвке.
    let car = place(&mut w, "машина");
    eprintln!("машина: {:?}", car.at);
    assert!(car.at[1] > -0.75 && car.at[1] < -0.4);
    let doll = place(&mut w, "кукла 1 голова");
    eprintln!("кукла 1: {:?}", doll.at);
    assert!(doll.at[1] > -0.3);
    let t = run(&mut w, t, 10.0);
    let scene = w.snapshot();
    let stats = scene.physics.unwrap();
    eprintln!("через {t:.0} с: {stats:?}");
    // Кубики накопились на полке, и её сварка оторвалась.
    assert!(stats.broken >= 1, "полка не оторвалась");
}

fn small(nodes: &str) -> SceneWorld {
    let text = format!("(format: \"reyvec-scene\", version: 1, nodes: [{nodes}])");
    let file = SceneFile::parse(&text).unwrap();
    SceneWorld::new(&file, &mut library(), &mut ShapeCache::new()).unwrap()
}

/// Выброшенные тела ложатся на тонкую полку, а не пролетают сквозь неё.
#[test]
fn spawned_bodies_land_on_a_shelf() {
    let mut w = small(
        r##"
        (name: "пол", solids: [(at: (0.0, -0.88), size: (8.0, 0.2))]),
        (name: "полка", at: (0.25, -0.25), body: (size: (0.5, 0.04)),
            shapes: [Rect(at: (0.0, 0.0), size: (0.5, 0.04), color: "#fff")],
            joints: [(kind: Weld, to: World, at: (-0.25, 0.0))]),
        (name: "песок", at: (0.4, 0.55), body: (size: (0.045, 0.045), density: 3.0),
            shapes: [Rect(at: (0.0, 0.0), size: (0.045, 0.045), color: "#fff")],
            spawn: (every: 0.25, life: 30.0, max: 4)),
    "##,
    );
    // Тела вылетают в 0,25, 0,5 и 0,75 с; к 1,2 с первые три лежат на полке.
    run(&mut w, 0.0, 1.2);
    for i in 1..=3 {
        let p = place(&mut w, &format!("песок ~{i}"));
        assert!(
            p.at[1] > -0.25 && p.at[1] < -0.1,
            "песок ~{i} не на полке: {:?}",
            p.at
        );
    }
}

#[test]
fn joint_to_a_missing_body_is_an_error() {
    let text = r##"(format: "reyvec-scene", version: 1, nodes: [
        (name: "ящик", body: (size: (0.1, 0.1)),
            joints: [(kind: Hinge, to: Node("нет такого"))]),
    ])"##;
    let file = SceneFile::parse(text).unwrap();
    let err = SceneWorld::new(&file, &mut library(), &mut ShapeCache::new())
        .err()
        .expect("ошибка");
    assert!(err.to_string().contains("нет такого"), "{err}");
}

#[test]
fn repeat_makes_named_copies_and_chains_them_with_prev() {
    let mut w = small(
        r##"
        (name: "звено", at: (0.0, 0.5), body: (size: (0.1, 0.02)),
            shapes: [Rect(at: (0.0, 0.0), size: (0.1, 0.02), color: "#fff")],
            repeat: (count: 5, step: (0.1, 0.0)),
            joints: [(kind: Hinge, to: Prev, at: (-0.05, 0.0))]),
    "##,
    );
    run(&mut w, 0.0, 2.0);
    let stats = w.snapshot().physics.unwrap();
    assert_eq!(stats.bodies, 5);
    assert_eq!(stats.joints, 5);
    // Цепь висит на первом шарнире: концы звеньев остались вместе.
    let (a, b) = (place(&mut w, "звено 2"), place(&mut w, "звено 3"));
    let gap = ((a.at[0] - b.at[0]).powi(2) + (a.at[1] - b.at[1]).powi(2)).sqrt();
    assert!((gap - 0.1).abs() < 0.01, "звенья разошлись: {gap}");
    assert!(b.at[1] < 0.45, "цепь не провисла: {:?}", b.at);
}

#[test]
fn shapes_of_the_polygon_collide_with_their_real_form() {
    // 9.3: ящик съезжает по дуге холма, герой стоит на вращающейся звезде, а с края
    // скруглённой банки и с капли соскальзывает на пол.
    let mut w = polygon();
    let mut t = run(&mut w, 0.0, 0.2);
    let start = place(&mut w, "ящик на холме").at;
    w.set_player_body(reyvec_game::Body::new([-2.7, 0.1]));
    let mut on_star = 0;
    let star = w.find("звезда").unwrap().to_bits();
    let mut star_ids = std::collections::HashSet::new();
    let (solids, _) = w.colliders();
    for (c, s) in &solids {
        if s.shape.as_polyline().is_some() {
            star_ids.insert(c.id);
        }
    }
    assert!(!star_ids.is_empty(), "звезда — контур, а не прямоугольник");
    for _ in 0..180 {
        t = run(&mut w, t, 1.0 / 60.0);
        let b = w.player_body().unwrap();
        on_star += usize::from(b.on_ground && b.ground().is_some_and(|g| star_ids.contains(&g)));
    }
    let _ = star;
    assert!(on_star > 60, "на звезде шагов: {on_star}");
    // Ящик соскользнул с вершины холма вправо, к подножию.
    let end = place(&mut w, "ящик на холме").at;
    eprintln!("ящик на холме: {start:?} → {end:?}");
    assert!(
        end[0] > start[0] + 0.3,
        "ящик не съехал: {start:?} → {end:?}"
    );
    assert!(end[1] < -0.6, "ящик не внизу: {end:?}");
    // С края банки и с капли герой не стоит внутри: ставим над краем — оказывается на полу.
    for x in [-5.43, -6.18] {
        w.set_player_body(reyvec_game::Body::new([x, -0.2]));
        t = run(&mut w, t, 2.0);
        let b = w.player_body().unwrap();
        let (solids, _) = w.colliders();
        let bb = reyvec_game::Aabb::around(b.pos, [0.07, 0.14]);
        for (_, s) in &solids {
            let mut one = reyvec_game::Space::new();
            one.push(s.clone());
            assert!(
                !one.overlaps(bb, &reyvec_game::Filter::default()),
                "герой внутри {:?} у {x}: {:?}",
                s.shape.shape_type(),
                b.pos
            );
        }
    }
}

#[test]
fn hero_stands_on_the_soft_jelly_and_squashes_it() {
    // 9.4: желе — опора героя. Мягкое желе под ним сминается, но не пропускает до пола.
    let mut w = polygon();
    let mut t = run(&mut w, 0.0, 2.0);
    w.set_player_body(reyvec_game::Body::new([15.1, 0.3]));
    let mut on_soft = 0;
    for _ in 0..180 {
        t = run(&mut w, t, 1.0 / 60.0);
        let b = w.player_body().unwrap();
        on_soft +=
            usize::from(b.on_ground && b.ground().is_some_and(|g| g >= (1 << 62) + (1 << 56)));
    }
    let b = w.player_body().unwrap();
    let feet = b.pos[1] - 0.14;
    eprintln!("герой на желе: {:?}, на мягком шагов {on_soft}", b.pos);
    assert!(on_soft > 120, "на желе шагов: {on_soft}");
    assert!(feet > -0.78 + 0.03, "не провалился до пола: {feet}");
}

#[test]
fn particles_see_figures_bodies_and_soft_bodies() {
    // 9.5: препятствия для частиц — настоящие формы: контур звезды (тонкие отрезки),
    // ящики физики (повёрнутые прямоугольники), желе (капсулы по краю).
    use reyvec_game::ObstacleShape;
    let mut w = polygon();
    run(&mut w, 0.0, 0.5);
    let scene = w.snapshot();
    let lines = scene
        .obstacles
        .iter()
        .filter(|o| matches!(o.shape, ObstacleShape::Capsule { r, .. } if r == 0.0))
        .count();
    let round = scene
        .obstacles
        .iter()
        .filter(|o| matches!(o.shape, ObstacleShape::Capsule { r, .. } if r > 0.0))
        .count();
    let boxes = scene
        .obstacles
        .iter()
        .filter(|o| matches!(o.shape, ObstacleShape::Box { .. }))
        .count();
    eprintln!("линий {lines}, круглых {round}, прямоугольников {boxes}");
    assert!(lines >= 10, "контуры фигур: {lines}");
    assert!(round >= 48, "желе и шары: {round}");
    assert!(boxes >= 200, "ящики и уровень: {boxes}");
    // Вращающаяся звезда движется: у её отрезков есть скорость.
    assert!(
        scene.obstacles.iter().any(|o| o.spin.abs() > 0.1
            && matches!(o.shape, ObstacleShape::Capsule { r, .. } if r == 0.0)),
        "звезда вращается"
    );
}

#[test]
fn crate_pushed_off_the_high_shelf_hurts_the_dummy() {
    // 9.6: на полигоне железный ящик, сброшенный с высокой полки, ранит манекен под ней.
    let mut w = polygon();
    let mut t = run(&mut w, 0.0, 1.0);
    let e = w.find("манекен под полкой").unwrap();
    let hp = w
        .world_mut()
        .get::<crate::world::Target>(e)
        .unwrap()
        .dummy
        .hp;
    // Тянем мышью железный ящик за правый край полки и отпускаем над манекеном.
    let crate_at = place(&mut w, "железный ящик на полке").at;
    let grab = |at: [f64; 2], held: bool| crate::Pointer {
        at,
        held,
        blast: false,
    };
    for i in 0..90 {
        let x = crate_at[0] + 0.3 * (f64::from(i) / 40.0).min(1.0);
        w.set_pointer(grab([x, crate_at[1] + 0.05], true));
        t = run(&mut w, t, 1.0 / 60.0);
    }
    w.set_pointer(grab([crate_at[0] + 0.3, crate_at[1]], false));
    run(&mut w, t, 2.0);
    let left = w
        .world_mut()
        .get::<crate::world::Target>(e)
        .unwrap()
        .dummy
        .hp;
    eprintln!("здоровье манекена {hp} → {left}");
    assert!(left < hp, "ящик не ранил: {hp} → {left}");
}

#[test]
fn ghost_beetle_runs_through_the_grate_to_the_hero() {
    // 9.6а: в полигоне жук-призрак видит героя за решёткой и проходит сквозь неё.
    let mut w = polygon();
    let mut t = run(&mut w, 0.0, 0.5);
    let e = w.find("жук-призрак").unwrap();
    let start = w
        .world_mut()
        .get::<crate::world::Target>(e)
        .unwrap()
        .dummy
        .body
        .pos[0];
    let mut crossed = false;
    for _ in 0..240 {
        w.set_player_body(reyvec_game::Body::new([-10.2, -0.6]));
        t = run(&mut w, t, 1.0 / 60.0);
        let x = w
            .world_mut()
            .get::<crate::world::Target>(e)
            .unwrap()
            .dummy
            .body
            .pos[0];
        crossed |= x > -10.5;
    }
    assert!(start < -10.7, "жук начинает за решёткой: {start}");
    assert!(crossed, "жук прошёл сквозь решётку");
}

#[test]
fn magnet_holds_the_iron_bar_in_the_air() {
    // 9.6б: в полигоне магнит поднимает железный брусок, а деревянный ящик рядом лежит.
    let mut w = polygon();
    run(&mut w, 0.0, 3.0);
    let bar = place(&mut w, "железный брусок").at;
    let wood = place(&mut w, "деревянный ящик").at;
    eprintln!("брусок {bar:?}, ящик {wood:?}");
    assert!(bar[1] > -0.6, "брусок в воздухе: {bar:?}");
    assert!(wood[1] < -0.7, "дерево на полу: {wood:?}");
}

fn press_grab(w: &mut SceneWorld, t: f64, held: &[reyvec_game::Action]) -> f64 {
    use reyvec_game::{Action, InputFrame};
    w.set_input(InputFrame::new(held, &[Action::Grab]));
    let t = run(w, t, 1.0 / 60.0);
    w.set_input(InputFrame::new(held, &[]));
    t
}

fn dummy_hp(w: &mut SceneWorld, name: &str) -> u32 {
    let e = w.find(name).unwrap();
    w.world_mut()
        .get::<crate::world::Target>(e)
        .unwrap()
        .dummy
        .hp
}

#[test]
fn hero_lifts_the_wooden_crate_and_throws_it_at_the_dummy() {
    // 9.6в: на полигоне герой берёт деревянный ящик, несёт его над головой и бросает в
    // манекен под полкой.
    let mut w = polygon();
    let mut t = run(&mut w, 0.0, 1.0);
    let hp = dummy_hp(&mut w, "манекен под полкой");
    let mut body = reyvec_game::Body::new([-7.66, -0.64]);
    body.facing = -1.0;
    w.set_player_body(body);
    t = run(&mut w, t, 0.3);
    t = press_grab(&mut w, t, &[]);
    t = run(&mut w, t, 0.5);
    let hero = w.player_body().unwrap();
    let held = place(&mut w, "деревянный ящик").at;
    eprintln!("герой {:?}, ящик {held:?}", hero.pos);
    assert!(
        held[1] > hero.pos[1] + 0.14 + 0.05,
        "ящик не над головой: {held:?}"
    );
    assert!((held[0] - hero.pos[0]).abs() < 0.06);
    // Бросок влево: ящик бьёт манекен.
    t = press_grab(&mut w, t, &[]);
    run(&mut w, t, 1.0);
    let left = dummy_hp(&mut w, "манекен под полкой");
    let at = place(&mut w, "деревянный ящик").at;
    eprintln!("здоровье манекена {hp} → {left}, ящик {at:?}");
    assert!(left < hp, "бросок не ранил: {hp} → {left}");
}

#[test]
fn hero_drags_the_stone_plate_slowly() {
    // 9.6в: каменную плиту не поднять, её тянут волоком, и герой идёт медленнее.
    use reyvec_game::Action::Left;
    let mut w = polygon();
    let mut t = run(&mut w, 0.0, 0.5);
    let start = place(&mut w, "каменная плита").at;
    // Герой вплотную слева, смотрит на плиту.
    let mut body = reyvec_game::Body::new([start[0] - 0.13 - 0.07 - 0.03, -0.64]);
    body.facing = 1.0;
    w.set_player_body(body);
    t = run(&mut w, t, 0.2);
    t = press_grab(&mut w, t, &[]);
    // Идёт влево, держа плиту.
    let from = w.player_body().unwrap().pos[0];
    w.set_input(reyvec_game::InputFrame::new(&[Left], &[]));
    t = run(&mut w, t, 1.0);
    let hero = w.player_body().unwrap();
    let plate = place(&mut w, "каменная плита").at;
    let walked = from - hero.pos[0];
    eprintln!("герой прошёл {walked:.2}, плита {start:?} → {plate:?}");
    assert!(walked > 0.4 && walked < 1.6 * 0.9, "{walked}");
    assert!(
        start[0] - plate[0] > walked - 0.08,
        "плита отстала: {plate:?}"
    );
    // Отпустил — плита остаётся, герой уходит.
    t = press_grab(&mut w, t, &[Left]);
    let from = w.player_body().unwrap().pos[0];
    let left_at = place(&mut w, "каменная плита").at;
    run(&mut w, t, 0.5);
    let gone = from - w.player_body().unwrap().pos[0];
    let plate = place(&mut w, "каменная плита").at;
    eprintln!("без плиты прошёл {gone:.2} за 0,5 с, плита {plate:?}");
    assert!(gone > 0.3, "{gone}");
    // Плита в голубой зоне слабой тяжести: скользит дальше, но недалеко.
    assert!(
        (plate[0] - left_at[0]).abs() < 0.3,
        "{left_at:?} → {plate:?}"
    );
}
