//! Кэш солнца (опыт Р2, «Р2» в docs/3d-beyond-triangles.md): тень солнца от неподвижных
//! предметов печётся один раз в большую карту и потом только читается.
//!
//! Как у Quake: свет неподвижного не пересчитывается каждый кадр. Каждый кадр луч тени
//! солнца шагает по всем предметам сцены, хотя у завода двигается малая часть (ящики на
//! лентах, погрузчики, вентиляторы).
//!
//! Карта решает только то, что знает точно (`sun_static_known` в
//! `include/shadow3d.slang`): «неподвижное точно не затеняет» — живой луч шагает только по
//! движущимся; «точка точно в полной тени неподвижного» — тень без луча. У края тени и в
//! полутени — живой луч по всем предметам, как без кэша, поэтому картинка та же, что без
//! кэша. (Сначала карта сама рисовала полутень, как карты режима «треугольники», — на
//! складе тени выходили размытыми, Роман 2026-11-24.)
//!
//! Картинка — R32F 2·[`SIDE`] × [`SIDE`] точек, прямоугольная проекция вдоль солнца на все
//! неподвижные предметы (`space::sun_cache_view_proj`). Слева — глубина первой формы от
//! солнца; её печёт проход «кэш солнца» (`sun_bake.slang`) по отдельной сцене из одних
//! неподвижных предметов, квадратами [`TILE`]² по [`TILES_PER_FRAME`] за кадр. Справа —
//! запас (`sun_envelope.slang`): насколько ниже соседей может быть точка, чтобы полутень
//! неподвижного её не задела; считается за несколько проходов в кадре, когда глубина
//! готова. Пока карта не готова, тени идут, как без кэша. Если неподвижное поменялось
//! (другая сцена, правка в редакторе), карта печётся заново.

use ash::vk;
use reyvec_sdf::SpaceFrame;
use reyvec_sdf::shape3d::Material3;

use crate::bindless::Bindless;
use crate::context::GpuContext;
use crate::resources::{BufferId, BufferMemory, GpuResources, TextureId};

/// Точек карты по стороне. На заводе (около 60 м) — полтора сантиметра на точку.
pub(crate) const SIDE: u32 = 4096;
/// Точек квадрата, который печётся одним запуском.
pub(crate) const TILE: u32 = 512;
/// Квадратов за кадр: у [`SIDE`] 4096 карта готова за 16 кадров.
pub(crate) const TILES_PER_FRAME: u32 = 4;
/// Жёсткость конуса запаса (как SUN_STATIC_CONE в `include/shadow3d.slang`): живой луч
/// солнца (k = 12) не тронут предметом дальше t / 12 от него; 10 — с запасом.
const CONE: f32 = 10.0;

/// Что проходу «кэш солнца» и теням нужно знать в этом кадре.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SunFrame {
    /// Сцена из неподвижных предметов (адрес слов).
    scene: vk::DeviceAddress,
    /// Матрица «мир → карта».
    pub matrix: glam::Mat4,
    /// Номер картинки карты в общем списке текстур.
    pub slot: u32,
    /// Карта готова к чтению в этом кадре (печь больше нечего или всё допекается сейчас).
    pub ready: bool,
    image: vk::Image,
    /// Буфер глубин всей карты и буфер запаса (из них копируется в картинку).
    depth: vk::Buffer,
    envelope: vk::Buffer,
    /// Адреса: глубины, два промежуточных, запас.
    addresses: [vk::DeviceAddress; 4],
    /// Проходов запаса в этом кадре (0 — не в этом).
    passes: u32,
    /// Жёсткость конуса на точку карты, метры глубины на точку вбок.
    cone_texel: f32,
    side: u32,
    tile: u32,
    /// Квадраты этого кадра: номер квадрата в карте (по строкам).
    tiles: [u32; TILES_PER_FRAME as usize * 4],
    count: u32,
    /// Первый квадрат печи: прошлое содержимое картинки не нужно.
    fresh: bool,
}

/// Кэш солнца сцены «3D».
pub(crate) struct SunCache {
    /// Включён ли (флажок на панели).
    pub enabled: bool,
    /// Точек карты по стороне и квадрата (тесты берут меньше).
    pub side: u32,
    pub tile: u32,
    /// Квадратов за кадр (тесты печут всё в одном кадре).
    pub per_frame: u32,
    /// Неподвижное, по которому печётся карта.
    source: Option<SpaceFrame>,
    matrix: Option<glam::Mat4>,
    scene: Option<BufferId>,
    texture: Option<TextureId>,
    /// Буферы на всю карту: глубины, два промежуточных для запаса, запас.
    work: Option<[BufferId; 4]>,
    /// Сколько квадратов уже испечено.
    done: u32,
    /// Запас посчитан.
    enveloped: bool,
}

impl SunCache {
    pub fn new() -> Self {
        SunCache {
            enabled: false,
            side: SIDE,
            tile: TILE,
            per_frame: TILES_PER_FRAME,
            source: None,
            matrix: None,
            scene: None,
            texture: None,
            work: None,
            done: 0,
            enveloped: false,
        }
    }

    fn tiles_total(&self) -> u32 {
        let across = self.side.div_ceil(self.tile);
        across * across
    }

    /// Готовит кадр: `frame` — кадр сцены до отсева по взгляду. `None` — кэша в кадре нет
    /// (выключен, нечего печь, не вышло создать картинку).
    pub fn prepare(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        frame: &SpaceFrame,
        materials: &[Material3],
    ) -> Option<SunFrame> {
        if !self.enabled {
            if self.source.is_some() || self.texture.is_some() {
                self.release(res);
            }
            return None;
        }
        let changed = self
            .source
            .as_ref()
            .is_none_or(|s| !crate::space::same_still(frame, s));
        if changed {
            self.rebuild(ctx, res, crate::space::still_frame(frame), materials);
        }
        let matrix = self.matrix?;
        let scene = self.scene?;
        if self.texture.is_none() {
            match res.create_texture_format(
                ctx,
                self.side * 2,
                self.side,
                vk::Format::R32_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                "кэш солнца",
            ) {
                Ok(id) => {
                    self.texture = Some(id);
                    self.done = 0;
                    self.enveloped = false;
                }
                Err(e) => {
                    tracing::warn!("картинка кэша солнца: {e}");
                    self.enabled = false;
                    return None;
                }
            }
        }
        let per_frame = self.per_frame.clamp(1, TILES_PER_FRAME * 4);
        if self.work.is_none() {
            let bytes = u64::from(self.side) * u64::from(self.side) * 4;
            let names = [
                "кэш солнца: глубины",
                "кэш солнца: запас, промежуточный 1",
                "кэш солнца: запас, промежуточный 2",
                "кэш солнца: запас",
            ];
            let mut ids = Vec::with_capacity(4);
            for name in names {
                match res.create_buffer(
                    ctx,
                    bytes,
                    vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
                    BufferMemory::GpuOnly,
                    name,
                ) {
                    Ok(id) => ids.push(id),
                    Err(e) => {
                        tracing::warn!("буфер кэша солнца: {e}");
                        for id in ids {
                            res.destroy_buffer(id);
                        }
                        self.enabled = false;
                        return None;
                    }
                }
            }
            self.work = Some([ids[0], ids[1], ids[2], ids[3]]);
            self.done = 0;
            self.enveloped = false;
        }
        let texture = self.texture?;
        let work = self.work?;
        let total = self.tiles_total();
        let mut tiles = [0u32; TILES_PER_FRAME as usize * 4];
        let fresh = self.done == 0;
        let mut count = 0;
        while self.done < total && count < per_frame {
            tiles[count as usize] = self.done;
            self.done += 1;
            count += 1;
        }
        let (passes, cone_texel) = envelope_passes(matrix, self.side);
        let passes = if self.done == total && !self.enveloped {
            self.enveloped = true;
            passes
        } else {
            0
        };
        let address = |k: usize| res.buffer_address(work[k]);
        Some(SunFrame {
            scene: res.buffer_address(scene),
            matrix,
            slot: texture.0,
            ready: self.done == total,
            image: res.texture_image(texture)?,
            depth: res.buffer_raw(work[0])?,
            envelope: res.buffer_raw(work[3])?,
            addresses: [address(0), address(1), address(2), address(3)],
            passes,
            cone_texel,
            side: self.side,
            tile: self.tile,
            tiles,
            count,
            fresh: fresh && count > 0,
        })
    }

    /// Неподвижное поменялось: новая сцена для печи, карта печётся с начала.
    fn rebuild(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        still: SpaceFrame,
        materials: &[Material3],
    ) {
        if let Some(old) = self.scene.take() {
            res.destroy_buffer(old);
        }
        self.done = 0;
        self.enveloped = false;
        let programs = crate::space::programs(&still);
        self.matrix = crate::space::sun_cache_view_proj(&programs);
        if self.matrix.is_some() {
            let packed = crate::space::pack_cached(
                &still,
                materials,
                true,
                true,
                reyvec_sdf::grid3d::GridOptions::default(),
                &mut crate::space::PackCache::default(),
            );
            if packed.overflow.is_none() {
                match res.create_buffer(
                    ctx,
                    (packed.words.len() * 4) as u64,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    BufferMemory::CpuToGpu,
                    "кэш солнца: неподвижная сцена",
                ) {
                    Ok(id) => {
                        res.write_words(id, 0, &packed.words);
                        self.scene = Some(id);
                    }
                    Err(e) => tracing::warn!("сцена кэша солнца: {e}"),
                }
            }
        }
        tracing::info!(
            "кэш солнца: неподвижных предметов {}, печётся заново",
            programs.len()
        );
        self.source = Some(still);
    }

    /// Освобождает картинку и буферы (выключение, смена устройства).
    pub fn release(&mut self, res: &mut GpuResources) {
        if let Some(id) = self.texture.take() {
            res.destroy_texture(id);
        }
        let work = self.work.take().into_iter().flatten();
        for id in self.scene.take().into_iter().chain(work) {
            res.destroy_buffer(id);
        }
        self.source = None;
        self.matrix = None;
        self.done = 0;
        self.enveloped = false;
    }

    /// Записывает проход «кэш солнца»: квадраты кадра — расчёт в буфер глубин и
    /// копирование в левую половину картинки; когда глубина готова — проходы запаса и
    /// копирование его в правую половину. После него картинка снова в раскладке для чтения
    /// шейдерами.
    ///
    /// # Safety
    /// `cmd` записывается; `pipeline` (`sun_bake.slang`), `envelope` (`sun_envelope.slang`)
    /// и `layout` живы; буферы и картинка `frame` живы, пока кадр выполняется.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn record(
        d: &ash::Device,
        cmd: vk::CommandBuffer,
        bindless: &Bindless,
        layout: vk::PipelineLayout,
        pipeline: vk::Pipeline,
        envelope: vk::Pipeline,
        frame: &SunFrame,
    ) {
        if frame.count == 0 && frame.passes == 0 {
            return;
        }
        let shaders =
            vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COMPUTE_SHADER;
        let storage =
            vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE;
        // Прошлый кадр копировал из буферов; проходы этого кадра читают и пишут их по очереди.
        let compute_after = |src: vk::PipelineStageFlags2, access: vk::AccessFlags2| {
            [vk::MemoryBarrier2::default()
                .src_stage_mask(src)
                .src_access_mask(access)
                .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                .dst_access_mask(storage)]
        };
        let before = compute_after(
            vk::PipelineStageFlags2::COPY | vk::PipelineStageFlags2::COMPUTE_SHADER,
            vk::AccessFlags2::TRANSFER_READ | storage,
        );
        let between = compute_after(vk::PipelineStageFlags2::COMPUTE_SHADER, storage);
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let to_copy_buffer = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)];
        // Картинку прошлые кадры читали шейдерами; в первом квадрате печи её прошлое
        // содержимое не нужно.
        let to_copy_image = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(shaders)
            .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(if frame.fresh {
                vk::ImageLayout::UNDEFINED
            } else {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            })
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .image(frame.image)
            .subresource_range(range)];
        let to_read = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(shaders)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(frame.image)
            .subresource_range(range)];
        let inverse = frame.matrix.inverse();
        let across = frame.side.div_ceil(frame.tile);
        let region = |x: u32, y: u32, width: u32, height: u32, offset: u64| {
            vk::BufferImageCopy::default()
                .buffer_offset(offset)
                .buffer_row_length(frame.side)
                .buffer_image_height(frame.side)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_offset(vk::Offset3D {
                    x: x as i32,
                    y: y as i32,
                    z: 0,
                })
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                })
        };
        let mut copies = Vec::with_capacity(frame.count as usize);
        let groups = frame.side.div_ceil(8);
        // SAFETY: условия функции; барьеры упорядочивают буферы и картинку с прошлыми
        // кадрами, проходы этого кадра между собой и чтение в этом кадре; квадраты лежат
        // внутри карты (обрезаны по её стороне), буферы — на всю карту, картинка — на две.
        unsafe {
            d.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&before));
            if frame.count > 0 {
                bindless.bind_compute(d, cmd);
                d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
            }
            for &tile in &frame.tiles[..frame.count as usize] {
                let (x, y) = ((tile % across) * frame.tile, (tile / across) * frame.tile);
                let push = bake_push(
                    inverse,
                    frame.scene,
                    frame.side,
                    [x, y, 0, frame.tile],
                    frame.addresses[0],
                );
                d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                let tile_groups = frame.tile.div_ceil(8);
                d.cmd_dispatch(cmd, tile_groups, tile_groups, 1);
                let width = frame.tile.min(frame.side - x);
                let height = frame.tile.min(frame.side - y);
                let offset = (u64::from(y) * u64::from(frame.side) + u64::from(x)) * 4;
                copies.push(region(x, y, width, height, offset));
            }
            if frame.passes > 0 {
                d.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().memory_barriers(&between),
                );
                d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, envelope);
                for (j, (step, radius)) in envelope_steps(frame.passes).enumerate() {
                    let src = if j == 0 { 0 } else { 1 + (j + 1) % 2 };
                    let dst = 1 + j % 2;
                    // Новое в окне — дальше `radius` точек.
                    let push = envelope_push(
                        frame.addresses[src],
                        frame.addresses[dst],
                        frame.addresses[3],
                        frame.side,
                        step,
                        frame.cone_texel * (radius + 1) as f32,
                        j == 0,
                    );
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_dispatch(cmd, groups, groups, 1);
                    d.cmd_pipeline_barrier2(
                        cmd,
                        &vk::DependencyInfo::default().memory_barriers(&between),
                    );
                }
            }
            d.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .memory_barriers(&to_copy_buffer)
                    .image_memory_barriers(&to_copy_image),
            );
            if !copies.is_empty() {
                d.cmd_copy_buffer_to_image(
                    cmd,
                    frame.depth,
                    frame.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &copies,
                );
            }
            if frame.passes > 0 {
                d.cmd_copy_buffer_to_image(
                    cmd,
                    frame.envelope,
                    frame.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region(frame.side, 0, frame.side, frame.side, 0)],
                );
            }
            d.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_read),
            );
        }
    }
}

/// Сколько проходов запаса нужно и жёсткость конуса на точку карты: окно растёт, пока
/// полутень от самого высокого неподвижного может дотянуться дальше него.
fn envelope_passes(matrix: glam::Mat4, side: u32) -> (u32, f32) {
    let inverse = matrix.inverse();
    let sun = reyvec_sdf::view3d::SUN;
    let depth = |z: f32| {
        -inverse
            .project_point3(glam::Vec3::new(0.0, 0.0, z))
            .dot(sun)
    };
    let range = (depth(0.0) - depth(1.0)).abs();
    let texel = 2.0 / (matrix.row(0).truncate().length() * side as f32);
    let cone_texel = CONE * texel;
    let reach = (range / cone_texel.max(1e-9)).min(side as f32);
    let passes = envelope_steps(64)
        .position(|(step, radius)| (radius + step) as f32 >= reach)
        .map_or(64, |k| k as u32 + 1);
    (passes, cone_texel)
}

/// Шаги проходов запаса: (шаг, радиус окна до прохода). Окно — квадрат радиуса R; проход
/// с шагом s берёт меньшее из девяти окон со сдвигом на s и даёт окно радиуса R + s (без
/// дыр, пока s ≤ 2R + 1). Шаг — половина радиуса: окно растёт в полтора раза, а не вдвое,
/// и запас точнее (заслоняющее в новой кайме считается на расстоянии R + 1, а не дальше).
fn envelope_steps(passes: u32) -> impl Iterator<Item = (u32, u32)> {
    let mut radius = 0u32;
    (0..passes).map(move |_| {
        let step = radius.div_ceil(2).max(1);
        let before = radius;
        radius += step;
        (step, before)
    })
}

/// Константы `sun_envelope.slang`.
fn envelope_push(
    src: vk::DeviceAddress,
    dst: vk::DeviceAddress,
    env: vk::DeviceAddress,
    side: u32,
    step: u32,
    add: f32,
    first: bool,
) -> [u8; 40] {
    let mut push = [0u8; 40];
    for (k, a) in [src, dst, env].into_iter().enumerate() {
        push[8 * k..8 * k + 8].copy_from_slice(&a.to_ne_bytes());
    }
    push[24..28].copy_from_slice(&side.to_ne_bytes());
    push[28..32].copy_from_slice(&step.to_ne_bytes());
    push[32..36].copy_from_slice(&add.to_ne_bytes());
    push[36..40].copy_from_slice(&u32::from(first).to_ne_bytes());
    push
}

/// Константы `sun_bake.slang` в раскладке Push из `include/space.slang`: обратная матрица
/// карты, адрес сцены, сторона карты (flags), угол квадрата и сторона квадрата (gbuffer,
/// shadows, cone; target не нужен), адрес буфера глубин всей карты (edges).
fn bake_push(
    inverse: glam::Mat4,
    scene: vk::DeviceAddress,
    side: u32,
    tile: [u32; 4],
    staging: vk::DeviceAddress,
) -> [u8; 128] {
    let mut push = [0u8; 128];
    for (i, v) in inverse.to_cols_array().into_iter().enumerate() {
        push[4 * i..4 * i + 4].copy_from_slice(&v.to_ne_bytes());
    }
    push[80..88].copy_from_slice(&scene.to_ne_bytes());
    push[96..100].copy_from_slice(&side.to_ne_bytes());
    for (i, w) in tile.into_iter().enumerate() {
        push[100 + 4 * i..104 + 4 * i].copy_from_slice(&w.to_ne_bytes());
    }
    push[120..128].copy_from_slice(&staging.to_ne_bytes());
    push
}

impl Default for SunCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_layout_matches_space_push() {
        let push = bake_push(
            glam::Mat4::IDENTITY,
            0x1234,
            4096,
            [512, 1024, 2, 512],
            0x99,
        );
        let env = envelope_push(1, 2, 3, 4096, 8, 0.5, true);
        assert_eq!(u64::from_ne_bytes(env[16..24].try_into().unwrap()), 3);
        assert_eq!(f32::from_ne_bytes(env[32..36].try_into().unwrap()), 0.5);
        assert_eq!(u32::from_ne_bytes(env[36..40].try_into().unwrap()), 1);
        assert_eq!(f32::from_ne_bytes(push[0..4].try_into().unwrap()), 1.0);
        assert_eq!(u64::from_ne_bytes(push[80..88].try_into().unwrap()), 0x1234);
        let word = |at: usize| u32::from_ne_bytes(push[at..at + 4].try_into().unwrap());
        assert_eq!(
            [word(96), word(100), word(104), word(108), word(112)],
            [4096, 512, 1024, 2, 512]
        );
        assert_eq!(u64::from_ne_bytes(push[120..128].try_into().unwrap()), 0x99);
    }

    /// Проходов запаса хватает, чтобы окно дотянулось до полутени самого высокого
    /// неподвижного: радиус окна не меньше разницы глубин / (CONE · точка).
    #[test]
    fn envelope_reaches_the_tallest_penumbra() {
        let matrix = glam::Mat4::orthographic_rh(-5.0, 5.0, -5.0, 5.0, 1.0, 4.0)
            * glam::Mat4::look_at_rh(glam::Vec3::ZERO, -reyvec_sdf::view3d::SUN, glam::Vec3::Y);
        let (passes, cone_texel) = envelope_passes(matrix, 4096);
        let texel = 10.0 / 4096.0;
        assert!((cone_texel - CONE * texel).abs() < 1e-6);
        let reach = 3.0 / cone_texel;
        let radius = |n: u32| envelope_steps(n).map(|(s, _)| s).sum::<u32>();
        assert!(radius(passes) as f32 >= reach, "{passes} проходов");
        assert!((radius(passes - 1) as f32) < reach);
        // Окно без дыр: шаг не больше 2R + 1; растёт в полтора раза.
        for (step, before) in envelope_steps(20) {
            assert!(step <= 2 * before + 1);
        }
        assert_eq!(radius(6), 12);
    }

    #[test]
    fn tiles_cover_the_map() {
        let mut cache = SunCache::new();
        assert_eq!(cache.tiles_total(), 64);
        cache.side = 1000;
        cache.tile = 512;
        assert_eq!(cache.tiles_total(), 4);
    }
}
