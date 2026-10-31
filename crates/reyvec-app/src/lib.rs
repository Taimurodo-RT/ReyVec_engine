//! `reyvec-app`: верхний слой движка ReyVec.
//!
//! Сейчас (этап 0) крейт знает только версию движка и умеет включать логи.
//! На этапе 1 здесь появятся окно, цикл кадра и ввод.

/// Название движка.
pub const ENGINE_NAME: &str = "ReyVec";

/// Версия движка из `Cargo.toml`.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Строка вида `ReyVec 0.1.0` для заголовков окон и логов.
pub fn engine_title() -> String {
    format!("{ENGINE_NAME} {ENGINE_VERSION}")
}

/// Включает логи в консоль.
///
/// Уровень по умолчанию `info`. Его можно поменять переменной окружения
/// `RUST_LOG`, например `RUST_LOG=debug`. Повторный вызов ничего не ломает.
pub fn init_logging() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_contains_name_and_version() {
        let title = engine_title();
        assert!(title.starts_with("ReyVec "));
        assert!(title.ends_with(ENGINE_VERSION));
    }

    #[test]
    fn init_logging_twice_is_safe() {
        init_logging();
        init_logging();
    }
}
