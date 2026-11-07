//! Запечённое поле расстояний сцены «3D» на видеокарте (7.10б,
//! `docs/decisions/0019-baked-distance-field.md`).
//!
//! Процессор решает, какие кубики пересчитать ([`reyvec_sdf::field3d::FieldWindow`]:
//! вошедшие в слой при полёте камеры и лежащие у сдвинутых предметов), и пишет их
//! заданием кадра вместе с формами и списками предметов у каждого кубика. Проход «поле»
//! (`field_bake.slang`) считает эти кубики в общий буфер поля до лучей кадра. Поле
//! считается по всем предметам сцены, без отсева по взгляду: предмет за спиной камеры
//! тоже даёт тень.

use ash::vk;
use reyvec_sdf::Program;
use reyvec_sdf::field3d::{BrickJob, FIELD_LEVELS, FieldWindow, brick_objects, changed_bounds};
use reyvec_sdf::shape3d::pack_scene;

use crate::context::GpuContext;
use crate::resources::{BufferId, BufferMemory, GpuResources};

/// Слов в заголовке задания кадра (раскладка — в `field_bake.slang`).
const HEADER_WORDS: usize = 4;
/// Слов на запись кубика.
const JOB_WORDS: usize = 8;

/// Задание кадра для `field_bake.slang`: заголовок, записи кубиков, списки предметов,
/// формы (`pack_scene` без материалов и ламп). `size` — точек по стороне слоя.
pub(crate) fn pack_jobs(programs: &[Program], jobs: &[BrickJob], size: i32) -> Vec<u32> {
    let lists: Vec<Vec<u32>> = jobs.iter().map(|j| brick_objects(programs, j)).collect();
    let jobs_at = HEADER_WORDS;
    let lists_at = jobs_at + jobs.len() * JOB_WORDS;
    let scene_at = lists_at + lists.iter().map(Vec::len).sum::<usize>();
    let mut out = vec![
        jobs.len() as u32,
        jobs_at as u32,
        scene_at as u32,
        size as u32,
    ];
    let mut at = lists_at;
    for (job, list) in jobs.iter().zip(&lists) {
        out.push(job.level as u32);
        out.extend(job.first.to_array().map(|v| v as u32));
        out.extend([at as u32, list.len() as u32, 0, 0]);
        at += list.len();
    }
    for list in &lists {
        out.extend(list);
    }
    out.extend(pack_scene(programs, &[], &[]));
    out
}

/// Что проходу «поле» нужно в этом кадре.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FieldFrame {
    pub jobs: vk::DeviceAddress,
    pub field: vk::DeviceAddress,
    /// Сколько кубиков посчитать (0 — поле уже верное, проход ничего не запускает).
    pub count: u32,
    /// Буфер поля: из него посчитанные кубики копируются в слои-картинки.
    pub buffer: vk::Buffer,
    /// Слои-картинки (7.10е), из них шейдеры читают поле.
    pub volumes: [vk::Image; FIELD_LEVELS],
    /// Слои только что созданы: перевести в раскладку GENERAL.
    pub fresh: bool,
}

/// Что скопировать из буфера поля в слои в этом кадре: по слою список кусков.
pub(crate) type FieldCopies = std::sync::Arc<Vec<Vec<vk::BufferImageCopy>>>;

/// Куски копирования буфера поля в слои-картинки: по куску на кубик, или весь слой, если
/// кубик не помещается в слой целиком (на стыке кольца).
fn brick_copies(jobs: &[BrickJob], size: i32) -> Vec<Vec<vk::BufferImageCopy>> {
    let cells = (size as u64).pow(3);
    let brick = reyvec_sdf::field3d::BRICK;
    let layers = vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1);
    let region = |level: usize, q: glam::IVec3, extent: u32| {
        let slot = ((q.z * size + q.y) * size + q.x) as u64;
        vk::BufferImageCopy::default()
            .buffer_offset((level as u64 * cells + slot) * 2)
            .buffer_row_length(size as u32)
            .buffer_image_height(size as u32)
            .image_subresource(layers)
            .image_offset(vk::Offset3D {
                x: q.x,
                y: q.y,
                z: q.z,
            })
            .image_extent(vk::Extent3D {
                width: extent,
                height: extent,
                depth: extent,
            })
    };
    let mut out = vec![Vec::new(); FIELD_LEVELS];
    let mut whole = [false; FIELD_LEVELS];
    for job in jobs {
        let q = job.first.rem_euclid(glam::IVec3::splat(size));
        if (q + brick).cmpgt(glam::IVec3::splat(size)).any() {
            whole[job.level] = true;
        } else {
            out[job.level].push(region(job.level, q, brick as u32));
        }
    }
    for (level, regions) in out.iter_mut().enumerate() {
        if whole[level] {
            *regions = vec![region(level, glam::IVec3::ZERO, size as u32)];
        }
    }
    out
}

/// Поле сцены «3D»: окно слоёв, буфер чисел и задания кадров в полёте.
pub(crate) struct SpaceField {
    pub window: FieldWindow,
    /// Слои подряд, по два half float в слове (16 МБ). Создаётся при первой надобности.
    buffer: Option<BufferId>,
    /// Задание кадра, своё у каждого кадра в полёте (буфер и его размер в байтах).
    jobs: Vec<Option<(BufferId, u64)>>,
    /// Формы прошлого кадра с полем: по ним видно, какие предметы сдвинулись. `None` —
    /// прошлый кадр поля не считал (выключено, другая сцена): пересчитать всё.
    previous: Option<Vec<Program>>,
    /// Слои-картинки (7.10е); `None` — ещё не созданы.
    volumes: Option<[vk::Image; FIELD_LEVELS]>,
    /// Куски копирования в слои последнего подготовленного кадра.
    pub copies: FieldCopies,
    /// Включено ли поле (настройка в панели).
    pub enabled: bool,
    /// Кубиков в последнем кадре (для журнала и панели).
    pub last_bricks: usize,
    /// Дальние пробы затенения в углах берут число поля (7.10г, настройка в панели).
    pub ao_from_field: bool,
}

impl SpaceField {
    pub fn new(frames_in_flight: usize, size: i32) -> Self {
        SpaceField {
            window: FieldWindow::new(size),
            buffer: None,
            jobs: vec![None; frames_in_flight],
            previous: None,
            volumes: None,
            copies: FieldCopies::default(),
            enabled: false,
            last_bricks: 0,
            ao_from_field: false,
        }
    }

    /// Буфер поля, если он уже есть.
    pub fn buffer(&self) -> Option<BufferId> {
        self.buffer
    }

    /// Кадр без поля: в следующем кадре с полем всё считается заново.
    pub fn skip(&mut self) {
        self.previous = None;
    }

    fn field_bytes(&self) -> u64 {
        let size = u64::from(self.window.size.unsigned_abs());
        size * size * size * FIELD_LEVELS as u64 * 2
    }

    /// Готовит задание кадра `slot`: камера в `eye`, формы сцены `programs` (все, без
    /// отсева по взгляду). `None` — поле выключено или не вышло создать буфер.
    pub fn prepare(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        slot: usize,
        eye: glam::Vec3,
        programs: Vec<Program>,
    ) -> Option<FieldFrame> {
        if !self.enabled {
            self.skip();
            return None;
        }
        if self.buffer.is_none() {
            match res.create_buffer(
                ctx,
                self.field_bytes(),
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
                BufferMemory::GpuOnly,
                "поле расстояний 3D",
            ) {
                Ok(id) => self.buffer = Some(id),
                Err(e) => {
                    tracing::warn!("буфер поля расстояний: {e}");
                    self.enabled = false;
                    return None;
                }
            }
        }
        let mut fresh = false;
        if self.volumes.is_none() {
            let size = self.window.size.unsigned_abs();
            match res.field_volumes(ctx, size, FIELD_LEVELS as u32) {
                Ok(v) if v.len() == FIELD_LEVELS => {
                    self.volumes = Some(std::array::from_fn(|i| v[i]));
                    fresh = true;
                    // Слои пустые: всё поле считается заново.
                    self.window.reset();
                    self.previous = None;
                }
                Ok(_) => {
                    tracing::warn!("слои поля расстояний: не все созданы");
                    self.enabled = false;
                    return None;
                }
                Err(e) => {
                    tracing::warn!("слои поля расстояний: {e}");
                    self.enabled = false;
                    return None;
                }
            }
        }
        let changed = self
            .previous
            .as_ref()
            .and_then(|before| changed_bounds(before, &programs));
        let jobs = match &changed {
            Some(c) => self.window.plan(eye, c, false),
            None => self.window.plan(eye, &[], true),
        };
        let words = pack_jobs(&programs, &jobs, self.window.size);
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
        let entry = self.jobs.get_mut(slot)?;
        let need = bytes.len() as u64;
        if entry.is_some_and(|(_, have)| have < need)
            && let Some((id, _)) = entry.take()
        {
            res.destroy_buffer(id);
        }
        if entry.is_none() {
            // С запасом: задания растут, когда камера летит быстрее.
            let size = need.next_power_of_two().max(64 * 1024);
            match res.create_buffer(
                ctx,
                size,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                BufferMemory::CpuToGpu,
                &format!("поле 3D: задание кадра {slot}"),
            ) {
                Ok(id) => *entry = Some((id, size)),
                Err(e) => {
                    tracing::warn!("задание поля расстояний: {e}");
                    // Кубики этого кадра не посчитаны: в следующем — всё заново.
                    self.window.reset();
                    self.previous = None;
                    return None;
                }
            }
        }
        let (id, _) = (*entry)?;
        res.write_buffer(id, 0, &bytes);
        self.previous = Some(programs);
        self.last_bricks = jobs.len();
        let field_buffer = self.buffer?;
        self.copies = std::sync::Arc::new(brick_copies(&jobs, self.window.size));
        Some(FieldFrame {
            jobs: res.buffer_address(id),
            field: res.buffer_address(field_buffer),
            count: jobs.len() as u32,
            buffer: res.buffer_raw(field_buffer)?,
            volumes: self.volumes?,
            fresh,
        })
    }

    /// Записывает проход «поле»: барьер от чтения поля прошлым кадром, расчёт кубиков,
    /// барьер до чтения этим кадром.
    ///
    /// # Safety
    /// `cmd` записывается; `pipeline` (`field_bake.slang`) и `layout` живы; буферы
    /// `frame` живы, пока кадр выполняется.
    pub unsafe fn record(
        d: &ash::Device,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        pipeline: vk::Pipeline,
        frame: &FieldFrame,
        copies: &[Vec<vk::BufferImageCopy>],
    ) {
        if frame.count == 0 {
            return;
        }
        let shaders =
            vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COMPUTE_SHADER;
        // До расчёта: прошлые кадры читали буфер поля (копирование в слои).
        let before = [vk::MemoryBarrier2::default()
            .src_stage_mask(shaders | vk::PipelineStageFlags2::COPY)
            .src_access_mask(
                vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::TRANSFER_READ,
            )
            .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)];
        // После расчёта: буфер поля читает копирование в слои (и чтение в тестах), а
        // слои, которые прошлые кадры читали шейдерами, перезаписывает копирование.
        let to_copy = [
            vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ),
            vk::MemoryBarrier2::default()
                .src_stage_mask(shaders)
                .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE),
        ];
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let fresh: Vec<vk::ImageMemoryBarrier2> = if frame.fresh {
            frame
                .volumes
                .iter()
                .map(|&image| {
                    vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::NONE)
                        .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                        .old_layout(vk::ImageLayout::UNDEFINED)
                        .new_layout(vk::ImageLayout::GENERAL)
                        .image(image)
                        .subresource_range(range)
                })
                .collect()
        } else {
            Vec::new()
        };
        let after = [
            vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
                .dst_stage_mask(shaders | vk::PipelineStageFlags2::COPY)
                .dst_access_mask(
                    vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::TRANSFER_READ,
                ),
            vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(shaders)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ),
        ];
        let mut push = [0u8; 16];
        push[..8].copy_from_slice(&frame.jobs.to_ne_bytes());
        push[8..].copy_from_slice(&frame.field.to_ne_bytes());
        // SAFETY: условия функции; барьеры упорядочивают буфер и слои поля с прошлым и
        // этим кадром; куски копирования лежат внутри буфера и слоёв (brick_copies).
        unsafe {
            d.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&before));
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
            d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
            d.cmd_dispatch(cmd, frame.count, 1, 1);
            d.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .memory_barriers(&to_copy)
                    .image_memory_barriers(&fresh),
            );
            for (image, regions) in frame.volumes.iter().zip(copies) {
                if !regions.is_empty() {
                    d.cmd_copy_buffer_to_image(
                        cmd,
                        frame.buffer,
                        *image,
                        vk::ImageLayout::GENERAL,
                        regions,
                    );
                }
            }
            d.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&after));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_sdf::field3d::FieldWindow;
    use reyvec_sdf::shape3d::compile;

    fn programs() -> Vec<Program> {
        let scene = reyvec_sdf::SpaceScene::parse(reyvec_sdf::space_file::EXAMPLE).unwrap();
        scene
            .frame(2.0)
            .shapes
            .iter()
            .map(|s| compile(s).unwrap())
            .collect()
    }

    #[test]
    fn jobs_layout_matches_the_shader() {
        let programs = programs();
        let mut window = FieldWindow::new(16);
        let jobs = window.plan(glam::Vec3::new(1.0, 1.0, 1.0), &[], true);
        let words = pack_jobs(&programs, &jobs, 16);
        assert_eq!(words[0] as usize, jobs.len());
        assert_eq!(words[3], 16);
        let scene = &words[words[2] as usize..];
        assert_eq!(scene[0] as usize, programs.len(), "формы после списков");
        for (k, job) in jobs.iter().enumerate() {
            let at = words[1] as usize + k * JOB_WORDS;
            assert_eq!(words[at] as usize, job.level);
            let first = [1, 2, 3].map(|i| words[at + i] as i32);
            assert_eq!(first, job.first.to_array());
            let (start, count) = (words[at + 4] as usize, words[at + 5] as usize);
            let list = &words[start..start + count];
            assert_eq!(list, brick_objects(&programs, job).as_slice());
            assert!(list.iter().all(|&o| (o as usize) < programs.len()));
        }
    }
}
