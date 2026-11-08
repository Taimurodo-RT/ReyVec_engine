//! Счётчики лучей 3D (7.18д2): сколько лучей пустил каждый проход, сколько шагов они
//! сделали и сколько раз считалась сцена. Считают варианты шейдеров `*_counted.slang`
//! (`include/counters.slang`), только пока на панели включён флажок «Счётчики лучей 3D»:
//! в обычных шейдерах счётчиков нет, и замер времени по ним честный.
//!
//! Раскладка буфера — как в `counters_flush` (`include/space.slang`): на каждый проход
//! [`COUNTERS`] счётчиков, у каждого [`COUNTER_SPREAD`] копий (разные волны кадра
//! складывают в разные слова, а не все в одно по очереди), копия — 64 бита парой слов
//! (младшее, старшее).

use ash::vk;

/// Проходов со счётчиками (номера PASS_* в `include/counters.slang`).
pub const RAY_PASSES: usize = 6;
/// Названия проходов для панели, по номерам PASS_*.
pub const RAY_PASS_NAMES: [&str; RAY_PASSES] = [
    "грубо",
    "реймарчинг",
    "тени",
    "углы",
    "края: лучи",
    "края: тени",
];
/// Номера проходов (как PASS_* в `include/counters.slang`).
#[cfg_attr(not(test), allow(dead_code))]
pub const PASS_CONE: usize = 0;
#[cfg_attr(not(test), allow(dead_code))]
pub const PASS_MARCH: usize = 1;
#[cfg_attr(not(test), allow(dead_code))]
pub const PASS_SHADOWS: usize = 2;
#[cfg_attr(not(test), allow(dead_code))]
pub const PASS_AO: usize = 3;
#[cfg_attr(not(test), allow(dead_code))]
pub const PASS_EDGES: usize = 4;
#[cfg_attr(not(test), allow(dead_code))]
pub const PASS_EDGE_SHADOWS: usize = 5;
/// Счётчиков у прохода (поля [`RayCounts`] по порядку).
const COUNTERS: usize = 8;
/// Копий каждого счётчика.
const COUNTER_SPREAD: usize = 32;
/// Размер буфера счётчиков, байт.
pub(crate) const COUNTER_BYTES: u64 = (RAY_PASSES * COUNTERS * COUNTER_SPREAD * 2 * 4) as u64;

/// Счётчики одного прохода за кадр.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RayCounts {
    /// Лучей: пикселей («грубо» — квадратов 8×8), лучей тени, точек затенения в углах.
    pub rays: u64,
    /// Шагов по лучу (у затенения в углах — проб).
    pub steps: u64,
    /// Вычислений сцены (`scene_near`), в том числе для нормалей.
    pub scene: u64,
    /// Из них — для нормали (`normal_at`, по 4 на нормаль).
    pub normal: u64,
    /// Объектов, дошедших до проверки по шару-границе.
    pub objects: u64,
    /// Форм, посчитанных целиком (программа формы).
    pub forms: u64,
    /// Чтений поля слоя.
    pub field: u64,
    /// Чтений картинки материалов поля слоя.
    pub material: u64,
}

impl RayCounts {
    fn add(&mut self, counter: usize, value: u64) {
        let slot = match counter {
            0 => &mut self.rays,
            1 => &mut self.steps,
            2 => &mut self.scene,
            3 => &mut self.normal,
            4 => &mut self.objects,
            5 => &mut self.forms,
            6 => &mut self.field,
            _ => &mut self.material,
        };
        *slot = slot.wrapping_add(value);
    }
}

/// Складывает копии счётчиков из слов буфера (как их пишет `counters_flush`).
pub(crate) fn sum(words: &[u32]) -> [RayCounts; RAY_PASSES] {
    let mut out = [RayCounts::default(); RAY_PASSES];
    for (pass, counts) in out.iter_mut().enumerate() {
        for counter in 0..COUNTERS {
            for spread in 0..COUNTER_SPREAD {
                let at = ((pass * COUNTERS + counter) * COUNTER_SPREAD + spread) * 2;
                let lo = words.get(at).copied().unwrap_or(0);
                let hi = words.get(at + 1).copied().unwrap_or(0);
                counts.add(counter, u64::from(lo) | (u64::from(hi) << 32));
            }
        }
    }
    out
}

/// Складывает счётчики из байтов буфера, скопированного для процессора.
pub(crate) fn sum_bytes(bytes: &[u8]) -> [RayCounts; RAY_PASSES] {
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    sum(&words)
}

/// Обнуляет буфер счётчиков перед первым проходом лучей кадра.
///
/// # Safety
/// `cmd` записывается и не внутри прохода рисования; `buffer` жив, пока кадр рисуется, и
/// создан с `TRANSFER_DST`.
pub(crate) unsafe fn record_reset(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    buffer: vk::Buffer,
) {
    // В прошлый раз буфер этого кадра складывали шейдеры и читала копия для процессора.
    let before = [vk::MemoryBarrier2::default()
        .src_stage_mask(
            vk::PipelineStageFlags2::FRAGMENT_SHADER
                | vk::PipelineStageFlags2::COMPUTE_SHADER
                | vk::PipelineStageFlags2::ALL_TRANSFER,
        )
        .src_access_mask(
            vk::AccessFlags2::SHADER_STORAGE_READ
                | vk::AccessFlags2::SHADER_STORAGE_WRITE
                | vk::AccessFlags2::TRANSFER_READ,
        )
        .dst_stage_mask(vk::PipelineStageFlags2::ALL_TRANSFER)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)];
    let after = [vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_TRANSFER)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(
            vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COMPUTE_SHADER,
        )
        .dst_access_mask(
            vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
        )];
    // SAFETY: условия функции; барьеры упорядочивают обнуление с прошлым и этим кадром.
    unsafe {
        device.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&before));
        device.cmd_fill_buffer(cmd, buffer, 0, vk::WHOLE_SIZE, 0);
        device.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&after));
    }
}

/// Копирует счётчики кадра в буфер, который читает процессор, после последнего прохода
/// лучей.
///
/// # Safety
/// `cmd` записывается и не внутри прохода рисования; оба буфера живы, пока кадр рисуется;
/// `gpu` создан с `TRANSFER_SRC`, `host` — с `TRANSFER_DST`, оба не меньше
/// [`COUNTER_BYTES`].
pub(crate) unsafe fn record_copy(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    gpu: vk::Buffer,
    host: vk::Buffer,
) {
    let before = [vk::MemoryBarrier2::default()
        .src_stage_mask(
            vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COMPUTE_SHADER,
        )
        .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::COPY)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)];
    let copy = [vk::BufferCopy::default().size(COUNTER_BYTES)];
    let to_host = [vk::BufferMemoryBarrier2::default()
        .buffer(host)
        .size(vk::WHOLE_SIZE)
        .src_stage_mask(vk::PipelineStageFlags2::COPY)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::HOST)
        .dst_access_mask(vk::AccessFlags2::HOST_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)];
    // SAFETY: условия функции; барьеры упорядочивают копию с проходами лучей и чтением.
    unsafe {
        device.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&before));
        device.cmd_copy_buffer(cmd, gpu, host, &copy);
        device.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().buffer_memory_barriers(&to_host),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Копии складываются, старшее слово — перенос; проходы и счётчики не путаются.
    #[test]
    fn sum_adds_spread_copies_with_carry() {
        let mut words = vec![0u32; (COUNTER_BYTES / 4) as usize];
        let at = |pass: usize, counter: usize, spread: usize| {
            ((pass * COUNTERS + counter) * COUNTER_SPREAD + spread) * 2
        };
        words[at(PASS_MARCH, 1, 0)] = u32::MAX;
        words[at(PASS_MARCH, 1, 31)] = 2;
        words[at(PASS_MARCH, 1, 31) + 1] = 1;
        words[at(PASS_AO, 0, 5)] = 7;
        words[at(PASS_EDGE_SHADOWS, 7, 3)] = 9;
        let counts = sum(&words);
        assert_eq!(
            counts[PASS_MARCH].steps,
            u64::from(u32::MAX) + 2 + (1 << 32)
        );
        assert_eq!(counts[PASS_MARCH].rays, 0);
        assert_eq!(counts[PASS_AO].rays, 7);
        assert_eq!(counts[PASS_EDGE_SHADOWS].material, 9);
        assert_eq!(counts[PASS_CONE], RayCounts::default());
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
        assert_eq!(sum_bytes(&bytes), counts);
    }
}
