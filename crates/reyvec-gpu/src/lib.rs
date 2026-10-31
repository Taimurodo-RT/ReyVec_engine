//! `reyvec-gpu`: безопасная обёртка над Vulkan для движка ReyVec.
//!
//! Правило проекта: весь `unsafe`-код Vulkan живёт только в этом крейте.
//! Снаружи видны безопасные типы:
//!
//! - [`list_adapters`] перечисляет видеокарты и объясняет, почему какая-то не подходит;
//! - [`WindowRenderer`] рисует кадры в окно (этап 1: вращающийся треугольник);
//! - [`render_headless`] рисует тот же кадр без окна, для автотестов;
//! - [`DemoScene`]: какую сцену песочницы рисовать (этап 2 или векторы этапа 3).
//!
//! Требования к видеокарте: Vulkan 1.3 с `dynamicRendering`, `synchronization2`,
//! индексированием дескрипторов, адресами буферов и `multiDrawIndirect` (см. `docs/decisions/0001-stack.md`).

#[doc(hidden)]
pub use tracy_client as __tracy;

/// Зона процессора для Tracy: живёт до конца области видимости. Если Tracy не запущен
/// (`tracy_client::Client::start()` не вызывали), ничего не делает.
///
/// ```ignore
/// let _zone = reyvec_gpu::cpu_zone!("обновление сцены");
/// ```
#[macro_export]
macro_rules! cpu_zone {
    ($name:literal) => {
        $crate::__tracy::Client::running()
            .map(|c| c.span($crate::__tracy::span_location!($name), 0))
    };
}

mod adapter;
mod bindless;
mod context;
mod demo;
mod demo_vector;
mod graph;
mod headless;
mod pipeline;
mod resources;
mod shaders;
mod slangc;
mod timing;
mod ui;
mod vector;
mod window;

pub use adapter::{AdapterInfo, AdapterKind, pick_adapter};
pub use bindless::{MAX_TEXTURES, SamplerKind};
pub use context::{GpuOptions, list_adapters, validation_error_count};
pub use demo::DemoScene;
pub use graph::GraphReport;
pub use headless::{
    FrameImage, HeadlessImage, HeadlessScene, render_headless, render_triangle_headless,
};
pub use pipeline::CLEAR_COLOR;
pub use resources::{BufferId, BufferMemory, TextureId};
pub use timing::{GpuFrameTimings, GpuTimingAverage, PassTiming};
pub use ui::UiFrame;
pub use window::{FrameStatus, RendererOptions, WindowRenderer, WindowTarget};

use ash::vk;

/// Ошибки GPU-слоя. Тексты написаны так, чтобы их можно было показать человеку.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// Не удалось загрузить Vulkan (нет драйвера или `vulkan-1.dll`).
    #[error("не удалось загрузить Vulkan: {0}. Обновите драйвер видеокарты")]
    Loading(String),
    /// Вызов Vulkan вернул ошибку.
    #[error("ошибка Vulkan при «{what}»: {result:?}")]
    Vulkan {
        /// Что делали.
        what: &'static str,
        /// Код ошибки.
        result: vk::Result,
    },
    /// Нет видеокарты, которая подходит движку, или выбранная не подходит.
    #[error("{0}")]
    NoSuitableAdapter(String),
    /// Не хватило памяти видеокарты или распределитель вернул ошибку.
    #[error("память видеокарты: {0}")]
    Memory(String),
    /// Окно не отдало свои дескрипторы.
    #[error("окно недоступно для Vulkan: {0}")]
    Window(String),
}

pub(crate) trait VkResultExt<T> {
    fn ctx(self, what: &'static str) -> Result<T, GpuError>;
}

impl<T> VkResultExt<T> for Result<T, vk::Result> {
    fn ctx(self, what: &'static str) -> Result<T, GpuError> {
        self.map_err(|result| GpuError::Vulkan { what, result })
    }
}
