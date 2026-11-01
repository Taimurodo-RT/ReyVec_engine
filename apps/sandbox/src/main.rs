//! Песочница ReyVec: программа, которую запускают для проверки каждого этапа.
//!
//! Окно с тестовыми сценами (клавиши 0–9): треугольник и спрайты с текстурами
//! (этап 2), векторные фигуры, перетекание, слои, маски, SVG, фигуры Motif, поток новых фигур,
//! текст и стресс-тест на 10 000 фигур (этап 3). Камера векторных сцен: колесо мыши
//! приближает, перетаскивание левой кнопкой двигает, R возвращает. FPS в заголовке, панель счётчиков (F1 прячет).
//! Шейдеры можно править на ходу: после сохранения файла картинка обновится.
//!
//! Флаги:
//! - `--list-gpus`: показать видеокарты и выйти;
//! - `--gpu <номер или часть названия>`: выбрать видеокарту, например `--gpu radeon`;
//! - `--no-vsync`: без вертикальной синхронизации (показывает предельный FPS);
//! - `--no-hot-reload`: не следить за файлами шейдеров;
//! - `--no-tracy`: не ждать подключения программы Tracy;
//! - `--scene gpu|vector|morph|layers|masks|svg|motif|stream|text|stress|stress-morph`: с какой сцены начать (по умолчанию `vector`);
//! - `--msaa off|2|4|8`: сглаживание сцены (по умолчанию 2 на встроенной видеокарте, 4 на остальных);
//! - `--editor-edges`: сводить края MSAA «как в редакторах» (своим проходом, дороже);
//! - `--fill shape|triangles|stencil`: как рисовать заливки векторных фигур (по умолчанию
//!   у каждой фигуры свой способ, остальные варианты для сравнения);
//! - `--svg <файл>`: свой рисунок SVG в сцене «SVG»;
//! - `--motif <файл>`: свои перетекающие фигуры от Motif (`*.reyvec.json`) в сцене «Motif»;
//! - `--zoom`: лупа векторной сцены (увеличение в 8 раз у края сердца);
//! - `--validation`: включить слои валидации Vulkan (нужен Vulkan SDK);
//! - `--frames <N>`: закрыть окно после N кадров;
//! - `--seconds <S>`: закрыть окно через S секунд;
//! - `--snapshot <файл.png>`: нарисовать кадр без окна и сохранить его в файл
//!   (`--time <S>`: момент сцены, `--size <ШxВ>`: размер, по умолчанию 1280x720);
//! - `--version`: показать версию.

use std::process::ExitCode;

const GIT_COMMIT: &str = env!("REYVEC_GIT_COMMIT");

const HELP: &str = "\
Песочница ReyVec

Запуск: reyvec-sandbox [флаги]

  --list-gpus            показать видеокарты и выйти
  --gpu <номер|имя>      выбрать видеокарту, например --gpu radeon или --gpu 1
  --no-vsync             без вертикальной синхронизации
  --no-hot-reload        не перезагружать шейдеры после сохранения файлов
  --no-tracy             не включать замеры для программы Tracy
  --scene <имя>          с какой сцены начать: gpu (этап 2), vector (векторы, по умолчанию),
                         morph (перетекание), layers (слои), masks (маски), svg (рисунки SVG)
                         motif (перетекающие фигуры из Motif), stream (новые фигуры
                         во время работы), text (текст), stress (10 000 движущихся
                         фигур) или stress-morph (10 000 перетекающих фигур)
  --svg <файл.svg>       показать свой рисунок SVG в сцене «SVG» (клавиша 6, с неё и начнёт)
  --motif <файл.reyvec.json>  показать свои перетекающие фигуры от Motif в сцене «Motif»
                         (клавиша 7, с неё и начнёт)
  --msaa <off|2|4|8>     сглаживание краёв (по умолчанию 2 на встроенной, 4 на остальных)
  --editor-edges         края «как в редакторах» (дороже, особенно на встроенной видеокарте)
  --fill <shape|triangles|stencil>  заливки векторов: у каждой фигуры свой способ (по умолчанию),
                         все треугольниками или все трафаретом
  --zoom                 лупа векторной сцены: увеличение в 8 раз у края сердца
  --validation           слои валидации Vulkan (нужен Vulkan SDK)
  --frames <N>           закрыть окно после N кадров
  --seconds <S>          закрыть окно через S секунд
  --snapshot <файл.png>  без окна: нарисовать кадр и сохранить в PNG
  --time <S>             для --snapshot: момент сцены в секундах (по умолчанию 0)
  --size <ШxВ>           для --snapshot: размер картинки (по умолчанию 1280x720)
  --version, -V          версия
  --help, -h             эта справка

В окне: Esc закрывает окно, 1–8 переключают сцену, M переключает сглаживание, G вид краёв, F1 прячет и показывает панель счётчиков,
F12 сохраняет снимок кадра в папку snapshots рядом с программой.";

#[derive(Debug, Default, PartialEq)]
struct Args {
    help: bool,
    version: bool,
    list_gpus: bool,
    gpu: Option<String>,
    no_vsync: bool,
    no_hot_reload: bool,
    no_tracy: bool,
    validation: bool,
    frames: Option<u64>,
    seconds: Option<f64>,
    snapshot: Option<String>,
    time: Option<f32>,
    size: Option<(u32, u32)>,
    scene: Option<reyvec_app::DemoScene>,
    msaa: Option<reyvec_app::Msaa>,
    editor_edges: bool,
    fill: Option<reyvec_app::FillMode>,
    zoom: bool,
    svg: Option<String>,
    motif: Option<String>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => out.help = true,
            "--version" | "-V" => out.version = true,
            "--list-gpus" => out.list_gpus = true,
            "--no-vsync" => out.no_vsync = true,
            "--no-hot-reload" => out.no_hot_reload = true,
            "--no-tracy" => out.no_tracy = true,
            "--validation" => out.validation = true,
            "--editor-edges" => out.editor_edges = true,
            "--gpu" => {
                out.gpu = Some(it.next().ok_or("после --gpu нужен номер или название")?);
            }
            "--frames" => {
                let n = it.next().ok_or("после --frames нужно число")?;
                out.frames = Some(n.parse().map_err(|_| format!("--frames: «{n}» не число"))?);
            }
            "--seconds" => {
                let n = it.next().ok_or("после --seconds нужно число")?;
                out.seconds = Some(
                    n.parse()
                        .ok()
                        .filter(|s: &f64| s.is_finite() && *s >= 0.0)
                        .ok_or_else(|| format!("--seconds: «{n}» не число"))?,
                );
            }
            "--scene" => {
                let n = it.next().ok_or(
                    "после --scene нужно gpu, vector, morph, layers, masks, svg, motif, stream, text, stress или stress-morph",
                )?;
                out.scene = Some(reyvec_app::DemoScene::from_name(&n).ok_or_else(|| {
                    format!(
                        "--scene: «{n}», а нужно gpu, vector, morph, layers, masks, svg, motif, stream, text, stress или stress-morph"
                    )
                })?);
            }
            "--msaa" => {
                let n = it.next().ok_or("после --msaa нужно off, 2, 4 или 8")?;
                out.msaa = Some(
                    reyvec_app::Msaa::from_name(&n)
                        .ok_or_else(|| format!("--msaa: «{n}», а нужно off, 2, 4 или 8"))?,
                );
            }
            "--fill" => {
                let n = it
                    .next()
                    .ok_or("после --fill нужно shape, triangles или stencil")?;
                out.fill = Some(reyvec_app::FillMode::from_name(&n).ok_or_else(|| {
                    format!("--fill: «{n}», а нужно shape, triangles или stencil")
                })?);
            }
            "--zoom" => out.zoom = true,
            "--svg" => {
                out.svg = Some(it.next().ok_or("после --svg нужен путь к файлу SVG")?);
            }
            "--motif" => {
                out.motif = Some(
                    it.next()
                        .ok_or("после --motif нужен путь к файлу *.reyvec.json")?,
                );
            }
            "--snapshot" => {
                out.snapshot = Some(it.next().ok_or("после --snapshot нужно имя файла")?);
            }
            "--time" => {
                let n = it.next().ok_or("после --time нужно число")?;
                out.time = Some(
                    n.parse()
                        .ok()
                        .filter(|t: &f32| t.is_finite() && *t >= 0.0)
                        .ok_or_else(|| format!("--time: «{n}» не число"))?,
                );
            }
            "--size" => {
                let n = it
                    .next()
                    .ok_or("после --size нужен размер, например 1280x720")?;
                out.size =
                    Some(parse_size(&n).ok_or_else(|| {
                        format!("--size: «{n}» не размер (нужно вроде 1280x720)")
                    })?);
            }
            other => return Err(format!("неизвестный флаг «{other}». Справка: --help")),
        }
    }
    Ok(out)
}

/// «1280x720» (или с русской «х», или «×») в ширину и высоту, от 1 до 8192.
fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X', 'х', 'Х', '×'])?;
    let ok = |v: u32| (1..=8192).contains(&v).then_some(v);
    Some((ok(w.trim().parse().ok()?)?, ok(h.trim().parse().ok()?)?))
}

/// Рисует кадр без окна и сохраняет его в `path`.
fn snapshot_headless(
    path: &str,
    args: &Args,
    validation: bool,
    files: reyvec_app::UserFiles,
) -> ExitCode {
    let (width, height) = args.size.unwrap_or((1280, 720));
    let scene = reyvec_app::HeadlessScene {
        width,
        height,
        time: args.time.unwrap_or(0.0),
        shader_error: false,
        scene: args.scene.unwrap_or_default(),
        msaa: args.msaa.unwrap_or_default(),
        perceptual_edges: args.editor_edges,
        fill: args.fill.unwrap_or_default(),
        zoom: args.zoom,
        files,
    };
    let options = reyvec_app::GpuOptions {
        validation,
        gpu: args.gpu.clone(),
    };
    let image = match reyvec_app::render_headless(&options, &scene) {
        Ok(img) => img,
        Err(e) => {
            tracing::error!("Не удалось нарисовать кадр без окна: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Относительный путь считается от текущей папки; в логе пишем полный, чтобы файл было легко найти.
    let path = std::path::absolute(path).unwrap_or_else(|_| path.into());
    if let Err(e) = reyvec_app::snapshot::save_png(&path, &image.image) {
        tracing::error!("Снимок не сохранён: {e}");
        return ExitCode::FAILURE;
    }
    tracing::info!(
        "Кадр {width}×{height} (момент {:.2} с) нарисован без окна на «{}» и сохранён: {}",
        scene.time,
        image.adapter.name,
        path.display()
    );
    if let Some(t) = &image.gpu_timings {
        tracing::info!("Видеокарта на кадр: {}", t.describe());
    }
    let errors = reyvec_app::validation_error_count();
    if errors > 0 {
        tracing::error!("Слои валидации нашли ошибок: {errors}. Пришлите reyvec.log");
        return ExitCode::FAILURE;
    }
    if validation {
        tracing::info!("Ошибок валидации Vulkan: 0");
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let mut args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            return ExitCode::from(2);
        }
    };
    if args.help {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.version {
        println!("{} ({GIT_COMMIT})", reyvec_app::engine_title());
        return ExitCode::SUCCESS;
    }

    let log = reyvec_app::init_logging();
    tracing::info!(
        "{} запущен, коммит {GIT_COMMIT}",
        reyvec_app::engine_title()
    );
    if let Some(path) = &log.path {
        tracing::info!("Лог пишется в {}", path.display());
    }

    if args.list_gpus {
        return match reyvec_app::list_adapters(args.validation) {
            Ok(list) => {
                println!("Видеокарты:");
                for a in &list {
                    println!("  {a}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                tracing::error!("{e}");
                ExitCode::FAILURE
            }
        };
    }

    // В отладочной сборке валидация включена всегда: так ошибки ловятся сразу.
    let validation = args.validation || cfg!(debug_assertions);
    let read = |what: &str, path: &Option<String>| match path {
        None => Ok(None),
        Some(path) => match std::fs::read(path) {
            Ok(data) => {
                tracing::info!("Свой {what}: {path}");
                Ok(Some(std::sync::Arc::<[u8]>::from(data)))
            }
            Err(e) => {
                tracing::error!("Не прочитать {what} «{path}»: {e}");
                Err(ExitCode::FAILURE)
            }
        },
    };
    let files = match (read("SVG", &args.svg), read("файл Motif", &args.motif)) {
        (Ok(svg), Ok(motif)) => reyvec_app::UserFiles { svg, motif },
        (Err(code), _) | (_, Err(code)) => return code,
    };
    // Со своим файлом песочница начинает с его сцены.
    if args.scene.is_none() {
        if files.motif.is_some() {
            args.scene = Some(reyvec_app::DemoScene::Motif);
        } else if files.svg.is_some() {
            args.scene = Some(reyvec_app::DemoScene::Svg);
        }
    }
    if let Some(path) = &args.snapshot {
        return snapshot_headless(path, &args, validation, files);
    }
    let config = reyvec_app::AppConfig {
        title: reyvec_app::engine_title(),
        gpu: reyvec_app::GpuOptions {
            validation,
            gpu: args.gpu,
        },
        vsync: !args.no_vsync,
        hot_reload: !args.no_hot_reload,
        tracy: !args.no_tracy,
        scene: args.scene.unwrap_or_default(),
        msaa: args.msaa,
        perceptual_edges: args.editor_edges,
        fill: args.fill.unwrap_or_default(),
        zoom: args.zoom,
        files,
        max_frames: args.frames,
        max_seconds: args.seconds,
    };

    let result = reyvec_app::run(config);
    let errors = reyvec_app::validation_error_count();
    match result {
        Ok(summary) => {
            tracing::info!(
                "Готово: {} кадров на «{}», в среднем {:.0} FPS",
                summary.frames,
                summary.adapter,
                summary.average_fps
            );
            if let Some(gpu) = &summary.gpu {
                tracing::info!("Видеокарта на кадр в среднем: {}", gpu.describe());
            }
            if errors > 0 {
                tracing::error!("Слои валидации нашли ошибок: {errors}. Пришлите reyvec.log");
                return ExitCode::FAILURE;
            }
            if validation {
                tracing::info!("Ошибок валидации Vulkan: 0");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!("Не удалось запустить: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Args, String> {
        parse_args(s.split_whitespace().map(String::from))
    }

    #[test]
    fn empty_args() {
        assert_eq!(parse(""), Ok(Args::default()));
    }

    #[test]
    fn all_flags() {
        let a = parse(
            "--gpu radeon --no-vsync --no-hot-reload --no-tracy --validation --frames 120 --seconds 2.5",
        )
        .unwrap();
        assert!(a.no_tracy);
        assert_eq!(a.seconds, Some(2.5));
        assert_eq!(a.gpu.as_deref(), Some("radeon"));
        assert!(a.no_vsync && a.no_hot_reload && a.validation);
        assert_eq!(a.frames, Some(120));
    }

    #[test]
    fn snapshot_args() {
        let a = parse("--snapshot out/кадр.png --time 2.5 --size 640x360").unwrap();
        assert_eq!(a.snapshot.as_deref(), Some("out/кадр.png"));
        assert_eq!(a.time, Some(2.5));
        assert_eq!(a.size, Some((640, 360)));
        assert_eq!(parse_size("1920х1080"), Some((1920, 1080)));
        assert_eq!(parse_size("0x10"), None);
        assert!(parse("--size big").is_err());
        assert!(parse("--time -1").is_err());
    }

    #[test]
    fn scene_arg() {
        assert_eq!(parse("").unwrap().scene, None);
        assert_eq!(
            parse("--scene gpu").unwrap().scene,
            Some(reyvec_app::DemoScene::Gpu)
        );
        assert_eq!(
            parse("--scene vector").unwrap().scene,
            Some(reyvec_app::DemoScene::Vector)
        );
        assert_eq!(
            parse("--scene morph").unwrap().scene,
            Some(reyvec_app::DemoScene::Morph)
        );
        assert_eq!(
            parse("--scene layers").unwrap().scene,
            Some(reyvec_app::DemoScene::Layers)
        );
        assert_eq!(
            parse("--scene masks").unwrap().scene,
            Some(reyvec_app::DemoScene::Masks)
        );
        assert_eq!(
            parse("--scene svg").unwrap().scene,
            Some(reyvec_app::DemoScene::Svg)
        );
        assert_eq!(parse("--svg a.svg").unwrap().svg.as_deref(), Some("a.svg"));
        assert!(parse("--svg").is_err());
        assert_eq!(
            parse("--scene motif").unwrap().scene,
            Some(reyvec_app::DemoScene::Motif)
        );
        assert_eq!(
            parse("--scene text").unwrap().scene,
            Some(reyvec_app::DemoScene::Text)
        );
        assert_eq!(
            parse("--scene stress-morph").unwrap().scene,
            Some(reyvec_app::DemoScene::StressMorph)
        );
        assert_eq!(
            parse("--motif a.reyvec.json").unwrap().motif.as_deref(),
            Some("a.reyvec.json")
        );
        assert!(parse("--motif").is_err());
        assert_eq!(
            parse("--scene stream").unwrap().scene,
            Some(reyvec_app::DemoScene::Stream)
        );
        assert_eq!(
            parse("--fill shape").unwrap().fill,
            Some(reyvec_app::FillMode::PerShape)
        );
        assert!(parse("--fill quads").is_err());
        assert!(parse("--scene 3d").is_err());
        assert_eq!(parse("--msaa 8").unwrap().msaa, Some(reyvec_app::Msaa::X8));
        assert_eq!(
            parse("--msaa off").unwrap().msaa,
            Some(reyvec_app::Msaa::Off)
        );
        assert_eq!(parse("--msaa 2").unwrap().msaa, Some(reyvec_app::Msaa::X2));
        assert!(parse("--msaa 3").is_err());
        assert!(parse("--scene").is_err());
    }

    #[test]
    fn bad_args() {
        assert!(parse("--gpu").is_err());
        assert!(parse("--frames ten").is_err());
        assert!(parse("--seconds -1").is_err());
        assert!(parse("--fullscreen").is_err());
    }
}
