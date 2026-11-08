//! Рисование без окна: кадр рисуется в картинку в памяти видеокарты и читается обратно.
//! Нужно для автотестов и CI, где окна нет.

use ash::vk;

use crate::context::{Core, GpuContext, GpuOptions};
use crate::demo::{Demo, DemoScene, Msaa};
use crate::graph::{Access, ImageState, RenderGraph, TransientPool};
use crate::pipeline::color_range;
use crate::resources::GpuResources;
use crate::shaders::ShaderLibrary;
use crate::timing::{GpuFrameTimings, GpuTimer};
use crate::{AdapterInfo, GpuError, VkResultExt};

/// Картинка в sRGB, как окно: в файле и на экране цвета одинаковые.
const FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;

/// Картинка кадра: пиксели RGBA по 8 бит в sRGB (как на экране), строка за строкой сверху вниз.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameImage {
    /// Ширина в пикселях.
    pub width: u32,
    /// Высота в пикселях.
    pub height: u32,
    /// Пиксели RGBA, `width * height * 4` байт.
    pub rgba: Vec<u8>,
}

impl FrameImage {
    /// Цвет пикселя `(x, y)`, отсчёт от левого верхнего угла.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// Какой кадр песочницы нарисовать без окна.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadlessScene {
    /// Размер картинки в пикселях.
    pub width: u32,
    pub height: u32,
    /// Момент времени сцены, секунды с запуска.
    pub time: f32,
    /// Нарисовать красную рамку ошибки шейдера.
    pub shader_error: bool,
    /// Какая сцена песочницы.
    pub scene: DemoScene,
    /// Сглаживание (если видеокарта не умеет столько, берётся ближайшее меньшее).
    pub msaa: Msaa,
    /// Края при MSAA «как в редакторах» (`true`) или физически точные.
    pub perceptual_edges: bool,
    /// Как рисовать залитые векторные фигуры.
    pub fill: crate::FillMode,
    /// Лупа векторной сцены (увеличение в 8 раз у края сердца).
    pub zoom: bool,
    /// Свои файлы для сцен «SVG» и «Motif»; без них там примеры.
    pub files: crate::UserFiles,
    /// Опыт 4.6: свет каскадами лучей вместо лучей к источникам.
    pub cascades: bool,
}

/// Сценарий игры для кадра без окна (проверка в движении, 6.4): где герой сцены «из
/// файла» встаёт в начале и что игрок нажимает. Мир шагает шагами логики (60 в секунду)
/// от нуля до момента кадра, как в окне, только без потока.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeadlessPlay {
    /// Куда поставить героя в начале (середина его прямоугольника).
    pub start: Option<[f64; 2]>,
    /// Нажатия и отпускания по времени.
    pub inputs: Vec<PlayInput>,
    /// Фонтан частиц для замера (6.6) с начала сценария.
    pub fountain: bool,
}

/// Нажатие (`down`) или отпускание действия в момент `at`, секунды.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayInput {
    pub at: f64,
    pub action: reyvec_scene::game::Action,
    pub down: bool,
}

impl PlayInput {
    /// Держать `action` от `from` до `to`.
    pub fn hold(action: reyvec_scene::game::Action, from: f64, to: f64) -> [Self; 2] {
        [
            Self {
                at: from,
                action,
                down: true,
            },
            Self {
                at: to,
                action,
                down: false,
            },
        ]
    }
}

/// Кадр, нарисованный без окна.
pub struct HeadlessImage {
    /// Сама картинка.
    pub image: FrameImage,
    /// На какой видеокарте нарисовано.
    pub adapter: AdapterInfo,
    /// Сколько видеокарта потратила на кадр (нет, если видеокарта не умеет замерять).
    pub gpu_timings: Option<GpuFrameTimings>,
    /// Камера кадра: середина и приближение.
    pub camera: ([f64; 2], f64),
    /// Герой сцены «из файла» в этом кадре.
    pub player: Option<reyvec_scene::PlayerView>,
    /// Частицы в этом кадре (6.6).
    pub particles: crate::ParticleStats,
    /// Запечённое поле расстояний сцены «3D» после кадра (7.10б), если оно считалось.
    pub field: Option<FieldSnapshot>,
    /// Счётчики сцены «3D» или 2.5D-вида этого кадра (7.18б): объекты и слова сцены,
    /// время процессора, пиксели края (прочитаны после кадра). `None` — кадр плоский.
    pub space_stats: Option<crate::SpaceStats>,
}

/// Запечённое поле расстояний, прочитанное с видеокарты: где стоят слои и числа.
pub struct FieldSnapshot {
    pub window: reyvec_sdf::field3d::FieldWindow,
    /// Слои подряд, по `size³` half float.
    pub halves: Vec<u16>,
}

impl FieldSnapshot {
    /// Число в точке с мировым номером `w` слоя `level`.
    pub fn value(&self, level: usize, w: glam::IVec3) -> f32 {
        let size = self.window.size as usize;
        half_to_f32(self.halves[level * size * size * size + self.window.slot(w)])
    }
}

/// half float (IEEE 754, 16 бит) в f32.
fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let frac = f32::from(h & 0x3ff);
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

impl HeadlessImage {
    /// Пиксель под точкой сцены `p` (с учётом камеры), если он в кадре.
    pub fn scene_pixel(&self, p: [f64; 2]) -> Option<[u8; 4]> {
        let (w, h) = (f64::from(self.image.width), f64::from(self.image.height));
        let aspect = w / h;
        let ([cx, cy], zoom) = self.camera;
        let sx = (p[0] - cx) * zoom;
        let sy = (p[1] - cy) * zoom;
        let x = ((sx / aspect + 1.0) * 0.5 * w).floor();
        let y = ((1.0 - sy) * 0.5 * h).floor();
        ((0.0..w).contains(&x) && (0.0..h).contains(&y))
            .then(|| self.image.pixel(x as u32, y as u32))
    }
}

impl HeadlessImage {
    /// Цвет пикселя `(x, y)`, отсчёт от левого верхнего угла.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        self.image.pixel(x, y)
    }
}

/// Объекты, которые нужно убрать при любом исходе. Пустые дескрипторы убирать можно.
struct Resources<'a> {
    ctx: &'a GpuContext,
    image: vk::Image,
    image_memory: vk::DeviceMemory,
    view: vk::ImageView,
    buffer: vk::Buffer,
    buffer_memory: vk::DeviceMemory,
    pool: vk::CommandPool,
    fence: vk::Fence,
    gpu: Option<GpuResources>,
    demo: Option<Demo>,
    transients: TransientPool,
    timer: Option<GpuTimer>,
}

impl Drop for Resources<'_> {
    fn drop(&mut self) {
        let d = &self.ctx.device;
        // SAFETY: ждём видеокарту, затем уничтожаем; null-дескрипторы допустимы.
        unsafe {
            let _ = d.device_wait_idle();
            if let Some(demo) = self.demo.take() {
                demo.destroy(self.ctx);
            }
            self.transients.destroy(self.ctx);
            if let Some(mut timer) = self.timer.take() {
                timer.destroy(self.ctx);
            }
            if let Some(gpu) = self.gpu.take() {
                gpu.destroy(self.ctx);
            }
            d.destroy_fence(self.fence, None);
            d.destroy_command_pool(self.pool, None);
            d.destroy_buffer(self.buffer, None);
            d.free_memory(self.buffer_memory, None);
            d.destroy_image_view(self.view, None);
            d.destroy_image(self.image, None);
            d.free_memory(self.image_memory, None);
        }
    }
}

/// Рисует кадр сцены этапа 2 (треугольник и спрайты в момент `time = 0`) в картинку `width × height`.
pub fn render_triangle_headless(
    options: &GpuOptions,
    width: u32,
    height: u32,
) -> Result<HeadlessImage, GpuError> {
    render_headless(
        options,
        &HeadlessScene {
            width,
            height,
            time: 0.0,
            shader_error: false,
            scene: DemoScene::Gpu,
            msaa: Msaa::Off,
            perceptual_edges: false,
            fill: crate::FillMode::PerShape,
            zoom: false,
            files: crate::UserFiles::default(),
            cascades: false,
        },
    )
}

/// Рисует кадр песочницы без окна: тот же кадр, что в окне, только без панели счётчиков.
pub fn render_headless(
    options: &GpuOptions,
    scene: &HeadlessScene,
) -> Result<HeadlessImage, GpuError> {
    render_headless_play(options, scene, None)
}

/// Как [`render_headless`], но сцена «из файла» играется по сценарию `play` до момента
/// кадра: так проверяется, как герой в движении выглядит со светом, тенями и уровнем.
pub fn render_headless_play(
    options: &GpuOptions,
    scene: &HeadlessScene,
    play: Option<&HeadlessPlay>,
) -> Result<HeadlessImage, GpuError> {
    let core = Core::new(options.validation, None)?;
    let ctx = GpuContext::new(core, options, None).map_err(|failed| failed.0)?;
    let frame = render(&ctx, scene, play)?;
    Ok(HeadlessImage {
        image: FrameImage {
            width: scene.width,
            height: scene.height,
            rgba: frame.rgba,
        },
        adapter: ctx.adapter.clone(),
        gpu_timings: frame.timings,
        camera: frame.camera,
        player: frame.player,
        particles: frame.particles,
        field: frame.field,
        space_stats: frame.space_stats,
    })
}

struct Rendered {
    rgba: Vec<u8>,
    timings: Option<GpuFrameTimings>,
    camera: ([f64; 2], f64),
    player: Option<reyvec_scene::PlayerView>,
    particles: crate::ParticleStats,
    field: Option<FieldSnapshot>,
    space_stats: Option<crate::SpaceStats>,
}

/// Читает буфер запечённого поля после законченного кадра.
fn read_field(
    ctx: &GpuContext,
    gpu: &mut GpuResources,
    demo: &Demo,
) -> Result<Option<FieldSnapshot>, GpuError> {
    let Some((id, window)) = demo.space_field_buffer() else {
        return Ok(None);
    };
    let size = window.size as u64;
    let bytes = size * size * size * reyvec_sdf::field3d::FIELD_LEVELS as u64 * 2;
    let read = gpu.create_buffer(
        ctx,
        bytes,
        vk::BufferUsageFlags::TRANSFER_DST,
        crate::BufferMemory::GpuToCpu,
        "чтение поля",
    )?;
    gpu.copy_buffer(ctx, id, read, bytes)?;
    let data = gpu.read_buffer(read, bytes as usize);
    gpu.destroy_buffer(read);
    Ok(data.map(|data| FieldSnapshot {
        window,
        halves: data
            .chunks_exact(2)
            .map(|c| u16::from_ne_bytes([c[0], c[1]]))
            .collect(),
    }))
}

fn render(
    ctx: &GpuContext,
    scene: &HeadlessScene,
    play: Option<&HeadlessPlay>,
) -> Result<Rendered, GpuError> {
    let (width, height) = (scene.width, scene.height);
    let d = &ctx.device;
    let mut r = Resources {
        ctx,
        image: vk::Image::null(),
        image_memory: vk::DeviceMemory::null(),
        view: vk::ImageView::null(),
        buffer: vk::Buffer::null(),
        buffer_memory: vk::DeviceMemory::null(),
        pool: vk::CommandPool::null(),
        fence: vk::Fence::null(),
        gpu: None,
        demo: None,
        transients: TransientPool::new(),
        timer: None,
    };
    let extent = vk::Extent2D { width, height };
    let size = u64::from(width) * u64::from(height) * 4;
    let no_memory = GpuError::Vulkan {
        what: "подходящий тип памяти",
        result: vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
    };

    // SAFETY: все объекты создаются на этом устройстве и убираются в Drop у Resources.
    unsafe {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(FORMAT)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        r.image = d.create_image(&image_info, None).ctx("картинка")?;
        let req = d.get_image_memory_requirements(r.image);
        let mem_type = ctx
            .find_memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
            .or_else(|| {
                ctx.find_memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::empty())
            })
            .ok_or(no_memory)?;
        r.image_memory = d
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(mem_type),
                None,
            )
            .ctx("память картинки")?;
        d.bind_image_memory(r.image, r.image_memory, 0)
            .ctx("привязка памяти картинки")?;
        r.view = d
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(r.image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(FORMAT)
                    .subresource_range(color_range()),
                None,
            )
            .ctx("вид картинки")?;

        r.buffer = d
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .ctx("буфер чтения")?;
        let req = d.get_buffer_memory_requirements(r.buffer);
        let mem_type = ctx
            .find_memory_type(
                req.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or(GpuError::Vulkan {
                what: "память, видимая процессору",
                result: vk::Result::ERROR_OUT_OF_HOST_MEMORY,
            })?;
        r.buffer_memory = d
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(mem_type),
                None,
            )
            .ctx("память буфера")?;
        d.bind_buffer_memory(r.buffer, r.buffer_memory, 0)
            .ctx("привязка памяти буфера")?;

        r.gpu = Some(GpuResources::new(ctx)?);
        r.demo = Some(Demo::new(
            ctx,
            r.gpu.as_mut().expect("создано выше"),
            &ShaderLibrary::new(false),
            FORMAT,
            1,
            &scene.files,
        )?);
        // Шахматка меняет клетки со временем: загружаем ту, что нужна в момент scene.time.
        if let (Some(demo), Some(gpu)) = (r.demo.as_mut(), r.gpu.as_mut()) {
            if let Some(play) = play {
                demo.set_play(play.clone());
            }
            demo.update(ctx, gpu, scene.time)?;
            demo.set_msaa(ctx, gpu, &ShaderLibrary::new(false), scene.msaa)?;
            demo.set_perceptual_edges(scene.perceptual_edges);
            demo.set_fill(scene.fill);
            demo.set_zoom(scene.zoom);
            // Шум против полос случайно сдвигает пиксели на ступень: эталоны сравниваются без
            // него.
            demo.set_dither(false);
            demo.set_cascades(scene.cascades);
            // Тесты поля сравнивают кадры с полем и без него и с «углами из поля» и без них.
            #[cfg(test)]
            demo.set_space_field(!tests::FIELD_OFF.get());
            #[cfg(test)]
            demo.set_space_ao_field(tests::AO_FROM_FIELD.get());
            // Эталоны — в полном разрешении 3D на любой видеокарте; тесты 7.14 уменьшают.
            demo.set_space_scale(crate::SpaceScale::Full);
            #[cfg(test)]
            demo.set_space_scale(tests::SPACE_SCALE.get());
            #[cfg(test)]
            demo.set_level_view(tests::LEVEL_VIEW.get());
            demo.set_level_render(crate::LevelRender::Sdf);
            #[cfg(test)]
            demo.set_level_render(tests::LEVEL_RENDER.get());
            demo.settle_details(ctx, gpu, extent, scene.time, scene.scene)?;
        }
        r.timer = GpuTimer::new(ctx, 1)?;
        r.pool = d
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(ctx.queue_family),
                None,
            )
            .ctx("командный пул")?;
        let cmd = d
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(r.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .ctx("командный буфер")?[0];
        r.fence = d
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .ctx("забор")?;

        d.begin_command_buffer(
            cmd,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )
        .ctx("запись команд")?;
        let gpu = r.gpu.as_mut().expect("ресурсы созданы выше");
        let demo = r.demo.as_mut().expect("сцена создана выше");
        let data = demo.prepare(
            ctx,
            gpu,
            0,
            extent,
            scene.time,
            scene.shader_error,
            scene.scene,
        );
        let demo = &*demo;
        let camera = demo.camera_view();
        let player = demo.file_player();
        let particles = demo.particle_stats();
        let gpu = &*gpu;
        let mut graph = RenderGraph::new();
        let target = graph.import(
            "картинка теста",
            r.image,
            r.view,
            extent,
            FORMAT,
            ImageState::UNDEFINED,
            None,
        );
        demo.add_passes(&mut graph, &gpu.bindless, data, target);
        let buffer = r.buffer;
        // Последний проход копирует готовую картинку в буфер, который читает процессор.
        graph.add_pass("чтение", &[(target, Access::TransferSrc)], move |p| {
            let region = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                });
            let to_host = [vk::BufferMemoryBarrier2::default()
                .buffer(buffer)
                .size(vk::WHOLE_SIZE)
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)];
            // Замыкание внутри общего unsafe-блока функции. Граф перевёл картинку в
            // раскладку копирования, буфер жив до конца ожидания забора.
            p.device.cmd_copy_image_to_buffer(
                p.cmd,
                p.image(target),
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
            p.device.cmd_pipeline_barrier2(
                p.cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&to_host),
            );
        });
        if let Some(timer) = r.timer.as_mut() {
            timer.begin_frame(ctx, 0)?;
        }
        graph.execute(
            ctx,
            cmd,
            &mut r.transients,
            &gpu.bindless,
            0,
            r.timer.as_mut(),
        )?;
        d.end_command_buffer(cmd).ctx("конец записи команд")?;

        let cmds = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
        let submit = vk::SubmitInfo2::default().command_buffer_infos(&cmds);
        d.queue_submit2(ctx.queue, &[submit], r.fence)
            .ctx("отправка команд")?;
        d.wait_for_fences(&[r.fence], true, u64::MAX)
            .ctx("ожидание видеокарты")?;

        let ptr = d
            .map_memory(r.buffer_memory, 0, size, vk::MemoryMapFlags::empty())
            .ctx("чтение памяти")?;
        let pixels = std::slice::from_raw_parts(ptr.cast::<u8>(), size as usize).to_vec();
        d.unmap_memory(r.buffer_memory);
        // Кадр закончен: begin_frame той же ячейки читает его замеры.
        let timings = match r.timer.as_mut() {
            Some(timer) => {
                timer.begin_frame(ctx, 0)?;
                timer.last().cloned()
            }
            None => None,
        };
        let field = match (r.gpu.as_mut(), r.demo.as_ref()) {
            (Some(gpu), Some(demo)) if scene.scene == DemoScene::Space => {
                read_field(ctx, gpu, demo)?
            }
            _ => None,
        };
        // Счётчики кадра (7.18б): число краёв читается теперь, когда кадр закончен (в
        // окне его читает подготовка следующего кадра той же ячейки).
        let space_stats = match (r.gpu.as_ref(), r.demo.as_ref()) {
            (Some(gpu), Some(demo)) => demo.space_stats().map(|mut s| {
                s.edge_pixels = demo.edge_pixels(gpu, 0);
                s
            }),
            _ => None,
        };
        Ok(Rendered {
            rgba: pixels,
            timings,
            camera,
            player,
            particles,
            field,
            space_stats,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation_error_count;

    /// Поле, посчитанное видеокартой (7.10б), совпадает с моделью на процессоре
    /// (`reyvec_sdf::field3d`) в случайных точках всех слоёв.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn baked_field_matches_cpu_model() {
        use reyvec_sdf::field3d::{
            BRICK, FIELD_LEVELS, band, level_step, round_half, solid_distance,
        };
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let time = 2.0;
        let scene = HeadlessScene {
            width: 160,
            height: 90,
            time,
            shader_error: false,
            scene: DemoScene::Space,
            msaa: Msaa::Off,
            perceptual_edges: false,
            fill: crate::FillMode::default(),
            zoom: false,
            files: crate::UserFiles::default(),
            cascades: false,
        };
        let errors = validation_error_count();
        let img = render_headless(&options, &scene).expect("кадр без окна");
        assert_eq!(validation_error_count(), errors, "ошибки проверки Vulkan");
        let field = img.field.expect("поле считалось");
        let frame = reyvec_sdf::SpaceScene::parse(reyvec_sdf::space_file::EXAMPLE)
            .unwrap()
            .frame(time);
        let programs = crate::space::field_programs(&frame);
        let size = field.window.size;
        let mut seed = 99u32;
        let mut rnd = |n: i32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % n as u32) as i32
        };
        let mut near_surface = 0;
        for _ in 0..40000 {
            let level = rnd(FIELD_LEVELS as i32) as usize;
            let w = field.window.corners[level] + glam::IVec3::new(rnd(size), rnd(size), rnd(size));
            let step = level_step(level);
            let b = band(level);
            let cpu = round_half(solid_distance(&programs, w.as_vec3() * step, b).clamp(-b, b));
            let gpu = field.value(level, w);
            assert!(
                (cpu - gpu).abs() <= 1e-3 + cpu.abs() / 1024.0,
                "слой {level}, точка {w}: процессор {cpu}, видеокарта {gpu}"
            );
            if cpu.abs() < b {
                near_surface += 1;
            }
        }
        assert!(near_surface > 200, "точек у поверхностей: {near_surface}");
        assert_eq!(size % BRICK, 0);
    }

    thread_local! {
        /// Кадр без запечённого поля (для сравнения с кадром с полем).
        pub(super) static FIELD_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Кадр с «углами из поля» (7.10г).
        pub(super) static AO_FROM_FIELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Разрешение сцены «3D» (7.14).
        pub(super) static SPACE_SCALE: std::cell::Cell<crate::SpaceScale> =
            const { std::cell::Cell::new(crate::SpaceScale::Full) };
        /// 2.5D-вид уровня из файла (7.9).
        pub(super) static LEVEL_VIEW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Как рисуется 2.5D-вид (7.9б).
        pub(super) static LEVEL_RENDER: std::cell::Cell<crate::LevelRender> =
            const { std::cell::Cell::new(crate::LevelRender::Sdf) };
    }

    /// Вид на полую коробку сбоку (как эталон space-hollow-side-2s).
    fn hollow_side() -> HeadlessScene {
        let space = include_str!("../../../scenes/3d/space.ron").replacen(
            "version: 1,",
            "version: 1,\n    camera: Some((eye: (4.9, 0.9, 3.3), look_at: (3.4, 0.6, 2.6))),",
            1,
        );
        HeadlessScene {
            width: 640,
            height: 360,
            time: 2.0,
            shader_error: false,
            scene: DemoScene::Space,
            msaa: Msaa::Off,
            perceptual_edges: false,
            fill: crate::FillMode::default(),
            zoom: false,
            files: crate::UserFiles {
                space: Some(space.into()),
                ..Default::default()
            },
            cascades: false,
        }
    }

    /// Вид на полую коробку спереди сверху: её низ касается пола наклонной линией (камера
    /// со снимка Романа, 7.11).
    fn hollow_contact(width: u32, height: u32) -> HeadlessScene {
        let space = include_str!("../../../scenes/3d/space.ron").replacen(
            "version: 1,",
            "version: 1,\n    camera: Some((eye: (4.4, 1.1, 4.7), look_at: (3.4, 0.4, 2.6))),",
            1,
        );
        let mut scene = hollow_side();
        scene.width = width;
        scene.height = height;
        scene.files.space = Some(space.into());
        scene
    }

    /// Отдельных тёмных точек: пиксель темнее соседей сверху и снизу (через один — на 60
    /// ступеней, рядом — на 20).
    fn dark_dots(image: &FrameImage) -> usize {
        let (w, h) = (image.width as usize, image.height as usize);
        let light = |x: usize, y: usize| {
            let i = (y * w + x) * 4;
            i32::from(*image.rgba[i..i + 3].iter().max().unwrap_or(&0))
        };
        let mut dots = 0;
        for y in 2..h - 2 {
            for x in 0..w {
                let l = light(x, y);
                if light(x, y - 2) > l + 60
                    && light(x, y + 2) > l + 60
                    && light(x, y - 1) > l + 20
                    && light(x, y + 1) > l + 20
                {
                    dots += 1;
                }
            }
        }
        dots
    }

    /// Под коробкой у пола нет ряда чёрных точек (7.11). Пол под скруглённым низом
    /// поднимался для луча тени внутрь скругления и считался освещённым, а соседний —
    /// тёмным; на наклонной линии касания выходили «зубцы». Было 586 точек на весь кадр
    /// 1920×1080, стало 46 (тонкие линии сетки и края дыр).
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn no_dark_dots_under_boxes() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let frame = render_headless(&options, &hollow_contact(1920, 1080)).expect("кадр");
        let dots = dark_dots(&frame.image);
        eprintln!("тёмных точек: {dots}");
        assert!(dots < 150, "тёмных точек {dots}");
    }

    /// Под большой коробкой нет чёрного клина (7.13, снимок Романа: камера у пола между
    /// коробками). Пол под скруглённым низом не освещён солнцем, а прежнее затенение в
    /// углах давало ему 1 % неба: на этом виде было 2556 чёрных пикселей из 960×504.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn no_black_wedge_under_box() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let space = include_str!("../../../scenes/3d/space.ron").replacen(
            "version: 1,",
            "version: 1,\n    camera: Some((eye: (1.8, 0.3, 0.4), look_at: (2.733, 0.178, 0.74))),",
            1,
        );
        let mut scene = hollow_side();
        (scene.width, scene.height) = (960, 504);
        scene.files.space = Some(space.into());
        let frame = render_headless(&options, &scene).expect("кадр");
        let black = frame
            .image
            .rgba
            .chunks(4)
            .filter(|p| p[..3].iter().all(|&c| c < 12))
            .count();
        eprintln!("чёрных пикселей: {black}");
        assert!(black < 50, "чёрных пикселей {black}");
    }

    /// Сцена «3D» в половинном и трёх четвертях разрешения (7.14, встроенная Radeon):
    /// растянутый кадр похож на полный — отличается в основном по краям форм, без
    /// сдвига картинки и без чёрных или пустых мест.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn space_scale_matches_full() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let mut scene = hollow_side();
        (scene.width, scene.height) = (641, 361);
        let full = render_headless(&options, &scene).expect("полный кадр");
        for scale in [crate::SpaceScale::ThreeQuarters, crate::SpaceScale::Half] {
            SPACE_SCALE.set(scale);
            let small = render_headless(&options, &scene);
            SPACE_SCALE.set(crate::SpaceScale::Full);
            let small = small.expect("уменьшенный кадр");
            let (changed, worst, mean) = image_diff(&full.image, &small.image);
            let share = changed as f64 / f64::from(scene.width * scene.height);
            eprintln!(
                "{}: отличается {changed} пикселей ({:.1} %), наибольшая разница {worst}, \
                 средняя {mean:.2}",
                scale.title(),
                share * 100.0
            );
            assert!(mean < 3.0, "{}: средняя разница {mean:.2}", scale.title());
            assert!(share < 0.25, "{}: отличается {share:.3}", scale.title());
        }
    }

    /// 2.5D-вид уровня (7.9): встроенный уровень из файла рисуется объёмом — кадр не
    /// пустой и не плоский (много разных цветов), без ошибок валидации.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn level_view_draws_volume() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let scene = HeadlessScene {
            width: 640,
            height: 360,
            time: 1.0,
            shader_error: false,
            scene: DemoScene::File,
            msaa: Msaa::X4,
            perceptual_edges: false,
            fill: crate::FillMode::Triangles,
            zoom: false,
            files: crate::UserFiles::default(),
            cascades: false,
        };
        // Комната, подъёмник в коридоре и склон в конце.
        let plays = [
            ("room", 1.0, [-0.6, -0.5]),
            ("lift", 3.0, [9.6, 0.6]),
            ("slope", 1.0, [14.2, 0.2]),
        ];
        for (name, time, start) in plays {
            let scene = HeadlessScene {
                time,
                ..scene.clone()
            };
            let play = HeadlessPlay {
                start: Some(start),
                ..Default::default()
            };
            let flat = render_headless_play(&options, &scene, Some(&play)).expect("плоский кадр");
            LEVEL_VIEW.set(true);
            let volume = render_headless_play(&options, &scene, Some(&play));
            LEVEL_RENDER.set(crate::LevelRender::Triangles);
            let triangles = render_headless_play(&options, &scene, Some(&play));
            LEVEL_RENDER.set(crate::LevelRender::Sdf);
            LEVEL_VIEW.set(false);
            let volume = volume.expect("объёмный кадр");
            let triangles = triangles.expect("объёмный кадр треугольниками");
            assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
            // Счётчики панели (7.18б): плоский кадр их не имеет, объёмный — имеет, с
            // объектами, без переполнения и с прочитанным с видеокарты числом краёв.
            assert!(
                flat.space_stats.is_none(),
                "{name}: у плоского кадра нет счётчиков 3D"
            );
            let stats = volume.space_stats.expect("счётчики объёмного кадра");
            eprintln!("{name}: счётчики {stats:?}");
            assert!(stats.objects > 0 && stats.words > 0, "{name}: {stats:?}");
            assert_eq!(stats.dropped, 0, "{name}: {stats:?}");
            assert_eq!(stats.overflow, None, "{name}: {stats:?}");
            assert!(
                stats.edge_pixels.is_some_and(|n| n > 0 && n < stats.pixels),
                "{name}: пиксели края {:?} из {}",
                stats.edge_pixels,
                stats.pixels
            );
            // Треугольники рисуют тот же уровень: картинка близка к формам SDF (отличия —
            // тени и углы, у краёв фигур).
            let (_, _, mean) = image_diff(&volume.image, &triangles.image);
            eprintln!("{name}: треугольники против SDF, средняя разница {mean:.1}");
            // Время проходов (на lavapipe — только для сравнения двух способов).
            for (kind, f) in [("SDF", &volume), ("треугольники", &triangles)] {
                if let Some(t) = &f.gpu_timings {
                    eprintln!("{name}, {kind}: {}", t.describe());
                }
            }
            assert!(
                mean < 20.0,
                "{name}: треугольники не похожи на SDF ({mean:.1})"
            );
            if let Ok(dir) = std::env::var("PPM_DIR") {
                for (kind, f) in [
                    ("flat", &flat),
                    ("volume", &volume),
                    ("triangles", &triangles),
                ] {
                    let mut out =
                        format!("P6 {} {} 255\n", f.image.width, f.image.height).into_bytes();
                    for px in f.image.rgba.chunks(4) {
                        out.extend_from_slice(&px[..3]);
                    }
                    std::fs::write(format!("{dir}/level-{name}-{kind}.ppm"), out).expect("ppm");
                }
            }
            let (changed, _, mean) = image_diff(&flat.image, &volume.image);
            let mut colors = std::collections::HashSet::new();
            for px in volume.image.rgba.chunks(4) {
                colors.insert([px[0] >> 3, px[1] >> 3, px[2] >> 3]);
            }
            eprintln!(
                "{name}: отличается {changed} пикселей, средняя {mean:.1}, цветов {}",
                colors.len()
            );
            assert!(mean > 5.0, "{name}: объёмный кадр такой же, как плоский");
            assert!(colors.len() > 100, "{name}: мало цветов: {}", colors.len());
        }
    }

    /// Пикселей, отличающихся больше чем на 2 ступени, наибольшая разница канала и
    /// средняя разница.
    fn image_diff(a: &FrameImage, b: &FrameImage) -> (usize, u8, f64) {
        let (mut changed, mut worst, mut sum) = (0, 0, 0u64);
        for (a, b) in a.rgba.chunks(4).zip(b.rgba.chunks(4)) {
            let d = a
                .iter()
                .zip(b)
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap_or(0);
            worst = worst.max(d);
            sum += u64::from(d);
            if d > 2 {
                changed += 1;
            }
        }
        (changed, worst, sum as f64 / (a.rgba.len() / 4) as f64)
    }

    /// «Углы из поля» (7.10г) меняют картинку чуть-чуть: только у тонких краёв.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn ao_from_field_stays_close() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let exact = render_headless(&options, &hollow_side()).expect("кадр");
        AO_FROM_FIELD.set(true);
        let field = render_headless(&options, &hollow_side());
        AO_FROM_FIELD.set(false);
        let field = field.expect("кадр с углами из поля");
        let (changed, worst, mean) = image_diff(&exact.image, &field.image);
        eprintln!("углы из поля: {changed} пикселей, наибольшая {worst}, средняя {mean:.3}");
        let total = exact.image.rgba.len() / 4;
        assert!(
            changed * 20 < total,
            "отличаются {changed} пикселей из {total}"
        );
        assert!(mean < 1.0, "средняя разница {mean}");
    }

    /// Тени по полю (7.10в) рисуют то же, что точные: поле лишь пропускает точки луча, где
    /// полутени заведомо нет. Полая коробка сбоку — самый трудный вид: луч к солнцу идёт
    /// вдоль стенок, и там, где он перестаёт шагать по полю, сдвиг полос полутени виден
    /// первым.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn field_shadows_match_exact_ones() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let with = render_headless(&options, &hollow_side()).expect("кадр с полем");
        assert!(with.field.is_some(), "поле считалось");
        FIELD_OFF.set(true);
        let without = render_headless(&options, &hollow_side());
        FIELD_OFF.set(false);
        let without = without.expect("кадр без поля");
        assert!(without.field.is_none(), "поле выключено");
        let (changed, worst, _) = image_diff(&with.image, &without.image);
        eprintln!("поле: {changed} пикселей, наибольшая {worst}");
        // Полосы полутени сдвигаются чуть-чуть (до 21 из 255 у стыка стенок на lavapipe),
        // пятен с резкими краями нет. Пропуск свободных проб углов картинку не меняет.
        let total = with.image.rgba.len() / 4;
        assert!(
            changed * 200 < total,
            "отличаются {changed} пикселей из {total}"
        );
        assert!(worst < 48, "наибольшая разница канала {worst}");
    }

    /// Нужен драйвер Vulkan. В CI запускается на программном lavapipe:
    /// `cargo test -p reyvec-gpu -- --include-ignored`.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn triangle_is_drawn() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let img = render_triangle_headless(&options, 128, 64).expect("рисование без окна");
        eprintln!("Нарисовано на: {}", img.adapter);

        // Угол: фон. Картинка в sRGB, как окно: видеокарта перевела линейный цвет в sRGB.
        let corner = img.pixel(0, 0);
        let srgb = |c: f32| {
            if c <= 0.003_130_8 {
                c * 12.92
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            }
        };
        let [r, g, b, a] = crate::CLEAR_COLOR;
        let expect_bg = [srgb(r), srgb(g), srgb(b), a].map(|c| (c * 255.0).round() as u8);
        for ch in 0..4 {
            assert!(
                corner[ch].abs_diff(expect_bg[ch]) <= 1,
                "угол {corner:?}, ожидали фон {expect_bg:?}"
            );
        }

        // Центр: внутри треугольника, смесь трёх цветов, заметно ярче фона.
        let center = img.pixel(64, 32);
        assert!(
            center[0] > 60 && center[1] > 60 && center[2] > 60,
            "центр {center:?}"
        );

        // Верхняя вершина красная: точка чуть ниже неё заметно краснее остальных каналов.
        // Y вверх: вершина (0, 0.6) в NDC оказывается в верхней части картинки.
        let near_top = img.pixel(64, 17);
        assert!(
            near_top[0] > near_top[1] && near_top[0] > near_top[2],
            "у верхней вершины {near_top:?}"
        );

        // Слева: шахматка из текстуры (центр приходится на светлую клетку).
        // Её цвет хранится в sRGB, видеокарта переводит его в линейный при чтении.
        let checker = img.pixel(27, 32);
        assert!(
            checker[0] > 180 && checker[1] > 170 && checker[2] > 200,
            "шахматка {checker:?}"
        );
        // Справа: круг с оранжевым оттенком.
        let disk = img.pixel(101, 32);
        assert!(
            disk[0] > 220 && disk[1] > 180 && disk[2] < 160,
            "круг {disk:?}"
        );

        // Замеры проходов: на lavapipe метки времени есть, проходы графа по порядку.
        let t = img.gpu_timings.as_ref().expect("замеры видеокарты");
        eprintln!("Видеокарта на кадр: {}", t.describe());
        let names: Vec<_> = t.passes.iter().map(|p| p.name).collect();
        assert_eq!(names.last(), Some(&"чтение"), "проходы {names:?}");
        assert!(t.passes.iter().all(|p| p.ms >= 0.0));
        let sum: f64 = t.passes.iter().map(|p| p.ms).sum();
        assert!(
            (sum - t.total_ms).abs() < 1e-6,
            "сумма {sum} и всего {}",
            t.total_ms
        );

        assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
    }
}
