//! Сверка чтения SVG с эталонным рисовальщиком resvg: каждый файл из `assets/svg`
//! рисуется песочницей без окна (сцена «SVG», свой рисунок по центру) и resvg с тем же
//! размером и положением, после чего кадры сравниваются.
//!
//! Полного совпадения не ждём: сглаживание краёв у нас MSAA 4×, а у resvg точное покрытие
//! пикселя, и полупрозрачное мы смешиваем в линейном свете, а resvg (как редакторы) в sRGB
//! (`docs/vector-bottlenecks.md`). Поэтому сравниваем с допуском: средняя разница каналов и
//! доля сильно отличающихся пикселей. Получившиеся кадры, картинка resvg и отличия
//! сохраняются в `target/svg-reference`, чтобы на них можно было посмотреть глазами.
//!
//! Нужен драйвер Vulkan: `cargo test -p reyvec-app --test svg_reference -- --include-ignored`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use resvg::{tiny_skia, usvg};
use reyvec_app::snapshot::{FrameImage, diff_image, save_png};
use reyvec_app::{
    DemoScene, FillMode, GpuOptions, HeadlessScene, Msaa, render_headless, validation_error_count,
};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
/// Доля кадра, которую занимает рисунок (как в песочнице).
const FIT: f32 = 0.95;
/// Пиксель «сильно отличается», если какой-то канал разошёлся больше чем на столько.
/// Для каждого файла ниже: допустимая средняя разница каналов (из 255) и допустимая доля
/// сильно отличающихся пикселей.
const STRONG: u8 = 48;

/// Файлы и допуски (замер на lavapipe примерно вдвое меньше допуска). Где есть
/// полупрозрачное, разница больше: там сказывается смешивание в линейном свете. В
/// «gradients» это светлое пятно, которое у нас тает медленнее, в «groups» полупрозрачные
/// круги и квадраты, у нас они светлее.
const FILES: &[(&str, f64, f64)] = &[
    ("shapes", 0.6, 0.005),
    ("gradients", 3.0, 0.02),
    ("groups", 8.0, 0.005),
    ("clip-mask", 3.0, 0.002),
    ("transforms", 0.3, 0.002),
    ("illustration", 1.5, 0.001),
];

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/svg")
}

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .expect("CARGO_TARGET_TMPDIR внутри target")
        .join("svg-reference")
}

/// Линейный канал в sRGB-байт.
fn srgb8(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// Рисует SVG через resvg на фоне кадра песочницы, вписав его по центру как песочница.
fn render_resvg(data: &[u8]) -> FrameImage {
    let tree = usvg::Tree::from_data(data, &usvg::Options::default()).expect("SVG читается");
    let size = tree.size();
    let (w, h) = (WIDTH as f32, HEIGHT as f32);
    let f = FIT * (h / size.height()).min(w / size.width());
    let transform = tiny_skia::Transform::from_scale(f, f)
        .post_translate((w - size.width() * f) / 2.0, (h - size.height() * f) / 2.0);
    let mut pixmap = tiny_skia::Pixmap::new(WIDTH, HEIGHT).expect("размер картинки");
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // resvg отдаёт цвет, уже умноженный на прозрачность, в sRGB: кладём его на фон там же.
    let [r, g, b, _] = reyvec_app::CLEAR_COLOR;
    let bg = [srgb8(r), srgb8(g), srgb8(b)];
    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for px in pixmap.data().chunks_exact(4) {
        let a = f32::from(px[3]) / 255.0;
        for c in 0..3 {
            let v = f32::from(px[c]) + f32::from(bg[c]) * (1.0 - a);
            rgba.push(v.round().clamp(0.0, 255.0) as u8);
        }
        rgba.push(255);
    }
    FrameImage {
        width: WIDTH,
        height: HEIGHT,
        rgba,
    }
}

/// Средняя разница каналов RGB и доля пикселей, где канал разошёлся больше [`STRONG`].
fn difference(a: &FrameImage, b: &FrameImage) -> (f64, f64) {
    let mut sum = 0u64;
    let mut strong = 0usize;
    for (pa, pb) in a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)) {
        let d: Vec<u8> = (0..3).map(|c| pa[c].abs_diff(pb[c])).collect();
        sum += d.iter().map(|&x| u64::from(x)).sum::<u64>();
        if d.iter().any(|&x| x > STRONG) {
            strong += 1;
        }
    }
    let pixels = a.rgba.len() / 4;
    (
        sum as f64 / (pixels * 3) as f64,
        strong as f64 / pixels as f64,
    )
}

#[test]
#[ignore = "нужен драйвер Vulkan"]
fn svg_matches_resvg() {
    let options = GpuOptions {
        validation: true,
        gpu: None,
    };
    let mut problems = Vec::new();
    for &(name, max_mean, max_strong) in FILES {
        let path = assets().join(format!("{name}.svg"));
        let data: Arc<[u8]> = std::fs::read(&path).expect("тестовый SVG").into();
        let expected = render_resvg(&data);
        let scene = HeadlessScene {
            width: WIDTH,
            height: HEIGHT,
            time: 0.0,
            shader_error: false,
            scene: DemoScene::Svg,
            msaa: Msaa::X4,
            perceptual_edges: false,
            fill: FillMode::PerShape,
            zoom: false,
            files: reyvec_app::UserFiles {
                svg: Some(data),
                motif: None,
            },
        };
        let frame = render_headless(&options, &scene)
            .expect("кадр без окна")
            .image;
        let dir = out_dir();
        save_png(&dir.join(format!("{name}-reyvec.png")), &frame).expect("запись кадра");
        save_png(&dir.join(format!("{name}-resvg.png")), &expected).expect("запись resvg");
        save_png(
            &dir.join(format!("{name}-diff.png")),
            &diff_image(&frame, &expected, STRONG),
        )
        .expect("запись отличий");
        let (mean, strong) = difference(&frame, &expected);
        eprintln!(
            "{name}: средняя разница {mean:.2}, сильно отличаются {:.2}% пикселей",
            strong * 100.0
        );
        if mean > max_mean || strong > max_strong {
            problems.push(format!(
                "{name}: средняя разница {mean:.2} (можно {}), сильно отличаются {:.2}% (можно {:.2}%). Картинки в {}",
                max_mean,
                strong * 100.0,
                max_strong * 100.0,
                dir.display()
            ));
        }
    }
    assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
