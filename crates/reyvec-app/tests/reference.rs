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
    DemoScene, FillMode, GpuOptions, HeadlessScene, Msaa, render_headless, validation_error_count,
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
                problems.push(format!(
                    "{}: нет эталона ({e}). Снимите его с REYVEC_UPDATE_REFERENCES=1",
                    case.name
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
