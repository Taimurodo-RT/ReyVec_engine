//! Песочница ReyVec: программа, которую запускают для проверки каждого этапа.
//!
//! Окно с тестовой сценой: треугольник и спрайты с текстурами, FPS в заголовке,
//! панель счётчиков (F1 прячет).
//! Шейдеры можно править на ходу: после сохранения файла картинка обновится.
//!
//! Флаги:
//! - `--list-gpus`: показать видеокарты и выйти;
//! - `--gpu <номер или часть названия>`: выбрать видеокарту, например `--gpu radeon`;
//! - `--no-vsync`: без вертикальной синхронизации (показывает предельный FPS);
//! - `--no-hot-reload`: не следить за файлами шейдеров;
//! - `--no-tracy`: не ждать подключения программы Tracy;
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
  --validation           слои валидации Vulkan (нужен Vulkan SDK)
  --frames <N>           закрыть окно после N кадров
  --seconds <S>          закрыть окно через S секунд
  --snapshot <файл.png>  без окна: нарисовать кадр и сохранить в PNG
  --time <S>             для --snapshot: момент сцены в секундах (по умолчанию 0)
  --size <ШxВ>           для --snapshot: размер картинки (по умолчанию 1280x720)
  --version, -V          версия
  --help, -h             эта справка

В окне: Esc закрывает окно, F1 прячет и показывает панель счётчиков,
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
fn snapshot_headless(path: &str, args: &Args, validation: bool) -> ExitCode {
    let (width, height) = args.size.unwrap_or((1280, 720));
    let scene = reyvec_app::HeadlessScene {
        width,
        height,
        time: args.time.unwrap_or(0.0),
        shader_error: false,
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
    let args = match parse_args(std::env::args().skip(1)) {
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
    if let Some(path) = &args.snapshot {
        return snapshot_headless(path, &args, validation);
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
    fn bad_args() {
        assert!(parse("--gpu").is_err());
        assert!(parse("--frames ten").is_err());
        assert!(parse("--seconds -1").is_err());
        assert!(parse("--fullscreen").is_err());
    }
}
