//! Замер фильтра на тензорных ядрах (опыт Р4, «Р4» в docs/3d-beyond-triangles.md).
//!
//! Тензорные ядра умеют одно: умножать маленькие матрицы (D = A × B + C) очень быстро.
//! Лучам сцены это не помогает (там нет матриц), а фильтрам картинки — может: размытие по
//! Гауссу вдоль строки — это умножение квадрата 16×16 на матрицу весов. Опыт отвечает на
//! вопрос Романа «где предел»: одна и та же работа (размытие радиусом [`RADIUS`] по строкам,
//! потом по столбцам) считается обычными ядрами (`filter_plain.slang`) и тензорными
//! (`filter_tensor.slang`) на картинке выбранного размера (до 8K), а панель показывает
//! время обоих проходов. Картинку кадра опыт не меняет: числа — шум, загруженный один раз.

use ash::vk;

use crate::context::GpuContext;
use crate::resources::{BufferId, BufferMemory, GpuResources};

/// Радиус размытия, точек (шагов весов 2R + 1). Не больше 16: соседи — в квадратах слева
/// и справа, всего на 16 точек (`filter_tensor.slang`).
pub(crate) const RADIUS: u32 = 16;
/// Потоков в группе и квадратов на волну (как GROUP и TILES_PER_WAVE в `filter_tensor.slang`).
const TENSOR_GROUP_TILES: u32 = 4;

/// Размеры картинки замера: ширина и высота кратны 16 (стороне квадратов тензорных ядер).
pub const BENCH_SIZES: [(u32, u32); 3] = [(1920, 1088), (3840, 2160), (7680, 4320)];

/// Что проходам замера нужно знать в кадре.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BenchFrame {
    src: vk::DeviceAddress,
    tmp: vk::DeviceAddress,
    plain: vk::DeviceAddress,
    tensor: vk::DeviceAddress,
    taps: vk::DeviceAddress,
    width: u32,
    height: u32,
    /// Сторона матриц тензорных ядер (`GpuContext::tensor`).
    side: u32,
}

/// Замер фильтра.
pub(crate) struct TensorBench {
    /// Флажок на панели.
    pub enabled: bool,
    /// Какой из [`BENCH_SIZES`].
    pub size: usize,
    /// Буферы: шум, промежуточный, ответы обычных и тензорных ядер, веса; и для какого размера.
    buffers: Option<([BufferId; 5], usize)>,
}

impl TensorBench {
    pub fn new() -> Self {
        TensorBench {
            enabled: false,
            size: 0,
            buffers: None,
        }
    }

    /// Буферы ответов (обычные и тензорные ядра) и размер картинки (для теста).
    #[cfg(test)]
    pub fn answers(&self) -> Option<(BufferId, BufferId, (u32, u32))> {
        self.buffers
            .map(|(b, size)| (b[2], b[3], BENCH_SIZES[size]))
    }

    /// Готовит кадр: при первом включении или смене размера — буферы и шум. `None` — опыт
    /// выключен или памяти не хватило.
    pub fn prepare(&mut self, ctx: &GpuContext, res: &mut GpuResources) -> Option<BenchFrame> {
        if !self.enabled {
            if let Some((ids, _)) = self.buffers.take() {
                for id in ids {
                    res.destroy_buffer(id);
                }
            }
            return None;
        }
        let size = self.size.min(BENCH_SIZES.len() - 1);
        if self.buffers.is_some_and(|(_, s)| s != size)
            && let Some((ids, _)) = self.buffers.take()
        {
            for id in ids {
                res.destroy_buffer(id);
            }
        }
        if self.buffers.is_none() {
            match make_buffers(ctx, res, BENCH_SIZES[size]) {
                Ok(ids) => self.buffers = Some((ids, size)),
                Err(e) => {
                    tracing::warn!("замер фильтра: {e}");
                    self.enabled = false;
                    return None;
                }
            }
        }
        let (ids, _) = self.buffers?;
        let (width, height) = BENCH_SIZES[size];
        Some(BenchFrame {
            src: res.buffer_address(ids[0]),
            tmp: res.buffer_address(ids[1]),
            plain: res.buffer_address(ids[2]),
            tensor: res.buffer_address(ids[3]),
            taps: res.buffer_address(ids[4]),
            width,
            height,
            side: ctx.tensor.max(8),
        })
    }

    /// Записывает размытие (по строкам в промежуточный буфер, потом по столбцам в ответ)
    /// обычными (`tensor == false`) или тензорными ядрами.
    ///
    /// # Safety
    /// `cmd` записывается; `pipeline` и `layout` живы; буферы `frame` живы, пока кадр
    /// выполняется.
    pub unsafe fn record(
        d: &ash::Device,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        pipeline: vk::Pipeline,
        frame: &BenchFrame,
        tensor: bool,
    ) {
        let barrier = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .src_access_mask(
                vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::SHADER_STORAGE_READ,
            )
            .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .dst_access_mask(
                vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::SHADER_STORAGE_READ,
            )];
        let out = if tensor { frame.tensor } else { frame.plain };
        let groups = if tensor {
            // Квадратов side×side, по TENSOR_GROUP_TILES на группу (волна на группу — у
            // NVIDIA; где волна короче, лишние группы ничего не считают).
            let tiles = (frame.width / frame.side) * (frame.height / frame.side);
            [tiles.div_ceil(TENSOR_GROUP_TILES), 1]
        } else {
            [frame.width.div_ceil(8), frame.height.div_ceil(8)]
        };
        // SAFETY: условия функции; барьеры упорядочивают запись промежуточного буфера и
        // чтение его вторым проходом, а также с прошлым кадром.
        unsafe {
            d.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().memory_barriers(&barrier),
            );
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
            for (from, to, vertical) in [(frame.src, frame.tmp, 0), (frame.tmp, out, 1)] {
                let push = bench_push(from, to, frame.taps, frame.width, frame.height, vertical);
                d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                d.cmd_dispatch(cmd, groups[0], groups[1], 1);
                d.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().memory_barriers(&barrier),
                );
            }
        }
    }
}

impl Default for TensorBench {
    fn default() -> Self {
        Self::new()
    }
}

/// Веса размытия: Гаусс с σ = R / 3, сумма 1.
pub(crate) fn taps() -> Vec<f32> {
    let r = RADIUS as i32;
    let sigma = (RADIUS as f32 / 3.0).max(1.0);
    let raw: Vec<f32> = (-r..=r)
        .map(|o| (-(o * o) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = raw.iter().sum();
    raw.into_iter().map(|w| w / sum).collect()
}

/// Шум картинки замера: числа от 0 до 1, одни и те же при каждом запуске.
pub(crate) fn noise(width: u32, height: u32) -> Vec<f32> {
    let mut state = 0x1234_5678u32;
    (0..width * height)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 8) as f32 / (1u32 << 24) as f32
        })
        .collect()
}

fn make_buffers(
    ctx: &GpuContext,
    res: &mut GpuResources,
    (width, height): (u32, u32),
) -> Result<[BufferId; 5], crate::GpuError> {
    let bytes = u64::from(width) * u64::from(height) * 4;
    let usage = vk::BufferUsageFlags::STORAGE_BUFFER
        | vk::BufferUsageFlags::TRANSFER_DST
        | vk::BufferUsageFlags::TRANSFER_SRC;
    let taps = taps();
    let mut ids = Vec::with_capacity(5);
    let names = [
        "замер фильтра: шум",
        "замер фильтра: по строкам",
        "замер фильтра: обычные ядра",
        "замер фильтра: тензорные ядра",
        "замер фильтра: веса",
    ];
    for (k, name) in names.into_iter().enumerate() {
        let size = if k == 4 {
            (taps.len() * 4) as u64
        } else {
            bytes
        };
        match res.create_buffer(ctx, size, usage, BufferMemory::GpuOnly, name) {
            Ok(id) => ids.push(id),
            Err(e) => {
                for id in ids {
                    res.destroy_buffer(id);
                }
                return Err(e);
            }
        }
    }
    let to_bytes = |v: &[f32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_ne_bytes()).collect() };
    res.upload_buffer(ctx, ids[0], &to_bytes(&noise(width, height)))?;
    res.upload_buffer(ctx, ids[4], &to_bytes(&taps))?;
    Ok([ids[0], ids[1], ids[2], ids[3], ids[4]])
}

/// Константы `filter_plain.slang` и `filter_tensor.slang`.
fn bench_push(
    src: vk::DeviceAddress,
    dst: vk::DeviceAddress,
    taps: vk::DeviceAddress,
    width: u32,
    height: u32,
    vertical: u32,
) -> [u8; 40] {
    let mut push = [0u8; 40];
    push[0..8].copy_from_slice(&src.to_ne_bytes());
    push[8..16].copy_from_slice(&dst.to_ne_bytes());
    push[16..24].copy_from_slice(&taps.to_ne_bytes());
    for (i, w) in [width, height, vertical, RADIUS].into_iter().enumerate() {
        push[24 + 4 * i..28 + 4 * i].copy_from_slice(&w.to_ne_bytes());
    }
    push
}

/// Размытие на процессоре (как `filter_plain.slang`): по строкам, потом по столбцам.
#[cfg(test)]
pub(crate) fn blur_cpu(src: &[f32], width: u32, height: u32) -> Vec<f32> {
    let taps = taps();
    let r = RADIUS as i32;
    let (w, h) = (width as i32, height as i32);
    let pass = |from: &[f32], vertical: bool| -> Vec<f32> {
        let mut out = vec![0.0; from.len()];
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.0;
                for o in -r..=r {
                    let (sx, sy) = if vertical { (x, y + o) } else { (x + o, y) };
                    if sx >= 0 && sy >= 0 && sx < w && sy < h {
                        sum += from[(sy * w + sx) as usize] * taps[(o + r) as usize];
                    }
                }
                out[(y * w + x) as usize] = sum;
            }
        }
        out
    };
    pass(&pass(src, false), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_sum_to_one_and_sizes_fit_tiles() {
        let t = taps();
        assert_eq!(t.len(), 2 * RADIUS as usize + 1);
        assert!((t.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        const { assert!(RADIUS <= 16) };
        for (w, h) in BENCH_SIZES {
            assert_eq!((w % 16, h % 16), (0, 0));
        }
    }
}
