//! Проверка по эталонам: кадры песочницы без окна сравниваются с картинками в
//! `tests/reference`. Если кадр изменился случайно, тест упадёт и сохранит рядом с
//! целевой папкой сборки получившийся кадр и картинку отличий (в CI они прикладываются
//! к запуску как артефакт `reference-failures`).
//!
//! Нужен драйвер Vulkan. Эталоны сняты на программной видеокарте lavapipe (как в CI):
//! `cargo test -p reyvec-app --test reference -- --include-ignored`.
//!
//! Если картинка изменилась намеренно, эталоны обновляются так:
//! `REYVEC_UPDATE_REFERENCES=1 cargo test -p reyvec-app --test reference -- --include-ignored`,
//! после чего новые PNG коммитятся вместе с изменением.

use std::path::{Path, PathBuf};

use reyvec_app::snapshot::{compare, diff_image, load_png, save_png};
use reyvec_app::{
    DemoScene, FillMode, GpuOptions, HeadlessScene, LIGHT_VECTOR_TOP, Msaa, render_headless,
    validation_error_count,
};

/// Насколько может отличаться канал пикселя (из 255): разные версии драйвера чуть
/// по-разному сглаживают текстуры.
const TOLERANCE: u8 = 8;
/// Какая доля пикселей может отличаться больше допуска (края треугольника и кругов).
const MAX_DIFFERING: f64 = 0.002;

struct Case {
    name: &'static str,
    scene: HeadlessScene,
}

fn cases() -> Vec<Case> {
    let at = |scene, time, shader_error| HeadlessScene {
        width: 640,
        height: 360,
        time,
        shader_error,
        scene,
        msaa: Msaa::Off,
        perceptual_edges: false,
        fill: FillMode::Triangles,
        zoom: false,
        files: Default::default(),
        cascades: false,
    };
    let scene = |time, shader_error| at(DemoScene::Gpu, time, shader_error);
    vec![
        Case {
            name: "scene-0s",
            scene: scene(0.0, false),
        },
        // Через 2.5 с всё повёрнуто, а у шахматки другие клетки (новая текстура).
        Case {
            name: "scene-2.5s",
            scene: scene(2.5, false),
        },
        Case {
            name: "shader-error",
            scene: scene(1.0, true),
        },
        // Векторы этапа 3: звёзды с разными правилами заливки, сердце, кольцо, цветок из
        // одного лепестка, углы обводки, волна.
        Case {
            name: "vector-0s",
            scene: at(DemoScene::Vector, 0.0, false),
        },
        // Через 2 с всё повёрнуто и сдвинуто, сердце увеличено, кольцо сплющено.
        Case {
            name: "vector-2s",
            scene: at(DemoScene::Vector, 2.0, false),
        },
        // Та же сцена со сглаживанием MSAA 4×: края мягкие, без «лесенки».
        Case {
            name: "vector-2s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Vector, 2.0, false)
            },
        },
        Case {
            name: "vector-2s-stencil",
            scene: HeadlessScene {
                fill: FillMode::Stencil,
                ..at(DemoScene::Vector, 2.0, false)
            },
        },
        Case {
            name: "vector-2s-stencil-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                fill: FillMode::Stencil,
                ..at(DemoScene::Vector, 2.0, false)
            },
        },
        Case {
            name: "vector-zoom-triangles-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                zoom: true,
                ..at(DemoScene::Vector, 2.0, false)
            },
        },
        Case {
            name: "vector-zoom-stencil-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                fill: FillMode::Stencil,
                zoom: true,
                ..at(DemoScene::Vector, 2.0, false)
            },
        },
        // Способ у каждой фигуры свой: сердце трафаретом, остальное треугольниками.
        Case {
            name: "vector-zoom-per-shape-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                fill: FillMode::PerShape,
                zoom: true,
                ..at(DemoScene::Vector, 2.0, false)
            },
        },
        // Перетекание: в начале большая фигура круг, маленькие в разных формах.
        Case {
            name: "morph-0s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Morph, 0.0, false)
            },
        },
        // Через 2.2 с большая фигура между звездой и сердцем.
        Case {
            name: "morph-2.2s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Morph, 2.2, false)
            },
        },
        // Слои: снеговик частями и снеговик слоем 0.5, капсулы в режимах смешивания.
        Case {
            name: "layers-1s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Layers, 1.0, false)
            },
        },
        Case {
            name: "layers-1s-msaa-off",
            scene: HeadlessScene {
                msaa: Msaa::Off,
                ..at(DemoScene::Layers, 1.0, false)
            },
        },
        // Маски: обрезка по звезде, маска по яркости, обратная маска (дырка).
        Case {
            name: "masks-1s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Masks, 1.0, false)
            },
        },
        Case {
            name: "masks-1s-msaa-off",
            scene: HeadlessScene {
                msaa: Msaa::Off,
                ..at(DemoScene::Masks, 1.0, false)
            },
        },
        // Сетка тестовых SVG из assets/svg: фигуры, градиенты, группы, обрезка и маски,
        // повороты и наклоны, рисунок целиком.
        Case {
            name: "svg-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Svg, 0.0, false)
            },
        },
        // Фигуры из примера Motif на середине пути: звезда → сердце, «пауза», рот.
        Case {
            name: "motif-2.5s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Motif, 2.5, false)
            },
        },
        // Поток: к 25 с все 200 новых фигур добавлены во время работы и загружены
        // внутри кадра.
        Case {
            name: "stream-25s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Stream, 25.0, false)
            },
        },
        // Текст: заголовок, колонки с четырьмя выравниваниями, строки 10–32 пикселей,
        // текст в слое с маской и повёрнутое слово.
        Case {
            name: "text-3s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Text, 3.0, false)
            },
        },
        // Стресс: 10 000 фигур сеткой на весь кадр, вперемешку пять обычных фигур, и
        // те же 10 000 перетекающими (каждая трафаретом).
        Case {
            name: "stress-2s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Stress, 2.0, false)
            },
        },
        Case {
            name: "stress-morph-2s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::StressMorph, 2.0, false)
            },
        },
        Case {
            name: "vector-2s-msaa4-editor",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                perceptual_edges: true,
                ..at(DemoScene::Vector, 2.0, false)
            },
        },
        // Демо-сцена этапа 4 «лаборатория мага»: векторные предметы с мягкими тенями от
        // восьми источников, светящиеся SDF-зелья с пузырями, неон, свеча, котёл, шар,
        // свечение ярких мест и каменная стена.
        Case {
            name: "lab-2s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::Lab, 2.0, false)
            },
        },
        // Опыт 4.6: та же лаборатория со светом каскадами лучей (radiance cascades).
        Case {
            name: "lab-cascades-2s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                cascades: true,
                ..at(DemoScene::Lab, 2.0, false)
            },
        },
        // Этап 5: сцена из файла `scenes/first-room.ron` (встроенная копия примера): предметы
        // с вложенностью (5.2) и движением (5.3): через 1 с лампа отклонилась, стрелки ушли.
        // С 6.1 в комнате герой (уже стоит на полу) и ящики под полкой.
        Case {
            name: "file-first-room-hero-1s-msaa4",
            scene: HeadlessScene {
                msaa: Msaa::X4,
                ..at(DemoScene::File, 1.0, false)
            },
        }, // Этап 7.1: объёмный вид камерой 3D по умолчанию — пол с сеткой и осями, шар,
        // куб и тор, небо.
        Case {
            name: "space-1s",
            scene: at(DemoScene::Space, 1.0, false),
        },
        // Опыт 7.6ж: полигональные модели рядом с формами SDF (`scenes/3d/mixed.ron`) —
        // общий буфер геометрии, тени от заменителей, проход с глубиной.
        Case {
            name: "space-mixed-1s",
            scene: HeadlessScene {
                files: reyvec_app::UserFiles {
                    space: Some(include_str!("../../../scenes/3d/mixed.ron").into()),
                    ..Default::default()
                },
                ..at(DemoScene::Space, 1.0, false)
            },
        },
        // 7.7: векторные контуры, выдавленные и повёрнутые в объёмные формы
        // (`scenes/3d/vector.ron`).
        Case {
            name: "space-vector-1s",
            scene: HeadlessScene {
                files: reyvec_app::UserFiles {
                    space: Some(include_str!("../../../scenes/3d/vector.ron").into()),
                    ..Default::default()
                },
                ..at(DemoScene::Space, 1.0, false)
            },
        },
        // Коробка с дырами вблизи (искажение по кругу на стенках, 7.6п): наискосок
        // и прямо сквозь дыру.
        Case {
            name: "space-hollow-near-1s",
            scene: HeadlessScene {
                files: reyvec_app::UserFiles {
                    space: Some(with_camera(
                        include_str!("../../../scenes/3d/space.ron"),
                        "(eye: (5.0, 1.5, 4.4), look_at: (3.4, 0.55, 2.6))",
                    )),
                    ..Default::default()
                },
                ..at(DemoScene::Space, 1.0, false)
            },
        },
        Case {
            name: "space-hollow-through-0s",
            scene: HeadlessScene {
                files: reyvec_app::UserFiles {
                    space: Some(with_camera(
                        include_str!("../../../scenes/3d/space.ron"),
                        "(eye: (3.4, 0.75, 5.0), look_at: (3.4, 0.6, 2.6))",
                    )),
                    ..Default::default()
                },
                ..at(DemoScene::Space, 0.0, false)
            },
        },
        // Лучи глаза пиксельным шейдером (`compute: false`) дают тот же кадр, что
        // вычислительным (7.6м; эталоны — копии space-1s и space-mixed-1s).
        Case {
            name: "space-1s-fragment",
            scene: HeadlessScene {
                files: reyvec_app::UserFiles {
                    space: Some(fragment(include_str!("../../../scenes/3d/space.ron"))),
                    ..Default::default()
                },
                ..at(DemoScene::Space, 1.0, false)
            },
        },
        Case {
            name: "space-mixed-1s-fragment",
            scene: HeadlessScene {
                files: reyvec_app::UserFiles {
                    space: Some(fragment(include_str!("../../../scenes/3d/mixed.ron"))),
                    ..Default::default()
                },
                ..at(DemoScene::Space, 1.0, false)
            },
        },
    ]
}

/// Файл сцены «3D» с лучами пиксельным шейдером (`compute: false`).
fn fragment(text: &str) -> std::sync::Arc<str> {
    assert!(text.contains("version: 1,"));
    text.replacen("version: 1,", "version: 1,\n    compute: false,", 1)
        .into()
}

/// Файл сцены «3D» с другой камерой.
fn with_camera(text: &str, camera: &str) -> std::sync::Arc<str> {
    assert!(text.contains("version: 1,"));
    text.replacen(
        "version: 1,",
        &format!("version: 1,\n    camera: Some({camera}),"),
        1,
    )
    .into()
}

/// Печатает PNG в лог строками base64 между метками `--- PNG <имя> ---` и
/// `--- конец PNG ---` (собрать обратно: `base64 -d`).
fn print_png_text(name: &str, path: &Path) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let mut text = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                text.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                text.push('=');
            }
        }
    }
    println!("--- PNG {name} ---");
    for line in text.as_bytes().chunks(4000) {
        println!("{}", std::str::from_utf8(line).expect("base64 — ASCII"));
    }
    println!("--- конец PNG ---");
}

fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reference")
}

fn failures_dir() -> PathBuf {
    // target/reference-failures: CI забирает эту папку, если тест упал.
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .expect("CARGO_TARGET_TMPDIR внутри target")
        .join("reference-failures")
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn frames_match_references() {
    let update = std::env::var_os("REYVEC_UPDATE_REFERENCES").is_some();
    let options = GpuOptions {
        validation: true,
        gpu: None,
    };
    let mut problems = Vec::new();
    for case in cases() {
        let frame = render_headless(&options, &case.scene).expect("кадр без окна");
        let reference = reference_dir().join(format!("{}.png", case.name));
        if update {
            save_png(&reference, &frame.image).expect("запись эталона");
            eprintln!("обновлён эталон {}", reference.display());
            continue;
        }
        let expected = match load_png(&reference) {
            Ok(img) => img,
            Err(e) => {
                // Получившийся кадр кладём к упавшим: в CI его можно взять из артефакта
                // и, проверив глазами, сделать эталоном.
                let actual = failures_dir().join(format!("{}-actual.png", case.name));
                save_png(&actual, &frame.image).expect("запись кадра");
                // И в лог текстом: не везде можно скачать артефакт CI, а лог читается всегда.
                print_png_text(case.name, &actual);
                problems.push(format!(
                    "{}: нет эталона ({e}). Снимите его с REYVEC_UPDATE_REFERENCES=1 \
                     или возьмите {}",
                    case.name,
                    actual.display()
                ));
                continue;
            }
        };
        let Some(diff) = compare(&frame.image, &expected, TOLERANCE) else {
            problems.push(format!(
                "{}: размер {}×{}, а у эталона {}×{}",
                case.name, frame.image.width, frame.image.height, expected.width, expected.height
            ));
            continue;
        };
        eprintln!(
            "{}: отличаются {} пикселей из {} ({:.3}%), наибольшая разница канала {}",
            case.name,
            diff.differing_pixels,
            diff.total_pixels,
            diff.differing_fraction() * 100.0,
            diff.max_channel_diff
        );
        if diff.differing_fraction() > MAX_DIFFERING {
            let dir = failures_dir();
            let actual = dir.join(format!("{}-actual.png", case.name));
            let marked = dir.join(format!("{}-diff.png", case.name));
            save_png(&actual, &frame.image).expect("запись кадра");
            save_png(&marked, &diff_image(&frame.image, &expected, TOLERANCE))
                .expect("запись отличий");
            // Кадр и в лог: новый эталон можно взять из лога, не скачивая артефакт.
            print_png_text(case.name, &actual);
            problems.push(format!(
                "{}: кадр не совпал с эталоном ({:.3}% пикселей). Смотрите {} и {}",
                case.name,
                diff.differing_fraction() * 100.0,
                actual.display(),
                marked.display()
            ));
        }
    }
    assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

// Сцена «свет» сверяется не с картинкой, а с тем же расчётом на процессоре по формулам
// крейта reyvec-sdf (зеркало шейдерной библиотеки sdf2d.slang). Так проверяется сразу,
// что шейдер и процессор считают поля одинаково.

/// Сцена `sdf_demo.slang` на процессоре: цвет точки сцены `p` в линейном цвете.
fn light_scene_cpu(p: reyvec_sdf::V2, t: f32, pixel: f32) -> [f32; 3] {
    use reyvec_sdf::{V2, *};
    let at = |x: f32, y: f32| p - V2::new(x, y);
    let mut shapes: Vec<(f32, [f32; 3])> = Vec::new();
    shapes.push((
        sd_circle(at(-1.2, 0.4), 0.28 + 0.03 * (t * 2.0).sin()),
        [0.95, 0.45, 0.2],
    ));
    shapes.push((
        sd_round_box(rotate(at(-0.4, 0.4), t * 0.5), V2::new(0.3, 0.18), 0.07),
        [0.25, 0.7, 0.95],
    ));
    let ctrl = V2::new(0.8, 0.75 + 0.25 * (t * 1.3).sin());
    shapes.push((
        op_round(
            sd_bezier(p, V2::new(0.4, 0.2), ctrl, V2::new(1.25, 0.35)),
            0.04,
        ),
        [0.95, 0.85, 0.3],
    ));
    let sep = 0.22 + 0.16 * (t * 1.1).sin();
    let b1 = sd_circle(at(-1.0 - sep, -0.45), 0.17);
    let b2 = sd_circle(at(-1.0 + sep, -0.45), 0.2);
    shapes.push((op_smooth_union(b1, b2, 0.25), [0.4, 0.9, 0.45]));
    let hole = V2::new(0.18 * (t * 0.9).cos(), -0.45 + 0.1 * (t * 0.9).sin());
    shapes.push((
        op_smooth_subtract(
            sd_round_box(at(0.0, -0.45), V2::new(0.3, 0.22), 0.04),
            sd_circle(p - hole, 0.15),
            0.08,
        ),
        [0.85, 0.4, 0.9],
    ));
    let q = at(1.0, -0.45);
    shapes.push((
        op_smooth_intersect(
            sd_circle(q, 0.3),
            sd_box(rotate(q, -t * 0.4), V2::new(0.22, 0.22)),
            0.06,
        ),
        [0.95, 0.3, 0.4],
    ));

    let field = shapes.iter().map(|s| s.0).fold(1e9_f32, f32::min);
    let mut color = [0.02_f32, 0.025, 0.035];
    let spacing = 0.1;
    let x = field / spacing + 0.5;
    let line_d = (x - x.floor() - 0.5).abs() * spacing;
    let fade = (-3.0 * field.max(0.0)).exp();
    let line = fade * coverage(line_d - pixel * 0.5, pixel);
    let glow = (-12.0 * field.max(0.0)).exp();
    for (c, (l, g)) in color
        .iter_mut()
        .zip([0.05, 0.08, 0.12].into_iter().zip([0.04, 0.05, 0.07]))
    {
        *c += l * line + g * glow;
    }
    for (d, shape) in &shapes {
        let k = coverage(*d, pixel);
        for (c, s) in color.iter_mut().zip(shape) {
            *c += (s - *c) * k;
        }
    }
    let edge = 0.35 * coverage(field.abs() - pixel * 0.5, pixel);
    for c in &mut color {
        *c += (1.0 - *c) * edge;
    }
    color
}

fn linear_to_srgb8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn light_scene_matches_cpu() {
    let (width, height, time) = (640u32, 360u32, 1.0f32);
    let scene = HeadlessScene {
        width,
        height,
        time,
        shader_error: false,
        scene: DemoScene::Light,
        msaa: Msaa::X4,
        perceptual_edges: false,
        fill: FillMode::Triangles,
        zoom: false,
        files: Default::default(),
        cascades: false,
    };
    let options = GpuOptions {
        validation: true,
        gpu: None,
    };
    let frame = render_headless(&options, &scene).expect("кадр без окна");
    let aspect = width as f32 / height as f32;
    let pixel = 2.0 / height as f32;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            // Середина пикселя, как SV_Position; y экрана смотрит вверх.
            let nx = (x as f32 + 0.5) / width as f32 * 2.0 - 1.0;
            let ny = (y as f32 + 0.5) / height as f32 * 2.0 - 1.0;
            let p = reyvec_sdf::V2::new(nx * aspect, -ny);
            if p.y < LIGHT_VECTOR_TOP {
                // Ряд векторных фигур (4.2б): их проверяют векторные тесты, здесь
                // берётся то, что нарисовала видеокарта.
                let i = ((y * width + x) * 4) as usize;
                rgba.extend_from_slice(&frame.image.rgba[i..i + 4]);
                continue;
            }
            let c = light_scene_cpu(p, time, pixel);
            rgba.extend(c.iter().map(|v| linear_to_srgb8(*v)));
            rgba.push(255);
        }
    }
    let expected = reyvec_app::FrameImage {
        width,
        height,
        rgba,
    };
    let diff = compare(&frame.image, &expected, TOLERANCE).expect("размер совпадает");
    eprintln!(
        "свет против процессора: отличаются {} пикселей из {} ({:.3}%), наибольшая разница канала {}",
        diff.differing_pixels,
        diff.total_pixels,
        diff.differing_fraction() * 100.0,
        diff.max_channel_diff
    );
    if diff.differing_fraction() > MAX_DIFFERING {
        let dir = failures_dir();
        save_png(&dir.join("light-gpu.png"), &frame.image).expect("запись кадра");
        save_png(&dir.join("light-cpu.png"), &expected).expect("запись кадра");
        save_png(
            &dir.join("light-diff.png"),
            &diff_image(&frame.image, &expected, TOLERANCE),
        )
        .expect("запись отличий");
        panic!(
            "сцена «свет» на видеокарте не совпала с расчётом на процессоре ({:.3}% пикселей)",
            diff.differing_fraction() * 100.0
        );
    }
    assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn lamps_scene_lights_around_lamps() {
    // Сцена «лампы и тени» (4.3): проходы поля и света без ошибок валидации, и у тёплой
    // лампы светлее, чем в дальнем углу, где только лунный и общий свет.
    let (width, height, time) = (640u32, 360u32, 1.0f32);
    let scene = HeadlessScene {
        width,
        height,
        time,
        shader_error: false,
        scene: DemoScene::Lamps,
        msaa: Msaa::X4,
        perceptual_edges: false,
        fill: FillMode::Triangles,
        zoom: false,
        files: Default::default(),
        cascades: false,
    };
    let options = GpuOptions {
        validation: true,
        gpu: None,
    };
    let frame = render_headless(&options, &scene).expect("кадр без окна");
    assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
    let brightness = |cx: u32, cy: u32| {
        let mut sum = 0u32;
        for y in cy - 2..=cy + 2 {
            for x in cx - 2..=cx + 2 {
                let i = ((y * width + x) * 4) as usize;
                sum += frame.image.rgba[i..i + 3]
                    .iter()
                    .map(|&v| u32::from(v))
                    .sum::<u32>();
            }
        }
        sum
    };
    // Тёплая лампа при t = 1 (lights::demo_lights): x = cos 0,45, y = 0,05 + 0,55·sin 0,45.
    let aspect = width as f32 / height as f32;
    let (lx, ly) = (0.45f32.cos(), 0.05 + 0.55 * 0.45f32.sin());
    let px = ((lx / aspect + 1.0) * 0.5 * width as f32) as u32;
    let py = ((1.0 - ly) * 0.5 * height as f32) as u32;
    let lamp = brightness(px, py);
    let corner = brightness(4, 4);
    eprintln!("яркость у лампы {lamp}, в углу {corner}");
    assert!(lamp > corner * 2, "у лампы {lamp}, в углу {corner}");
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn blobs_scene_draws_slime_and_glowing_orb() {
    // Сцена «слизь и магия» (4.5): SDF-объекты в кадре и в маске теней, свет и свечение
    // без ошибок валидации. Слизь зелёная, магический шар ярче угла сцены.
    let (width, height, time) = (640u32, 360u32, 1.0f32);
    let scene = HeadlessScene {
        width,
        height,
        time,
        shader_error: false,
        scene: DemoScene::Blobs,
        msaa: Msaa::X4,
        perceptual_edges: false,
        fill: FillMode::Triangles,
        zoom: false,
        files: Default::default(),
        cascades: false,
    };
    let options = GpuOptions {
        validation: true,
        gpu: None,
    };
    let frame = render_headless(&options, &scene).expect("кадр без окна");
    assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
    let aspect = width as f32 / height as f32;
    let pixel = |x: f32, y: f32| {
        let px = ((x / aspect + 1.0) * 0.5 * width as f32) as u32;
        let py = ((1.0 - y) * 0.5 * height as f32) as u32;
        let i = ((py * width + px) * 4) as usize;
        [
            u32::from(frame.image.rgba[i]),
            u32::from(frame.image.rgba[i + 1]),
            u32::from(frame.image.rgba[i + 2]),
        ]
    };
    // Середина тела слизи (blobs::demo_objects).
    let slime = pixel(-0.8, -0.42);
    eprintln!("слизь {slime:?}");
    assert!(
        slime[1] > slime[0] && slime[1] > slime[2],
        "слизь не зелёная: {slime:?}"
    );
    // Магический шар при t = 1: (0; 0,3 + 0,05·sin 1,5).
    let orb = pixel(0.0, 0.3 + 0.05 * 1.5f32.sin());
    let corner = pixel(1.7, 0.9);
    eprintln!("шар {orb:?}, угол {corner:?}");
    let sum = |c: [u32; 3]| c.iter().sum::<u32>();
    // Шар почти белый от собственного света; угол освещён только общим и лунным светом
    // (на lavapipe шар [239, 218, 255], угол [67, 75, 96]).
    assert!(sum(orb) > sum(corner) * 2, "шар {orb:?}, угол {corner:?}");
    // Пол у тёплой лампы (сверху слева) освещён, а не в тени: однажды маска теней
    // очищалась цветом фона, каждая её точка считалась краем, и тень была везде.
    let floor = pixel(-1.1, 0.45);
    eprintln!("пол у лампы {floor:?}");
    // Ожидаемо около [190, 170, 150]; пол в тени был бы не светлее угла.
    assert!(
        sum(floor) * 2 > sum(corner) * 3,
        "пол у лампы {floor:?}, угол {corner:?}"
    );
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn file_scene_draws_what_the_text_says() {
    // Сцена «из файла» (5.1): красный квадрат в слое с тенью, лампа справа от него и
    // зелёный светящийся шар слева. Квадрат красный, шар зелёный и яркий, а за квадратом
    // (с другой стороны от лампы) тень темнее, чем пол перед ним.
    let text = r##"(
        format: "reyvec-scene",
        version: 1,
        layers: [(shadow: true, shapes: [Rect(at: (0.0, 0.0), size: (0.3, 0.3), color: "#ff2020")])],
        lights: [Point(at: (0.6, 0.0), color: "#ffffff", power: 2.0, reach: 3.0, size: 0.02)],
        objects: [(parts: [Circle(at: (-1.2, 0.6), r: 0.1)], color: "#20ff40", power: 3.0, glow: true)],
    )"##;
    let (width, height) = (640u32, 360u32);
    let scene = HeadlessScene {
        width,
        height,
        time: 0.0,
        shader_error: false,
        scene: DemoScene::File,
        msaa: Msaa::X4,
        perceptual_edges: false,
        fill: FillMode::Triangles,
        zoom: false,
        files: reyvec_app::UserFiles {
            scene: Some(text.into()),
            ..Default::default()
        },
        cascades: false,
    };
    let options = GpuOptions {
        validation: true,
        gpu: None,
    };
    let frame = render_headless(&options, &scene).expect("кадр без окна");
    assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
    let aspect = width as f32 / height as f32;
    let pixel = |x: f32, y: f32| {
        let px = ((x / aspect + 1.0) * 0.5 * width as f32) as u32;
        let py = ((1.0 - y) * 0.5 * height as f32) as u32;
        let i = ((py * width + px) * 4) as usize;
        [
            u32::from(frame.image.rgba[i]),
            u32::from(frame.image.rgba[i + 1]),
            u32::from(frame.image.rgba[i + 2]),
        ]
    };
    let sum = |c: [u32; 3]| c.iter().sum::<u32>();
    let rect = pixel(0.0, 0.0);
    let orb = pixel(-1.2, 0.6);
    let lit = pixel(0.3, 0.0);
    let shadow = pixel(-0.4, 0.0);
    eprintln!("квадрат {rect:?}, шар {orb:?}, пол у лампы {lit:?}, в тени {shadow:?}");
    assert!(
        rect[0] > rect[1] * 2 && rect[0] > rect[2] * 2,
        "квадрат не красный: {rect:?}"
    );
    assert!(
        orb[1] > orb[0] && orb[1] > orb[2],
        "шар не зелёный: {orb:?}"
    );
    assert!(
        sum(lit) > sum(shadow) * 3 / 2,
        "нет тени: у лампы {lit:?}, за квадратом {shadow:?}"
    );
}

/// Кадр сцены из файла `text` в момент `time`, сыгранной по сценарию `play`. Без
/// текста берётся встроенный пример со своей папкой (картинки и Motif по путям от неё).
fn play_frame(
    text: Option<&str>,
    time: f32,
    play: &reyvec_app::HeadlessPlay,
    msaa: Msaa,
) -> reyvec_app::HeadlessImage {
    let scene = HeadlessScene {
        width: 640,
        height: 360,
        time,
        shader_error: false,
        scene: DemoScene::File,
        msaa,
        perceptual_edges: false,
        fill: FillMode::Triangles,
        zoom: false,
        files: reyvec_app::UserFiles {
            scene: text.map(Into::into),
            ..Default::default()
        },
        cascades: false,
    };
    let options = GpuOptions {
        validation: true,
        gpu: None,
    };
    let frame = reyvec_app::render_headless_play(&options, &scene, Some(play)).expect("кадр");
    assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
    frame
}

/// Кадр в лог строками base64 (между `--- PNG имя ---` и `--- конец PNG ---`), даже
/// если тест прошёл: так кадры проверки в движении видно глазами в логе CI.
fn show_frame(name: &str, frame: &reyvec_app::HeadlessImage) {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("reyvec-{name}.png"));
    save_png(&path, &frame.image).expect("PNG");
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = std::fs::read(&path).expect("PNG");
    let mut text = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                text.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                text.push('=');
            }
        }
    }
    // Мимо перехвата вывода тестов: println! видно только у упавших.
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "--- PNG {name} ---");
    for line in text.as_bytes().chunks(4000) {
        let _ = err.write_all(line);
        let _ = writeln!(err);
    }
    let _ = writeln!(err, "--- конец PNG ---");
}

fn brightness(c: [u8; 4]) -> u32 {
    u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2])
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn hero_casts_shadow_and_is_drawn_where_logic_puts_him() {
    use reyvec_app::{HeadlessPlay, PlayInput};
    use reyvec_scene::game::Action;
    // Синий герой на полу, лампа справа на высоте его середины. Стоит — за ним (слева)
    // тень; прыгнул — тень ушла вместе с ним, там светло.
    let text = r##"(
        format: "reyvec-scene",
        version: 1,
        ambient: "#202428",
        nodes: [
            (name: "герой", at: (-0.25, -0.5), depth: 3, shadow: true, player: (),
             shapes: [Rect(at: (0.0, 0.0), size: (0.14, 0.28), color: "#3060e0")]),
            (name: "пол", solids: [(at: (0.0, -0.88), size: (6.0, 0.2))]),
        ],
        lights: [Point(at: (0.9, -0.64), color: "#ffffff", power: 2.0, reach: 3.0, size: 0.02)],
    )"##;
    let stand = HeadlessPlay::default();
    let jump = HeadlessPlay {
        start: None,
        inputs: PlayInput::hold(Action::Jump, 1.0, 1.6).to_vec(),
        ..Default::default()
    };
    let a = play_frame(Some(text), 1.0, &stand, Msaa::X4);
    let b = play_frame(Some(text), 1.36, &jump, Msaa::X4);
    show_frame("play-shadow-stand", &a);
    show_frame("play-shadow-jump", &b);
    let pa = a.player.expect("герой");
    let pb = b.player.expect("герой");
    eprintln!(
        "стоит {pa:?}, прыжок {pb:?}, камера {:?} / {:?}",
        a.camera, b.camera
    );
    assert!(pa.on_ground && (pa.at[1] - -0.64).abs() < 1e-3, "{pa:?}");
    assert!(pb.at[1] > -0.2, "не подпрыгнул: {pb:?}");
    // Герой нарисован там, где его поставила логика: середина синяя.
    for (frame, p) in [(&a, pa), (&b, pb)] {
        let c = frame
            .scene_pixel([f64::from(p.at[0]), f64::from(p.at[1])])
            .expect("герой в кадре");
        assert!(
            c[2] > c[0] + 40 && c[2] > c[1],
            "герой не там: {c:?} в {p:?}"
        );
    }
    // Тень: точка за героем (от лампы) темнее, пока он стоит, и светла в прыжке.
    let behind = [-0.6, -0.64];
    let lit_side = [0.45, -0.64];
    let (sa, sb) = (
        brightness(a.scene_pixel(behind).unwrap()),
        brightness(b.scene_pixel(behind).unwrap()),
    );
    let (la, lb) = (
        brightness(a.scene_pixel(lit_side).unwrap()),
        brightness(b.scene_pixel(lit_side).unwrap()),
    );
    eprintln!("за героем: стоит {sa}, прыжок {sb}; у лампы: {la}, {lb}");
    assert!(
        sa * 3 < sb * 2,
        "нет тени от героя: стоит {sa}, в прыжке {sb}"
    );
    assert!(
        la.abs_diff(lb) * 10 < la.max(lb),
        "у лампы свет поменялся: {la} / {lb}"
    );
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn hero_tours_the_example_level() {
    use reyvec_app::{HeadlessPlay, PlayInput};
    use reyvec_scene::game::Action;
    // Каждый кадр — свой сценарий: где герой встаёт и что нажимают.
    struct Shot {
        name: &'static str,
        time: f32,
        play: HeadlessPlay,
    }
    let at = |x: f64, y: f64, inputs: Vec<PlayInput>| HeadlessPlay {
        start: Some([x, y]),
        inputs,
        ..Default::default()
    };
    let shots = [
        // У лампы над столом, бежит вправо.
        Shot {
            name: "tour-room-run",
            time: 1.4,
            play: at(
                -0.6,
                -0.5,
                PlayInput::hold(Action::Right, 1.0, 2.0).to_vec(),
            ),
        },
        // На подъёмнике в коридоре (везёт вверх-вниз).
        Shot {
            name: "tour-lift",
            time: 3.0,
            play: at(9.6, 0.6, Vec::new()),
        },
        // Висит на краю высокого блока у уступов.
        Shot {
            name: "tour-ledge-hang",
            time: 2.0,
            play: at(
                13.75,
                -0.6,
                [
                    PlayInput::hold(Action::Right, 0.5, 3.0),
                    PlayInput::hold(Action::Jump, 1.0, 1.5),
                ]
                .concat(),
            ),
        },
        // Бежит вниз по длинному склону.
        Shot {
            name: "tour-slope-run",
            time: 1.35,
            play: at(14.2, 0.2, PlayInput::hold(Action::Right, 1.0, 3.0).to_vec()),
        },
    ];
    for shot in &shots {
        let frame = play_frame(None, shot.time, &shot.play, Msaa::X4);
        show_frame(shot.name, &frame);
        let p = frame.player.expect("герой");
        eprintln!("{}: {p:?}, камера {:?}", shot.name, frame.camera);
        let c = frame
            .scene_pixel([f64::from(p.at[0]), f64::from(p.at[1]) - 0.03])
            .expect("герой в кадре");
        // Плащ героя синий (#3b5f8a): синий канал больше красного.
        assert!(c[2] > c[0], "{}: герой не там: {c:?} в {p:?}", shot.name);
        match shot.name {
            "tour-lift" => assert!(
                p.on_ground && p.at[1] > -0.55 && (p.at[0] - 9.6).abs() < 0.2,
                "не на подъёмнике: {p:?}"
            ),
            "tour-ledge-hang" => assert!(p.hanging, "не висит: {p:?}"),
            "tour-slope-run" => assert!(p.on_ground && p.vel[0] > 1.0, "не бежит: {p:?}"),
            _ => assert!(p.on_ground, "{p:?}"),
        }
    }
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn hero_strikes_the_dummy_it_flashes_and_flies() {
    use reyvec_app::{HeadlessPlay, PlayInput};
    use reyvec_scene::game::Action;
    // Герой справа от манекена 1 (между ними 0,16: взмах не заслоняет манекен, а
    // хитбокс достаёт), поворачивается к нему и бьёт в 1,0 с.
    let play = HeadlessPlay {
        start: Some([-0.59, -0.64]),
        inputs: [
            PlayInput::hold(Action::Left, 0.5, 0.52),
            PlayInput::hold(Action::Attack, 1.0, 1.05),
        ]
        .concat(),
        ..Default::default()
    };
    let before = play_frame(None, 0.95, &play, Msaa::X4);
    let hit = play_frame(None, 1.09, &play, Msaa::X4);
    let after = play_frame(None, 1.5, &play, Msaa::X4);
    show_frame("strike-before", &before);
    show_frame("strike-hit", &hit);
    show_frame("strike-after", &after);
    let p = hit.player.expect("герой");
    eprintln!("удар: {p:?}, камера {:?}", hit.camera);
    assert!(p.striking && p.hits == 1, "в кадре удар и попадание: {p:?}");
    // Мешок манекена (середина в -0,9; -0,63) во вспышке намного ярче.
    let sack = [-0.9, -0.63];
    let (a, b) = (
        before.scene_pixel(sack).expect("манекен в кадре"),
        hit.scene_pixel(sack).expect("манекен в кадре"),
    );
    eprintln!("мешок до {a:?}, во вспышке {b:?}");
    assert!(
        brightness(b) > brightness(a) * 3 / 2,
        "нет вспышки: {a:?} → {b:?}"
    );
    // Взмах (светлый серп от 0,15 до 0,21 перед серединой героя) слева от героя.
    let slash = [f64::from(p.at[0]) - 0.18, f64::from(p.at[1])];
    let (a, b) = (
        before.scene_pixel(slash).expect("в кадре"),
        hit.scene_pixel(slash).expect("в кадре"),
    );
    eprintln!("место взмаха до {a:?}, в ударе {b:?}");
    assert!(
        brightness(b) > brightness(a) + 120,
        "взмаха не видно: {a:?} → {b:?}"
    );
    // Через 0,4 с манекен отлетел: на его месте снова фон.
    let c = after.scene_pixel(sack).expect("в кадре");
    eprintln!("место манекена после отлёта {c:?}");
    let p = after.player.expect("герой");
    assert!(!p.striking && p.hits == 1, "{p:?}");
}

/// Сколько пикселей кадра в прямоугольнике сцены `lo`–`hi` подходят под `pick`.
fn count_pixels(
    frame: &reyvec_app::HeadlessImage,
    lo: [f64; 2],
    hi: [f64; 2],
    pick: impl Fn([u8; 4]) -> bool,
) -> usize {
    // Шаг — пиксель кадра (высота кадра 2 / zoom единиц сцены).
    let step = 2.0 / (frame.camera.1 * f64::from(frame.image.height));
    let mut n = 0;
    let mut y = lo[1];
    while y < hi[1] {
        let mut x = lo[0];
        while x < hi[0] {
            if frame.scene_pixel([x, y]).is_some_and(&pick) {
                n += 1;
            }
            x += step;
        }
        y += step;
    }
    n
}

/// Искра: яркий тёплый пиксель (оранжевый или почти белый с жёлтым), не серый.
fn spark(c: [u8; 4]) -> bool {
    c[0] > 200 && u32::from(c[0]) > u32::from(c[2]) + 70
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn hit_throws_sparks_that_fly_fall_and_fade() {
    use reyvec_app::{HeadlessPlay, PlayInput};
    use reyvec_scene::game::Action;
    // Тот же удар, что в hero_strikes_the_dummy_it_flashes_and_flies.
    let play = HeadlessPlay {
        start: Some([-0.59, -0.64]),
        inputs: [
            PlayInput::hold(Action::Left, 0.5, 0.52),
            PlayInput::hold(Action::Attack, 1.0, 1.05),
        ]
        .concat(),
        ..Default::default()
    };
    let before = play_frame(None, 0.95, &play, Msaa::X4);
    let fly = play_frame(None, 1.16, &play, Msaa::X4);
    let late = play_frame(None, 1.3, &play, Msaa::X4);
    let gone = play_frame(None, 2.0, &play, Msaa::X4);
    show_frame("sparks-fly", &fly);
    show_frame("sparks-late", &late);
    eprintln!(
        "частицы: до {:?}, полёт {:?}, позже {:?}, через секунду {:?}",
        before.particles, fly.particles, late.particles, gone.particles
    );
    // Вокруг манекена и над полом: искры летят влево от места удара.
    let (lo, hi) = ([-1.5, -0.8], [-0.5, -0.1]);
    let count = |f: &reyvec_app::HeadlessImage| count_pixels(f, lo, hi, spark);
    let (a, b, c, d) = (count(&before), count(&fly), count(&late), count(&gone));
    eprintln!("пикселей искр: до {a}, полёт {b}, позже {c}, через секунду {d}");
    assert!(
        fly.particles.live >= 28,
        "искры в работе: {:?}",
        fly.particles
    );
    assert!(b > a + 20, "искр не видно: {a} → {b}");
    // Через секунду все искры погасли (пиксели сравнивать нельзя: манекен отлетел, а
    // тёплые пиксели есть и в самой комнате).
    assert_eq!(
        gone.particles.live, 0,
        "искры не гаснут: {:?}",
        gone.particles
    );
    eprintln!("тёплых пикселей через секунду {d}");
    assert_eq!(gone.particles.dropped, 0);
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn landing_kicks_up_dust() {
    use reyvec_app::HeadlessPlay;
    // Падает с высоты на пол комнаты (пол на -0,78).
    let play = HeadlessPlay {
        start: Some([0.4, 0.1]),
        ..Default::default()
    };
    let mut landed = None;
    for i in 1..=12 {
        let t = i as f32 * 0.05;
        let f = play_frame(None, t, &play, Msaa::X4);
        let p = f.player.expect("герой");
        if p.on_ground {
            landed = Some((t, f));
            break;
        }
    }
    let (t, _) = landed.expect("приземлился за 0,6 с");
    let dust = play_frame(None, t + 0.12, &play, Msaa::X4);
    show_frame("landing-dust", &dust);
    eprintln!(
        "приземлился к {t} с, частицы через 0,12 с: {:?}",
        dust.particles
    );
    assert!(dust.particles.live >= 8, "пыли нет: {:?}", dust.particles);
    let settled = play_frame(None, t + 1.5, &play, Msaa::X4);
    assert_eq!(settled.particles.live, 0, "пыль осела и погасла");
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn fountain_of_particles_runs_on_the_gpu() {
    use reyvec_app::HeadlessPlay;
    // 0,3 с фонтана: около 15 000 частиц (всё за один кадр, меньше предела за кадр).
    let play = HeadlessPlay {
        fountain: true,
        ..Default::default()
    };
    let frame = play_frame(None, 0.3, &play, Msaa::X4);
    show_frame("fountain", &frame);
    eprintln!(
        "фонтан: {:?}, время видеокарты {:?}",
        frame.particles, frame.gpu_timings
    );
    assert!(frame.particles.fountain);
    assert!(
        (14_000..=16_384).contains(&frame.particles.live),
        "{:?}",
        frame.particles
    );
    assert_eq!(frame.particles.dropped, 0);
    // Брызги фонтана видны над героем.
    let p = frame.player.expect("герой");
    let at = [f64::from(p.at[0]), f64::from(p.at[1])];
    let lit = count_pixels(
        &frame,
        [at[0] - 0.6, at[1] + 0.1],
        [at[0] + 0.6, at[1] + 1.0],
        |c| brightness(c) > 300,
    );
    eprintln!("ярких пикселей фонтана: {lit}");
    assert!(lit > 200, "фонтана не видно: {lit}");
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn hero_moves_in_poses_run_jump_fall_strike() {
    use reyvec_app::{HeadlessPlay, PlayInput};
    use reyvec_scene::game::Action;
    // Герой из костей (6.8) в примере: кадры каждой позы видно в логе CI.
    let at = |inputs: Vec<PlayInput>| HeadlessPlay {
        start: Some([0.4, -0.64]),
        inputs,
        ..Default::default()
    };
    let shots: [(&str, f32, HeadlessPlay, &str); 5] = [
        ("pose-idle", 1.0, at(Vec::new()), "стоит"),
        (
            "pose-run",
            1.3,
            at(PlayInput::hold(Action::Right, 0.8, 2.0).to_vec()),
            "бежит",
        ),
        (
            "pose-jump",
            1.12,
            at(PlayInput::hold(Action::Jump, 1.0, 1.4).to_vec()),
            "прыгает",
        ),
        (
            "pose-fall",
            1.5,
            at(PlayInput::hold(Action::Jump, 1.0, 1.4).to_vec()),
            "падает",
        ),
        (
            "pose-strike",
            1.08,
            at(PlayInput::hold(Action::Attack, 1.0, 1.05).to_vec()),
            "бьёт",
        ),
    ];
    for (name, time, play, anim) in &shots {
        let frame = play_frame(None, *time, play, Msaa::X4);
        show_frame(name, &frame);
        let p = frame.player.expect("герой");
        eprintln!("{name}: {p:?}");
        assert_eq!(p.anim, *anim, "{name}: {p:?}");
    }
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn scripted_beetle_chases_and_coin_sparkles() {
    use reyvec_app::{HeadlessPlay, PlayInput};
    use reyvec_scene::game::Action;
    // Скрипты примера (6.9): жук в начале коридора бежит к герою; монета в комнате
    // исчезает с блёстками, когда герой её касается.
    let beetle = HeadlessPlay {
        start: Some([2.6, -0.64]),
        ..Default::default()
    };
    let frame = play_frame(None, 1.2, &beetle, Msaa::X4);
    show_frame("script-beetle", &frame);
    let coin = HeadlessPlay {
        start: Some([0.9, -0.64]),
        inputs: PlayInput::hold(Action::Right, 0.3, 1.0).to_vec(),
        ..Default::default()
    };
    let frame = play_frame(None, 0.55, &coin, Msaa::X4);
    show_frame("script-coin", &frame);
    let p = frame.player.expect("герой");
    eprintln!("монета: {p:?}, частиц {:?}", frame.particles);
    assert!(p.at[0] > 1.1, "герой добежал до монеты: {p:?}");
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn physics_crate_flies_and_dummy_breaks_into_debris() {
    use reyvec_app::{HeadlessPlay, PlayInput};
    use reyvec_scene::game::Action;
    // Физика (6.10): удар по ящику в начале коридора — ящик отлетает и кувыркается.
    let crate_hit = HeadlessPlay {
        start: Some([2.5, -0.64]),
        inputs: PlayInput::hold(Action::Attack, 1.0, 1.05).to_vec(),
        ..Default::default()
    };
    for (name, time) in [("physics-crate-before", 0.95), ("physics-crate-flies", 1.3)] {
        let frame = play_frame(None, time, &crate_hit, Msaa::X4);
        show_frame(name, &frame);
        assert!(frame.player.is_some());
    }
    // Манекен у левой стены: герой бьёт и подходит за отброшенным, пятый удар
    // разбивает его — обломки цвета манекена летят и падают.
    let mut inputs = PlayInput::hold(Action::Left, 0.0, 0.05).to_vec();
    for k in 0..5 {
        let at = 1.0 + 0.45 * f64::from(k);
        inputs.extend(PlayInput::hold(Action::Attack, at, at + 0.05));
        inputs.extend(PlayInput::hold(Action::Left, at + 0.15, at + 0.35));
    }
    let beetle = HeadlessPlay {
        start: Some([-0.7, -0.64]),
        inputs,
        ..Default::default()
    };
    let frame = play_frame(None, 3.1, &beetle, Msaa::X4);
    show_frame("physics-debris", &frame);
    let p = frame.player.expect("герой");
    assert!(p.hits >= 5, "{p:?}");
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn space_scene_shows_sky_floor_and_the_ball_in_the_middle() {
    // Сцена «3D» (7.1): камера по умолчанию смотрит на шар, он в середине кадра; вверху
    // небо, внизу пол с сеткой; без ошибок валидации.
    let (width, height) = (640u32, 360u32);
    let scene = HeadlessScene {
        width,
        height,
        time: 0.0,
        shader_error: false,
        scene: DemoScene::Space,
        msaa: Msaa::Off,
        perceptual_edges: false,
        fill: FillMode::Triangles,
        zoom: false,
        files: Default::default(),
        cascades: false,
    };
    let options = GpuOptions {
        validation: true,
        gpu: None,
    };
    let frame = render_headless(&options, &scene).expect("кадр без окна");
    assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
    let pixel = |x: u32, y: u32| {
        let i = ((y * width + x) * 4) as usize;
        [
            i32::from(frame.image.rgba[i]),
            i32::from(frame.image.rgba[i + 1]),
            i32::from(frame.image.rgba[i + 2]),
        ]
    };
    let ball = pixel(width / 2, height / 2);
    let sky = pixel(width / 2, 4);
    let floor = pixel(width / 2, height - 4);
    eprintln!("шар {ball:?}, небо {sky:?}, пол {floor:?}");
    assert!(
        ball[0] > ball[1] && ball[1] > ball[2],
        "шар не оранжевый: {ball:?}"
    );
    assert!(sky[2] > sky[0], "небо не синее: {sky:?}");
    assert!(floor != sky && floor != ball, "пол не виден: {floor:?}");
}
