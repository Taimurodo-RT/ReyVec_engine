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
    /// 2.5D-вид уровня (7.9) и как он рисуется; `None` — плоский вид.
    pub level: Option<crate::LevelRender>,
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
    /// Сколько фигур сцены «3D» нарисовано через поле слоя (опыт 7.18б).
    pub field_shapes: usize,
    /// Поле уровня 2.5D-вида (7.18в).
    pub level_field: crate::LevelFieldStatus,
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

use reyvec_sdf::layer_field::half_to_f32;

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
        field_shapes: frame.field_shapes,
        level_field: frame.level_field,
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
    field_shapes: usize,
    level_field: crate::LevelFieldStatus,
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
            // Опыт 7.18б: выдавленные фигуры через поле слоя.
            #[cfg(test)]
            demo.set_space_extrude_field(tests::EXTRUDE_FIELD.get());
            // Эталоны — в полном разрешении 3D на любой видеокарте; тесты 7.14 уменьшают.
            demo.set_space_scale(crate::SpaceScale::Full);
            #[cfg(test)]
            demo.set_space_scale(tests::SPACE_SCALE.get());
            #[cfg(test)]
            demo.set_level_view(tests::LEVEL_VIEW.get());
            demo.set_level_render(crate::LevelRender::Sdf);
            #[cfg(test)]
            demo.set_level_render(tests::LEVEL_RENDER.get());
            if let Some(render) = play.and_then(|p| p.level) {
                demo.set_level_view(true);
                demo.set_level_render(render);
            }
            // Нормаль по всей сцене, как до 7.18д5 (для сравнения).
            #[cfg(test)]
            demo.set_full_normal(tests::FULL_NORMAL.get());
            // Без карт теней ламп (7.18е): где они затеняют.
            #[cfg(test)]
            demo.set_no_shadow_maps(tests::NO_SHADOW_MAPS.get());
            // Полигоны без образцов MSAA (7.18е2): края призм как до него.
            #[cfg(test)]
            demo.set_no_mesh_msaa(tests::NO_MESH_MSAA.get());
            // Кадр «3D» через картинку сцены, как до 7.20а (для сравнения).
            #[cfg(test)]
            demo.set_no_direct(tests::NO_DIRECT.get());
            #[cfg(test)]
            tests::MESH_COUNTS.set(demo.mesh_counts());
            // Шаг в пустоте не длиннее клетки, как до 7.18д4 (для сравнения).
            #[cfg(test)]
            demo.set_space_free(!tests::FREE_OFF.get());
            // Сетка отсева по шарам, как до 8.8а (для сравнения).
            #[cfg(test)]
            demo.set_space_boxes(!tests::SPHERE_GRID.get());
            // Поле уровня в общем переборе, как до 7.18д3 (для сравнения).
            #[cfg(test)]
            demo.set_level_field_apart(!tests::LEVEL_FIELD_IN_LOOP.get());
            // Счётчики лучей (7.18д2).
            #[cfg(test)]
            demo.set_space_counters(
                ctx,
                gpu,
                &ShaderLibrary::new(false),
                tests::SPACE_COUNTERS.get(),
            )?;
            demo.settle_details(ctx, gpu, extent, scene.time, scene.scene)?;
            // Перепекание поля на месте (7.18г): кадр до правки (поле печётся целиком),
            // потом правка снимка — и снимаемый кадр перепекает поле кусками.
            #[cfg(test)]
            {
                if tests::LEVEL_WARM.get() {
                    let _ = demo.prepare(
                        ctx,
                        gpu,
                        0,
                        extent,
                        scene.time,
                        scene.shader_error,
                        scene.scene,
                    );
                }
                demo.set_level_edit(tests::LEVEL_EDIT.get());
            }
        }
        // Проверка на чтение незаполненной памяти (8.0).
        #[cfg(test)]
        r.transients
            .set_poison(tests::POISON.get(), tests::POISON_ONLY.get());
        // Замер краёв по частям (8.1).
        #[cfg(test)]
        if let Some(demo) = r.demo.as_mut() {
            demo.set_edge_probe(tests::EDGE_PROBE.get());
            demo.set_edge_creases(tests::EDGE_CREASES.get());
            demo.set_edge_shadow_reuse(tests::EDGE_SHADOW_REUSE.get());
            demo.set_edge_far_meters(tests::EDGE_FAR.get());
            demo.set_post_aa(tests::POST_AA.get());
            demo.set_plane_shadows(tests::PLANE_SHADOWS.get());
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
        // Сломанный или оборванный скрипт сцены меняет кадр (монетки замирают): тест
        // должен упасть с понятной причиной, а не с разницей точек.
        #[cfg(test)]
        if let Some(info) = demo.script_info() {
            assert!(
                info.errors.is_empty(),
                "ошибки скриптов сцены: {:?}",
                info.errors
            );
        }
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
                s.edge_unreliable = demo.edge_unreliable(gpu, 0);
                s.edge_pairs = demo.edge_pairs(gpu, 0);
                if let Some((rays, edges)) = demo.ray_counts(gpu, 0) {
                    s.rays = Some(rays);
                    s.edge_kinds = s.edge_pixels.map(|_| edges);
                }
                s
            }),
            _ => None,
        };
        let field_shapes = r.demo.as_ref().map_or(0, Demo::field_shapes);
        let level_field = r
            .demo
            .as_ref()
            .map_or_else(Default::default, Demo::level_field_status);
        Ok(Rendered {
            rgba: pixels,
            timings,
            camera,
            player,
            particles,
            field,
            space_stats,
            field_shapes,
            level_field,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::validation_error_count;
    use reyvec_vector::SceneItem;

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
        /// Полигоны без образцов MSAA (7.18е2).
        pub(super) static NO_MESH_MSAA: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Кадр «3D» через картинку сцены, как до 7.20а.
        pub(super) static NO_DIRECT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Образцы полигонов, которые умеет видеокарта прошлого кадра (7.18е2).
        pub(super) static MESH_COUNTS: std::cell::Cell<ash::vk::SampleCountFlags> =
            const { std::cell::Cell::new(ash::vk::SampleCountFlags::empty()) };
        /// Треугольники без карт теней ламп (7.18е).
        pub(super) static NO_SHADOW_MAPS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Как рисуется 2.5D-вид (7.9б).
        pub(super) static LEVEL_RENDER: std::cell::Cell<crate::LevelRender> =
            const { std::cell::Cell::new(crate::LevelRender::Sdf) };
        /// Выдавленные фигуры сцены «3D» через поле слоя (опыт 7.18б).
        pub(super) static EXTRUDE_FIELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// …и поле без прямоугольника фигуры, бесконечное, как у поля уровня 7.18в.
        pub(crate) static FIELD_NO_RECT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Правка снимка мира в 2.5D-виде (7.18г)…
        pub(super) static LEVEL_EDIT: std::cell::Cell<Option<fn(&mut reyvec_scene::Scene)>> =
            const { std::cell::Cell::new(None) };
        /// …и кадр до неё, чтобы поле испеклось без правки и перепеклось кусками.
        pub(super) static LEVEL_WARM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Шаг в пустоте не длиннее клетки сетки, как до 7.18д4.
        pub(super) static FREE_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Сетка отсева по шарам-границам, как до 8.8а.
        pub(super) static SPHERE_GRID: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Нормаль по всей сцене, как до 7.18д5.
        pub(super) static FULL_NORMAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Поле уровня в общем переборе объектов, как до 7.18д3.
        pub(super) static LEVEL_FIELD_IN_LOOP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Кадр со счётчиками лучей (7.18д2).
        pub(super) static SPACE_COUNTERS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Замер краёв по частям (8.1).
        pub(super) static EDGE_PROBE: std::cell::Cell<crate::EdgeProbe> =
            const { std::cell::Cell::new(crate::EdgeProbe::Off) };
        /// Складки без доп. лучей (8.2); как в песочнице, по умолчанию включены.
        pub(super) static EDGE_CREASES: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
        /// Точные тени краёв только у границы тени (8.2б); по умолчанию включены.
        pub(super) static EDGE_SHADOW_REUSE: std::cell::Cell<bool> =
            const { std::cell::Cell::new(true) };
        /// Края: два луча вдали (8.11б), порог в метрах; как в песочнице, по умолчанию 0 —
        /// выключено.
        pub(super) static EDGE_FAR: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
        /// Сглаживание 3D по картинке вместо лучей краёв (8.12а); по умолчанию выключено.
        pub(super) static POST_AA: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Тени моделей от плоскостей граней, как до 8.15 (обычно — от кирпичей поля).
        pub(super) static PLANE_SHADOWS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Мусором залиты только картинки с этим именем (8.0).
        pub(super) static POISON_ONLY: std::cell::Cell<Option<&'static str>> =
            const { std::cell::Cell::new(None) };
        /// Временные картинки залиты мусором (8.0).
        pub(super) static POISON: std::cell::Cell<Option<crate::graph::Poison>> =
            const { std::cell::Cell::new(None) };
    }

    /// Замер краёв по частям (8.1): каждый вариант рисуется без ошибок Vulkan и меняет
    /// только края — остальная картинка та же, что «как обычно».
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn edge_probes_change_only_edges() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let errors = validation_error_count();
        let base = render_headless(&options, &hollow_side()).expect("кадр");
        let edges = base
            .space_stats
            .as_ref()
            .and_then(|s| s.edge_pixels)
            .unwrap_or(0);
        assert!(edges > 0, "в кадре есть края");
        for probe in crate::EdgeProbe::ALL {
            EDGE_PROBE.set(probe);
            let frame = render_headless(&options, &hollow_side());
            EDGE_PROBE.set(crate::EdgeProbe::Off);
            let frame = frame.expect("кадр с замером краёв");
            let (changed, _, _) = image_diff(&base.image, &frame.image);
            eprintln!(
                "{}: отличаются {changed} пикселей, краёв {edges}",
                probe.title()
            );
            assert!(
                changed as u64 <= u64::from(edges),
                "{}: отличаются {changed} пикселей, а краёв только {edges}",
                probe.title()
            );
            if probe == crate::EdgeProbe::Off {
                assert_eq!(changed, 0, "«как обычно» не меняет кадр");
            }
        }
        assert_eq!(validation_error_count(), errors, "ошибки валидации");
    }

    /// Складки без доп. лучей (8.2) рисуют края почти так же, как доп. лучи с шагами по
    /// сцене: отличаются только пиксели краёв, и ненамного.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn crease_edges_match_marched_edges() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let errors = validation_error_count();
        let room = HeadlessScene {
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
        let level = |render| HeadlessPlay {
            start: Some([-0.6, -0.5]),
            level: Some(render),
            ..Default::default()
        };
        let cases: Vec<(&str, HeadlessScene, Option<HeadlessPlay>)> = vec![
            ("3D", hollow_side(), None),
            (
                "2.5D, поле",
                room.clone(),
                Some(level(crate::LevelRender::Field)),
            ),
            (
                "2.5D, треугольники",
                room,
                Some(level(crate::LevelRender::Triangles)),
            ),
        ];
        for (name, scene, play) in &cases {
            EDGE_CREASES.set(false);
            let marched = render_headless_play(&options, scene, play.as_ref());
            EDGE_CREASES.set(true);
            let marched = marched.expect("кадр без складок");
            let creases = render_headless_play(&options, scene, play.as_ref()).expect("кадр");
            let edges = creases
                .space_stats
                .as_ref()
                .and_then(|s| s.edge_pixels)
                .unwrap_or(0);
            let (changed, worst, mean) = image_diff(&marched.image, &creases.image);
            eprintln!(
                "{name}: краёв {edges}, отличаются {changed} пикселей, \
                 наибольшая разница {worst}, средняя {mean:.4}"
            );
            assert!(edges > 0, "{name}: в кадре есть края");
            assert!(
                changed as u64 <= u64::from(edges),
                "{name}: отличаются {changed} пикселей, а краёв только {edges}"
            );
            assert!(mean < 0.5, "{name}: средняя разница {mean:.4}");
        }
        assert_eq!(validation_error_count(), errors, "ошибки валидации");
    }

    /// Точные тени краёв только у границы тени (8.2б): картинка почти та же, что с
    /// точными тенями у всех ненадёжных краёв (отличаются только пиксели краёв, и
    /// ненамного), а лучей тени краёв меньше.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn edge_shadow_reuse_matches_exact_shadows() {
        use crate::counters::PASS_EDGE_SHADOWS;
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let errors = validation_error_count();
        let room = HeadlessScene {
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
        let level = |render| HeadlessPlay {
            start: Some([-0.6, -0.5]),
            level: Some(render),
            ..Default::default()
        };
        let cases: Vec<(&str, HeadlessScene, Option<HeadlessPlay>)> = vec![
            ("3D", hollow_side(), None),
            (
                "2.5D, поле",
                room.clone(),
                Some(level(crate::LevelRender::Field)),
            ),
            (
                "2.5D, треугольники",
                room,
                Some(level(crate::LevelRender::Triangles)),
            ),
        ];
        SPACE_COUNTERS.set(true);
        for (name, scene, play) in &cases {
            EDGE_SHADOW_REUSE.set(false);
            let exact = render_headless_play(&options, scene, play.as_ref());
            EDGE_SHADOW_REUSE.set(true);
            let exact = exact.expect("кадр с точными тенями");
            let reuse = render_headless_play(&options, scene, play.as_ref()).expect("кадр");
            let shadow_rays = |image: &HeadlessImage| {
                image
                    .space_stats
                    .as_ref()
                    .and_then(|s| s.rays)
                    .map(|r| r[PASS_EDGE_SHADOWS].rays)
                    .expect("лучи посчитаны")
            };
            let unreliable = reuse
                .space_stats
                .as_ref()
                .and_then(|s| s.edge_unreliable)
                .unwrap_or(0);
            let (before, after) = (shadow_rays(&exact), shadow_rays(&reuse));
            let (changed, worst, mean) = image_diff(&exact.image, &reuse.image);
            eprintln!(
                "{name}: ненадёжных краёв {unreliable}, лучей тени краёв {before} → {after}, \
                 отличаются {changed} пикселей, наибольшая разница {worst}, средняя {mean:.4}"
            );
            assert!(unreliable > 0, "{name}: в кадре есть ненадёжные края");
            assert!(after < before, "{name}: лучей тени краёв меньше");
            assert!(
                changed as u64 <= u64::from(unreliable),
                "{name}: отличаются {changed} пикселей, а ненадёжных краёв только {unreliable}"
            );
            assert!(mean < 0.5, "{name}: средняя разница {mean:.4}");
        }
        SPACE_COUNTERS.set(false);
        assert_eq!(validation_error_count(), errors, "ошибки валидации");
    }

    /// Текстура цвета модели (8.14б2): ящик `models/crate.glb` рисуется с картинкой досок
    /// — коричневый, с тёмной рамкой и светлыми досками, а не ровно-белый, как тот же ящик
    /// с белым материалом сцены. Так в обычном кадре (образцы MSAA, края), без образцов
    /// и при уменьшенном разрешении 3D (освещение своим проходом).
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn model_texture_colors_crate() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let errors = validation_error_count();
        let text = |material: &str| {
            format!(
                r#"(
    format: "reyvec-space",
    version: 1,
    camera: Some((eye: (1.3, 1.1, 1.6), look_at: (0.0, 0.35, 0.0))),
    materials: [
        (name: "пол", color: (0.18, 0.19, 0.21), grid: true),
        (name: "белый", color: (1.0, 1.0, 1.0)),
    ],
    lamps: [(at: (1.5, 2.5, 2.0), color: (1.0, 1.0, 1.0), power: 6.0, range: 8.0)],
    objects: [(name: "пол", material: "пол", shape: Plane(normal: (0.0, 1.0, 0.0), offset: 0.0))],
    meshes: [(name: "ящик", {material} mesh: Model(file: "models/crate.glb"))],
)"#
            )
        };
        let scene = |material: &str| HeadlessScene {
            width: 320,
            height: 240,
            time: 0.0,
            shader_error: false,
            scene: DemoScene::Space,
            msaa: Msaa::X4,
            perceptual_edges: false,
            fill: crate::FillMode::default(),
            zoom: false,
            files: crate::UserFiles {
                space: Some(text(material).into()),
                space_dir: Some(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes/3d").into()),
                ..Default::default()
            },
            cascades: false,
        };
        for (name, scale, no_ms) in [
            ("обычно", crate::SpaceScale::Full, false),
            ("без образцов MSAA", crate::SpaceScale::Full, true),
            ("разрешение 3D 50 %", crate::SpaceScale::Half, false),
        ] {
            SPACE_SCALE.set(scale);
            NO_MESH_MSAA.set(no_ms);
            let textured = render_headless(&options, &scene(""));
            let white = render_headless(&options, &scene("material: \"белый\","));
            SPACE_SCALE.set(crate::SpaceScale::Full);
            NO_MESH_MSAA.set(false);
            let (textured, white) = (textured.expect("кадр"), white.expect("кадр"));
            let (a, b) = (&textured.image.rgba, &white.image.rgba);
            // Пиксели ящика — где кадры расходятся.
            let mut sum = [0.0f64; 3];
            let mut lum = Vec::new();
            for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
                if pa[..3]
                    .iter()
                    .zip(&pb[..3])
                    .any(|(x, y)| x.abs_diff(*y) > 8)
                {
                    for k in 0..3 {
                        sum[k] += f64::from(pa[k]);
                    }
                    lum.push(
                        f64::from(pa[0]) * 0.3 + f64::from(pa[1]) * 0.6 + f64::from(pa[2]) * 0.1,
                    );
                }
            }
            let n = lum.len().max(1) as f64;
            let mean = sum.map(|v| v / n);
            let avg = lum.iter().sum::<f64>() / n;
            let spread = (lum.iter().map(|l| (l - avg) * (l - avg)).sum::<f64>() / n).sqrt();
            eprintln!(
                "{name}: пикселей ящика {}, средний цвет {mean:.0?}, разброс яркости {spread:.1}",
                lum.len()
            );
            assert!(lum.len() > 3000, "{name}: ящик в кадре");
            // Доски коричневые: красного заметно больше синего.
            assert!(mean[0] > mean[2] * 1.4, "{name}: цвет {mean:?}");
            // Рамка темнее досок: текстура, а не ровный цвет.
            assert!(spread > 12.0, "{name}: разброс {spread}");
        }
        assert_eq!(validation_error_count(), errors, "ошибки валидации");
    }

    /// Сцена завода (8.7): больше 1000 объектов, те, что в кадре, помещаются в предел шейдера
    /// и буфер, кадр рисуется без ошибок проверки Vulkan. Время здесь не меряется
    /// (программная видеокарта) — его меряем на ПК.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn factory_scene_fits() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let errors = validation_error_count();
        let scene = HeadlessScene {
            width: 320,
            height: 180,
            time: 1.0,
            shader_error: false,
            scene: DemoScene::Space,
            msaa: Msaa::X4,
            perceptual_edges: false,
            fill: crate::FillMode::default(),
            zoom: false,
            files: crate::UserFiles {
                space: Some(include_str!("../../../scenes/3d/factory.ron").into()),
                space_dir: Some(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes/3d").into()),
                ..Default::default()
            },
            cascades: false,
        };
        let frame = render_headless(&options, &scene).expect("кадр");
        let stats = frame.space_stats.expect("числа сцены 3D");
        eprintln!(
            "завод: в кадре объектов {}, слов {}, {:?}, движется {}, процессор {:.2} + {:.2} мс",
            stats.objects, stats.words, stats.frame, stats.moving, stats.build_ms, stats.pack_ms
        );
        assert!(stats.objects > 500, "объектов в кадре {}", stats.objects);
        assert_eq!(stats.dropped, 0);
        assert!(stats.overflow.is_none());
        assert_eq!(stats.frame.proxies, 96);
        // Не пустой кадр: пол, станки и ящики дают разные цвета.
        let mut colors: Vec<[u8; 3]> = frame
            .image
            .rgba
            .chunks_exact(4)
            .map(|p| [p[0] / 32, p[1] / 32, p[2] / 32])
            .collect();
        colors.sort_unstable();
        colors.dedup();
        assert!(colors.len() > 20, "цветов {}", colors.len());
        // Сетка по шарам (как до 8.8а) даёт ту же картинку: сетка меняет только то,
        // сколько объектов перебирает шаг луча, и чуть — длину шагов в пустоте.
        SPHERE_GRID.set(true);
        let spheres = render_headless(&options, &scene);
        SPHERE_GRID.set(false);
        let spheres = spheres.expect("кадр");
        for (how, f) in [("коробки", &frame), ("шары", &spheres)] {
            if let Some(t) = &f.gpu_timings {
                eprintln!("завод, сетка {how}: {}", t.describe());
            }
        }
        let differ = frame
            .image
            .rgba
            .chunks_exact(4)
            .zip(spheres.image.rgba.chunks_exact(4))
            .filter(|(x, y)| x[..3].iter().zip(&y[..3]).any(|(p, q)| p.abs_diff(*q) > 16))
            .count();
        let pixels = frame.image.rgba.len() / 4;
        eprintln!("завод: по коробкам и по шарам заметно различаются {differ} из {pixels} точек");
        assert!(
            differ * 200 < pixels,
            "различаются {differ} из {pixels} точек"
        );
        assert_eq!(validation_error_count(), errors, "ошибки валидации");
    }

    /// Тени моделей от кирпичей поля (8.15) — почти те же, что от прежних заменителей из
    /// плоскостей граней: сцена моделей рисуется с кирпичами, с плоскостями и совсем без
    /// теней моделей. Кирпичи от плоскостей отличаются намного меньше, чем «без теней»
    /// от плоскостей: кирпич бросает ту же тень, а не пропал.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn brick_shadows_match_plane_shadows() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let errors = validation_error_count();
        let file = include_str!("../../../scenes/3d/models.ron");
        let scene = |text: String| HeadlessScene {
            width: 320,
            height: 240,
            time: 0.0,
            shader_error: false,
            scene: DemoScene::Space,
            msaa: Msaa::X4,
            perceptual_edges: false,
            fill: crate::FillMode::default(),
            zoom: false,
            files: crate::UserFiles {
                space: Some(text.into()),
                space_dir: Some(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes/3d").into()),
                ..Default::default()
            },
            cascades: false,
        };
        let bricks = render_headless(&options, &scene(file.to_owned())).expect("кадр");
        PLANE_SHADOWS.set(true);
        let planes = render_headless(&options, &scene(file.to_owned()));
        PLANE_SHADOWS.set(false);
        let planes = planes.expect("кадр");
        let none = render_headless(
            &options,
            &scene(file.replace("mesh: ", "shadow: false, mesh: ")),
        )
        .expect("кадр");
        let differ = |a: &[u8], b: &[u8]| {
            a.chunks_exact(4)
                .zip(b.chunks_exact(4))
                .filter(|(x, y)| x[..3].iter().zip(&y[..3]).any(|(p, q)| p.abs_diff(*q) > 16))
                .count()
        };
        let brick_vs_planes = differ(&bricks.image.rgba, &planes.image.rgba);
        let none_vs_planes = differ(&none.image.rgba, &planes.image.rgba);
        eprintln!(
            "пикселей отличается: кирпичи от плоскостей {brick_vs_planes}, без теней от \
             плоскостей {none_vs_planes}"
        );
        assert!(
            none_vs_planes > 1500,
            "тени моделей в кадре: {none_vs_planes}"
        );
        assert!(
            brick_vs_planes * 3 < none_vs_planes,
            "кирпичи бросают ту же тень: {brick_vs_planes} из {none_vs_planes}"
        );
        assert_eq!(validation_error_count(), errors, "ошибки валидации");
    }

    /// Ни один проход не читает то, что в этом кадре никто не записал (8.0). Кадр
    /// рисуется дважды: временные картинки перед ним залиты разным мусором (не-числом и
    /// обычным числом). Если проход читает незаполненную память, мусор попадает в кадр, и
    /// кадры расходятся. Так нашлось чтение по краю света в 2D-комнате: на ПК Романа оно
    /// не видно, но на другом драйвере может дать цветные точки.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn no_pass_reads_unwritten_memory() {
        use crate::graph::Poison;
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let base = HeadlessScene {
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
        let room = HeadlessPlay {
            start: Some([-0.6, -0.5]),
            ..Default::default()
        };
        let level = |render| HeadlessPlay {
            level: Some(render),
            ..room.clone()
        };
        let cases: Vec<(&str, HeadlessScene, Option<HeadlessPlay>)> = vec![
            ("2D-комната", base.clone(), Some(room.clone())),
            (
                "2D-комната, каскады",
                HeadlessScene {
                    cascades: true,
                    ..base.clone()
                },
                Some(room.clone()),
            ),
            (
                "лаборатория",
                HeadlessScene {
                    scene: DemoScene::Lab,
                    ..base.clone()
                },
                None,
            ),
            (
                "лампы",
                HeadlessScene {
                    scene: DemoScene::Lamps,
                    ..base.clone()
                },
                None,
            ),
            (
                "слизь и магия",
                HeadlessScene {
                    scene: DemoScene::Blobs,
                    ..base.clone()
                },
                None,
            ),
            ("3D", hollow_side(), None),
            (
                "2.5D, поле",
                base.clone(),
                Some(level(crate::LevelRender::Field)),
            ),
            (
                "2.5D, треугольники",
                base.clone(),
                Some(level(crate::LevelRender::Triangles)),
            ),
        ];
        // Имена временных картинок, по которым ищется виновная, если кадры разошлись.
        const NAMES: [&str; 19] = [
            "кадр сцены",
            "маска теней",
            "семена поля",
            "поле расстояний",
            "свет",
            "каскад света",
            "свечение вниз",
            "свечение вверх",
            "слой",
            "слой MSAA",
            "конусы 3D",
            "буфер геометрии 3D",
            "тени 3D",
            "края 3D",
            "цвет 3D",
            "полигоны 3D",
            "глубина полигонов 3D",
            "карты теней ламп",
            "глубина карт теней",
        ];
        // Сколько пикселей отличается, их границы и первый отличающийся пиксель.
        let compare = |scene: &HeadlessScene, play: Option<&HeadlessPlay>, only| {
            let mut frames = Vec::new();
            for poison in [Poison::Nan, Poison::Number] {
                POISON.set(Some(poison));
                POISON_ONLY.set(only);
                let frame = render_headless_play(&options, scene, play);
                POISON.set(None);
                POISON_ONLY.set(None);
                frames.push(frame.expect("кадр").image);
            }
            let (a, b) = (&frames[0], &frames[1]);
            let (mut count, mut lo, mut hi, mut first) = (0usize, [u32::MAX; 2], [0u32; 2], None);
            for y in 0..a.height {
                for x in 0..a.width {
                    if a.pixel(x, y) != b.pixel(x, y) {
                        count += 1;
                        lo = [lo[0].min(x), lo[1].min(y)];
                        hi = [hi[0].max(x), hi[1].max(y)];
                        first = first.or(Some(([x, y], a.pixel(x, y), b.pixel(x, y))));
                    }
                }
            }
            (count, lo, hi, first)
        };
        let errors = validation_error_count();
        let mut failed = Vec::new();
        for (name, scene, play) in &cases {
            let (count, lo, hi, first) = compare(scene, play.as_ref(), None);
            eprintln!("{name}: отличаются {count} пикселей");
            if count == 0 {
                continue;
            }
            eprintln!(
                "  область x {}..={}, y {}..={}; первый {first:?}",
                lo[0], hi[0], lo[1], hi[1]
            );
            for image in NAMES {
                let (count, lo, hi, _) = compare(scene, play.as_ref(), Some(image));
                if count > 0 {
                    eprintln!(
                        "  мусор в «{image}»: {count} пикселей, x {}..={}, y {}..={}",
                        lo[0], hi[0], lo[1], hi[1]
                    );
                }
            }
            failed.push(*name);
        }
        assert_eq!(validation_error_count(), errors, "ошибки валидации");
        assert!(failed.is_empty(), "читают незаполненную память: {failed:?}");
    }

    /// Сцена «3D» для опыта 7.18б: пол, две лампы и две выдавленные фигуры верхнего
    /// уровня, как сердце и облако в scenes/3d/vector.ron (со скруглением рёбер и без).
    fn extruded_scene() -> HeadlessScene {
        const SPACE: &str = r#"(
    format: "reyvec-space",
    version: 1,
    camera: Some((eye: (0.4, 2.3, 6.0), look_at: (0.0, 0.8, 0.0))),
    materials: [
        (name: "пол", color: (0.18, 0.19, 0.21), grid: true),
        (name: "красный", color: (0.85, 0.15, 0.20)),
        (name: "облако", color: (0.85, 0.88, 0.95)),
    ],
    lamps: [
        (at: (-1.0, 2.4, 2.0), color: (1.0, 0.65, 0.35), power: 7.0, range: 7.0, bulb: 0.08),
        (at: (2.4, 2.2, 0.5), color: (0.40, 0.65, 1.0), power: 5.0, range: 6.0, bulb: 0.08),
    ],
    objects: [
        (name: "пол", material: "пол", shape: Plane(normal: (0.0, 1.0, 0.0), offset: 0.0)),
        (name: "сердце", material: "красный", at: (-0.6, 1.2, 0.8), turn: (0.0, -15.0, 0.0),
            shape: Extrude(
                profile: "M 0 -0.6 C -0.2 -0.4 -0.7 -0.1 -0.7 0.25 C -0.7 0.55 -0.45 0.7 -0.25 0.7 C -0.1 0.7 0 0.6 0 0.5 C 0 0.6 0.1 0.7 0.25 0.7 C 0.45 0.7 0.7 0.55 0.7 0.25 C 0.7 -0.1 0.2 -0.4 0 -0.6 Z",
                depth: 0.35, round: 0.1)),
        (name: "облако", material: "облако", at: (0.0, 2.4, -3.0),
            shape: Extrude(
                profile: "M -1.0 0 A 0.35 0.35 0 0 0 -0.5 0.3 A 0.45 0.45 0 0 0 0.3 0.45 A 0.4 0.4 0 0 0 0.9 0.2 A 0.25 0.25 0 0 0 1.0 0 Z",
                depth: 0.03)),
    ],
)"#;
        HeadlessScene {
            width: 640,
            height: 360,
            time: 1.0,
            shader_error: false,
            scene: DemoScene::Space,
            msaa: Msaa::Off,
            perceptual_edges: false,
            fill: crate::FillMode::default(),
            zoom: false,
            files: crate::UserFiles {
                space: Some(SPACE.into()),
                ..Default::default()
            },
            cascades: false,
        }
    }

    /// Выдавленная фигура через поле слоя (7.18б) и через слова программы — одна
    /// картинка: условие 0020 — изменившихся точек не больше 0,5 %, худшая не больше
    /// 16/255. Поле 7.10 при этом выключено: сравнивается сам примитив. Поле без
    /// прямоугольника фигуры — бесконечный путь поля уровня 7.18в: без шара-границы, в
    /// списке бесконечных объектов, за сеткой только правило края. За маленькой сеткой
    /// профиля оно занижает расстояние, полутени рядом чуть темнее (на lavapipe
    /// 558 точек, худшая 15/255), поэтому худшая точка у него — до 48/255, как у поля 7.10;
    /// у поля уровня сетка накрывает весь уровень. Последний кадр — с
    /// полем 7.10, которое печёт кубики и по примитиву «поле слоя» (field_bake.slang
    /// читает картинку): картинка с полем отличается от кадра без поля так же мало, как у
    /// обычных форм.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn extrude_through_a_layer_field_draws_the_same_picture() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let errors = validation_error_count();
        FIELD_OFF.set(true);
        let plain = render_headless(&options, &extruded_scene());
        EXTRUDE_FIELD.set(true);
        let field = render_headless(&options, &extruded_scene());
        FIELD_NO_RECT.set(true);
        let unbounded = render_headless(&options, &extruded_scene());
        FIELD_NO_RECT.set(false);
        FIELD_OFF.set(false);
        let baked = render_headless(&options, &extruded_scene());
        EXTRUDE_FIELD.set(false);
        let plain = plain.expect("кадр выдавливанием");
        let field = field.expect("кадр через поле слоя");
        let unbounded = unbounded.expect("кадр через поле слоя без прямоугольника");
        let baked = baked.expect("кадр через поле слоя с полем 7.10");
        assert_eq!(validation_error_count(), errors, "ошибки проверки Vulkan");
        assert_eq!(plain.field_shapes, 0);
        assert_eq!(field.field_shapes, 2, "сердце и облако через поле слоя");
        assert_eq!(unbounded.field_shapes, 2);
        assert_eq!(baked.field_shapes, 2);
        assert!(baked.field.is_some(), "поле 7.10 считалось");
        let total = plain.image.rgba.len() / 4;
        for (what, other, limit) in [("", &field, 16), (" без прямоугольника", &unbounded, 47)]
        {
            let (changed, worst, mean) = image_diff(&plain.image, &other.image);
            eprintln!(
                "поле слоя{what} против выдавливания: {changed} пикселей, наибольшая {worst}, \
                 средняя {mean:.3}"
            );
            assert!(
                changed * 200 <= total,
                "поле слоя{what}: отличаются {changed} пикселей из {total}"
            );
            assert!(
                worst <= limit,
                "поле слоя{what}: наибольшая разница канала {worst}"
            );
        }
        if let Ok(dir) = std::env::var("PPM_DIR") {
            for (kind, f) in [
                ("extrude", &plain),
                ("layer-field", &field),
                ("unbounded", &unbounded),
                ("baked", &baked),
            ] {
                let mut out = format!("P6 {} {} 255\n", f.image.width, f.image.height).into_bytes();
                for px in f.image.rgba.chunks(4) {
                    out.extend_from_slice(&px[..3]);
                }
                std::fs::write(format!("{dir}/extrude-{kind}.ppm"), out).expect("ppm");
            }
        }
        let (changed, worst, mean) = image_diff(&field.image, &baked.image);
        eprintln!(
            "поле слоя с полем 7.10 и без: {changed} пикселей, наибольшая {worst}, средняя {mean:.3}"
        );
        assert!(
            changed * 200 < total,
            "с полем 7.10 отличаются {changed} пикселей из {total}"
        );
        assert!(worst < 48, "с полем 7.10 наибольшая разница канала {worst}");
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

    /// Сглаживание 3D по картинке (8.12а): без ошибок Vulkan в полном и половинном
    /// разрешении, прямо в окно и через кадр сцены; у большинства пикселей, где лучи краёв
    /// меняют кадр (против кадра, где лучи краёв берут середину пикселя), сглаживание по
    /// картинке сдвигает цвет к лучам, а меняет оно не намного больше пикселей, чем лучи.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn post_aa_is_closer_to_ray_edges() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let mut scene = hollow_side();
        (scene.width, scene.height) = (641, 361);
        for (scale, direct) in [
            (crate::SpaceScale::Full, true),
            (crate::SpaceScale::Full, false),
            (crate::SpaceScale::Half, true),
        ] {
            SPACE_SCALE.set(scale);
            NO_DIRECT.set(!direct);
            let errors = validation_error_count();
            let rays = render_headless(&options, &scene);
            EDGE_PROBE.set(crate::EdgeProbe::NoSubRays);
            let rough = render_headless(&options, &scene);
            EDGE_PROBE.set(crate::EdgeProbe::Off);
            POST_AA.set(true);
            let post = render_headless(&options, &scene);
            POST_AA.set(false);
            NO_DIRECT.set(false);
            SPACE_SCALE.set(crate::SpaceScale::Full);
            let rays = rays.expect("кадр с лучами краёв");
            let rough = rough.expect("кадр без доп. лучей");
            let post = post.expect("кадр со сглаживанием по картинке");
            assert_eq!(validation_error_count(), errors, "ошибки проверки Vulkan");
            let (rough_changed, _, rough_mean) = image_diff(&rays.image, &rough.image);
            let (post_changed, post_worst, post_mean) = image_diff(&rays.image, &post.image);
            let (smoothed, _, _) = image_diff(&rough.image, &post.image);
            eprintln!(
                "{}, {}: без сглаживания — отличается {rough_changed} пикселей, средняя \
                 {rough_mean:.3}; по картинке — {post_changed}, средняя {post_mean:.3}, \
                 худшая {post_worst}; по картинке изменено {smoothed}",
                scale.title(),
                if direct {
                    "прямо в окно"
                } else {
                    "через сцену"
                },
            );
            assert!(smoothed > 0, "сглаживание по картинке ничего не изменило");
            // Средняя разница — плохая мера: сглаживание по картинке смешивает цвет иначе,
            // чем лучи (на глаз ступеньки так же сглажены, но смесь у края другая). Мера —
            // доля пикселей, где лучи меняют кадр, а сглаживание по картинке сдвигает цвет к
            // лучам.
            let toward = rough
                .image
                .rgba
                .chunks(4)
                .zip(post.image.rgba.chunks(4))
                .zip(rays.image.rgba.chunks(4))
                .filter(|((r, p), t)| {
                    let d = |a: &[u8]| -> u32 {
                        a.iter()
                            .zip(t.iter())
                            .take(3)
                            .map(|(x, y)| u32::from(x.abs_diff(*y)))
                            .sum()
                    };
                    d(r) > 6 && d(p) < d(r)
                })
                .count();
            let rough_edges = rough
                .image
                .rgba
                .chunks(4)
                .zip(rays.image.rgba.chunks(4))
                .filter(|(r, t)| {
                    r.iter()
                        .zip(t.iter())
                        .take(3)
                        .map(|(x, y)| u32::from(x.abs_diff(*y)))
                        .sum::<u32>()
                        > 6
                })
                .count();
            eprintln!("ближе к лучам {toward} из {rough_edges} пикселей, где лучи меняют кадр");
            assert!(toward * 2 > rough_edges, "{toward} из {rough_edges}");
            if scale == crate::SpaceScale::Full {
                // Меняются только пиксели у краёв: их не больше, чем пикселей, где лучи
                // краёв что-то меняют, с запасом на соседей ступенек.
                assert!(
                    smoothed <= rough_changed * 3,
                    "{smoothed} > 3 × {rough_changed}"
                );
            }
        }
    }

    /// Прямой вывод «3D» в окно (7.20а): кадр без картинки сцены с MSAA и без прохода
    /// «сцена» тот же, что через неё, — в полном разрешении 3D и в половинном, с MSAA и без.
    /// Отличия только от округления цвета в кадре сцены (компактный формат B10G11R11):
    /// почти у каждого пикселя на ступень, не больше чем на 3 из 255 (lavapipe). Прямой
    /// вывод этого округления не делает.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn space_direct_output_matches_scene_pass() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let mut scene = hollow_side();
        (scene.width, scene.height) = (641, 361);
        for (scale, msaa) in [
            (crate::SpaceScale::Full, Msaa::Off),
            (crate::SpaceScale::Full, Msaa::X2),
            (crate::SpaceScale::Half, Msaa::X2),
        ] {
            scene.msaa = msaa;
            SPACE_SCALE.set(scale);
            let errors = validation_error_count();
            let direct = render_headless(&options, &scene);
            NO_DIRECT.set(true);
            let old = render_headless(&options, &scene);
            NO_DIRECT.set(false);
            SPACE_SCALE.set(crate::SpaceScale::Full);
            let (direct, old) = (direct.expect("прямой вывод"), old.expect("через сцену"));
            assert_eq!(validation_error_count(), errors, "ошибки проверки Vulkan");
            let (changed, worst, mean) = image_diff(&old.image, &direct.image);
            eprintln!(
                "{} {msaa:?}: отличается {changed} пикселей, наибольшая разница {worst}, \
                 средняя {mean:.3}",
                scale.title()
            );
            for (name, img) in [("прямой", &direct), ("через сцену", &old)] {
                if let Some(t) = &img.gpu_timings {
                    eprintln!("  {name}: {}", t.describe());
                }
            }
            assert!(worst <= 4, "{}: наибольшая разница {worst}", scale.title());
            assert!(mean < 1.5, "{}: средняя разница {mean:.3}", scale.title());
            // Пути и правда разные: у прямого вывода нет прохода «сцена», у прежнего — есть.
            for (img, want) in [(&direct, false), (&old, true)] {
                if let Some(t) = &img.gpu_timings {
                    let names: Vec<_> = t.passes.iter().map(|p| p.name).collect();
                    assert_eq!(names.contains(&"сцена"), want, "проходы: {names:?}");
                }
            }
        }
    }

    /// Убирает из снимка неподвижную фигуру у стола в комнате (ближайшую к точке
    /// (−0,45; −0,45)): как будто её убрали из файла сцены.
    fn remove_still_near_table(scene: &mut reyvec_scene::Scene) {
        let mut dynamic = 0u32;
        let mut best: Option<(usize, f64)> = None;
        for (i, item) in scene.items.iter().enumerate() {
            match item {
                SceneItem::BeginDynamic => dynamic += 1,
                SceneItem::EndDynamic => dynamic = dynamic.saturating_sub(1),
                SceneItem::Shape(instance) if dynamic == 0 => {
                    let t = instance.transform.translation();
                    let d = (t.x + 0.45).hypot(t.y + 0.45);
                    if best.is_none_or(|(_, b)| d < b) {
                        best = Some((i, d));
                    }
                }
                _ => {}
            }
        }
        if let Some((i, _)) = best {
            scene.items.remove(i);
        }
    }

    /// Перепекание поля на месте (7.18г) на видеокарте: поле испеклось по кадру до
    /// правки, потом из уровня убрали неподвижную фигуру — кадр перепекает кусок вокруг
    /// неё и грузит его в картинки поля до лучей. Картинка — та же, что у SDF и у поля,
    /// испечённого целиком уже без неё; без ошибок проверки Vulkan.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn level_field_patch_matches_full_bake() {
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
        let play = HeadlessPlay {
            start: Some([-0.6, -0.5]),
            ..Default::default()
        };
        let errors = validation_error_count();
        LEVEL_VIEW.set(true);
        LEVEL_EDIT.set(Some(remove_still_near_table));
        let sdf = render_headless_play(&options, &scene, Some(&play));
        LEVEL_RENDER.set(crate::LevelRender::Field);
        let whole = render_headless_play(&options, &scene, Some(&play));
        LEVEL_WARM.set(true);
        let patched = render_headless_play(&options, &scene, Some(&play));
        LEVEL_WARM.set(false);
        LEVEL_EDIT.set(None);
        let unedited = render_headless_play(&options, &scene, Some(&play));
        LEVEL_RENDER.set(crate::LevelRender::Sdf);
        LEVEL_VIEW.set(false);
        // Убранная фигура видна в кадре: иначе одинаковые картинки ничего не доказывают
        // (без загрузки куска она осталась бы в картинке поля).
        let unedited = unedited.expect("кадр без правки");
        let (changed, _, _) = image_diff(&unedited.image, &patched.as_ref().expect("кадр").image);
        assert!(
            changed > 200,
            "убранная фигура не видна: {changed} пикселей"
        );
        let (sdf, whole, patched) = (
            sdf.expect("кадр SDF"),
            whole.expect("кадр с полем целиком"),
            patched.expect("кадр с перепечённым полем"),
        );
        assert_eq!(validation_error_count(), errors, "ошибки валидации Vulkan");
        eprintln!("поле по кускам: {:?}", patched.level_field);
        let crate::LevelFieldStatus::Ready {
            patched: Some(p), ..
        } = patched.level_field
        else {
            panic!("поле не перепеклось на месте: {:?}", patched.level_field);
        };
        assert!(p.pieces >= 1 && p.points > 0, "{p:?}");
        let pixels = (sdf.image.width * sdf.image.height) as usize;
        let (changed, worst, mean) = image_diff(&whole.image, &patched.image);
        eprintln!(
            "по кускам против целиком: {changed} пикселей, наибольшая {worst}, средняя {mean:.3}"
        );
        assert!(
            changed * 1000 <= pixels,
            "по кускам и целиком — одна картинка"
        );
        let (changed, worst, mean) = image_diff(&sdf.image, &patched.image);
        eprintln!(
            "по кускам против SDF: {changed} пикселей, наибольшая {worst}, средняя {mean:.2}"
        );
        assert!(
            changed * 1000 <= pixels * 25 && mean < 1.0,
            "по кускам — та же картинка, что у SDF"
        );
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
            // У левой стены (7.18в): левая треть вида — за краем уровня и за сеткой поля
            // уровня; там не должно появиться стены, которой нет.
            ("left", 1.0, [-1.75, 0.0]),
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
            LEVEL_RENDER.set(crate::LevelRender::Field);
            let field = render_headless_play(&options, &scene, Some(&play));
            LEVEL_RENDER.set(crate::LevelRender::Sdf);
            LEVEL_VIEW.set(false);
            let volume = volume.expect("объёмный кадр");
            let triangles = triangles.expect("объёмный кадр треугольниками");
            let field = field.expect("объёмный кадр с полем уровня");
            assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
            // Поле уровня (7.18в): готово с первого кадра (без потока загрузки печётся в
            // кадре), неподвижное в нём, подвижное формами, и картинка та же, что у SDF.
            eprintln!("{name}: поле уровня {:?}", field.level_field);
            let crate::LevelFieldStatus::Ready {
                plates,
                solids,
                shapes,
                ..
            } = field.level_field
            else {
                panic!("{name}: поле уровня не готово: {:?}", field.level_field);
            };
            assert!(
                plates > 50 && solids > 20 && shapes > 0,
                "{name}: {:?}",
                field.level_field
            );
            let field_stats = field.space_stats.expect("счётчики кадра с полем");
            eprintln!("{name}: счётчики с полем {field_stats:?}");
            assert!(
                field_stats.objects < stats_objects(&volume),
                "{name}: с полем объектов не меньше"
            );
            let (changed, worst, mean) = image_diff(&volume.image, &field.image);
            let pixels = (field.image.width * field.image.height) as usize;
            eprintln!(
                "{name}: поле против SDF: отличается {changed} пикселей из {pixels}, \
                 наибольшая {worst}, средняя {mean:.2}"
            );
            if let Some(t) = &field.gpu_timings {
                eprintln!("{name}, поле: {}", t.describe());
            }
            // Неподвижные стопки в поле — в одной плоскости, а не через 2 мм: отличия в
            // пиксель по краям деталей (корешки книг, картинка на полке) и полутени в
            // пределах нескольких ступеней.
            let field_same = changed * 1000 <= pixels * 25 && mean < 1.0;
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
                    ("field", &field),
                ] {
                    let mut out =
                        format!("P6 {} {} 255\n", f.image.width, f.image.height).into_bytes();
                    for px in f.image.rgba.chunks(4) {
                        out.extend_from_slice(&px[..3]);
                    }
                    std::fs::write(format!("{dir}/level-{name}-{kind}.ppm"), out).expect("ppm");
                }
            }
            assert!(
                field_same,
                "{name}: поле рисует не то, что SDF (снимки — PPM_DIR=папка)"
            );
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

    fn stats_objects(f: &HeadlessImage) -> u32 {
        f.space_stats.map_or(0, |s| s.objects)
    }

    /// Тени от карт ламп (7.18е): в 2.5D-виде треугольниками карты дают тень, без ошибок
    /// Vulkan, и там, где они затеняют гладкую без них картинку, нет грубых скачков между
    /// соседними точками. Прежние ступени полутени этот тест НЕ отличает: при 640×360 они
    /// мельче его порога, и прежний фильтр его тоже проходит. Их смотреть глазом на снимках
    /// с картами и без (PPM_DIR=папка).
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn level_view_map_shadows() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let mut jumps = 0usize;
        let mut pairs = 0usize;
        // Комната (висячая лампа и шар-светильник) в двух фазах качания лампы и у левой стены.
        for (time, start) in [
            (1.0, [-0.6, -0.5]),
            (0.8, [-0.6, -0.5]),
            (1.0, [-1.75, 0.0]),
        ] {
            let scene = HeadlessScene {
                width: 640,
                height: 360,
                time,
                shader_error: false,
                scene: DemoScene::File,
                msaa: Msaa::X4,
                perceptual_edges: false,
                fill: crate::FillMode::Triangles,
                zoom: false,
                files: crate::UserFiles::default(),
                cascades: false,
            };
            let play = HeadlessPlay {
                start: Some(start),
                level: Some(crate::LevelRender::Triangles),
                ..Default::default()
            };
            let maps = render_headless_play(&options, &scene, Some(&play)).expect("кадр");
            NO_SHADOW_MAPS.set(true);
            let bare = render_headless_play(&options, &scene, Some(&play));
            NO_SHADOW_MAPS.set(false);
            let bare = bare.expect("кадр без карт");
            assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
            let (w, h) = (maps.image.width as usize, maps.image.height as usize);
            let at = |img: &HeadlessImage, x: usize, y: usize| {
                let i = (y * w + x) * 4;
                img.image.rgba[i..i + 3]
                    .iter()
                    .map(|&v| i32::from(v))
                    .sum::<i32>()
            };
            // Точка тени карт: без карт ярче, и без карт вокруг гладко (не край формы).
            let shaded = |x: usize, y: usize| {
                let b = at(&bare, x, y);
                b - at(&maps, x, y) > 12
                    && (x + 1 >= w || (at(&bare, x + 1, y) - b).abs() <= 6)
                    && (y + 1 >= h || (at(&bare, x, y + 1) - b).abs() <= 6)
            };
            let (mut j, mut n) = (0, 0);
            for y in 0..h - 1 {
                for x in 0..w - 1 {
                    if !shaded(x, y) {
                        continue;
                    }
                    for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                        if shaded(nx, ny) {
                            n += 1;
                            if (at(&maps, nx, ny) - at(&maps, x, y)).abs() >= 24 {
                                j += 1;
                            }
                        }
                    }
                }
            }
            eprintln!("время {time}, {start:?}: скачков {j} из {n} пар в тени карт");
            if let Ok(dir) = std::env::var("PPM_DIR") {
                for (kind, f) in [("maps", &maps), ("bare", &bare)] {
                    let mut out = format!("P6 {w} {h} 255\n").into_bytes();
                    for px in f.image.rgba.chunks(4) {
                        out.extend_from_slice(&px[..3]);
                    }
                    std::fs::write(
                        format!("{dir}/penumbra-{time}-{}-{kind}.ppm", start[0]),
                        out,
                    )
                    .expect("ppm");
                }
            }
            jumps += j;
            pairs += n;
        }
        assert!(pairs > 2000, "тени карт почти нет: {pairs} пар");
        assert!(
            jumps * 1000 <= pairs * 5,
            "полутень карт ступенями: {jumps} скачков из {pairs} пар"
        );
    }

    /// Ровные края призм (7.18е2): в 2.5D-виде треугольниками лучи краёв берут свои образцы
    /// MSAA картинки полигонов. Картинка меняется только у краёв (доля точек мала), и у
    /// краёв она ближе к SDF (там края форм сглажены теми же лучами), чем без образцов —
    /// и в полном разрешении 3D (четыре луча), и в половинном (два).
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn level_view_triangle_edges() {
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
        for scale in [crate::SpaceScale::Full, crate::SpaceScale::Half] {
            SPACE_SCALE.set(scale);
            let frame = |render, no_msaa: bool| {
                NO_MESH_MSAA.set(no_msaa);
                let play = HeadlessPlay {
                    start: Some([-0.6, -0.5]),
                    level: Some(render),
                    ..Default::default()
                };
                let f = render_headless_play(&options, &scene, Some(&play));
                NO_MESH_MSAA.set(false);
                f.expect("кадр")
            };
            let sdf = frame(crate::LevelRender::Sdf, false);
            let before = frame(crate::LevelRender::Triangles, true);
            let after = frame(crate::LevelRender::Triangles, false);
            SPACE_SCALE.set(crate::SpaceScale::Full);
            assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
            let counts = MESH_COUNTS.get();
            assert!(
                counts.intersects(
                    ash::vk::SampleCountFlags::TYPE_2 | ash::vk::SampleCountFlags::TYPE_4
                ),
                "видеокарта теста не умеет образцы для полигонов: {counts:?}"
            );
            let (w, h) = (sdf.image.width as usize, sdf.image.height as usize);
            let px = |f: &HeadlessImage, x: usize, y: usize| {
                let i = (y * w + x) * 4;
                [0, 1, 2].map(|c| i32::from(f.image.rgba[i + c]))
            };
            let differ =
                |a: [i32; 3], b: [i32; 3]| a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<i32>();
            // Края SDF-картинки: соседи заметно отличаются.
            let (mut edges, mut far_before, mut far_after, mut changed) = (0, 0i64, 0i64, 0);
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let c = px(&sdf, x, y);
                    let edge = [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
                        .iter()
                        .any(|&(nx, ny)| differ(c, px(&sdf, nx, ny)) > 60);
                    if differ(px(&before, x, y), px(&after, x, y)) > 6 {
                        changed += 1;
                    }
                    if edge {
                        edges += 1;
                        far_before += i64::from(differ(c, px(&before, x, y)));
                        far_after += i64::from(differ(c, px(&after, x, y)));
                    }
                }
            }
            let pixels = w * h;
            eprintln!(
                "{scale:?}: образцы {counts:?}; изменилось {changed} точек из {pixels}; \
                 у {edges} точек краёв разница с SDF {far_before} → {far_after}"
            );
            if let Ok(dir) = std::env::var("PPM_DIR") {
                for (kind, f) in [("sdf", &sdf), ("before", &before), ("after", &after)] {
                    let mut out = format!("P6 {w} {h} 255\n").into_bytes();
                    for p in f.image.rgba.chunks(4) {
                        out.extend_from_slice(&p[..3]);
                    }
                    std::fs::write(format!("{dir}/edges-{scale:?}-{kind}.ppm"), out).expect("ppm");
                }
            }
            assert!(
                changed * 1000 >= pixels * 3 && changed * 10 <= pixels,
                "{scale:?}: изменилось {changed} точек из {pixels} — не только края"
            );
            assert!(
                far_after < far_before,
                "{scale:?}: края призм не ближе к SDF: {far_before} → {far_after}"
            );
        }
    }

    /// Щель уже пикселя между полигональными коробками с общей передней гранью (7.18е2,
    /// находка ревью): луч середины пикселя берёт ближайший накрытый образец и щель не
    /// видит, соседи по буферу геометрии одинаковы. Поиск краёв ставит такие пиксели в
    /// список по расхождению образцов (`mesh_mixed`), и лучи края показывают щель по
    /// доле накрытия. Три щели шириной 0,4 пикселя с разным сдвигом внутри пикселя;
    /// без образцов MSAA видна только та, что накрывает середину пикселя. Сравнение — с
    /// кадром, где вместо четырёх коробок одна сплошная.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn mesh_slit_thinner_than_a_pixel_stays_visible() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        // Пиксель на передней грани (z 0,01, глаз на z 10, обзор 60°, высота 360). Коробки
        // тонкие: их боковые грани в щелях видны сбоку меньше чем на 0,05 пикселя.
        let p = 2.0 * 9.99 * (30f64).to_radians().tan() / 360.0;
        let edge = |k: f64| (k - 320.0) * p;
        let slits = [(300.0, 0.05), (320.0, 0.3), (340.0, 0.55)].map(|(k, phase)| {
            let left = edge(k) + phase * p;
            (left, left + 0.4 * p)
        });
        let block = |name: &str, from: f64, to: f64| {
            format!(
                "(name: \"{name}\", material: \"белый\", at: ({:.6}, 0.0, 0.0), \
                 mesh: Block(size: ({:.6}, 3.0, 0.02))),",
                (from + to) / 2.0,
                to - from
            )
        };
        let scene_text = |split: bool| {
            let mut meshes = String::from(
                "(name: \"фон\", material: \"чёрный\", at: (0.0, 0.0, -3.0), \
                 mesh: Block(size: (30.0, 30.0, 0.2))),",
            );
            if split {
                let mut from = -4.0;
                for (i, (l, r)) in slits.iter().enumerate() {
                    meshes += &block(&format!("коробка {i}"), from, *l);
                    from = *r;
                }
                meshes += &block("коробка 3", from, 4.0);
            } else {
                meshes += &block("сплошная", -4.0, 4.0);
            }
            format!(
                r#"(
    format: "reyvec-space",
    version: 1,
    camera: Some((eye: (0.0, 0.0, 10.0), look_at: (0.0, 0.0, 0.0))),
    materials: [
        (name: "белый", color: (0.9, 0.9, 0.9)),
        (name: "чёрный", color: (0.02, 0.02, 0.02)),
    ],
    lamps: [
        (at: (0.0, 1.0, 6.0), color: (1.0, 1.0, 1.0), power: 30.0, range: 20.0, bulb: 0.05),
    ],
    objects: [],
    meshes: [{meshes}],
)"#
            )
        };
        let frame = |split: bool, no_msaa: bool| {
            NO_MESH_MSAA.set(no_msaa);
            let scene = HeadlessScene {
                width: 640,
                height: 360,
                time: 1.0,
                shader_error: false,
                scene: DemoScene::Space,
                msaa: Msaa::X4,
                perceptual_edges: false,
                fill: crate::FillMode::default(),
                zoom: false,
                files: crate::UserFiles {
                    space: Some(scene_text(split).into()),
                    ..Default::default()
                },
                cascades: false,
            };
            let f = render_headless(&options, &scene);
            NO_MESH_MSAA.set(false);
            f.expect("кадр")
        };
        let errors = validation_error_count();
        let whole = frame(false, false);
        let split = frame(true, false);
        let split_one = frame(true, true);
        assert_eq!(validation_error_count(), errors, "ошибки валидации Vulkan");
        // Сдвиги щелей подобраны под стандартные места 4 образцов.
        let counts = MESH_COUNTS.get();
        assert!(
            counts.contains(ash::vk::SampleCountFlags::TYPE_4),
            "видеокарта теста не умеет 4 образца для полигонов: {counts:?}"
        );
        // Насколько щели темнее сплошной коробки, по каждому столбцу у щелей.
        let (w, h) = (whole.image.width as usize, whole.image.height as usize);
        let darker = |f: &HeadlessImage, x: usize| -> i64 {
            (h / 2 - 30..h / 2 + 30)
                .map(|y| {
                    let i = (y * w + x) * 4;
                    (0..3)
                        .map(|c| {
                            i64::from(whole.image.rgba[i + c]) - i64::from(f.image.rgba[i + c])
                        })
                        .sum::<i64>()
                        .max(0)
                })
                .sum()
        };
        let mut report = Vec::new();
        for (k, _) in [(300usize, ()), (320, ()), (340, ())] {
            let ms: i64 = (k - 2..k + 3).map(|x| darker(&split, x)).sum();
            let one: i64 = (k - 2..k + 3).map(|x| darker(&split_one, x)).sum();
            report.push((k, ms, one));
        }
        eprintln!("щели (столбец, с образцами, без): {report:?}");
        if let Ok(dir) = std::env::var("PPM_DIR") {
            for (kind, f) in [("whole", &whole), ("ms", &split), ("one", &split_one)] {
                let mut out = format!("P6 {w} {h} 255\n").into_bytes();
                for px in f.image.rgba.chunks(4) {
                    out.extend_from_slice(&px[..3]);
                }
                std::fs::write(format!("{dir}/slit-{kind}.ppm"), out).expect("ppm");
            }
        }
        // Полная разница одного столбца на 60 строк: сплошной белый против чёрного фона.
        let full = darker(&split_one, 320).max(darker(&split, 320));
        assert!(
            full > 60 * 300,
            "щель у середины пикселя не видна вовсе: {report:?}"
        );
        // Щель в 0,4 пикселя накрывает 1–2 образца из 4; после перевода в sRGB это около
        // пятой части столбца (на lavapipe 7 500–8 100 против 39 000; без поиска краёв по
        // образцам — меньше 120).
        for (k, ms, _) in &report {
            assert!(
                *ms * 100 >= full * 8,
                "щель у столбца {k} с образцами MSAA пропала: {report:?}"
            );
        }
    }

    /// Счётчики лучей (7.18д2): в 2.5D-виде «комнаты» каждый проход считает столько лучей,
    /// сколько пускает, шаги и вычисления сцены сходятся между собой, поле читается только
    /// там, где оно есть, а картинка с ними та же, что без них.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn level_view_counts_rays() {
        use crate::counters::{
            PASS_AO, PASS_CONE, PASS_EDGE_SHADOWS, PASS_EDGES, PASS_MARCH, PASS_SHADOWS,
            RAY_PASS_NAMES,
        };
        // Каждый край — 4 луча с шагами по сцене: без складок 8.2 (у складки лучи не
        // шагают), и у каждого ненадёжного — свои лучи тени (без 8.2б). Каждый тест идёт в
        // своём потоке, переключатели потом не возвращаются.
        EDGE_CREASES.set(false);
        EDGE_SHADOW_REUSE.set(false);
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
        let play = HeadlessPlay {
            start: Some([-0.6, -0.5]),
            ..Default::default()
        };
        LEVEL_VIEW.set(true);
        let plain = render_headless_play(&options, &scene, Some(&play));
        SPACE_COUNTERS.set(true);
        let counted = render_headless_play(&options, &scene, Some(&play));
        LEVEL_RENDER.set(crate::LevelRender::Field);
        let field = render_headless_play(&options, &scene, Some(&play));
        LEVEL_RENDER.set(crate::LevelRender::Sdf);
        SPACE_COUNTERS.set(false);
        LEVEL_VIEW.set(false);
        let plain = plain.expect("кадр без счётчиков");
        let counted = counted.expect("кадр со счётчиками");
        let field = field.expect("кадр с полем и счётчиками");
        assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
        assert!(
            plain.space_stats.expect("счётчики кадра").rays.is_none(),
            "без флажка лучи не считаются"
        );
        let (changed, worst, _) = image_diff(&plain.image, &counted.image);
        eprintln!("счётчики: отличается {changed} пикселей, наибольшая {worst}");
        assert_eq!(changed, 0, "со счётчиками картинка та же");
        for (name, image) in [("SDF", &counted), ("поле", &field)] {
            let stats = image.space_stats.expect("счётчики кадра");
            let rays = stats.rays.expect("лучи посчитаны");
            for (pass, c) in rays.iter().enumerate() {
                eprintln!("{name}, {}: {c:?}", RAY_PASS_NAMES[pass]);
                assert!(
                    c.forms <= c.objects,
                    "{name}, {}: {c:?}",
                    RAY_PASS_NAMES[pass]
                );
                assert!(
                    c.normal <= c.scene,
                    "{name}, {}: {c:?}",
                    RAY_PASS_NAMES[pass]
                );
                assert!(
                    c.objects <= c.scene * u64::from(stats.objects),
                    "{name}, {}: {c:?}",
                    RAY_PASS_NAMES[pass]
                );
                if name == "SDF" {
                    assert_eq!((c.field, c.material), (0, 0), "{name}: поля нет");
                }
            }
            let pixels = u64::from(stats.pixels);
            let (w, h) = (u64::from(scene.width), u64::from(scene.height));
            assert_eq!(rays[PASS_MARCH].rays, pixels, "{name}: луч на пиксель");
            assert_eq!(
                rays[PASS_CONE].rays,
                w.div_ceil(8) * h.div_ceil(8),
                "{name}: конус на квадрат 8×8"
            );
            let march = rays[PASS_MARCH];
            assert!(march.steps >= march.rays, "{name}: {march:?}");
            assert_eq!(march.normal % 4, 0, "{name}: нормаль — 4 вычисления");
            assert!(march.normal <= 4 * march.rays, "{name}: {march:?}");
            let ao = rays[PASS_AO];
            assert!(ao.rays > 0 && ao.rays <= w * h / 4, "{name}: {ao:?}");
            assert_eq!(ao.steps, 11 * ao.rays, "{name}: 11 проб на точку");
            let shadows = rays[PASS_SHADOWS];
            assert!(
                shadows.rays > 0 && shadows.rays <= 2 * w * h / 4,
                "{name}: {shadows:?}"
            );
            let edge_pixels = u64::from(stats.edge_pixels.expect("края посчитаны"));
            assert_eq!(
                rays[PASS_EDGES].rays,
                4 * edge_pixels,
                "{name}: 4 луча на край"
            );
            assert!(rays[PASS_EDGE_SHADOWS].rays > 0, "{name}");
            // Два списка краёв (7.19а): лучи тени — только у ненадёжных, не больше одного
            // на долю света (солнце, углы, две лампы).
            let unreliable = u64::from(stats.edge_unreliable.expect("края посчитаны"));
            assert!(
                unreliable > 0 && unreliable < edge_pixels,
                "{name}: ненадёжных {unreliable} из {edge_pixels}"
            );
            // Угол (1 луч) у каждого ненадёжного пикселя точно, у остальных долей — если
            // есть солнце и лампы.
            assert!(
                rays[PASS_EDGE_SHADOWS].rays >= unreliable
                    && rays[PASS_EDGE_SHADOWS].rays <= 4 * unreliable,
                "{name}: {:?}, ненадёжных {unreliable}",
                rays[PASS_EDGE_SHADOWS]
            );
            // Края по причине (8.11а): каждый пиксель края — ровно одна причина.
            let kinds = stats.edge_kinds.expect("края по причине посчитаны");
            eprintln!("{name}, края: {kinds:?}");
            assert_edge_kinds(&kinds, edge_pixels, name);
            if let Some(t) = &image.gpu_timings {
                eprintln!("{name} со счётчиками: {}", t.describe());
            }
        }
        let rays = field.space_stats.and_then(|s| s.rays).expect("лучи");
        assert!(rays[PASS_MARCH].field > 0, "поле читается");
    }

    /// Счётчики краёв (8.11а) сходятся: причины в сумме дают все пиксели края, почти
    /// невидимых не больше, чем краёв с соседом (у краёв моделей соседа нет).
    fn assert_edge_kinds(k: &crate::EdgeCounts, edge_pixels: u64, name: &str) {
        assert_eq!(k.pixels, edge_pixels, "{name}: {k:?}");
        assert_eq!(
            k.mesh + k.sky + k.material + k.normal + k.depth,
            k.pixels,
            "{name}: {k:?}"
        );
        assert!(k.faint2 <= k.faint8, "{name}: {k:?}");
        assert!(k.faint8 <= k.pixels - k.mesh, "{name}: {k:?}");
    }

    /// Края завода по причине (8.11а): какие края цеха в большинстве и сколько из них
    /// почти не видно. Числа — события, а не время: на любой видеокарте при том же
    /// размере кадра те же. Печатает долю каждой причины для выбора следующих шагов.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn factory_edges_by_reason() {
        let options = GpuOptions {
            validation: true,
            gpu: None,
        };
        let errors = validation_error_count();
        let scene = HeadlessScene {
            width: 960,
            height: 540,
            time: 1.0,
            shader_error: false,
            scene: DemoScene::Space,
            msaa: Msaa::X4,
            perceptual_edges: false,
            fill: crate::FillMode::default(),
            zoom: false,
            files: crate::UserFiles {
                space: Some(include_str!("../../../scenes/3d/factory.ron").into()),
                space_dir: Some(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenes/3d").into()),
                ..Default::default()
            },
            cascades: false,
        };
        let plain = render_headless(&options, &scene).expect("кадр");
        SPACE_COUNTERS.set(true);
        let counted = render_headless(&options, &scene);
        SPACE_COUNTERS.set(false);
        let counted = counted.expect("кадр со счётчиками");
        assert_eq!(validation_error_count(), errors, "ошибки валидации Vulkan");
        let (changed, _, _) = image_diff(&plain.image, &counted.image);
        assert_eq!(changed, 0, "со счётчиками картинка та же");
        let stats = counted.space_stats.expect("числа сцены 3D");
        let edge_pixels = u64::from(stats.edge_pixels.expect("края посчитаны"));
        let k = stats.edge_kinds.expect("края по причине");
        let share = |n: u64| n as f64 * 100.0 / k.pixels.max(1) as f64;
        eprintln!(
            "завод {}×{}: краёв {} ({:.1} % кадра); модели {:.1} %, небо {:.1} %, материал \
             {:.1} %, нормаль {:.1} %, глубина {:.1} %; почти не видно: ≤2/255 {:.1} %, \
             ≤8/255 {:.1} %",
            scene.width,
            scene.height,
            k.pixels,
            k.pixels as f64 * 100.0 / f64::from(stats.pixels),
            share(k.mesh),
            share(k.sky),
            share(k.material),
            share(k.normal),
            share(k.depth),
            share(k.faint2),
            share(k.faint8),
        );
        assert_edge_kinds(&k, edge_pixels, "завод");
        assert!(k.pixels > 1000, "{k:?}");
        if let Some(rays) = stats.rays {
            for (pass, c) in rays.iter().enumerate() {
                let per = |a: u64| a as f64 / c.rays.max(1) as f64;
                eprintln!(
                    "завод, {}: лучей {}, шаг/луч {:.1}, сцена/луч {:.1}, объект/сцена {:.2}, форма/сцена {:.2}",
                    crate::counters::RAY_PASS_NAMES[pass],
                    c.rays,
                    per(c.steps),
                    per(c.scene),
                    c.objects as f64 / c.scene.max(1) as f64,
                    c.forms as f64 / c.scene.max(1) as f64,
                );
            }
        }
        eprintln!("ненадёжных краёв {:?}", stats.edge_unreliable);
        assert_eq!(
            stats.edge_pairs,
            Some(0),
            "без опыта 8.11б — четыре луча у всех"
        );

        // «Края: два луча вдали» (8.11б) с разными порогами (метров при высоте 1080): чем
        // больше порог, тем меньше краёв с двумя лучами; лучей краёв меньше, картинка
        // отличается только на этих краях. С порогом по умолчанию — часть краёв, не все.
        // Замер 960×540 (порог при этой высоте вдвое больше): 2.5 см — 77 % краёв с
        // двумя лучами, 5 см — 13 %, 10 см и больше — ни одного.
        let edge_rays = |s: &crate::demo::SpaceStats| s.rays.map(|r| r[4].rays);
        let mut last = u64::MAX;
        for meters in [0.025, crate::demo::EDGE_FAR_METERS, 0.1] {
            EDGE_FAR.set(meters);
            SPACE_COUNTERS.set(true);
            let far = render_headless(&options, &scene);
            SPACE_COUNTERS.set(false);
            EDGE_FAR.set(0.0);
            let far = far.expect("кадр с двумя лучами вдали");
            assert_eq!(validation_error_count(), errors, "ошибки валидации Vulkan");
            let far_stats = far.space_stats.expect("числа сцены 3D");
            let pairs = u64::from(far_stats.edge_pairs.expect("края посчитаны"));
            assert_eq!(far_stats.edge_pixels, stats.edge_pixels, "те же края");
            let (changed, worst, mean) = image_diff(&plain.image, &far.image);
            eprintln!(
                "два луча дальше {meters} м: {pairs} из {edge_pixels} краёв, лучей краёв \
                 {:?} → {:?}; картинка: отличается {changed} пикселей, худшее {worst}, \
                 среднее {mean:.4}",
                edge_rays(&stats),
                edge_rays(&far_stats),
            );
            assert!(pairs <= last, "{pairs} > {last}");
            if meters == crate::demo::EDGE_FAR_METERS {
                assert!(pairs > 0 && pairs < edge_pixels, "{pairs} из {edge_pixels}");
            }
            last = pairs;
            assert!(changed as u64 <= pairs, "{changed} > {pairs}");
            if let (Some(a), Some(b)) = (edge_rays(&stats), edge_rays(&far_stats)) {
                assert!(pairs == 0 || b < a, "{a} → {b}");
            }
        }
    }

    /// Поле уровня до перебора объектов (7.18д3): та же картинка до байта и те же шаги и
    /// вычисления сцены, что у поля в общем переборе; объектов и форм меньше ровно на
    /// чтения поля, материал первого слоя читается только в точках попадания.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn level_field_apart_same_picture() {
        use crate::counters::{PASS_EDGES, PASS_MARCH, RAY_PASS_NAMES};
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
        // Комната (точка теста уровня), у стола и у полки.
        for start in [[-0.6, -0.5], [0.0, -0.6], [0.4, -0.5]] {
            let play = HeadlessPlay {
                start: Some(start),
                ..Default::default()
            };
            LEVEL_VIEW.set(true);
            LEVEL_RENDER.set(crate::LevelRender::Field);
            let mut frames = Vec::new();
            for (in_loop, counted) in [(true, false), (false, false), (true, true), (false, true)] {
                LEVEL_FIELD_IN_LOOP.set(in_loop);
                SPACE_COUNTERS.set(counted);
                frames.push(render_headless_play(&options, &scene, Some(&play)));
            }
            LEVEL_FIELD_IN_LOOP.set(false);
            SPACE_COUNTERS.set(false);
            LEVEL_RENDER.set(crate::LevelRender::Sdf);
            LEVEL_VIEW.set(false);
            let frames: Vec<HeadlessImage> = frames.into_iter().map(|f| f.expect("кадр")).collect();
            assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
            for (k, what) in [(0, "в переборе"), (1, "до перебора")] {
                if let Some(t) = &frames[k].gpu_timings {
                    eprintln!("{start:?}, поле {what}: {}", t.describe());
                }
            }
            for (a, b, what) in [(0, 1, "без счётчиков"), (2, 3, "со счётчиками")]
            {
                assert!(
                    frames[a].image.rgba == frames[b].image.rgba,
                    "{start:?} {what}: картинка та же до байта"
                );
            }
            let old = frames[2].space_stats.and_then(|s| s.rays).expect("лучи");
            let new = frames[3].space_stats.and_then(|s| s.rays).expect("лучи");
            for (pass, name) in RAY_PASS_NAMES.iter().enumerate() {
                let (o, n) = (old[pass], new[pass]);
                eprintln!("{start:?} {name}: было {o:?}");
                eprintln!("{start:?} {name}: стало {n:?}");
                assert_eq!(
                    (o.rays, o.steps, o.scene, o.normal, o.field),
                    (n.rays, n.steps, n.scene, n.normal, n.field),
                    "{name}"
                );
                assert!(o.field > 0, "{name}: поле читается");
                assert_eq!(n.objects, o.objects - o.field, "{name}");
                assert_eq!(n.forms, o.forms - o.field, "{name}");
                if pass == PASS_MARCH || pass == PASS_EDGES {
                    // Не больше одного чтения на попадание (нормаль — 4 вычисления).
                    assert!(n.material <= n.normal / 4, "{name}: {n:?}");
                } else {
                    assert_eq!(n.material, 0, "{name}: материал только в попадании");
                }
            }
        }
    }

    /// Нормаль одним ближайшим объектом (7.18д5): та же картинка до байта, те же края, лучи,
    /// шаги и вычисления сцены, что у нормали по всей сцене; объектов и форм в
    /// реймарчинге и краях меньше — и у SDF, и у поля.
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn level_view_normal_shortcut_is_exact() {
        use crate::counters::{PASS_EDGES, PASS_MARCH, RAY_PASS_NAMES};
        // Сравнение лучей краёв с прежним способом нормали — на одном и том же наборе лучей
        // с шагами: без складок 8.2.
        EDGE_CREASES.set(false);
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
        let starts = [[-0.6, -0.5], [0.4, -0.5]];
        for render in [crate::LevelRender::Sdf, crate::LevelRender::Field] {
            for start in starts {
                let play = HeadlessPlay {
                    start: Some(start),
                    ..Default::default()
                };
                LEVEL_VIEW.set(true);
                LEVEL_RENDER.set(render);
                let mut frames = Vec::new();
                for (full, counted) in [(true, false), (false, false), (true, true), (false, true)]
                {
                    FULL_NORMAL.set(full);
                    SPACE_COUNTERS.set(counted);
                    frames.push(render_headless_play(&options, &scene, Some(&play)));
                }
                FULL_NORMAL.set(false);
                SPACE_COUNTERS.set(false);
                LEVEL_RENDER.set(crate::LevelRender::Sdf);
                LEVEL_VIEW.set(false);
                let frames: Vec<HeadlessImage> =
                    frames.into_iter().map(|f| f.expect("кадр")).collect();
                assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
                let what = format!("{render:?} {start:?}");
                for (k, how) in [(0, "по всей сцене"), (1, "одним объектом")]
                {
                    if let Some(t) = &frames[k].gpu_timings {
                        eprintln!("{what}, нормаль {how}: {}", t.describe());
                    }
                }
                for (a, b) in [(0, 1), (2, 3)] {
                    assert!(
                        frames[a].image.rgba == frames[b].image.rgba,
                        "{what}: картинка та же до байта"
                    );
                    let edges = |f: &HeadlessImage| f.space_stats.and_then(|s| s.edge_pixels);
                    assert_eq!(edges(&frames[a]), edges(&frames[b]), "{what}: те же края");
                }
                let old = frames[2].space_stats.and_then(|s| s.rays).expect("лучи");
                let new = frames[3].space_stats.and_then(|s| s.rays).expect("лучи");
                for (pass, name) in RAY_PASS_NAMES.iter().enumerate() {
                    let (o, n) = (old[pass], new[pass]);
                    assert_eq!(
                        (o.rays, o.steps, o.scene, o.normal),
                        (n.rays, n.steps, n.scene, n.normal),
                        "{what} {name}"
                    );
                    if pass == PASS_MARCH || pass == PASS_EDGES {
                        eprintln!("{what} {name}: было {o:?}");
                        eprintln!("{what} {name}: стало {n:?}");
                        assert!(
                            n.objects * 100 <= o.objects * 90,
                            "{what} {name}: {o:?} → {n:?}"
                        );
                        assert!(n.forms < o.forms, "{what} {name}: {o:?} → {n:?}");
                    } else {
                        assert_eq!(o, n, "{what} {name}: нормали там нет");
                    }
                }
            }
        }
    }

    /// Длинный шаг в пустоте (7.18д4): шагов у грубого прохода и теней меньше, лучей
    /// столько же; картинка почти та же — сдвигаются точки мягких полутеней и затенения
    /// углов (у поля углы чуть светлее: раньше дальняя проба упиралась в размер клетки).
    #[test]
    #[ignore = "нужен драйвер Vulkan"]
    fn level_view_free_step() {
        use crate::counters::{PASS_CONE, PASS_MARCH, PASS_SHADOWS, RAY_PASS_NAMES};
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
        for render in [crate::LevelRender::Sdf, crate::LevelRender::Field] {
            let play = HeadlessPlay {
                start: Some([-0.6, -0.5]),
                ..Default::default()
            };
            LEVEL_VIEW.set(true);
            LEVEL_RENDER.set(render);
            let mut frames = Vec::new();
            for (off, counted) in [(true, false), (false, false), (true, true), (false, true)] {
                FREE_OFF.set(off);
                SPACE_COUNTERS.set(counted);
                frames.push(render_headless_play(&options, &scene, Some(&play)));
            }
            FREE_OFF.set(false);
            SPACE_COUNTERS.set(false);
            LEVEL_RENDER.set(crate::LevelRender::Sdf);
            LEVEL_VIEW.set(false);
            let frames: Vec<HeadlessImage> = frames.into_iter().map(|f| f.expect("кадр")).collect();
            assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
            let what = format!("{render:?}");
            for (k, how) in [(0, "шаг по клетке"), (1, "длинный шаг")] {
                if let Some(t) = &frames[k].gpu_timings {
                    eprintln!("{what}, {how}: {}", t.describe());
                }
            }
            let (changed, worst, mean) = image_diff(&frames[0].image, &frames[1].image);
            let pixels = frames[0].image.rgba.len() / 4;
            let far = frames[0]
                .image
                .rgba
                .chunks(4)
                .zip(frames[1].image.rgba.chunks(4))
                .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 8))
                .count();
            eprintln!(
                "{what}: больше 2 ступеней {changed}, больше 8 — {far} из {pixels}, \
                 наибольшая {worst}, среднее {mean:.3}"
            );
            assert!(
                far * 2000 <= pixels,
                "{what}: {far} точек сильно отличаются"
            );
            let old = frames[2].space_stats.and_then(|s| s.rays).expect("лучи");
            let new = frames[3].space_stats.and_then(|s| s.rays).expect("лучи");
            for (pass, name) in RAY_PASS_NAMES.iter().enumerate() {
                let (o, n) = (old[pass], new[pass]);
                let per = |c: crate::RayCounts| c.steps as f64 / c.rays.max(1) as f64;
                eprintln!("{what} {name}: шагов на луч {:.2} → {:.2}", per(o), per(n));
                if pass == PASS_MARCH {
                    assert_eq!(o.rays, n.rays, "{what} {name}: луч на пиксель");
                }
            }
            let per = |c: crate::RayCounts| c.steps as f64 / c.rays.max(1) as f64;
            let (oc, nc) = (old[PASS_CONE], new[PASS_CONE]);
            assert!(per(nc) < per(oc) * 0.97, "{what}: грубо {oc:?} → {nc:?}");
            let (os, ns) = (old[PASS_SHADOWS], new[PASS_SHADOWS]);
            assert!(per(ns) <= per(os), "{what}: тени {os:?} → {ns:?}");
            if render == crate::LevelRender::Field {
                assert!(per(ns) < per(os) * 0.92, "{what}: тени {os:?} → {ns:?}");
            }
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
