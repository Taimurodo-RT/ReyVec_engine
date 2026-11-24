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
    /// Шумодав (шаг 5): адрес буфера его памяти, 0 — шумодава нет.
    pub history: vk::DeviceAddress,
    /// ReSTIR GI (шаг 4): запасы отражённого света — в буфере `sums` после запасов DI.
    pub gi: bool,
}

/// Что из прошлого кадра годится (как PREV_* в include/restir.slang).
pub(crate) const PREV_RESTIR: u32 = 1;
pub(crate) const PREV_HISTORY: u32 = 2;
pub(crate) const PREV_GI: u32 = 4;
/// Слов памяти шумодава на пиксель (как HISTORY_WORDS в raymarch_denoise.slang).
const HISTORY_WORDS: u64 = 10;

/// Слов на запас пикселя (как RESTIR_WORDS в include/restir.slang) и слоёв запасов.
const RESTIR_WORDS: u64 = 8;
const RESTIR_LAYERS: u64 = 2;
/// Слов на запас ReSTIR GI (как GI_WORDS в include/restir_gi.slang) и слоёв.
const GI_WORDS: u64 = 11;
const GI_LAYERS: u64 = 2;

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
    /// ReSTIR GI в режиме «качество» (флажок на панели).
    pub gi: bool,
    /// Запасы DI и GI (один буфер: у прохода одно место для адреса).
    restir_buffer: Option<(BufferId, u64)>,
    /// Что было в буфере запасов в прошлом кадре: (DI, GI).
    restir_last: (bool, bool),
    /// Шумодав режима «качество» (флажок на панели).
    pub denoise: bool,
    history_buffer: Option<(BufferId, u64)>,
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
            gi: true,
            restir_buffer: None,
            restir_last: (false, false),
            denoise: true,
            history_buffer: None,
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

    /// Кадр без режима «качество»: буферы запасов и памяти шумодава отдаются.
    pub fn forget_live(&mut self, res: &mut GpuResources) {
        self.prev = None;
        for slot in [&mut self.restir_buffer, &mut self.history_buffer] {
            if let Some((id, _)) = slot.take() {
                res.destroy_buffer(id);
            }
        }
    }

    /// Кадр режима «качество»: свой номер для случайных чисел, буфер запасов (с ReSTIR)
    /// и буфер памяти шумодава. Второе — камера прошлого кадра и что из него годится
    /// (биты PREV_*, в слова сцены), если он был того же размера.
    pub fn live_frame(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        view_proj: glam::Mat4,
        extent: vk::Extent2D,
    ) -> (PathFrame, Option<(glam::Mat4, u32)>) {
        self.live_frame = self.live_frame.wrapping_add(1);
        let mut frame = PathFrame {
            sums: 0,
            count: self.live_frame,
            live: true,
            restir: false,
            bounces: self.bounces,
            history: 0,
            gi: false,
        };
        let size = (extent.width, extent.height);
        let same = self
            .prev
            .filter(|&(_, s)| s == size)
            .map(|(matrix, _)| matrix);
        self.prev = Some((view_proj, size));
        let pixels = u64::from(extent.width) * u64::from(extent.height);
        let mut valid = 0;
        if let Some((address, fresh)) = keep_buffer(
            ctx,
            res,
            &mut self.restir_buffer,
            self.restir || self.gi,
            pixels * 4 * (RESTIR_WORDS * RESTIR_LAYERS + GI_WORDS * GI_LAYERS),
            "ReSTIR: запасы",
        ) {
            frame.sums = address;
            frame.restir = self.restir;
            frame.gi = self.gi;
            let (di_was, gi_was) = if fresh {
                (false, false)
            } else {
                self.restir_last
            };
            valid |= (u32::from(di_was) * PREV_RESTIR) | (u32::from(gi_was) * PREV_GI);
        }
        self.restir_last = (frame.restir, frame.gi);
        if let Some((address, fresh)) = keep_buffer(
            ctx,
            res,
            &mut self.history_buffer,
            self.denoise,
            pixels * HISTORY_WORDS * 4 * 2,
            "шумодав: память",
        ) {
            frame.history = address;
            valid |= u32::from(!fresh) * PREV_HISTORY;
        }
        (frame, same.map(|m| (m, valid)))
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
            history: 0,
            gi: false,
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

/// Буфер не меньше `size`, пока `on`; иначе отдаётся. Адрес и «новый ли» (в новом нет
/// прошлого кадра).
fn keep_buffer(
    ctx: &GpuContext,
    res: &mut GpuResources,
    slot: &mut Option<(BufferId, u64)>,
    on: bool,
    size: u64,
    name: &str,
) -> Option<(vk::DeviceAddress, bool)> {
    if (!on || slot.is_some_and(|(_, have)| have < size))
        && let Some((id, _)) = slot.take()
    {
        res.destroy_buffer(id);
    }
    if !on {
        return None;
    }
    let mut fresh = false;
    if slot.is_none() {
        match res.create_buffer(
            ctx,
            size,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            BufferMemory::GpuOnly,
            name,
        ) {
            Ok(id) => {
                *slot = Some((id, size));
                fresh = true;
            }
            Err(e) => {
                tracing::warn!("буфер «{name}»: {e}");
                return None;
            }
        }
    }
    slot.map(|(id, _)| (res.buffer_address(id), fresh))
}
