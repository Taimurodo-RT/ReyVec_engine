use super::*;
use reyvec_vector::Instance;

fn library() -> ShapeLibrary {
    ShapeLibrary::new(0.001)
}

fn minimal(body: &str) -> String {
    format!("(format: \"reyvec-scene\", version: 1, {body})")
}

#[test]
fn example_scene_parses_and_builds() {
    let file = SceneFile::parse(EXAMPLE).expect("пример сцены корректен");
    assert_eq!(file.background, Background::Lab);
    let mut lib = library();
    let mut cache = ShapeCache::new();
    let scene = file.build(&mut lib, &mut cache).unwrap();
    assert!(scene.shapes > 20);
    assert_eq!(scene.lights.len(), 4);
    assert_eq!(scene.objects.len(), 3);
    // Шар светится, слизь отбрасывает тень.
    assert_eq!(scene.objects.iter().filter(|o| o.glow).count(), 2);
    assert_eq!(scene.objects.iter().filter(|o| o.shadow).count(), 1);
    // Стол и полка — два отдельных слоя с тенью, окно без слоя.
    let shadow_layers = scene
        .items
        .iter()
        .filter(|i| {
            matches!(
                i,
                SceneItem::BeginLayer {
                    casts_shadow: true,
                    ..
                }
            )
        })
        .count();
    assert_eq!(shadow_layers, 2);
    // Сущности: предметы, фигуры, источники, объекты и общий корень.
    assert!(scene.entities > scene.shapes + 7);
    // Одинаковые формы (ножки стола, крышки) нарезаются один раз.
    assert!(cache.len() < scene.shapes);
    assert_eq!(lib.len(), cache.len());
}

fn build(text: &str) -> Scene {
    SceneFile::parse(text)
        .unwrap()
        .build(&mut library(), &mut ShapeCache::new())
        .unwrap()
}

fn shape_at(scene: &Scene, i: usize) -> Instance {
    scene
        .items
        .iter()
        .filter_map(|it| match it {
            SceneItem::Shape(s) => Some(*s),
            _ => None,
        })
        .nth(i)
        .expect("фигура")
}

#[test]
fn children_move_with_parent() {
    // Стол в (1; 0), книги на нём в (0,5; 0,1), шар на книгах: всё складывается.
    let text = minimal(
        r##"nodes: [(name: "стол", at: (1.0, 0.0), children: [
            (name: "книги", at: (0.5, 0.1), shapes: [Circle(at: (0.0, 0.0), r: 0.1, color: "#fff")],
             lights: [Point(at: (0.0, 0.2), color: "#fff", reach: 1.0)],
             objects: [(parts: [Circle(at: (0.1, 0.0), r: 0.05)], color: "#fff")])
        ])]"##,
    );
    let scene = build(&text);
    let t = shape_at(&scene, 0).transform.as_coeffs();
    assert!(
        (t[4] - 1.5).abs() < 1e-9 && (t[5] - 0.1).abs() < 1e-9,
        "{t:?}"
    );
    assert_eq!(scene.lights[0].a, [1.5, 0.3]);
    let p = scene.objects[0].parts[0];
    assert!(
        (p.a[0] - 1.6).abs() < 1e-6 && (p.a[1] - 0.1).abs() < 1e-6,
        "{p:?}"
    );
}

#[test]
fn rotation_and_scale_reach_lights_and_objects() {
    // Предмет повёрнут на 90° и увеличен вдвое: точка (0,1; 0) уходит в (0; 0,2),
    // дальность света и толщина частей растут вдвое, солнце поворачивается.
    let text = minimal(
        r##"nodes: [(angle: 90, scale: 2.0,
            lights: [Point(at: (0.1, 0.0), color: "#fff", reach: 1.0, size: 0.05),
                     Sun(from: (1.0, 0.0), color: "#fff", reach: 1.0)],
            objects: [(parts: [Circle(at: (0.1, 0.0), r: 0.05)], blend: 0.02, color: "#fff")])]"##,
    );
    let scene = build(&text);
    let lamp = scene.lights[0];
    assert!(
        lamp.a[0].abs() < 1e-6 && (lamp.a[1] - 0.2).abs() < 1e-6,
        "{lamp:?}"
    );
    assert!((lamp.reach - 2.0).abs() < 1e-6 && (lamp.softness - 0.1).abs() < 1e-6);
    let sun = scene.lights[1];
    assert!(
        sun.a[0].abs() < 1e-6 && (sun.a[1] - 1.0).abs() < 1e-6,
        "{sun:?}"
    );
    assert!(
        (sun.reach - 1.0).abs() < 1e-6,
        "длина теней солнца от размера не зависит"
    );
    let o = &scene.objects[0];
    assert!((o.parts[0].r - 0.1).abs() < 1e-6 && (o.k - 0.04).abs() < 1e-6);
}

#[test]
fn depth_decides_what_is_on_top() {
    // В файле сначала красный, потом синий; у красного глубина больше — он поверх.
    let text = minimal(
        r##"nodes: [
            (depth: 2, shapes: [Circle(at: (0.0, 0.0), r: 0.1, color: "#ff0000")]),
            (depth: 1, shapes: [Circle(at: (0.0, 0.0), r: 0.2, color: "#0000ff")]),
        ]"##,
    );
    let scene = build(&text);
    assert!(shape_at(&scene, 0).tint.b > 0.9, "сначала синий (глубже)");
    assert!(shape_at(&scene, 1).tint.r > 0.9, "поверх красный");
}

#[test]
fn shadow_and_opacity_are_inherited() {
    // Тень и прозрачность родителя переходят к детям; ребёнок может отключить тень.
    let text = minimal(
        r##"nodes: [(shadow: true, opacity: 0.5,
            shapes: [Circle(at: (0.0, 0.0), r: 0.1, color: "#fff")],
            children: [
                (shapes: [Circle(at: (0.3, 0.0), r: 0.1, color: "#fff")]),
                (shadow: false, shapes: [Circle(at: (0.6, 0.0), r: 0.1, color: "#fff")]),
            ])]"##,
    );
    let scene = build(&text);
    let layers: Vec<(f32, bool)> = scene
        .items
        .iter()
        .filter_map(|i| match i {
            SceneItem::BeginLayer {
                opacity,
                casts_shadow,
                ..
            } => Some((*opacity, *casts_shadow)),
            _ => None,
        })
        .collect();
    // Родитель с первым ребёнком — один слой с тенью, второй ребёнок — свой слой без тени.
    assert_eq!(layers, vec![(0.5, true), (0.5, false)]);
}

#[test]
fn old_layers_still_work() {
    // Файлы 5.1 со слоями читаются как раньше: два слоя с тенью и один без слоя.
    let text = minimal(
        r##"layers: [
            (shadow: true, shapes: [Rect(at: (0.0, 0.0), size: (0.2, 0.2), color: "#fff")]),
            (shadow: true, shapes: [Rect(at: (0.5, 0.0), size: (0.2, 0.2), color: "#fff")]),
            (shapes: [Rect(at: (1.0, 0.0), size: (0.2, 0.2), color: "#fff")]),
        ]"##,
    );
    let scene = build(&text);
    let begins = scene
        .items
        .iter()
        .filter(|i| {
            matches!(
                i,
                SceneItem::BeginLayer {
                    casts_shadow: true,
                    ..
                }
            )
        })
        .count();
    let ends = scene
        .items
        .iter()
        .filter(|i| matches!(i, SceneItem::EndLayer))
        .count();
    assert_eq!((begins, ends, scene.shapes), (2, 2, 3));
    assert!(matches!(scene.items.last(), Some(SceneItem::Shape(_))));
}

#[test]
fn limits_count_lights_inside_nodes() {
    let lamp = r##"Point(at: (0, 0), color: "#fff", reach: 1.0)"##;
    let some = [lamp; 10].join(", ");
    let text = minimal(&format!(
        "lights: [{some}], nodes: [(children: [(lights: [{some}])])]"
    ));
    let e = error(&text);
    assert!(e.message.contains("20"), "{e}");
    let e = error(&minimal(
        r##"nodes: [(children: [(name: "свеча", scale: 0.0)])]"##,
    ));
    assert!(
        e.message.contains("«свеча»") && e.message.contains("scale"),
        "{e}"
    );
}

#[test]
fn moving_and_recoloring_adds_no_shapes() {
    let a = minimal(
        r##"layers: [(shapes: [Rect(at: (0.0, 0.0), size: (0.5, 0.2), color: "#ff0000")])]"##,
    );
    let b = minimal(
        r##"layers: [(shapes: [Rect(at: (0.7, -0.3), size: (0.5, 0.2), angle: 30.0, color: "#00ff0080")])]"##,
    );
    let mut lib = library();
    let mut cache = ShapeCache::new();
    SceneFile::parse(&a)
        .unwrap()
        .build(&mut lib, &mut cache)
        .unwrap();
    let scene = SceneFile::parse(&b)
        .unwrap()
        .build(&mut lib, &mut cache)
        .unwrap();
    assert_eq!(lib.len(), 1);
    let SceneItem::Shape(inst) = &scene.items[0] else {
        panic!("ждали фигуру");
    };
    assert!((inst.tint.a - 128.0 / 255.0).abs() < 1e-6);
    // Новый размер — новая форма.
    let c = minimal(
        r##"layers: [(shapes: [Rect(at: (0.0, 0.0), size: (0.6, 0.2), color: "#ff0000")])]"##,
    );
    SceneFile::parse(&c)
        .unwrap()
        .build(&mut lib, &mut cache)
        .unwrap();
    assert_eq!(lib.len(), 2);
}

#[test]
fn empty_scene_has_defaults() {
    let file = SceneFile::parse("(format: \"reyvec-scene\", version: 1)").unwrap();
    assert_eq!(file.background, Background::Plain);
    let scene = file.build(&mut library(), &mut ShapeCache::new()).unwrap();
    assert!(scene.items.is_empty() && scene.lights.is_empty() && scene.objects.is_empty());
    assert!((scene.ambient[0] - 0.06).abs() < 1e-6);
}

#[test]
fn notepad_bom_is_accepted() {
    let text = format!("\u{feff}{}", minimal(""));
    assert!(SceneFile::parse(&text).is_ok());
}

#[test]
fn colors_parse() {
    let c = Hex::parse("#ffffff").unwrap().0;
    assert!((c.r - 1.0).abs() < 1e-6 && (c.a - 1.0).abs() < 1e-6);
    assert!(Hex::parse("#fff").is_ok());
    assert!(Hex::parse("#ff000080").is_ok());
    assert!(Hex::parse("ff0000").is_err());
    assert!(Hex::parse("#ff00").is_err());
    assert!(Hex::parse("#gg0000").is_err());
}

#[test]
fn lights_and_objects_convert() {
    let text = minimal(
        r##"lights: [Sun(from: (0.0, 2.0), color: "#ffffff", power: 2.0, reach: 1.0)],
            objects: [(parts: [Capsule(from: (0.0, 0.0), to: (1.0, 0.0), r: 0.1)], color: "#ffffff", power: 3.0, glow: true)]"##,
    );
    let scene = SceneFile::parse(&text)
        .unwrap()
        .build(&mut library(), &mut ShapeCache::new())
        .unwrap();
    let sun = scene.lights[0];
    assert_eq!(sun.kind, LightKind::Directional);
    assert_eq!(sun.a, [0.0, 1.0]);
    assert!((sun.color[0] - 2.0).abs() < 1e-5);
    assert!((sun.softness - 0.02).abs() < 1e-6);
    let o = &scene.objects[0];
    assert_eq!(o.parts[0].b, [1.0, 0.0]);
    assert!((o.color[0] - 3.0).abs() < 1e-5 && (o.color[3] - 1.0).abs() < 1e-6);
    assert!((o.k - 0.04).abs() < 1e-6);
}

fn error(text: &str) -> SceneError {
    SceneFile::parse(text).expect_err("ждали ошибку")
}

#[test]
fn errors_point_to_line_in_russian() {
    let e =
        error("(\n  format: \"reyvec-scene\",\n  version: 1,\n  lights: [Lamp(at: (0, 0))],\n)");
    assert_eq!(e.line, Some(4));
    assert!(
        e.message.contains("Lamp") && e.message.contains("`Point`"),
        "{e}"
    );
    assert!(e.to_string().starts_with("строка 4"), "{e}");

    let e = error(&minimal(
        r##"layers: [(shapes: [Circle(at: (0, 0), color: "#fff")])]"##,
    ));
    assert!(e.message.contains("`r`"), "{e}");

    let e = error(&minimal(
        r##"layers: [(shapes: [Circle(at: (0, 0), r: 0.1, colour: "#fff")])]"##,
    ));
    assert!(e.message.contains("colour"), "{e}");

    let e = error(&minimal(
        r##"layers: [(shapes: [Circle(at: (0, 0), r: 0.1, color: "red")])]"##,
    ));
    assert!(e.message.contains("#rrggbb"), "{e}");

    let e = error(&minimal("layers: [(shapes: []"));
    assert!(e.line.is_some(), "{e}");
}

#[test]
fn checks_catch_bad_numbers() {
    let e = error("(format: \"svg\", version: 1)");
    assert!(e.message.contains("reyvec-scene"), "{e}");
    let e = error("(format: \"reyvec-scene\", version: 7)");
    assert!(e.message.contains('7'), "{e}");
    let e = error(&minimal(
        r##"layers: [(name: "пол", shapes: [Circle(at: (0, 0), r: -1.0, color: "#fff")])]"##,
    ));
    assert!(
        e.message.contains("«пол»") && e.message.contains("фигура 1"),
        "{e}"
    );
    let e = error(&minimal(
        r##"layers: [(shapes: [Polygon(points: [(0, 0), (1, 0)], color: "#fff")])]"##,
    ));
    assert!(e.message.contains("3"), "{e}");
    let e = error(&minimal(
        r##"layers: [(shapes: [Path(svg: "M 0 0 X", color: "#fff")])]"##,
    ));
    assert!(e.message.contains("svg"), "{e}");
    let lamp = r##"Point(at: (0, 0), color: "#fff", reach: 1.0)"##;
    let many = vec![lamp; MAX_LIGHTS + 1].join(", ");
    let e = error(&minimal(&format!("lights: [{many}]")));
    assert!(e.message.contains("16"), "{e}");
    let e = error(&minimal(r##"objects: [(parts: [], color: "#fff")]"##));
    assert!(e.message.contains("SDF-объект 1"), "{e}");
}

#[test]
fn watcher_sees_saves() {
    let dir = std::env::temp_dir().join(format!("reyvec-scene-watch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scene.ron");
    std::fs::write(&path, minimal("")).unwrap();
    let mut watcher = SceneWatcher::new(&path).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(watcher.poll().is_none());
    std::fs::write(&path, minimal("background: Lab")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        if let Some(t) = watcher.poll() {
            break t.unwrap();
        }
        assert!(Instant::now() < deadline, "сохранение не замечено");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(text.contains("Lab"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn world(text: &str) -> SceneWorld {
    let file = SceneFile::parse(text).unwrap();
    let mut lib = library();
    SceneWorld::new(&file, &mut lib, &mut ShapeCache::new()).unwrap()
}

#[test]
fn motion_moves_children_over_time() {
    // Маятник: точка подвеса в (0; 1), лампа на 0,5 ниже, качается на 30° за 4 с.
    // Через 1 с (четверть периода) отклонение наибольшее: лампа уходит вправо.
    let text = minimal(
        r##"nodes: [(at: (0.0, 1.0), swing: (angle: 30, period: 4.0),
            lights: [Point(at: (0.0, -0.5), color: "#fff", reach: 1.0)])]"##,
    );
    let mut w = world(&text);
    w.update_at(0.0);
    let a = w.snapshot().lights[0].a;
    assert!(a[0].abs() < 1e-6 && (a[1] - 0.5).abs() < 1e-6, "{a:?}");
    w.update_at(1.0);
    let a = w.snapshot().lights[0].a;
    assert!((a[0] - 0.25).abs() < 1e-5, "sin 30° · 0,5 = 0,25: {a:?}");
    // Вращение и покачивание.
    let text = minimal(
        r##"nodes: [(spin: 90, bob: (by: (0.0, 0.1), period: 2.0),
            shapes: [Circle(at: (0.5, 0.0), r: 0.05, color: "#fff")])]"##,
    );
    let mut w = world(&text);
    w.update_at(0.5);
    // Через 0,5 с: повернулся на 45°, поднялся на 0,1 (четверть периода покачивания).
    let t = shape_at(&w.snapshot(), 0).transform.as_coeffs();
    let r = 0.5 * std::f64::consts::FRAC_1_SQRT_2;
    assert!(
        (t[4] - r).abs() < 1e-6 && (t[5] - r - 0.1).abs() < 1e-6,
        "{t:?}"
    );
}

#[test]
fn bad_motion_is_reported() {
    let e = error(&minimal(
        r##"nodes: [(name: "лампа", swing: (angle: 10, period: 0))]"##,
    ));
    assert!(
        e.message.contains("«лампа»") && e.message.contains("period"),
        "{e}"
    );
    let e = error(&minimal(r##"nodes: [(swing: (angle: 10))]"##));
    assert!(e.message.contains("period"), "{e}");
}

#[test]
fn lerp_goes_between_snapshots() {
    let text = minimal(
        r##"nodes: [(bob: (by: (1.0, 0.0), period: 4.0),
            shapes: [Circle(at: (0.0, 0.0), r: 0.05, color: "#fff")],
            lights: [Point(at: (0.0, 0.0), color: "#fff", reach: 1.0)],
            objects: [(parts: [Circle(at: (0.0, 0.0), r: 0.05)], color: "#fff")])]"##,
    );
    let mut w = world(&text);
    w.update_at(0.0);
    let a = w.snapshot();
    w.update_at(1.0);
    let b = w.snapshot();
    let m = Scene::lerp(&a, &b, 0.25);
    assert!((shape_at(&m, 0).transform.as_coeffs()[4] - 0.25).abs() < 1e-9);
    assert!((m.lights[0].a[0] - 0.25).abs() < 1e-6);
    assert!((m.objects[0].parts[0].a[0] - 0.25).abs() < 1e-6);
    // Снимки разного устройства (сменился файл): берётся новый.
    let other = build(&minimal(""));
    assert_eq!(Scene::lerp(&a, &other, 0.5).shapes, 0);
}

#[test]
fn logic_thread_steps_and_takes_new_worlds() {
    let text = minimal(
        r##"nodes: [(spin: 60, shapes: [Rect(at: (0.5, 0.0), size: (0.1, 0.1), color: "#fff")])]"##,
    );
    let logic = SceneLogic::start(world(&text), 0.0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while logic.steps() < 6 {
        assert!(Instant::now() < deadline, "логика не шагает");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Фигура повернулась: x уже не 0,5.
    let t = shape_at(&logic.frame(Instant::now()), 0)
        .transform
        .as_coeffs();
    assert!(t[4] < 0.4999, "{t:?}");
    // Новый мир без фигур.
    logic.replace(world(&minimal("")));
    let steps = logic.steps();
    while logic.steps() < steps + 3 {
        assert!(Instant::now() < deadline, "логика не шагает");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(logic.frame(Instant::now()).shapes, 0);
    assert!(logic.step_ms() < 50.0);
}

#[test]
fn prepared_scene_needs_no_cutting_on_the_frame_thread() {
    // Сразу: эталон.
    let mut lib = library();
    let mut cache = ShapeCache::new();
    let direct = SceneFile::parse(EXAMPLE)
        .unwrap()
        .build(&mut lib, &mut cache)
        .unwrap();
    // В фоне: нарезка в пачку, потом только положить.
    let mut lib2 = library();
    let mut cache2 = ShapeCache::new();
    let prepared = SceneFile::prepare(EXAMPLE, lib2.tessellator(), &cache2.keys()).unwrap();
    assert_eq!(prepared.new_shapes(), cache.len());
    assert!(lib2.is_empty(), "prepare не трогает набор");
    let mut world = prepared.finish(&mut lib2, &mut cache2).unwrap();
    assert_eq!(lib2.len(), lib.len(), "finish ничего не режет сверх пачки");
    world.update();
    assert_eq!(world.snapshot().items, direct.items);
    // Второй раз знакомые формы не режутся вовсе.
    let again = SceneFile::prepare(EXAMPLE, lib2.tessellator(), &cache2.keys()).unwrap();
    assert_eq!(again.new_shapes(), 0);
    again.finish(&mut lib2, &mut cache2).unwrap();
    assert_eq!(lib2.len(), lib.len());
    // Ошибка разбора приходит из фона с той же строкой.
    let err = SceneFile::prepare("(format: 1", lib2.tessellator(), &HashSet::new())
        .err()
        .unwrap();
    assert_eq!(err.line, Some(1));
}

#[test]
fn loader_cuts_in_background_and_frame_only_appends() {
    use reyvec_vector::DETAIL_FINEST;
    let mut lib = library();
    // Что-то уже лежит в наборе: номера пачки должны сдвинуться.
    let circle = kurbo::Circle::new((0.0, 0.0), 0.3).to_path(1e-3);
    let base = lib.add(Shape::fill(circle, Color::WHITE)).unwrap();
    let mut loader = Loader::start(lib.tessellator());
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <rect width="10" height="10" fill="#f00"/><circle cx="5" cy="5" r="3" fill="#0f0"/></svg>"##;
    let motif = include_bytes!("../../../assets/motif/morph.reyvec.json");
    loader.submit(LoadJob::Svg {
        name: "тест".into(),
        data: svg.as_slice().into(),
        options: reyvec_svg::SvgOptions::default(),
    });
    loader.submit(LoadJob::Motif {
        name: "пример".into(),
        data: motif.as_slice().into(),
    });
    loader.submit(LoadJob::Scene {
        text: EXAMPLE.into(),
        known: HashSet::new(),
    });
    loader.submit(LoadJob::Details(vec![
        lib.detail_job(base, DETAIL_FINEST).unwrap(),
    ]));
    assert_eq!(loader.pending(), 4);
    let mut kinds = Vec::new();
    while let Some(done) = loader.wait() {
        match done.result {
            LoadResult::Svg { result, .. } => {
                let (mut drawing, batch) = result.unwrap();
                assert_eq!(batch.len(), 2);
                let first = lib.append(batch);
                drawing.offset_shapes(first);
                let ids: Vec<u32> = drawing
                    .items
                    .iter()
                    .filter_map(|i| match i {
                        SceneItem::Shape(s) => Some(s.shape.0),
                        _ => None,
                    })
                    .collect();
                assert_eq!(ids, [first, first + 1]);
                assert!(lib.mesh(ShapeId(first + 1)).unwrap().triangle_count() > 0);
                kinds.push("svg");
            }
            LoadResult::Motif { result, .. } => {
                let (mut file, batch) = result.unwrap();
                let first = lib.append(batch);
                file.offset_shapes(first);
                assert!(file.shapes.iter().all(|s| s.shape.0 >= first));
                kinds.push("motif");
            }
            LoadResult::Scene(result) => {
                let mut cache = ShapeCache::new();
                let before = lib.len();
                let prepared = result.unwrap();
                let n = prepared.new_shapes();
                prepared.finish(&mut lib, &mut cache).unwrap();
                assert_eq!(lib.len(), before + n);
                kinds.push("scene");
            }
            LoadResult::Details(details) => {
                assert_eq!(details.len(), 1);
                for d in details {
                    let id = lib.insert_detail(d);
                    assert_eq!(lib.detail(base, DETAIL_FINEST), Some(id));
                }
                kinds.push("details");
            }
        }
    }
    // По порядку заданий.
    assert_eq!(kinds, ["svg", "motif", "scene", "details"]);
    assert_eq!(loader.pending(), 0);
    assert!(loader.poll().is_none());
}
