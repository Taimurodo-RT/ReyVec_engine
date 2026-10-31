//! Песочница ReyVec: программа, которую запускают для проверки каждого этапа.
//!
//! Этап 1: окно с вращающимся цветным треугольником, FPS в заголовке.
//!
//! Флаги:
//! - `--list-gpus`: показать видеокарты и выйти;
//! - `--gpu <номер или часть названия>`: выбрать видеокарту, например `--gpu radeon`;
//! - `--no-vsync`: без вертикальной синхронизации (показывает предельный FPS);
//! - `--validation`: включить слои валидации Vulkan (нужен Vulkan SDK);
//! - `--frames <N>`: закрыть окно после N кадров;
//! - `--seconds <S>`: закрыть окно через S секунд;
//! - `--version`: показать версию.

use std::process::ExitCode;

const GIT_COMMIT: &str = env!("REYVEC_GIT_COMMIT");

const HELP: &str = "\
Песочница ReyVec

Запуск: reyvec-sandbox [флаги]

  --list-gpus            показать видеокарты и выйти
  --gpu <номер|имя>      выбрать видеокарту, например --gpu radeon или --gpu 1
  --no-vsync             без вертикальной синхронизации
  --validation           слои валидации Vulkan (нужен Vulkan SDK)
  --frames <N>           закрыть окно после N кадров
  --seconds <S>          закрыть окно через S секунд
  --version, -V          версия
  --help, -h             эта справка

В окне: Esc закрывает окно.";

#[derive(Debug, Default, PartialEq)]
struct Args {
    help: bool,
    version: bool,
    list_gpus: bool,
    gpu: Option<String>,
    no_vsync: bool,
    validation: bool,
    frames: Option<u64>,
    seconds: Option<f64>,
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
            other => return Err(format!("неизвестный флаг «{other}». Справка: --help")),
        }
    }
    Ok(out)
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
    let config = reyvec_app::AppConfig {
        title: reyvec_app::engine_title(),
        gpu: reyvec_app::GpuOptions {
            validation,
            gpu: args.gpu,
        },
        vsync: !args.no_vsync,
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
        let a = parse("--gpu radeon --no-vsync --validation --frames 120 --seconds 2.5").unwrap();
        assert_eq!(a.seconds, Some(2.5));
        assert_eq!(a.gpu.as_deref(), Some("radeon"));
        assert!(a.no_vsync && a.validation);
        assert_eq!(a.frames, Some(120));
    }

    #[test]
    fn bad_args() {
        assert!(parse("--gpu").is_err());
        assert!(parse("--frames ten").is_err());
        assert!(parse("--seconds -1").is_err());
        assert!(parse("--fullscreen").is_err());
    }
}
