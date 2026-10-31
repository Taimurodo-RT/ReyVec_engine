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
#[derive(Clone, Copy, Debug, PartialEq)]
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
    pub fill: crate::FillMethod,
    /// Лупа векторной сцены (увеличение в 8 раз у края сердца).
    pub zoom: bool,
}

/// Кадр, нарисованный без окна.
pub struct HeadlessImage {
    /// Сама картинка.
    pub image: FrameImage,
    /// На какой видеокарте нарисовано.
    pub adapter: AdapterInfo,
    /// Сколько видеокарта потратила на кадр (нет, если видеокарта не умеет замерять).
    pub gpu_timings: Option<GpuFrameTimings>,
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
            fill: crate::FillMethod::Triangles,
            zoom: false,
        },
    )
}

/// Рисует кадр песочницы без окна: тот же кадр, что в окне, только без панели счётчиков.
pub fn render_headless(
    options: &GpuOptions,
    scene: &HeadlessScene,
) -> Result<HeadlessImage, GpuError> {
    let core = Core::new(options.validation, None)?;
    let ctx = GpuContext::new(core, options, None).map_err(|failed| failed.0)?;
    let (rgba, gpu_timings) = render(&ctx, scene)?;
    Ok(HeadlessImage {
        image: FrameImage {
            width: scene.width,
            height: scene.height,
            rgba,
        },
        adapter: ctx.adapter.clone(),
        gpu_timings,
    })
}

fn render(
    ctx: &GpuContext,
    scene: &HeadlessScene,
) -> Result<(Vec<u8>, Option<GpuFrameTimings>), GpuError> {
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
        )?);
        // Шахматка меняет клетки со временем: загружаем ту, что нужна в момент scene.time.
        if let (Some(demo), Some(gpu)) = (r.demo.as_mut(), r.gpu.as_mut()) {
            demo.update(ctx, gpu, scene.time)?;
            demo.set_msaa(ctx, gpu, &ShaderLibrary::new(false), scene.msaa)?;
            demo.set_perceptual_edges(scene.perceptual_edges);
            demo.set_fill(scene.fill);
            demo.set_zoom(scene.zoom);
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
        let data = demo.prepare(gpu, 0, extent, scene.time, scene.shader_error, scene.scene);
        let demo = &*demo;
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
        Ok((pixels, timings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation_error_count;

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
