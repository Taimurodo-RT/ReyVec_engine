//! Рисование без окна: кадр рисуется в картинку в памяти видеокарты и читается обратно.
//! Нужно для автотестов и CI, где окна нет.

use ash::vk;

use crate::context::{Core, GpuContext, GpuOptions};
use crate::demo::Demo;
use crate::pipeline::{color_range, image_barrier};
use crate::resources::GpuResources;
use crate::shaders::ShaderLibrary;
use crate::{AdapterInfo, GpuError, VkResultExt};

const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

/// Картинка, нарисованная без окна.
pub struct HeadlessImage {
    /// Ширина в пикселях.
    pub width: u32,
    /// Высота в пикселях.
    pub height: u32,
    /// Пиксели RGBA по 8 бит, строка за строкой сверху вниз.
    pub rgba: Vec<u8>,
    /// На какой видеокарте нарисовано.
    pub adapter: AdapterInfo,
}

impl HeadlessImage {
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

/// Рисует кадр песочницы (треугольник и спрайты в момент `time = 0`) в картинку `width × height`.
pub fn render_triangle_headless(
    options: &GpuOptions,
    width: u32,
    height: u32,
) -> Result<HeadlessImage, GpuError> {
    let core = Core::new(options.validation, None)?;
    let ctx = GpuContext::new(core, options, None).map_err(|failed| failed.0)?;
    let rgba = render(&ctx, width, height)?;
    Ok(HeadlessImage {
        width,
        height,
        rgba,
        adapter: ctx.adapter.clone(),
    })
}

fn render(ctx: &GpuContext, width: u32, height: u32) -> Result<Vec<u8>, GpuError> {
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
        let to_color = [image_barrier(
            r.image,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::PipelineStageFlags2::NONE,
            vk::AccessFlags2::NONE,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        )];
        d.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(&to_color),
        );
        r.demo.as_ref().expect("сцена создана выше").record(
            ctx,
            r.gpu.as_mut().expect("ресурсы созданы выше"),
            cmd,
            0,
            r.view,
            extent,
            0.0,
            false,
        );
        let to_transfer = [image_barrier(
            r.image,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::TRANSFER_READ,
        )];
        d.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(&to_transfer),
        );
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
        d.cmd_copy_image_to_buffer(
            cmd,
            r.image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            r.buffer,
            &[region],
        );
        let to_host = [vk::BufferMemoryBarrier2::default()
            .buffer(r.buffer)
            .size(vk::WHOLE_SIZE)
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)];
        d.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().buffer_memory_barriers(&to_host),
        );
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
        Ok(pixels)
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

        // Угол: фон. UNORM-картинка хранит линейный цвет как есть.
        let corner = img.pixel(0, 0);
        let expect_bg = crate::CLEAR_COLOR.map(|c| (c * 255.0).round() as u8);
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
            disk[0] > 220 && disk[1] > 150 && disk[2] < 100,
            "круг {disk:?}"
        );

        assert_eq!(validation_error_count(), 0, "ошибки валидации Vulkan");
    }
}
