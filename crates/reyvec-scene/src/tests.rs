use super::*;
use reyvec_vector::Instance;

fn library() -> ShapeLibrary {
    ShapeLibrary::new(0.001)
}

fn example_dir() -> Option<&'static std::path::Path> {
    Some(std::path::Path::new(EXAMPLE_DIR))
}

fn minimal(body: &str) -> String {
    format!("(format: \"reyvec-scene\", version: 1, {body})")
}

#[test]
fn example_scene_parses_and_builds() {
    let file = SceneFile::parse_in(EXAMPLE, example_dir()).expect("пример сцены корректен");
    assert_eq!(file.background, Background::Lab);
    let mut lib = library();
    let mut cache = ShapeCache::new();
    let scene = file.build(&mut lib, &mut cache).unwrap();
    assert!(scene.shapes > 20);
    // В комнате 4 источника и 3 объекта, в коридоре (5.5в) 16 бра с огоньками и 4 лужи:
    // больше, чем видеокарта берёт за кадр, в кадр идут только те, что на экране.
    // С 6.2 ещё награда на столбе в конце коридора: свет и светящийся шар, с 6.4 — две
    // лампы с огоньками над уступами.
    assert_eq!(scene.lights.len(), 4 + 16 + 1 + 2);
    assert_eq!(scene.objects.len(), 3 + 16 + 4 + 1 + 2);
    // Шар, неон, огоньки и награда светятся, слизь отбрасывает тень.
    assert_eq!(
        scene.objects.iter().filter(|o| o.glow).count(),
        2 + 16 + 1 + 2
    );
    assert_eq!(scene.objects.iter().filter(|o| o.shadow).count(), 1 + 4);
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
    // Слои с тенью: стол, полка, герой, ящики под полкой, 6 препятствий коридора (6.1),
    // подъёмник и столб (6.2), уступы (6.4), 2 манекена (6.5), жук (6.9), мяч и 2 ящика
    // с физикой (6.10); окно без слоя.
    assert_eq!(shadow_layers, 19);
    // Герой (6.1) и твёрдое: пол, стена слева, столешница, 6 банок с крышками, 3 книги,
    // полка, 2 ящика, 6 препятствий коридора, подъёмник и столб; с 6.4 край уступов и
    // сами уступы: 13 склонов, 4 доски и всего 6 прямоугольников из сотен клеток блоков.
    let player = scene.player.expect("в примере есть герой");
    assert_eq!(
        player.solids,
        2 + 1 + 6 + 3 + 1 + 2 + 6 + 2 + 1 + 13 + 4 + 6
    );
    // Сущности: предметы, фигуры, рисунки, источники, объекты и общий корень.
    assert!(scene.entities > 40, "{}", scene.entities);
    // Рисунки из файлов (5.5): фото SVG на полке и клякса Motif за окном.
    assert_eq!(cache.files(), 2);
    let mut morphs_at = |t: f64| {
        let mut w = SceneWorld::new(&file, &mut lib, &mut cache).unwrap();
        w.update_at(t);
        w.snapshot()
            .items
            .iter()
            .filter_map(|i| match i {
                SceneItem::Shape(s)
                    if lib.method(s.shape) == Some(reyvec_vector::FillMethod::Stencil)
                        && !lib.morph_keyframes(s.shape).is_empty() =>
                {
                    Some(s.morph)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let (a, b) = (morphs_at(0.5), morphs_at(1.5));
    assert_eq!(a.len(), 1, "одна перетекающая клякса");
    assert_ne!(a, b, "клякса перетекает со временем");
    // Одинаковые формы (ножки стола, крышки) нарезаются один раз; остальное в наборе —
    // фигуры рисунков.
    assert!(cache.len() < scene.shapes);
    assert!(lib.len() > cache.len());
}

#[test]
fn unused_shapes_and_files_are_released_after_a_new_world() {
    let dir = std::env::temp_dir().join(format!("reyvec-release-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("pic.svg"),
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
            <rect width="20" height="10" fill="#f00"/><circle cx="5" cy="5" r="3"/></svg>"##,
    )
    .unwrap();
    let mut lib = library();
    let mut cache = ShapeCache::new();
    let world = |text: &str, lib: &mut ShapeLibrary, cache: &mut ShapeCache| {
        let file = SceneFile::parse_in(&minimal(text), Some(&dir)).unwrap();
        SceneWorld::new(&file, lib, cache)
    };
    let full = r##"nodes: [(
        shapes: [
            Circle(at: (0, 0), r: 0.1, color: "#fff"),
            Rect(at: (1, 0), size: (0.2, 0.2), color: "#fff"),
        ],
        pictures: [(svg: "pic.svg", height: 0.5)],
    )]"##;
    world(full, &mut lib, &mut cache).unwrap();
    cache.release_unused(&mut lib);
    assert_eq!(lib.live(), 4, "две формы и две фигуры рисунка");
    let circle = *cache.shapes.values().min_by_key(|id| id.0).unwrap();
    // Ошибка в файле: мир не построен, ничего не выбрасываем (старый мир ещё рисуется).
    assert!(
        world(
            r#"nodes: [(pictures: [(svg: "нет.svg", height: 0.5)])]"#,
            &mut lib,
            &mut cache
        )
        .is_err()
    );
    // Сохранили без квадрата и без рисунка.
    world(
        r##"nodes: [(shapes: [Circle(at: (0.5, 0), r: 0.1, color: "#f00")])]"##,
        &mut lib,
        &mut cache,
    )
    .unwrap();
    let released = cache.release_unused(&mut lib);
    assert_eq!(
        released,
        Released {
            shapes: 3,
            files: 1
        }
    );
    assert_eq!(lib.live(), 1);
    assert!(lib.contains(circle), "круг остался под тем же номером");
    assert_eq!((cache.len(), cache.files()), (1, 0));
    assert_eq!(cache.release_unused(&mut lib), Released::default());
    // Вернули квадрат и рисунок: режутся заново под новыми номерами.
    let before = lib.len();
    world(full, &mut lib, &mut cache).unwrap();
    assert_eq!(lib.len(), before + 3);
    assert_eq!(lib.live(), 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pictures_and_morphs_come_from_files_next_to_the_scene() {
    let dir = std::env::temp_dir().join(format!("reyvec-assets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let svg = dir.join("pic.svg");
    std::fs::write(
        &svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
            <rect width="20" height="10" fill="#f00"/></svg>"##,
    )
    .unwrap();
    let text = minimal(r#"nodes: [(at: (0.5, 0), pictures: [(svg: "pic.svg", height: 0.5)])]"#);
    let mut lib = library();
    let mut cache = ShapeCache::new();
    let file = SceneFile::parse_in(&text, Some(&dir)).unwrap();
    let scene = file.build(&mut lib, &mut cache).unwrap();
    assert_eq!(scene.shapes, 1);
    assert_eq!(lib.len(), 1);
    // Пересохранили сцену, рисунок тот же: не режется снова.
    file.build(&mut lib, &mut cache).unwrap();
    assert_eq!(lib.len(), 1);
    // Поменяли рисунок: перечитывается (другой размер файла — другой ключ).
    std::fs::write(
        &svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
            <rect width="20" height="10" fill="#f00"/><circle cx="5" cy="5" r="3"/></svg>"##,
    )
    .unwrap();
    let scene = file.build(&mut lib, &mut cache).unwrap();
    assert_eq!(scene.shapes, 2);
    // Рисунок стоит в предмете: сдвинут на at предмета.
    let first_x = |scene: &Scene| match scene.items[0] {
        SceneItem::Shape(s) => s.transform.translation().x,
        other => panic!("{other:?}"),
    };
    let at_zero = SceneFile::parse_in(
        &minimal(r#"nodes: [(pictures: [(svg: "pic.svg", height: 0.5)])]"#),
        Some(&dir),
    )
    .unwrap()
    .build(&mut lib, &mut cache)
    .unwrap();
    assert!((first_x(&scene) - first_x(&at_zero) - 0.5).abs() < 1e-9);
    // Ошибки по-русски: нет файла, нет фигуры, нет папки.
    let err = |t: &str, dir: Option<&std::path::Path>| {
        SceneFile::parse_in(&minimal(t), dir)
            .unwrap()
            .build(&mut library(), &mut ShapeCache::new())
            .unwrap_err()
            .message
    };
    let e = err(
        r#"nodes: [(pictures: [(svg: "нет.svg", height: 0.5)])]"#,
        Some(&dir),
    );
    assert!(e.contains("рисунок 1") && e.contains("не открыть"), "{e}");
    let e = err(
        r#"nodes: [(pictures: [(svg: "pic.svg", height: 0.5)])]"#,
        None,
    );
    assert!(e.contains("путь не от чего считать"), "{e}");
    // Полный путь в кавычках RON: `{:?}` удваивает обратные косые черты Windows.
    let motif = format!(
        "{:?}",
        format!("{EXAMPLE_DIR}/../assets/motif/morph.reyvec.json")
    );
    let e = err(
        &format!(r#"nodes: [(morphs: [(motif: {motif}, shape: "нет")])]"#),
        None,
    );
    assert!(
        e.contains("нет фигуры «нет»") && e.contains("«blob»"),
        "{e}"
    );
    let e = err(&format!(r#"nodes: [(morphs: [(motif: {motif})])]"#), None);
    assert!(e.contains("несколько фигур"), "{e}");
    let e = err(
        &format!(r#"nodes: [(morphs: [(motif: {motif}, shape: "blob", animation: "Прыг")])]"#),
        None,
    );
    assert!(
        e.contains("нет анимации «Прыг»") && e.contains("«Flow»"),
        "{e}"
    );
    let _ = std::fs::remove_dir_all(&dir);
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
    let some = vec![lamp; MAX_SCENE_LIGHTS / 2 + 1].join(", ");
    let text = minimal(&format!(
        "lights: [{some}], nodes: [(children: [(lights: [{some}])])]"
    ));
    let e = error(&text);
    assert!(
        e.message.contains(&format!("{}", MAX_SCENE_LIGHTS + 2)),
        "{e}"
    );
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
    // Больше 16 источников в сцене можно (в кадр идут только те, что на экране, 5.5в),
    // больше MAX_SCENE_LIGHTS — нет.
    let many = vec![lamp; MAX_LIGHTS + 1].join(", ");
    assert!(SceneFile::parse(&minimal(&format!("lights: [{many}]"))).is_ok());
    let many = vec![lamp; MAX_SCENE_LIGHTS + 1].join(", ");
    let e = error(&minimal(&format!("lights: [{many}]")));
    assert!(e.message.contains("1024"), "{e}");
    let circle = "Circle(at: (0, 0), r: 0.1)";
    let parts = vec![circle; MAX_PARTS + 1].join(", ");
    let e = error(&minimal(&format!(
        r##"objects: [(parts: [{parts}], color: "#fff")]"##
    )));
    assert!(e.message.contains("128"), "{e}");
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

/// Список кадра буквами: S фигура, D{ … } движущиеся, L( … ) слой.
fn marks(items: &[SceneItem]) -> String {
    items
        .iter()
        .map(|i| match i {
            SceneItem::Shape(_) => "S",
            SceneItem::Text(_) => "T",
            SceneItem::BeginLayer { .. } => "L(",
            SceneItem::EndLayer => ")",
            SceneItem::BeginMask { .. } => "M[",
            SceneItem::EndMask => "]",
            SceneItem::BeginDynamic => "D{",
            SceneItem::EndDynamic => "}",
        })
        .collect()
}

#[test]
fn moving_things_are_marked_in_the_snapshot() {
    // Неподвижная полка, лифт (покачивание) с твёрдой площадкой и ещё одна неподвижная
    // фигура поверх: метки окружают только лифт, твёрдое помечено так же.
    let text = minimal(
        r##"nodes: [
            (depth: 0.0, shapes: [Rect(at: (0.0, 0.0), size: (1.0, 0.2), color: "#fff", solid: true)]),
            (depth: 1.0, bob: (by: (0.0, 0.5), period: 2.0), children: [
                (shapes: [Rect(at: (2.0, 0.0), size: (1.0, 0.1), color: "#888", solid: true),
                          Circle(at: (2.0, 0.3), r: 0.05, color: "#888")])]),
            (depth: 2.0, shapes: [Circle(at: (-2.0, 0.0), r: 0.1, color: "#f00")]),
        ]"##,
    );
    let mut w = world(&text);
    w.update_at(0.25);
    let scene = w.snapshot();
    assert_eq!(marks(&scene.items), "SD{SS}S");
    assert_eq!(scene.solids.len(), 2);
    assert_eq!(scene.solids_dynamic.len(), 2);
    let lift = scene
        .solids
        .iter()
        .position(|c| c.aabb.min[0] > 1.0)
        .expect("площадка лифта");
    assert!(scene.solids_dynamic[lift]);
    assert!(!scene.solids_dynamic[1 - lift]);

    // Номер мира один на все снимки мира и новый у нового мира; у пустой сцены — 0.
    let first = scene.generation;
    assert_ne!(first, 0);
    w.update_at(0.5);
    assert_eq!(w.snapshot().generation, first);
    assert_ne!(world(&text).snapshot().generation, first);
    assert_eq!(Scene::default().generation, 0);
}

#[test]
fn example_scene_marks_the_hero_and_the_lift() {
    let file = SceneFile::parse_in(EXAMPLE, example_dir()).unwrap();
    let scene = file.build(&mut library(), &mut ShapeCache::new()).unwrap();
    let mut depth = 0;
    let (mut moving, mut still) = (0, 0);
    for item in &scene.items {
        match item {
            SceneItem::BeginDynamic => {
                assert_eq!(depth, 0, "метки не вкладываются");
                depth += 1;
            }
            SceneItem::EndDynamic => {
                assert_eq!(depth, 1, "конец без начала");
                depth -= 1;
            }
            SceneItem::Shape(_) if depth > 0 => moving += 1,
            SceneItem::Shape(_) => still += 1,
            _ => {}
        }
    }
    assert_eq!(depth, 0, "метки закрыты");
    // Герой, лифт, стрелки часов, лампа, шар, монеты со скриптами — подвижные; мебель,
    // стены и пол — нет, и их больше.
    assert!(moving >= 17, "{moving} подвижных");
    assert!(still > moving, "{still} неподвижных, {moving} подвижных");
    // Площадка лифта — единственное движущееся твёрдое первой комнаты.
    let lift: Vec<_> = (0..scene.solids.len())
        .filter(|&i| scene.solids_dynamic[i])
        .collect();
    assert!(!lift.is_empty(), "лифт помечен");
    assert!(lift.len() < scene.solids.len() / 4, "{lift:?}");
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
    let direct = SceneFile::parse_in(EXAMPLE, example_dir())
        .unwrap()
        .build(&mut lib, &mut cache)
        .unwrap();
    // В фоне: нарезка в пачку, потом только положить.
    let mut lib2 = library();
    let mut cache2 = ShapeCache::new();
    let prepared =
        SceneFile::prepare(EXAMPLE, example_dir(), lib2.tessellator(), &cache2.keys()).unwrap();
    assert_eq!(prepared.new_shapes(), lib.len());
    assert_eq!(prepared.new_files(), cache.files());
    assert!(lib2.is_empty(), "prepare не трогает набор");
    let mut world = prepared.finish(&mut lib2, &mut cache2).unwrap();
    assert_eq!(lib2.len(), lib.len(), "finish ничего не режет сверх пачки");
    world.update();
    assert_eq!(world.snapshot().items, direct.items);
    // Второй раз знакомые формы не режутся вовсе.
    let again =
        SceneFile::prepare(EXAMPLE, example_dir(), lib2.tessellator(), &cache2.keys()).unwrap();
    assert_eq!(again.new_shapes(), 0);
    again.finish(&mut lib2, &mut cache2).unwrap();
    assert_eq!(lib2.len(), lib.len());
    // Ошибка разбора приходит из фона с той же строкой.
    let err = SceneFile::prepare("(format: 1", None, lib2.tessellator(), &HashSet::new())
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
        dir: example_dir().map(Into::into),
        known: HashSet::new(),
    });
    loader.submit(LoadJob::Details(vec![
        lib.detail_job(base, DETAIL_FINEST).unwrap(),
    ]));
    // Память удалённой фигуры освобождается в фоне.
    lib.defer_drops(true);
    let gone = lib
        .add(Shape::fill(
            kurbo::Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-3),
            Color::WHITE,
        ))
        .unwrap();
    lib.remove(gone);
    loader.submit(LoadJob::Discard(lib.take_trash()));
    assert!(lib.take_trash().is_empty());
    // Поле уровня (7.18в) считается тут же, со своим номером запроса.
    use reyvec_sdf::layer_bake::{Flat2, LayerBake, Placed2};
    loader.submit(LoadJob::LevelField {
        ticket: 7,
        bake: Box::new(LayerBake {
            cell: 0.05,
            band: 0.5,
            first: vec![Placed2 {
                flat: Flat2::Rect {
                    center: glam::Vec2::ZERO,
                    half: glam::Vec2::ONE,
                },
                at: glam::Vec2::ZERO,
                angle: 0.0,
                scale: 1.0,
                mirror: false,
                material: 0,
            }],
            second: Vec::new(),
            second_layer: None,
            threads: 2,
            cancel: Default::default(),
        }),
    });
    assert_eq!(loader.pending(), 6);
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
                    assert_eq!(lib.detail(base, DETAIL_FINEST), id);
                }
                kinds.push("details");
            }
            LoadResult::Discarded(n) => {
                assert_eq!(n, 1);
                kinds.push("discard");
            }
            LoadResult::LevelField { ticket, result } => {
                assert_eq!(ticket, 7);
                let (field, stats) = *result.expect("поле уровня");
                assert_eq!(stats.points, (field.size().0 * field.size().1) as usize);
                kinds.push("field");
            }
        }
    }
    // По порядку заданий.
    assert_eq!(
        kinds,
        ["svg", "motif", "scene", "details", "discard", "field"]
    );
    assert_eq!(loader.pending(), 0);
    assert!(loader.poll().is_none());
}

const LEVEL: &str = r##"nodes: [
    (name: "пол", solids: [(at: (0.0, -0.9), size: (6.0, 0.2))]),
    (name: "ящик", at: (0.6, -0.7), shapes: [Rect(at: (0.0, 0.0), size: (0.2, 0.2), color: "#a06030", solid: true)]),
    (name: "герой", at: (0.0, 0.0), depth: 2, player: (size: (0.1, 0.2)),
        shapes: [Rect(at: (0.0, 0.0), size: (0.1, 0.2), color: "#fff")]),
]"##;

/// Мир прогоняется шагами логики по 1/60 секунды.
fn steps(w: &mut SceneWorld, from: usize, n: usize) -> usize {
    for i in from..from + n {
        w.update_at(i as f64 / 60.0);
    }
    from + n
}

#[test]
fn player_falls_onto_floor_and_runs_into_a_solid_box() {
    let mut w = world(&minimal(LEVEL));
    let t = steps(&mut w, 0, 90);
    let p = w.snapshot().player.expect("персонаж есть в снимке");
    assert!(p.on_ground);
    // Низ персонажа (половина высоты 0,1) на верху пола (-0,8).
    assert!((p.at[1] - -0.7).abs() < 1e-4, "{:?}", p.at);
    assert_eq!(p.solids, 2);
    // Бежим вправо и упираемся в ящик: его левый край 0,5, половина ширины героя 0,05.
    w.set_input(reyvec_game::InputFrame::new(
        &[reyvec_game::Action::Right],
        &[],
    ));
    steps(&mut w, t, 120);
    let p = w.snapshot().player.unwrap();
    assert!((p.at[0] - 0.45).abs() < 1e-4, "{:?}", p.at);
    // Фигура героя едет вместе с ним.
    let s = w.snapshot();
    let hero = s
        .items
        .iter()
        .rev()
        .find_map(|i| match i {
            SceneItem::Shape(s) => Some(s.transform.as_coeffs()),
            _ => None,
        })
        .unwrap();
    assert!((hero[4] - 0.45).abs() < 1e-4, "{hero:?}");
}

#[test]
fn player_jumps_onto_the_box() {
    let mut w = world(&minimal(LEVEL));
    let mut t = steps(&mut w, 0, 60);
    use reyvec_game::{Action, InputFrame};
    // Прыжок с места, а над ящиком — вправо, пока не окажемся над его серединой.
    w.set_input(InputFrame::new(&[], &[Action::Jump]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[Action::Jump], &[]));
    t = steps(&mut w, t, 3);
    w.set_input(InputFrame::new(&[Action::Right, Action::Jump], &[]));
    while w.snapshot().player.unwrap().at[0] < 0.5 {
        t = steps(&mut w, t, 1);
        assert!(t < 400);
    }
    w.set_input(InputFrame::new(&[Action::Left], &[]));
    t = steps(&mut w, t, 4);
    w.set_input(InputFrame::new(&[], &[]));
    steps(&mut w, t, 60);
    let p = w.snapshot().player.unwrap();
    assert!(p.on_ground);
    // Стоит на ящике (верх -0,6) — на 0,2 выше пола.
    assert!((p.at[1] - -0.5).abs() < 1e-4, "{:?}", p.at);
}

#[test]
fn player_looks_left_by_mirroring() {
    let mut w = world(&minimal(LEVEL));
    let t = steps(&mut w, 0, 30);
    w.set_input(reyvec_game::InputFrame::new(
        &[reyvec_game::Action::Left],
        &[],
    ));
    steps(&mut w, t, 5);
    let s = w.snapshot();
    let hero = s
        .items
        .iter()
        .rev()
        .find_map(|i| match i {
            SceneItem::Shape(s) => Some(s.transform.as_coeffs()),
            _ => None,
        })
        .unwrap();
    assert!(hero[0] < 0.0, "{hero:?}");
}

#[test]
fn player_that_falls_out_comes_back_to_spawn() {
    let mut w = world(&minimal(
        r##"nodes: [(name: "герой", at: (0.3, 0.0), player: ())]"##,
    ));
    steps(&mut w, 0, 200);
    let p = w.snapshot().player.unwrap();
    assert!(p.at[1] > -5.0, "{:?}", p.at);
}

#[test]
fn player_keeps_place_when_scene_file_is_saved() {
    let mut w = world(&minimal(LEVEL));
    let t = steps(&mut w, 0, 60);
    w.set_input(reyvec_game::InputFrame::new(
        &[reyvec_game::Action::Right],
        &[],
    ));
    steps(&mut w, t, 10);
    let body = w.player_body().unwrap();
    let mut new = world(&minimal(LEVEL));
    new.set_player_body(body);
    new.update_at(0.0);
    let p = new.snapshot().player.unwrap();
    assert!((f64::from(p.at[0]) - body.pos[0]).abs() < 1e-6);
    assert!(p.at[0] > 0.01);
}

#[test]
fn player_errors_are_in_russian() {
    let nested = minimal(r##"nodes: [(children: [(player: ())])]"##);
    let e = SceneFile::parse(&nested).unwrap_err().to_string();
    assert!(e.contains("верхнего уровня"), "{e}");
    let two = minimal(r##"nodes: [(player: ()), (player: ())]"##);
    let e = SceneFile::parse(&two).unwrap_err().to_string();
    assert!(e.contains("персонаж игрока в сцене один"), "{e}");
    let bad = minimal(r##"nodes: [(player: (speed: -1))]"##);
    let e = SceneFile::parse(&bad).unwrap_err().to_string();
    assert!(e.contains("speed"), "{e}");
}

#[test]
fn logic_thread_takes_input() {
    let mut text = minimal(LEVEL);
    text = text.replace("at: (0.0, 0.0), depth: 2", "at: (-1.0, -0.7), depth: 2");
    let logic = SceneLogic::start(world(&text), 0.0);
    let deadline = Instant::now() + Duration::from_secs(5);
    logic.set_action(reyvec_game::Action::Right, true);
    while logic.frame(Instant::now()).player.unwrap().at[0] < -0.8 {
        assert!(Instant::now() < deadline, "персонаж не бежит");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(logic.input_ms().is_some());
    // Снимок помнит, какое нажатие уже взято в шаг: по нему кадр меряет задержку (6.3).
    let stamp = logic.frame(Instant::now()).input_stamp;
    assert_ne!(stamp, 0);
    assert!(logic.press_age_ms(stamp) > 0.0);
    logic.set_action(reyvec_game::Action::Right, false);
}

/// Простой «игрок» для проверки уровня: бежит к `x`, прыгает, когда упёрся. Возвращает
/// место героя, когда он остановился у цели (или после `limit` шагов).
fn walk_to(w: &mut SceneWorld, t: &mut usize, x: f32, limit: usize) -> PlayerView {
    use reyvec_game::{Action, InputFrame};
    let mut last = w.snapshot().player.unwrap();
    for _ in 0..limit {
        let dir = if x > last.at[0] {
            Action::Right
        } else {
            Action::Left
        };
        if (last.at[0] - x).abs() < 0.02 {
            w.set_input(InputFrame::new(&[], &[]));
            *t = steps(w, *t, 40);
            return w.snapshot().player.unwrap();
        }
        let stuck = last.on_ground && last.vel[0].abs() < 0.01;
        let input = if stuck {
            InputFrame::new(&[dir], &[Action::Jump])
        } else {
            InputFrame::new(&[dir, Action::Jump], &[])
        };
        w.set_input(input);
        *t = steps(w, *t, 1);
        last = w.snapshot().player.unwrap();
    }
    last
}

#[test]
fn hero_of_the_example_climbs_crates_onto_the_shelf() {
    let file = SceneFile::parse_in(EXAMPLE, example_dir()).unwrap();
    let mut lib = library();
    let mut w = SceneWorld::new(&file, &mut lib, &mut ShapeCache::new()).unwrap();
    let mut t = steps(&mut w, 0, 60);
    let p = w.snapshot().player.unwrap();
    assert!(p.on_ground && (p.at[1] - -0.64).abs() < 1e-3, "{p:?}");
    // На высокий ящик (верх -0,28).
    let p = walk_to(&mut w, &mut t, 1.85, 600);
    assert!((p.at[1] - -0.14).abs() < 1e-3, "на ящике: {p:?}");
    // С ящика на полку (верх 0,22).
    let p = walk_to(&mut w, &mut t, 1.3, 600);
    assert!((p.at[1] - 0.36).abs() < 1e-3, "на полке: {p:?}");
    // В конце коридора — стена: дальше x ≈ 11,2 не пройти.
    let p = walk_to(&mut w, &mut t, 12.0, 3000);
    assert!(p.at[0] < 11.21 && p.at[0] > 10.5, "у стены: {p:?}");
}

#[test]
fn platforms_hold_from_above_and_lifts_carry() {
    use reyvec_game::{Action, InputFrame};
    let text = minimal(
        r##"nodes: [
        (solids: [(at: (0.0, -0.9), size: (6.0, 0.2))]),
        (name: "доска", at: (0.0, -0.45), shapes: [Rect(at: (0.0, 0.0), size: (1.0, 0.04), color: "#fff", oneway: true)]),
        (name: "лифт", at: (1.5, -0.3), bob: (by: (0.0, 0.25), period: 2.0),
            shapes: [Rect(at: (0.0, 0.0), size: (0.5, 0.04), color: "#fff", solid: true)]),
        (name: "герой", at: (0.0, -0.7), player: (size: (0.1, 0.2))),
    ]"##,
    );
    let mut w = world(&text);
    let mut t = steps(&mut w, 0, 30);
    // Прыжок с пола сквозь доску снизу — и на ней стоим (верх доски -0,43).
    w.set_input(InputFrame::new(&[], &[Action::Jump]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[Action::Jump], &[]));
    t = steps(&mut w, t, 60);
    let p = w.snapshot().player.unwrap();
    assert!(p.on_ground && (p.at[1] - -0.33).abs() < 1e-3, "{p:?}");
    // Вниз + прыжок — обратно на пол.
    w.set_input(InputFrame::new(&[Action::Down], &[Action::Jump]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 60);
    let p = w.snapshot().player.unwrap();
    assert!(p.on_ground && (p.at[1] - -0.7).abs() < 1e-3, "{p:?}");
    // Ставим героя над лифтом: встаёт на него и ездит с ним вверх-вниз.
    let mut body = w.player_body().unwrap();
    body.pos = [1.5, 0.2];
    w.set_player_body(body);
    t = steps(&mut w, t, 60);
    assert!(w.snapshot().player.unwrap().on_ground);
    let mut ys = Vec::new();
    for _ in 0..120 {
        t = steps(&mut w, t, 1);
        let p = w.snapshot().player.unwrap();
        assert!(p.on_ground, "упал с лифта: {p:?}");
        ys.push(p.at[1]);
    }
    let (lo, hi) = ys
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &y| (a.min(y), b.max(y)));
    assert!(hi - lo > 0.4, "лифт возит героя: {lo}..{hi}");
}

#[test]
fn hero_of_the_example_wall_jumps_up_to_the_reward() {
    use reyvec_game::{Action, InputFrame};
    let file = SceneFile::parse_in(EXAMPLE, example_dir()).unwrap();
    let mut lib = library();
    let mut w = SceneWorld::new(&file, &mut lib, &mut ShapeCache::new()).unwrap();
    let mut t = steps(&mut w, 0, 30);
    // Ставим героя у колодца и забегаем в щель под столбом.
    let mut body = w.player_body().unwrap();
    body.pos = [10.4, -0.64];
    w.set_player_body(body);
    let p = walk_to(&mut w, &mut t, 11.15, 600);
    assert!(p.on_ground && p.at[0] > 11.0, "в щели: {p:?}");
    // Прыжки от стены: к правой стене, оттолкнулись, к левой, оттолкнулись...
    let mut dir = Action::Right;
    w.set_input(InputFrame::new(&[dir], &[Action::Jump]));
    t = steps(&mut w, t, 1);
    let mut top = p.at[1];
    for _ in 0..600 {
        let p = w.snapshot().player.unwrap();
        top = top.max(p.at[1]);
        if p.at[1] > 0.95 {
            break;
        }
        let at_wall = !p.on_ground && p.vel[0] == 0.0;
        if at_wall {
            dir = if dir == Action::Right {
                Action::Left
            } else {
                Action::Right
            };
            w.set_input(InputFrame::new(&[dir], &[Action::Jump]));
        } else {
            w.set_input(InputFrame::new(&[dir, Action::Jump], &[]));
        }
        t = steps(&mut w, t, 1);
    }
    assert!(top > 0.95, "выше столба не забрался: {top}");
    // Наверху — влево на столб, к награде (верх столба 0,75).
    let p = walk_to(&mut w, &mut t, 10.9, 300);
    assert!(
        p.on_ground && (p.at[1] - 0.89).abs() < 1e-3,
        "на столбе: {p:?}"
    );
}

/// Держит ввод `held` (и `pressed` на первом шаге), пока `done` не скажет «хватит» или не
/// пройдёт `limit` шагов; `each` видит героя после каждого шага.
fn hold_until(
    w: &mut SceneWorld,
    t: &mut usize,
    held: &[reyvec_game::Action],
    pressed: &[reyvec_game::Action],
    limit: usize,
    mut each: impl FnMut(&PlayerView) -> bool,
) -> PlayerView {
    use reyvec_game::InputFrame;
    w.set_input(InputFrame::new(held, pressed));
    for _ in 0..limit {
        *t = steps(w, *t, 1);
        w.set_input(InputFrame::new(held, &[]));
        let p = w.snapshot().player.unwrap();
        if each(&p) {
            return p;
        }
    }
    w.snapshot().player.unwrap()
}

#[test]
fn hero_of_the_example_crosses_the_ledges() {
    use reyvec_game::Action::{Jump, Left, Right, Up};
    let file = SceneFile::parse_in(EXAMPLE, example_dir()).unwrap();
    let mut lib = library();
    let mut w = SceneWorld::new(&file, &mut lib, &mut ShapeCache::new()).unwrap();
    let mut t = steps(&mut w, 0, 30);
    // Высота середины героя, стоящего на высоте `y` (половина роста 0,14).
    let on = |y: f32| y + 0.14;
    let floor = -0.78;
    // На верху стены колодца (0,9) и вниз к уступам.
    let mut body = w.player_body().unwrap();
    body.pos = [11.35, 1.1];
    w.set_player_body(body);
    t = steps(&mut w, t, 40);
    let p = w.snapshot().player.unwrap();
    assert!(
        p.on_ground && (p.at[1] - on(0.9)).abs() < 1e-3,
        "на стене: {p:?}"
    );
    // Со стены на доску (0,4), с неё на склон холма, через площадку (0,3) и вниз:
    // с площадки до пола не отрываясь от земли.
    let mut top = f32::MIN;
    let p = hold_until(&mut w, &mut t, &[Right], &[], 600, |p| {
        if p.at[0] > 12.8 {
            assert!(p.on_ground, "оторвался на холме: {p:?}");
            top = top.max(p.at[1]);
        }
        p.at[0] > 13.6
    });
    assert!(p.at[0] > 13.6);
    assert!((p.at[1] - on(floor)).abs() < 1e-3, "за холмом: {p:?}");
    assert!(
        (top - on(floor + 0.3)).abs() < 1e-3,
        "площадка холма: {top}"
    );

    // Высокий блок (верх на 0,7 над полом) выше прыжка: повиснуть на краю и забраться.
    let p = hold_until(&mut w, &mut t, &[Right], &[], 200, |p| p.vel[0] == 0.0);
    assert!(p.on_ground && p.at[0] < 13.93, "у блока: {p:?}");
    let p = hold_until(&mut w, &mut t, &[Right, Jump], &[Jump], 120, |p| p.hanging);
    assert!(p.hanging, "не повис: {p:?}");
    let ledge = floor + 0.7;
    assert!(
        (p.at[1] + 0.14 - (ledge + 0.02)).abs() < 1e-3,
        "руки на краю: {p:?}"
    );
    // Висит, пока не нажмёшь вверх.
    let p = hold_until(&mut w, &mut t, &[Right], &[], 60, |_| false);
    assert!(p.hanging);
    let p = hold_until(&mut w, &mut t, &[Up], &[Up], 30, |p| p.on_ground);
    assert!(
        p.on_ground && !p.hanging && (p.at[1] - on(ledge)).abs() < 1e-3,
        "наверху блока: {p:?}"
    );

    // Длинный спуск по склону: прижимается, а не скачет ступеньками.
    let p = hold_until(&mut w, &mut t, &[Right], &[], 200, |p| p.at[0] > 14.4);
    assert!(p.on_ground);
    let p = hold_until(&mut w, &mut t, &[Right], &[], 400, |p| {
        assert!(p.on_ground, "оторвался на спуске: {p:?}");
        p.at[0] > 15.2
    });
    assert!((p.at[1] - on(floor)).abs() < 1e-3, "внизу спуска: {p:?}");

    // Обратно: с верхней доски у колодца (0,9 над полом) — за край стены и на неё.
    let mut body = w.player_body().unwrap();
    body.pos = [11.55, f64::from(on(floor + 0.9)) + 0.05];
    body.vel = [0.0, 0.0];
    w.set_player_body(body);
    w.set_input(reyvec_game::InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 30);
    let p = w.snapshot().player.unwrap();
    assert!(p.on_ground, "на доске: {p:?}");
    let p = hold_until(&mut w, &mut t, &[Left, Jump], &[Jump], 120, |p| p.hanging);
    assert!(p.hanging, "не повис на стене: {p:?}");
    let p = hold_until(&mut w, &mut t, &[Up], &[Up], 30, |p| p.on_ground);
    assert!(
        p.on_ground && (p.at[1] - on(0.9)).abs() < 1e-3,
        "на стене: {p:?}"
    );
}

#[test]
fn lift_never_crushes_the_hero_standing_under_it() {
    // Подъёмник в коридоре в нижней точке оставляет над полом место выше героя: стоящий
    // под ним герой стоит, и подъёмник его не касается (наезд на героя всё равно
    // выталкивает его, это проверяет контроллер).
    let file = SceneFile::parse_in(EXAMPLE, example_dir()).unwrap();
    let mut lib = library();
    let mut w = SceneWorld::new(&file, &mut lib, &mut ShapeCache::new()).unwrap();
    let mut t = steps(&mut w, 0, 10);
    w.set_player_body(reyvec_game::Body::new([9.6, -0.64]));
    w.set_input(reyvec_game::InputFrame::new(&[], &[]));
    for _ in 0..300 {
        t = steps(&mut w, t, 1);
        let p = w.snapshot().player.unwrap();
        assert!(
            p.on_ground && (p.at[0] - 9.6).abs() < 1e-3 && (p.at[1] - -0.64).abs() < 1e-3,
            "подъёмник задел героя: {p:?}"
        );
    }
}

#[test]
fn hero_mashing_random_keys_never_ends_up_inside_the_level() {
    // «Обезьяна» жмёт случайные клавиши по минуте игры в разных местах примера. После
    // каждого шага: герой не внутри твёрдого, не ниже склона, не телепортировался; с 6.5
    // ещё и бьёт: манекены не внутри твёрдого, герой не застревает в манекене.
    use reyvec_game::{Aabb, Action, InputFrame};
    let file = SceneFile::parse_in(EXAMPLE, example_dir()).unwrap();
    let mut lib = library();
    let mut w = SceneWorld::new(&file, &mut lib, &mut ShapeCache::new()).unwrap();
    let mut t = steps(&mut w, 0, 10);
    let mut seed: u64 = 0x5eed_1234_abcd_0001;
    let mut rand = move |n: u64| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % n
    };
    let starts = [
        [-1.17, -0.62],
        [0.4, -0.5],
        [4.0, -0.5],
        [9.6, 0.3],
        [11.15, -0.6],
        [12.8, 0.2],
        [14.6, 0.5],
    ];
    let mut grabs = 0;
    let mut slopes = 0;
    let mut stuck = 0;
    let mut dummy_hits = 0;
    for start in starts {
        w.set_player_body(reyvec_game::Body::new(start));
        let mut last = w.snapshot().player.unwrap();
        last.at = [start[0] as f32, start[1] as f32];
        last.hanging = false;
        let mut held: Vec<Action> = Vec::new();
        let mut history: Vec<String> = Vec::new();
        let mut left = 0;
        for i in 0..3600 {
            let mut pressed = Vec::new();
            if left == 0 {
                left = 4 + rand(30);
                held.clear();
                match rand(3) {
                    0 => held.push(Action::Left),
                    1 => held.push(Action::Right),
                    _ => {}
                }
                for a in [
                    Action::Jump,
                    Action::Dash,
                    Action::Down,
                    Action::Up,
                    Action::Attack,
                ] {
                    if rand(4) == 0 {
                        held.push(a);
                        pressed.push(a);
                    }
                }
            }
            left -= 1;
            w.set_input(InputFrame::new(&held, &pressed));
            t = steps(&mut w, t, 1);
            let p = w.snapshot().player.unwrap();
            let (solids, tuning) = w.colliders();
            let tuning = tuning.unwrap();
            // Точное место (в снимке оно в f32).
            let pos = w.player_body().unwrap().pos;
            let b = Aabb::around(pos, [tuning.size[0] * 0.5, tuning.size[1] * 0.5]);
            history.push(format!("{i}: {pos:?} {p:?} {held:?}/{pressed:?}"));
            if history.len() > 8 {
                history.remove(0);
            }
            let place = format!("старт {start:?}, шаг {i}: {p:?}\n{}", history.join("\n"));
            assert!(p.at[0].is_finite() && p.at[1].is_finite(), "{place}");
            let jump = (p.at[0] - last.at[0]).hypot(p.at[1] - last.at[1]);
            // Здоровье кончилось (6.10б) — герой появляется в начале с полным.
            let respawned = p.hp > last.hp;
            assert!(jump < 0.2 || respawned, "телепорт на {jump}: {place}");
            grabs += usize::from(p.hanging && !last.hanging);
            for c in &solids {
                if c.oneway {
                    continue;
                }
                let s = &c.aabb;
                if c.slope != 0.0 {
                    let x = pos[0];
                    if x > s.min[0] + 1e-6 && x < s.max[0] - 1e-6 && b.min[1] > s.min[1] {
                        let k = (x - s.min[0]) / (s.max[0] - s.min[0]);
                        let k = if c.slope > 0.0 { k } else { 1.0 - k };
                        let surface = s.max[1] - c.slope.abs() * (1.0 - k);
                        assert!(b.min[1] > surface - 1e-6, "ниже склона {s:?}: {place}");
                        slopes += usize::from(p.on_ground && (b.min[1] - surface).abs() < 1e-6);
                    }
                } else {
                    assert!(!b.overlaps(s), "внутри твёрдого {s:?}: {place}");
                }
            }
            // Манекены: не внутри твёрдого уровня. Герой может оказаться в манекене на
            // шаг (манекен шагает после героя и может въехать в него), но на следующем
            // шаге манекен его выталкивает.
            let mut inside = false;
            let alive: Vec<_> = w.dummies().into_iter().filter(|d| d.alive()).collect();
            for (k, d) in alive.iter().enumerate() {
                for other in &alive[k + 1..] {
                    assert!(
                        !d.bounds().overlaps(&other.bounds()),
                        "манекены друг в друге: {d:?} {other:?}\n{place}"
                    );
                }
            }
            for d in &alive {
                let db = d.bounds();
                for c in solids.iter().filter(|c| !c.oneway && c.slope == 0.0) {
                    assert!(
                        !db.overlaps(&c.aabb),
                        "манекен внутри {:?}: {d:?}\n{place}",
                        c.aabb
                    );
                }
                inside |= db.overlaps(&b);
            }
            stuck = if inside { stuck + 1 } else { 0 };
            assert!(stuck < 2, "герой застрял в манекене: {place}");
            dummy_hits = dummy_hits.max(p.hits);
            last = p;
        }
    }
    eprintln!(
        "схватился за край {grabs} раз, шагов на склонах {slopes}, попаданий по манекенам {dummy_hits}"
    );
    assert!(slopes > 0, "обезьяна ни разу не постояла на склоне");
}

/// Мир примера после того, как всё упало на пол.
fn example_world() -> (SceneWorld, usize) {
    let file = SceneFile::parse_in(EXAMPLE, example_dir()).unwrap();
    let mut lib = library();
    let mut w = SceneWorld::new(&file, &mut lib, &mut ShapeCache::new()).unwrap();
    let t = steps(&mut w, 0, 30);
    (w, t)
}

/// Самая яркая составляющая оттенка фигур снимка: больше 1 — только во вспышке.
fn brightest_tint(scene: &Scene) -> f32 {
    scene
        .items
        .iter()
        .filter_map(|i| match i {
            SceneItem::Shape(s) => Some(s.tint.r.max(s.tint.g).max(s.tint.b)),
            _ => None,
        })
        .fold(0.0, f32::max)
}

#[test]
fn hero_hits_the_dummy_world_freezes_then_the_dummy_flies() {
    use reyvec_game::Action::{Attack, Left};
    use reyvec_game::InputFrame;
    let (mut w, mut t) = example_world();
    let d0 = w.dummies()[0].clone();
    assert_eq!(d0.spawn, [-0.9, -0.63]);
    assert!(d0.body.on_ground, "манекен стоит на полу: {:?}", d0.body);
    // Герой справа от манекена, лицом к нему, между ними 0,05.
    w.set_player_body(reyvec_game::Body::new([-0.9 + 0.08 + 0.07 + 0.05, -0.64]));
    w.set_input(InputFrame::new(&[Left], &[Left]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 20);
    let idle = w.snapshot();
    assert!(idle.player.unwrap().on_ground);
    assert!(brightest_tint(&idle) <= 1.0, "без попадания вспышки нет");
    let hero0 = w.player_body().unwrap().pos;
    w.set_input(InputFrame::new(&[Attack], &[Attack]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[], &[]));
    // Удар доходит до попадания за замах и шаг-другой.
    let mut hit_at = None;
    for i in 0..10 {
        let s = w.snapshot();
        if s.player.unwrap().hits > 0 {
            hit_at = Some(i);
            break;
        }
        t = steps(&mut w, t, 1);
    }
    let hit_at = hit_at.expect("удар попал");
    assert!(hit_at <= 4, "попадание через {hit_at} шагов");
    let s = w.snapshot();
    let p = s.player.unwrap();
    assert!(p.striking, "в момент попадания идёт фаза удара");
    assert_eq!(s.items.len(), idle.items.len() + 1, "виден взмах");
    assert!(s.shake > 0.0, "камера трясётся");
    assert!(brightest_tint(&s) > 1.0, "манекен вспыхнул");
    let d = w.dummies()[0].clone();
    assert_eq!(d.hp, 4);
    // Стоп-кадр: герой и манекен стоят 3–4 шага.
    let (hero, dummy) = (w.player_body().unwrap().pos, d.body.pos);
    let mut frozen = 0;
    for _ in 0..10 {
        t = steps(&mut w, t, 1);
        if w.player_body().unwrap().pos == hero && w.dummies()[0].body.pos == dummy {
            frozen += 1;
        } else {
            break;
        }
    }
    assert!((3..=5).contains(&frozen), "стоп-кадр {frozen} шагов");
    assert!((hero[0] - hero0[0]).abs() < 0.02, "герой бил стоя");
    // Манекен отлетел влево, подпрыгнул, приземлился и не провалился.
    let mut top = dummy[1];
    let mut min_x = dummy[0];
    for _ in 0..90 {
        t = steps(&mut w, t, 1);
        let d = &w.dummies()[0];
        top = top.max(d.body.pos[1]);
        min_x = min_x.min(d.body.pos[0]);
        assert!(
            d.bounds().min[1] >= -0.78 - 1e-6,
            "манекен в полу: {:?}",
            d.bounds()
        );
    }
    let d = w.dummies()[0].clone();
    assert!(top > dummy[1] + 0.04, "подлетел до {top}");
    assert!(min_x < dummy[0] - 0.1, "отлетел влево до {min_x}");
    assert!(
        d.body.on_ground && d.body.vel[0].abs() < 1e-9,
        "лёг на пол и встал"
    );
    let s = w.snapshot();
    assert_eq!(s.shake, 0.0, "тряска кончилась");
    assert!(brightest_tint(&s) <= 1.0, "вспышка кончилась");
    assert_eq!(s.items.len(), idle.items.len(), "взмах спрятан");
}

#[test]
fn dummy_breaks_after_five_hits_and_comes_back_on_its_spot() {
    use reyvec_game::Action::{Attack, Left};
    use reyvec_game::InputFrame;
    let (mut w, mut t) = example_world();
    let idle = w.snapshot().items.len();
    w.set_player_body(reyvec_game::Body::new([-0.7, -0.64]));
    // Повернуться к манекену.
    w.set_input(InputFrame::new(&[Left], &[Left]));
    t = steps(&mut w, t, 1);
    // Герой идёт за манекеном и бьёт каждые 0,4 с.
    let mut broke = None;
    for i in 0..900 {
        let d = &w.dummies()[0];
        let hero = w.player_body().unwrap();
        let near = hero.pos[0] - d.body.pos[0] < 0.25;
        let held: &[reyvec_game::Action] = if near { &[] } else { &[Left] };
        let pressed: &[reyvec_game::Action] = if near && i % 24 == 0 { &[Attack] } else { &[] };
        w.set_input(InputFrame::new(held, pressed));
        t = steps(&mut w, t, 1);
        if !w.dummies()[0].alive() {
            broke = Some(t);
            break;
        }
    }
    broke.unwrap_or_else(|| {
        panic!(
            "манекен не разбит: {:?}, герой {:?}",
            w.dummies(),
            w.snapshot().player
        )
    });
    let s = w.snapshot();
    assert!(s.player.unwrap().hits >= 5);
    // Разбитый не рисуется (его фигуры прозрачны), зато летят обломки (6.10).
    let _ = idle;
    let mut q = w
        .world_mut()
        .query::<(&crate::world::Target, &crate::world::Placement)>();
    let hidden = q
        .iter(w.world_mut())
        .filter(|(t, p)| !t.dummy.alive() && p.opacity == 0.0)
        .count();
    assert_eq!(hidden, 1, "разбитый не рисуется");
    // Отходим с места появления и ждём 2 с.
    w.set_player_body(reyvec_game::Body::new([0.4, -0.64]));
    w.set_input(InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 125);
    let d = w.dummies()[0].clone();
    assert!(d.alive(), "манекен появился снова");
    assert_eq!(d.hp, d.max_hp);
    steps(&mut w, t, 20);
    let d = w.dummies()[0].clone();
    assert!((d.body.pos[0] - d.spawn[0]).abs() < 1e-9 && d.body.on_ground);
}

#[test]
fn game_events_for_particles_come_once_with_their_step() {
    use reyvec_game::Action::{Attack, Left, Right};
    use reyvec_game::InputFrame;
    let (mut w, mut t) = example_world();
    // Падает с высоты на пол: приземление с местом под ногами и скоростью.
    w.set_player_body(reyvec_game::Body::new([0.4, -0.2]));
    let mut seen = 0u64;
    let mut all = Vec::new();
    let mut take = |w: &SceneWorld, all: &mut Vec<GameEvent>| {
        for (step, e) in w.snapshot().events {
            if step > seen {
                all.push(e);
            }
        }
        if let Some(&(s, _)) = w.snapshot().events.last() {
            seen = seen.max(s);
        }
    };
    for _ in 0..40 {
        t = steps(&mut w, t, 1);
        take(&w, &mut all);
    }
    let landed: Vec<_> = all
        .iter()
        .filter_map(|e| match e {
            GameEvent::Landed { at, speed } => Some((*at, *speed)),
            _ => None,
        })
        .collect();
    assert_eq!(landed.len(), 1, "одно приземление: {all:?}");
    let (at, speed) = landed[0];
    assert!(
        (at[1] - -0.78).abs() < 0.01 && (at[0] - 0.4).abs() < 0.01,
        "{at:?}"
    );
    assert!(speed > 1.0, "падал быстро: {speed}");
    // Бег: шаги пыли каждые ~0,12 с.
    all.clear();
    w.set_input(InputFrame::new(&[Right], &[Right]));
    for _ in 0..60 {
        t = steps(&mut w, t, 1);
        take(&w, &mut all);
    }
    let runs = all
        .iter()
        .filter(|e| matches!(e, GameEvent::Run { dir, .. } if *dir > 0.0))
        .count();
    assert!(
        (5..=9).contains(&runs),
        "шагов бега за секунду {runs}: {all:?}"
    );
    // Удар по манекену: одно попадание с местом у края манекена.
    w.set_player_body(reyvec_game::Body::new([-0.59, -0.64]));
    w.set_input(InputFrame::new(&[Left], &[Left]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 30);
    take(&w, &mut all);
    all.clear();
    w.set_input(InputFrame::new(&[Attack], &[Attack]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[], &[]));
    take(&w, &mut all);
    for _ in 0..20 {
        t = steps(&mut w, t, 1);
        take(&w, &mut all);
    }
    let hits: Vec<_> = all
        .iter()
        .filter_map(|e| match e {
            GameEvent::Hit { at, dir } => Some((*at, *dir)),
            _ => None,
        })
        .collect();
    assert_eq!(hits.len(), 1, "{all:?}");
    let d = w.dummies()[0].clone();
    assert!(
        hits[0].1 < 0.0 && (hits[0].0[0] - (d.spawn[0] + 0.08) as f32).abs() < 0.03,
        "{hits:?}"
    );
    // Твёрдое уровня в снимке для частиц.
    assert!(!w.snapshot().solids.is_empty());
}

#[test]
fn hero_bones_move_in_poses_and_feet_make_steps() {
    use reyvec_game::Action::*;
    use reyvec_game::InputFrame;
    let (mut w, mut t) = example_world();
    let bones = |w: &mut SceneWorld| {
        let mut q = w
            .world_mut()
            .query::<(&crate::world::BoneOf, &crate::world::Placement)>();
        let mut all: Vec<_> = q
            .iter(w.world_mut())
            .map(|(b, p)| (b.bone, p.angle - b.base.angle))
            .collect();
        all.sort_by_key(|(b, _)| *b as usize);
        all
    };
    let idle = bones(&mut w);
    assert_eq!(idle.len(), 7, "все кости героя примера: {idle:?}");
    assert_eq!(w.snapshot().player.unwrap().anim, "стоит");
    // Бежит: ноги расходятся в разные стороны, тело наклонено вперёд.
    w.set_input(InputFrame::new(&[Right], &[Right]));
    t = steps(&mut w, t, 40);
    let run = bones(&mut w);
    let angle = |all: &[(reyvec_game::Bone, f64)], b| {
        all.iter().find(|(x, _)| *x == b).map(|(_, a)| *a).unwrap()
    };
    use reyvec_game::Bone as B;
    assert_eq!(w.snapshot().player.unwrap().anim, "бежит");
    assert!(angle(&run, B::Body) < -5.0, "{run:?}");
    let legs = (angle(&run, B::LegFront), angle(&run, B::LegBack));
    assert!(
        legs.0 * legs.1 <= 0.0 && (legs.0 - legs.1).abs() > 5.0,
        "{legs:?}"
    );
    // Прыжок: руки вверх.
    w.set_input(InputFrame::new(&[Jump], &[Jump]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[], &[Jump]));
    t = steps(&mut w, t, 12);
    let jump = bones(&mut w);
    assert_eq!(w.snapshot().player.unwrap().anim, "прыгает");
    assert!(angle(&jump, B::ArmFront) > 90.0, "{jump:?}");
    // Удар: рука за голову, потом вперёд.
    w.set_input(InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 90);
    w.set_input(InputFrame::new(&[Attack], &[Attack]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[], &[]));
    let mut most: f64 = 0.0;
    let mut least: f64 = 360.0;
    for _ in 0..12 {
        t = steps(&mut w, t, 1);
        let a = angle(&bones(&mut w), B::ArmFront);
        most = most.max(a);
        least = least.min(a);
    }
    assert!(most > 150.0 && least < 80.0, "взмах {most}…{least}");
}

#[test]
fn bones_errors_are_in_russian() {
    let e = error(&minimal(
        r#"nodes: [(name: "г", player: (), children: [(bone: "хвост")])]"#,
    ));
    assert!(e.to_string().contains("кости «хвост» нет"), "{e}");
    let e = error(&minimal(
        r#"nodes: [(name: "стол", children: [(bone: "рука")])]"#,
    ));
    assert!(
        e.to_string().contains("только внутри предмета с player"),
        "{e}"
    );
}

/// Сцена со скриптами во временной папке: жук-враг и монета.
fn script_level(dir: &std::path::Path, beetle: &str) -> SceneWorld {
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("scripts/beetle.luau"), beetle).unwrap();
    std::fs::copy(
        std::path::Path::new(EXAMPLE_DIR).join("scripts/coin.luau"),
        dir.join("scripts/coin.luau"),
    )
    .unwrap();
    let text = minimal(
        r##"nodes: [
        (name: "пол", solids: [(at: (0.0, -0.9), size: (8.0, 0.2))]),
        (name: "герой", at: (-1.0, -0.65), player: (size: (0.14, 0.28)), shapes: [Rect(at: (0.0, 0.0), size: (0.14, 0.28), color: "#fff")]),
        (name: "жук", at: (1.0, -0.7), dummy: (size: (0.2, 0.12), hp: 3), script: "scripts/beetle.luau",
            shapes: [Rect(at: (0.0, 0.0), size: (0.2, 0.12), color: "#3d6b3a")]),
        (name: "монета", at: (-0.6, -0.65), script: "scripts/coin.luau", touch: (0.08, 0.08),
            shapes: [Circle(at: (0.0, 0.0), r: 0.035, color: "#e0a830")]),
    ]"##,
    );
    let file = SceneFile::parse_in(&text, Some(dir)).unwrap();
    let mut lib = library();
    SceneWorld::new(&file, &mut lib, &mut ShapeCache::new()).unwrap()
}

#[test]
fn scripts_drive_an_enemy_and_a_coin_and_pick_up_edits() {
    use reyvec_game::Action::Right;
    use reyvec_game::InputFrame;
    let dir = std::env::temp_dir().join(format!("reyvec-scripts-{}", std::process::id()));
    let beetle =
        std::fs::read_to_string(std::path::Path::new(EXAMPLE_DIR).join("scripts/beetle.luau"))
            .unwrap();
    let mut w = script_level(&dir, &beetle);
    let mut t = steps(&mut w, 0, 30);
    let info = w.snapshot().scripts;
    assert_eq!((info.files, info.actors), (2, 2), "{info:?}");
    assert!(info.errors.is_empty(), "{info:?}");
    // Жук гуляет у своего места (герой далеко).
    let start = w.dummies()[0].spawn[0];
    let mut xs = Vec::new();
    for _ in 0..240 {
        t = steps(&mut w, t, 1);
        xs.push(w.dummies()[0].body.pos[0]);
    }
    let (lo, hi) = xs
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(
        hi - lo > 0.6 && lo > start - 0.6 && hi < start + 0.6,
        "{lo}…{hi}"
    );
    // Герой бежит вправо: монета подобрана (событие один раз), жук бежит к герою.
    w.set_input(InputFrame::new(&[Right], &[Right]));
    let mut pickups = 0;
    let mut seen = 0;
    let mut chased = false;
    for _ in 0..70 {
        t = steps(&mut w, t, 1);
        for (s, e) in w.snapshot().events {
            if s > seen && matches!(e, GameEvent::Pickup { .. }) {
                pickups += 1;
            }
            seen = seen.max(s);
        }
        let hero = w.snapshot().player.unwrap().at[0] as f64;
        let d = &w.dummies()[0];
        chased |= (hero - d.body.pos[0]).abs() < 0.9 && d.body.vel[0] < -0.5;
    }
    assert_eq!(pickups, 1, "монета подобрана один раз");
    assert!(chased, "жук не бежит к герою");
    // Правка скрипта на лету: жук стоит на месте. Ошибка в правке видна, старое
    // поведение остаётся.
    let path = dir.join("scripts/beetle.luau");
    let touch = |text: &str, secs: u64| {
        std::fs::write(&path, text).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(secs))
            .unwrap();
    };
    touch("function update(me)\n  me.move = = 0\nend", 5);
    t = steps(&mut w, t, 20);
    let info = w.snapshot().scripts;
    assert!(info.errors[0].starts_with("beetle.luau:2"), "{info:?}");
    touch("function update(me) me.move = 0 end", 10);
    w.set_input(InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 60);
    let a = w.dummies()[0].body.pos[0];
    steps(&mut w, t, 30);
    let b = w.dummies()[0].body.pos[0];
    let info = w.snapshot().scripts;
    assert!(info.errors.is_empty() && info.reloads == 2, "{info:?}");
    assert!((a - b).abs() < 1e-6, "после правки стоит: {a} → {b}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn script_errors_are_in_russian() {
    let e = error(&minimal(
        r#"nodes: [(name: "г", player: (), script: "x.luau")]"#,
    ));
    assert!(e.to_string().contains("героем управляет игрок"), "{e}");
    let e = error(&minimal(
        r#"nodes: [(name: "стол", children: [(script: "x.luau")])]"#,
    ));
    assert!(
        e.to_string()
            .contains("script можно только у предмета верхнего уровня"),
        "{e}"
    );
    // Нет файла: мир строится, ошибка видна на панели.
    let text = minimal(r#"nodes: [(name: "м", script: "нет-такого.luau")]"#);
    let file = SceneFile::parse(&text).unwrap();
    let mut w = SceneWorld::new(&file, &mut library(), &mut ShapeCache::new()).unwrap();
    w.update_at(0.0);
    let info = w.snapshot().scripts;
    assert!(info.errors[0].contains("не прочитать файл"), "{info:?}");
}

/// Место физического тела по имени предмета.
fn body_at(w: &mut SceneWorld, name: &str) -> [f64; 2] {
    let e = w.find(name).expect("предмет есть");
    w.world_mut()
        .get::<crate::world::Placement>(e)
        .expect("место")
        .at
}

#[test]
fn hero_kicks_the_ball_stands_on_a_crate_and_breaks_into_debris() {
    use reyvec_game::Action::{Attack, Left, Right};
    use reyvec_game::InputFrame;
    let (mut w, mut t) = example_world();
    // Мяч упал на пол.
    t = steps(&mut w, t, 60);
    let ball = body_at(&mut w, "мяч");
    assert!(
        (ball[1] - (-0.78 + 0.05)).abs() < 0.01,
        "мяч на полу: {ball:?}"
    );
    // Герой бежит вправо в мяч: мяч катится впереди.
    w.set_player_body(reyvec_game::Body::new([0.5, -0.64]));
    w.set_input(InputFrame::new(&[Right], &[Right]));
    t = steps(&mut w, t, 30);
    let pushed = body_at(&mut w, "мяч");
    assert!(
        pushed[0] > ball[0] + 0.1,
        "мяч не покатился: {ball:?} → {pushed:?}"
    );
    // Ящик в начале коридора: прыжок на него — герой стоит на ящике.
    w.set_input(InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 60);
    let crate_at = body_at(&mut w, "ящик лёгкий");
    w.set_player_body(reyvec_game::Body::new([crate_at[0], crate_at[1] + 0.35]));
    t = steps(&mut w, t, 40);
    let p = w.snapshot().player.unwrap();
    let crate_now = body_at(&mut w, "ящик лёгкий");
    assert!(p.on_ground, "{p:?}");
    assert!(
        f64::from(p.at[1]) - 0.14 > crate_now[1] + 0.05,
        "герой на ящике, а не на полу: {p:?}, ящик {crate_now:?}"
    );
    // Удар по ящику: он отлетает.
    w.set_player_body(reyvec_game::Body::new([crate_now[0] - 0.2, -0.64]));
    w.set_input(InputFrame::new(&[Right], &[Right]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 20);
    let before = body_at(&mut w, "ящик лёгкий");
    w.set_input(InputFrame::new(&[Attack], &[Attack]));
    t = steps(&mut w, t, 1);
    w.set_input(InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 30);
    let after = body_at(&mut w, "ящик лёгкий");
    assert!(
        after[0] > before[0] + 0.15,
        "удар отбросил ящик: {before:?} → {after:?}"
    );
    // Манекен разбит: обломки летят, потом гаснут.
    let visible = |w: &mut SceneWorld| {
        let mut q = w
            .world_mut()
            .query::<(&bevy_ecs::name::Name, &crate::world::Placement)>();
        q.iter(w.world_mut())
            .filter(|(n, p)| n.as_str() == "обломок" && p.opacity > 0.0)
            .count()
    };
    assert_eq!(visible(&mut w), 0);
    w.set_player_body(reyvec_game::Body::new([-0.7, -0.64]));
    w.set_input(InputFrame::new(&[Left], &[Left]));
    t = steps(&mut w, t, 1);
    // Герой идёт за манекеном и бьёт каждые 0,4 с, пока тот не разобьётся.
    let mut most = 0;
    for i in 0..900 {
        let d = &w.dummies()[0];
        let hero = w.player_body().unwrap();
        let near = hero.pos[0] - d.body.pos[0] < 0.25;
        let held: &[reyvec_game::Action] = if near { &[] } else { &[Left] };
        let pressed: &[reyvec_game::Action] = if near && i % 24 == 0 { &[Attack] } else { &[] };
        w.set_input(InputFrame::new(held, pressed));
        t = steps(&mut w, t, 1);
        most = most.max(visible(&mut w));
        if !w.dummies()[0].alive() {
            break;
        }
    }
    w.set_input(InputFrame::new(&[], &[]));
    t = steps(&mut w, t, 1);
    most = most.max(visible(&mut w));
    assert_eq!(most, 10, "обломки разбитого манекена");
    t = steps(&mut w, t, 60 * 4);
    let _ = t;
    assert_eq!(visible(&mut w), 0, "обломки погасли");
}

#[test]
fn enemy_touch_hurts_the_hero_blinks_and_respawns_when_health_runs_out() {
    use reyvec_game::Action::Right;
    use reyvec_game::InputFrame;
    let text = minimal(
        r##"nodes: [
    (name: "пол", solids: [(at: (0.0, -0.9), size: (6.0, 0.2))]),
    (name: "герой", at: (0.0, -0.7), player: (size: (0.1, 0.2), hp: 2),
        shapes: [Rect(at: (0.0, 0.0), size: (0.1, 0.2), color: "#fff")]),
    (name: "шип", at: (0.5, -0.74), dummy: (size: (0.12, 0.12), hp: 3, contact: 1),
        shapes: [Rect(at: (0.0, 0.0), size: (0.12, 0.12), color: "#f00")]),
]"##,
    );
    let mut w = world(&text);
    let mut t = steps(&mut w, 0, 30);
    assert_eq!(w.snapshot().player.unwrap().hp, 2);
    // Бежит в шип: ранен на 1, отброшен назад, неуязвим и мигает.
    w.set_input(InputFrame::new(&[Right], &[Right]));
    let mut hurt = None;
    let mut seen = 0;
    for _ in 0..60 {
        t = steps(&mut w, t, 1);
        for e in fresh(&w, &mut seen) {
            if let GameEvent::Hurt { amount, dir, .. } = e {
                hurt = Some((amount, dir));
            }
        }
        if hurt.is_some() {
            break;
        }
    }
    assert_eq!(hurt, Some((1, -1.0)), "касание ранит и отбрасывает влево");
    let p = w.snapshot().player.unwrap();
    assert_eq!(p.hp, 1);
    assert!(p.invulnerable && p.vel[0] < 0.0, "{p:?}");
    let hero = w.find("герой").unwrap();
    let mut opacities = Vec::new();
    for _ in 0..20 {
        t = steps(&mut w, t, 1);
        opacities.push(w.world_mut().get::<Placement>(hero).unwrap().opacity);
    }
    assert!(
        opacities.contains(&1.0) && opacities.iter().any(|o| *o < 1.0),
        "мигает: {opacities:?}"
    );
    // Неуязвимость прошла, снова в шип: здоровье кончилось — герой в начале, полный.
    let mut died = false;
    for _ in 0..240 {
        t = steps(&mut w, t, 1);
        died |= fresh(&w, &mut seen)
            .iter()
            .any(|e| matches!(e, GameEvent::Died { .. }));
        if died {
            break;
        }
    }
    assert!(died, "{:?}", w.snapshot().player);
    let p = w.snapshot().player.unwrap();
    assert_eq!(p.hp, 2);
    assert!(p.at[0].abs() < 0.05, "появился в начале: {p:?}");
}

#[test]
fn hero_hits_show_damage_numbers_with_hp_left() {
    use reyvec_game::Action::Attack;
    use reyvec_game::InputFrame;
    let text = minimal(
        r##"nodes: [
    (name: "пол", solids: [(at: (0.0, -0.9), size: (6.0, 0.2))]),
    (name: "герой", at: (0.0, -0.7), player: (size: (0.1, 0.2)),
        shapes: [Rect(at: (0.0, 0.0), size: (0.1, 0.2), color: "#fff")]),
    (name: "манекен", at: (0.16, -0.65), dummy: (size: (0.16, 0.3), hp: 2),
        shapes: [Rect(at: (0.0, 0.0), size: (0.16, 0.3), color: "#f00")]),
]"##,
    );
    let mut w = world(&text);
    let mut t = steps(&mut w, 0, 30);
    let mut damage = Vec::new();
    let mut seen = 0;
    for _ in 0..2 {
        w.set_input(InputFrame::new(&[Attack], &[Attack]));
        t = steps(&mut w, t, 1);
        w.set_input(InputFrame::new(&[], &[]));
        for _ in 0..40 {
            t = steps(&mut w, t, 1);
            for e in fresh(&w, &mut seen) {
                if let GameEvent::Damage { amount, left, .. } = e {
                    damage.push((amount, left));
                }
            }
        }
        // Отброшенный манекен — снова к герою.
        let e = w.find("манекен").unwrap();
        let mut target = w.world_mut().get_mut::<crate::world::Target>(e).unwrap();
        target.dummy.body = reyvec_game::Body::new([0.16, -0.65]);
    }
    assert_eq!(damage, vec![(1, 1), (1, 0)]);
}

/// События, которых ещё не было (после шага `seen`).
fn fresh(w: &SceneWorld, seen: &mut u64) -> Vec<GameEvent> {
    let events = w.snapshot().events;
    let out = events
        .iter()
        .filter(|(s, _)| *s > *seen)
        .map(|(_, e)| *e)
        .collect();
    if let Some(&(s, _)) = events.last() {
        *seen = (*seen).max(s);
    }
    out
}

#[test]
fn console_commands_move_heal_and_protect_the_hero() {
    use crate::DebugCommand::{God, Health, Respawn, Teleport, Where};
    use reyvec_game::Action::Right;
    use reyvec_game::InputFrame;
    let text = minimal(
        r##"nodes: [
    (name: "пол", solids: [(at: (0.0, -0.9), size: (6.0, 0.2))]),
    (name: "герой", at: (0.0, -0.7), player: (size: (0.1, 0.2), hp: 3),
        shapes: [Rect(at: (0.0, 0.0), size: (0.1, 0.2), color: "#fff")]),
    (name: "шип", at: (0.5, -0.74), dummy: (size: (0.12, 0.12), hp: 3, contact: 1),
        shapes: [Rect(at: (0.0, 0.0), size: (0.12, 0.12), color: "#f00")]),
]"##,
    );
    let mut w = world(&text);
    let mut t = steps(&mut w, 0, 30);
    // Телепорт: стоит там, без скорости, и падает на пол.
    assert!(w.debug(&Teleport([-1.0, 0.0])).unwrap().contains("-1.00"));
    w.refresh();
    let p = w.snapshot().player.unwrap();
    assert_eq!(p.at, [-1.0, 0.0]);
    assert_eq!(p.vel, [0.0, 0.0]);
    assert_eq!(p.size, [0.1, 0.2]);
    t = steps(&mut w, t, 60);
    assert!(w.snapshot().player.unwrap().on_ground);
    // Здоровье — в пределах от 1 до наибольшего.
    assert_eq!(w.debug(&Health(1)).unwrap(), "здоровье 1 из 3");
    assert_eq!(w.debug(&Health(99)).unwrap(), "здоровье 3 из 3");
    assert!(w.debug(&Health(0)).unwrap().starts_with("здоровье 1"));
    assert!(w.debug(&Where).unwrap().contains("начало в (0.00"));
    // Бессмертный бежит в шип и не ранен.
    w.debug(&God(Some(true))).unwrap();
    w.debug(&Teleport([0.2, -0.7])).unwrap();
    w.set_input(InputFrame::new(&[Right], &[Right]));
    let mut seen = 0;
    for _ in 0..60 {
        t = steps(&mut w, t, 1);
        for e in fresh(&w, &mut seen) {
            assert!(!matches!(e, GameEvent::Hurt { .. }), "бессмертный ранен");
        }
    }
    let p = w.snapshot().player.unwrap();
    assert!(p.god && p.hp == 1, "{p:?}");
    // Переключение и возврат в начало.
    assert_eq!(w.debug(&God(None)).unwrap(), "бессмертие выключено");
    w.set_input(InputFrame::new(&[], &[]));
    w.debug(&Respawn).unwrap();
    steps(&mut w, t, 1);
    let p = w.snapshot().player.unwrap();
    assert_eq!(p.hp, 3);
    assert!(p.at[0].abs() < 1e-3, "{p:?}");
    // Без героя — понятная ошибка.
    let mut empty = world(&minimal(""));
    assert_eq!(empty.debug(&Where), Err("в сцене нет героя".into()));
}

#[test]
fn logic_thread_pauses_steps_and_slows_down_from_the_console() {
    let text = minimal(
        r##"nodes: [
    (name: "пол", solids: [(at: (0.0, -0.9), size: (6.0, 0.2))]),
    (name: "герой", at: (0.0, -0.7), player: (size: (0.1, 0.2)),
        shapes: [Rect(at: (0.0, 0.0), size: (0.1, 0.2), color: "#fff")]),
]"##,
    );
    let logic = SceneLogic::start(world(&text), 0.0);
    let deadline = Instant::now() + Duration::from_secs(10);
    let wait = |what: &str, done: &dyn Fn() -> bool| {
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    wait("логика не шагает", &|| logic.steps() >= 3);
    // Пауза: шаги стоят; «шаг 2» — ровно два шага.
    logic.set_debug_paused(true);
    std::thread::sleep(Duration::from_millis(40));
    let at = logic.steps();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(logic.steps(), at, "на паузе шагает");
    logic.step(2);
    wait("шаг на паузе", &|| logic.steps() == at + 2);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(logic.steps(), at + 2);
    // Команда на паузе видна сразу, без шага.
    logic.debug(crate::DebugCommand::Teleport([1.0, 0.5]));
    wait("ответ команды", &|| {
        logic.frame(Instant::now()).player.unwrap().at == [1.0, 0.5]
    });
    let replies = logic.take_replies();
    assert_eq!(replies.len(), 1);
    assert!(replies[0].as_ref().unwrap().contains("1.00"));
    assert_eq!(logic.steps(), at + 2);
    // Замедление в 4 раза: шаги реже. Пределы скорости соблюдаются.
    assert_eq!(logic.set_time_scale(100.0), crate::TIME_SCALE.1);
    assert_eq!(logic.set_time_scale(f64::NAN), 1.0);
    assert_eq!(logic.set_time_scale(0.25), 0.25);
    logic.set_debug_paused(false);
    let from = logic.steps();
    std::thread::sleep(Duration::from_millis(500));
    let slow = logic.steps() - from;
    // 0,5 с при 15 шагах в секунду — около 7 шагов (обычно было бы 30).
    assert!((3..=12).contains(&slow), "замедленно сделано {slow} шагов");
}

#[test]
fn mouse_attack_swings_toward_the_cursor() {
    use reyvec_game::Action::Attack;
    use reyvec_game::InputFrame;
    let text = minimal(
        r##"nodes: [
    (name: "пол", solids: [(at: (0.0, -0.9), size: (6.0, 0.2))]),
    (name: "герой", at: (0.0, -0.7), player: (size: (0.1, 0.2)),
        shapes: [Rect(at: (0.0, 0.0), size: (0.1, 0.2), color: "#fff")]),
]"##,
    );
    let mut w = world(&text);
    let mut t = steps(&mut w, 0, 30);
    let mut seen = 0;
    let mut swings = Vec::new();
    // Курсор слева, потом справа: удар туда, куда он показывает.
    for x in [-1.0, 1.0] {
        w.set_input(InputFrame::new(&[Attack], &[Attack]).with_aim([x, -0.7], &[Attack]));
        t = steps(&mut w, t, 1);
        w.set_input(InputFrame::new(&[], &[]));
        for _ in 0..40 {
            t = steps(&mut w, t, 1);
            for e in fresh(&w, &mut seen) {
                if let GameEvent::Swing { dir, .. } = e {
                    swings.push(dir);
                }
            }
        }
    }
    assert_eq!(swings, vec![-1.0, 1.0]);
}
