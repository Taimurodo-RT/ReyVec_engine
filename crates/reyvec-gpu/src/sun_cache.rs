//! Кэш солнца (опыт Р2, «Р2» в docs/3d-beyond-triangles.md): тень солнца от неподвижных
//! предметов печётся один раз в большую карту и потом только читается.
//!
//! Как у Quake: свет неподвижного не пересчитывается каждый кадр. Каждый кадр луч тени
//! солнца шагает по всем предметам сцены, хотя у завода двигается малая часть (ящики на
//! лентах, погрузчики, вентиляторы). С кэшем внутри карты тень солнца — меньшее из карты
//! (неподвижное) и живого луча только по движущимся предметам: мягкая тень — минимум по
//! предметам, поэтому сумма та же.
//!
//! Карта — R32F [`SIDE`]² точек, прямоугольная проекция вдоль солнца на все неподвижные
//! предметы (`space::sun_cache_view_proj`); читает её тот же код, что карты солнца режима
//! «треугольники» (`map_shadow`). Печёт её проход «кэш солнца» (`sun_bake.slang`) по
//! отдельной сцене из одних неподвижных предметов: квадратами [`TILE`]² по
//! [`TILES_PER_FRAME`] за кадр. Пока карта не допечена, тени идут, как без кэша. Если
//! неподвижное поменялось (другая сцена, правка в редакторе), карта печётся заново.

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
    staging: vk::Buffer,
    staging_address: vk::DeviceAddress,
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
    /// Буфер квадратов кадра и на сколько квадратов он.
    staging: Option<(BufferId, u32)>,
    /// Сколько квадратов уже испечено.
    done: u32,
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
            staging: None,
            done: 0,
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
                self.side,
                self.side,
                vk::Format::R32_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                "кэш солнца",
            ) {
                Ok(id) => {
                    self.texture = Some(id);
                    self.done = 0;
                }
                Err(e) => {
                    tracing::warn!("картинка кэша солнца: {e}");
                    self.enabled = false;
                    return None;
                }
            }
        }
        let per_frame = self.per_frame.clamp(1, TILES_PER_FRAME * 4);
        if self.staging.is_some_and(|(_, tiles)| tiles < per_frame)
            && let Some((old, _)) = self.staging.take()
        {
            res.destroy_buffer(old);
        }
        if self.staging.is_none() {
            let bytes = u64::from(per_frame) * u64::from(self.tile * self.tile) * 4;
            match res.create_buffer(
                ctx,
                bytes,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
                BufferMemory::GpuOnly,
                "кэш солнца: квадраты кадра",
            ) {
                Ok(id) => self.staging = Some((id, per_frame)),
                Err(e) => {
                    tracing::warn!("буфер кэша солнца: {e}");
                    self.enabled = false;
                    return None;
                }
            }
        }
        let texture = self.texture?;
        let (staging, _) = self.staging?;
        let total = self.tiles_total();
        let mut tiles = [0u32; TILES_PER_FRAME as usize * 4];
        let fresh = self.done == 0;
        let mut count = 0;
        while self.done < total && count < per_frame {
            tiles[count as usize] = self.done;
            self.done += 1;
            count += 1;
        }
        Some(SunFrame {
            scene: res.buffer_address(scene),
            matrix,
            slot: texture.0,
            ready: self.done == total,
            image: res.texture_image(texture)?,
            staging: res.buffer_raw(staging)?,
            staging_address: res.buffer_address(staging),
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
        let staging = self.staging.take().map(|(id, _)| id);
        for id in [self.scene.take(), staging].into_iter().flatten() {
            res.destroy_buffer(id);
        }
        self.source = None;
        self.matrix = None;
        self.done = 0;
    }

    /// Записывает проход «кэш солнца»: квадраты кадра — расчёт в буфер, копирование в
    /// картинку карты; после него картинка снова в раскладке для чтения шейдерами.
    ///
    /// # Safety
    /// `cmd` записывается; `pipeline` (`sun_bake.slang`) и `layout` живы; буферы и картинка
    /// `frame` живы, пока кадр выполняется.
    pub unsafe fn record(
        d: &ash::Device,
        cmd: vk::CommandBuffer,
        bindless: &Bindless,
        layout: vk::PipelineLayout,
        pipeline: vk::Pipeline,
        frame: &SunFrame,
    ) {
        if frame.count == 0 {
            return;
        }
        let shaders =
            vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COMPUTE_SHADER;
        // Прошлый кадр копировал из буфера квадратов.
        let before = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)];
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
        let words_per_tile = u64::from(frame.tile * frame.tile);
        let mut copies = Vec::with_capacity(frame.count as usize);
        // SAFETY: условия функции; барьеры упорядочивают буфер квадратов и картинку с
        // прошлыми кадрами и чтением в этом кадре; квадраты лежат внутри буфера (его
        // размер — квадратов за кадр) и внутри картинки (обрезаны по её стороне).
        unsafe {
            d.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&before));
            bindless.bind_compute(d, cmd);
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
            for (index, &tile) in frame.tiles[..frame.count as usize].iter().enumerate() {
                let (x, y) = ((tile % across) * frame.tile, (tile / across) * frame.tile);
                let push = bake_push(
                    inverse,
                    frame.scene,
                    frame.side,
                    [x, y, index as u32, frame.tile],
                    frame.staging_address,
                );
                d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                let groups = frame.tile.div_ceil(8);
                d.cmd_dispatch(cmd, groups, groups, 1);
                let width = frame.tile.min(frame.side - x);
                let height = frame.tile.min(frame.side - y);
                copies.push(
                    vk::BufferImageCopy::default()
                        .buffer_offset(index as u64 * words_per_tile * 4)
                        .buffer_row_length(frame.tile)
                        .buffer_image_height(frame.tile)
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
                        }),
                );
            }
            d.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .memory_barriers(&to_copy_buffer)
                    .image_memory_barriers(&to_copy_image),
            );
            d.cmd_copy_buffer_to_image(
                cmd,
                frame.staging,
                frame.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &copies,
            );
            d.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_read),
            );
        }
    }
}

/// Константы `sun_bake.slang` в раскладке Push из `include/space.slang`: обратная матрица
/// карты, адрес сцены, сторона карты (flags), угол квадрата, место в буфере и сторона
/// квадрата (gbuffer, shadows, target, cone), адрес буфера квадратов (edges).
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
        assert_eq!(f32::from_ne_bytes(push[0..4].try_into().unwrap()), 1.0);
        assert_eq!(u64::from_ne_bytes(push[80..88].try_into().unwrap()), 0x1234);
        let word = |at: usize| u32::from_ne_bytes(push[at..at + 4].try_into().unwrap());
        assert_eq!(
            [word(96), word(100), word(104), word(108), word(112)],
            [4096, 512, 1024, 2, 512]
        );
        assert_eq!(u64::from_ne_bytes(push[120..128].try_into().unwrap()), 0x99);
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
