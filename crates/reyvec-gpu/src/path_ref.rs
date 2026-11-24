//! Эталон 3D (путь ReSTIR, шаг 1): трассировка путей с накоплением, `raymarch_path.slang`.
//!
//! Пока камера, размер кадра и сцена те же, каждый кадр добавляет в сумму по пути на
//! пиксель, и картинка сходится к правде. Любое изменение — сумма с нуля. Чтобы сцена не
//! менялась, пока эталон включён, время сцены замирает ([`PathRef::time`]).
//!
//! Сумма лежит в буфере вне графа (4 числа на пиксель): её читают и пишут кадры подряд,
//! порядок между ними держит барьер в начале прохода.

use ash::vk;
use std::hash::{Hash, Hasher};

use crate::context::GpuContext;
use crate::resources::{BufferId, BufferMemory, GpuResources};

/// Что проходу кадра нужно знать.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PathFrame {
    /// Адрес буфера суммы.
    pub sums: vk::DeviceAddress,
    /// Сколько кадров уже в сумме (0 — начать заново).
    pub count: u32,
}

/// Сколько отскоков у пути.
pub(crate) const PATH_BOUNCES: u32 = 4;

/// Эталон 3D.
pub(crate) struct PathRef {
    /// Флажок на панели.
    pub enabled: bool,
    buffer: Option<(BufferId, u64)>,
    count: u32,
    key: Option<(glam::Mat4, (u32, u32), u64)>,
    /// Время сцены, на котором она замерла.
    frozen: Option<f32>,
}

impl PathRef {
    pub fn new() -> Self {
        PathRef {
            enabled: false,
            buffer: None,
            count: 0,
            key: None,
            frozen: None,
        }
    }

    /// Время сцены для кадра: пока эталон включён — то, на котором он включился.
    pub fn time(&mut self, time: f32) -> f32 {
        if self.enabled {
            *self.frozen.get_or_insert(time)
        } else {
            self.frozen = None;
            time
        }
    }

    /// Сколько кадров в сумме (для панели); `None` — эталон выключен.
    pub fn samples(&self) -> Option<u32> {
        (self.enabled && self.key.is_some()).then_some(self.count)
    }

    /// Кадр без эталона: буфер отдаётся, следующий включённый начнёт с нуля.
    pub fn forget(&mut self, res: &mut GpuResources) {
        self.key = None;
        self.count = 0;
        if let Some((id, _)) = self.buffer.take() {
            res.destroy_buffer(id);
        }
    }

    /// Готовит кадр: `view_proj` — камера, `extent` — размер картинки, `words` — сцена
    /// (её слова сверяются целиком).
    pub fn prepare(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        view_proj: glam::Mat4,
        extent: vk::Extent2D,
        words: &[u32],
    ) -> Option<PathFrame> {
        let size = u64::from(extent.width) * u64::from(extent.height) * 16;
        if self.buffer.is_some_and(|(_, have)| have < size)
            && let Some((id, _)) = self.buffer.take()
        {
            res.destroy_buffer(id);
        }
        if self.buffer.is_none() {
            match res.create_buffer(
                ctx,
                size,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                BufferMemory::GpuOnly,
                "эталон 3D: сумма",
            ) {
                Ok(id) => {
                    self.buffer = Some((id, size));
                    self.key = None;
                }
                Err(e) => {
                    tracing::warn!("буфер эталона 3D: {e}");
                    self.enabled = false;
                    return None;
                }
            }
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        words.hash(&mut hasher);
        let key = (view_proj, (extent.width, extent.height), hasher.finish());
        if self.key != Some(key) {
            self.key = Some(key);
            self.count = 0;
        }
        let (id, _) = self.buffer?;
        let frame = PathFrame {
            sums: res.buffer_address(id),
            count: self.count,
        };
        self.count = self.count.saturating_add(1);
        Some(frame)
    }
}

impl Default for PathRef {
    fn default() -> Self {
        Self::new()
    }
}
