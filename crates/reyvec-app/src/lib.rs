//! `reyvec-app`: верхний слой движка ReyVec.
//!
//! Этап 1: окно, цикл кадра и вывод первого кадра через `reyvec-gpu`.
//! Потом здесь появятся время с фиксированным шагом логики и ввод.

mod app;
mod logging;
mod panel;
pub mod snapshot;

pub use app::{AppConfig, AppError, RunSummary, run};
pub use logging::{LogGuard, init_logging};
pub use reyvec_gpu::{
    AdapterInfo, FrameImage, GpuOptions, HeadlessImage, HeadlessScene, list_adapters,
    render_headless, validation_error_count,
};

/// Название движка.
pub const ENGINE_NAME: &str = "ReyVec";

/// Версия движка из `Cargo.toml`.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Строка вида `ReyVec 0.1.0` для заголовков окон и логов.
pub fn engine_title() -> String {
    format!("{ENGINE_NAME} {ENGINE_VERSION}")
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
}
