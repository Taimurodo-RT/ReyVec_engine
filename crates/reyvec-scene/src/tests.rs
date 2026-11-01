use super::*;

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
    assert!(scene.objects[0].glow && !scene.objects[0].shadow);
    assert!(scene.objects[1].shadow);
    // Два слоя с тенью, третий — без своего слоя.
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
    // Одинаковые формы (ножки стола, крышки) нарезаются один раз.
    assert!(cache.len() < scene.shapes);
    assert_eq!(lib.len(), cache.len());
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
