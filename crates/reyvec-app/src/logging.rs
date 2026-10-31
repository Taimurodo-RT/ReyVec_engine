//! Логи: в консоль и в файл `reyvec.log` рядом с программой.

use std::fs::File;
use std::path::PathBuf;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Держит запись лога в файл. Пока он жив, строки дописываются в файл;
/// при завершении программы остаток сбрасывается на диск.
pub struct LogGuard {
    _file: Option<tracing_appender::non_blocking::WorkerGuard>,
    /// Куда пишется лог, если файл удалось создать.
    pub path: Option<PathBuf>,
}

/// Включает логи в консоль и в файл `reyvec.log` рядом с программой.
///
/// Уровень по умолчанию `info`. Его можно поменять переменной окружения
/// `RUST_LOG`, например `RUST_LOG=debug`. Файл перезаписывается при каждом запуске,
/// его удобно прислать, если что-то пошло не так. Повторный вызов ничего не ломает.
pub fn init_logging() -> LogGuard {
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let console = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_filter(filter());

    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("reyvec.log")));
    let file = path.as_ref().and_then(|p| File::create(p).ok());

    match file {
        Some(file) => {
            let (writer, guard) = tracing_appender::non_blocking(file);
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer)
                .with_filter(filter());
            let _ = tracing_subscriber::registry()
                .with(console)
                .with(file_layer)
                .try_init();
            LogGuard {
                _file: Some(guard),
                path,
            }
        }
        None => {
            let _ = tracing_subscriber::registry().with(console).try_init();
            LogGuard {
                _file: None,
                path: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn init_logging_twice_is_safe() {
        let _a = super::init_logging();
        let _b = super::init_logging();
    }
}
