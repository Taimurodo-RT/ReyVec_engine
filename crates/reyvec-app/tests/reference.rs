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
    ]
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
