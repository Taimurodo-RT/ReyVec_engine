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
