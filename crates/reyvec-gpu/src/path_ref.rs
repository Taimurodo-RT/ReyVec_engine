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
    /// Сколько кадров уже в сумме (0 — начать заново); в режиме «качество» — номер кадра.
    pub count: u32,
    /// Режим «качество» (шаг 2): путь от буфера геометрии, без накопления.
    pub live: bool,
    /// ReSTIR DI (шаг 3): `sums` — адрес буфера запасов.
    pub restir: bool,
    /// Сколько отскоков у пути.
    pub bounces: u32,
}

/// Слов на запас пикселя (как RESTIR_WORDS в include/restir.slang) и слоёв запасов.
const RESTIR_WORDS: u64 = 8;
const RESTIR_LAYERS: u64 = 2;

/// Сколько отскоков у пути.
pub(crate) const PATH_BOUNCES: u32 = 4;

/// Эталон 3D.
pub(crate) struct PathRef {
    /// Флажок на панели.
    pub enabled: bool,
    /// Режим «качество» (путь ReSTIR, шаг 2): по пути на пиксель каждый кадр, без
    /// накопления; эталон, если включён, важнее.
    pub live: bool,
    live_frame: u32,
    /// ReSTIR DI в режиме «качество» (флажок на панели).
    pub restir: bool,
    /// Сколько отскоков у пути (тесты ставят 0 — только прямой свет).
    pub bounces: u32,
    restir_buffer: Option<(BufferId, u64)>,
    /// Камера и размер прошлого кадра с запасами.
    prev: Option<(glam::Mat4, (u32, u32))>,
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
            live: false,
            live_frame: 0,
            restir: true,
            bounces: PATH_BOUNCES,
            restir_buffer: None,
            prev: None,
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

    /// Кадр без режима «качество»: буфер запасов отдаётся.
    pub fn forget_live(&mut self, res: &mut GpuResources) {
        self.prev = None;
        if let Some((id, _)) = self.restir_buffer.take() {
            res.destroy_buffer(id);
        }
    }

    /// Кадр режима «качество»: свой номер для случайных чисел и (с ReSTIR) буфер запасов.
    /// Второе — камера прошлого кадра, если его запасы годятся (в слова сцены).
    pub fn live_frame(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        view_proj: glam::Mat4,
        extent: vk::Extent2D,
    ) -> (PathFrame, Option<glam::Mat4>) {
        self.live_frame = self.live_frame.wrapping_add(1);
        let mut frame = PathFrame {
            sums: 0,
            count: self.live_frame,
            live: true,
            restir: false,
            bounces: self.bounces,
        };
        if !self.restir {
            self.forget_live(res);
            return (frame, None);
        }
        let size =
            u64::from(extent.width) * u64::from(extent.height) * RESTIR_WORDS * 4 * RESTIR_LAYERS;
        if self.restir_buffer.is_some_and(|(_, have)| have < size) {
            self.forget_live(res);
        }
        if self.restir_buffer.is_none() {
            match res.create_buffer(
                ctx,
                size,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                BufferMemory::GpuOnly,
                "ReSTIR: запасы",
            ) {
                Ok(id) => self.restir_buffer = Some((id, size)),
                Err(e) => {
                    tracing::warn!("буфер запасов ReSTIR: {e}");
                    self.restir = false;
                    return (frame, None);
                }
            }
        }
        let Some((id, _)) = self.restir_buffer else {
            return (frame, None);
        };
        frame.sums = res.buffer_address(id);
        frame.restir = true;
        let size = (extent.width, extent.height);
        let prev = self
            .prev
            .filter(|&(_, s)| s == size)
            .map(|(matrix, _)| matrix);
        self.prev = Some((view_proj, size));
        (frame, prev)
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
            live: false,
            restir: false,
            bounces: self.bounces,
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
