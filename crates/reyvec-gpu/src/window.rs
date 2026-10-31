//! Вывод кадров в окно: поверхность, swapchain, синхронизация кадров.

use std::sync::Arc;

use ash::{khr, vk};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::context::{Core, GpuContext, GpuOptions};
use crate::demo::Demo;
use crate::pipeline::{color_range, image_barrier};
use crate::resources::GpuResources;
use crate::shaders::ShaderLibrary;
use crate::{AdapterInfo, GpuError, VkResultExt};

/// Сколько кадров видеокарта может готовить одновременно с процессором.
const FRAMES_IN_FLIGHT: usize = 2;

/// Окно, в которое умеет рисовать [`WindowRenderer`]. Подходит, например, `winit::window::Window`.
pub trait WindowTarget: HasWindowHandle + HasDisplayHandle + Send + Sync {}
impl<T: HasWindowHandle + HasDisplayHandle + Send + Sync> WindowTarget for T {}

/// Настройки вывода в окно.
#[derive(Clone, Debug)]
pub struct RendererOptions {
    /// Общие настройки GPU.
    pub gpu: GpuOptions,
    /// Вертикальная синхронизация: кадров не больше частоты монитора, без разрывов.
    pub vsync: bool,
    /// Следить за файлами шейдеров и перезагружать их на лету.
    pub hot_reload: bool,
}

/// Что произошло с кадром.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameStatus {
    /// Кадр нарисован и отправлен на экран.
    Presented,
    /// Кадр пропущен: окно свёрнуто или swapchain пересоздаётся.
    Skipped,
}

struct FrameSync {
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    image_available: vk::Semaphore,
    in_flight: vk::Fence,
}

/// Рисует кадры в окно.
///
/// Владеет окном (через `Arc`), поэтому поверхность Vulkan не переживёт окно.
pub struct WindowRenderer {
    frames: Vec<FrameSync>,
    frame_index: usize,
    /// Один семафор на каждое изображение swapchain: им ждёт показ кадра.
    render_finished: Vec<vk::Semaphore>,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    swapchain: vk::SwapchainKHR,
    format: vk::SurfaceFormatKHR,
    extent: vk::Extent2D,
    present_mode: vk::PresentModeKHR,
    vsync: bool,
    needs_recreate: bool,
    resources: Option<GpuResources>,
    demo: Option<Demo>,
    shaders: ShaderLibrary,
    /// Сквозной номер кадра, нужен очереди отложенного удаления.
    frame_number: u64,
    swapchain_loader: khr::swapchain::Device,
    surface_loader: khr::surface::Instance,
    surface: vk::SurfaceKHR,
    // Порядок важен: сначала устройство и экземпляр, потом окно.
    ctx: GpuContext,
    _window: Arc<dyn WindowTarget>,
}

impl WindowRenderer {
    /// Создаёт Vulkan, выбирает видеокарту и готовит вывод в окно размера `size` (в пикселях).
    pub fn new(
        window: Arc<dyn WindowTarget>,
        size: [u32; 2],
        options: &RendererOptions,
    ) -> Result<Self, GpuError> {
        let display = window
            .display_handle()
            .map_err(|e| GpuError::Window(e.to_string()))?
            .as_raw();
        let window_handle = window
            .window_handle()
            .map_err(|e| GpuError::Window(e.to_string()))?
            .as_raw();

        let core = Core::new(options.gpu.validation, Some(display))?;
        let surface_loader = khr::surface::Instance::new(&core.entry, &core.instance);
        // SAFETY: дескрипторы окна действительны, окно живёт дольше поверхности
        // (WindowRenderer держит Arc на окно и уничтожает поверхность первым).
        let surface = unsafe {
            ash_window::create_surface(&core.entry, &core.instance, display, window_handle, None)
        }
        .ctx("поверхность окна")?;

        let ctx = match GpuContext::new(core, &options.gpu, Some((&surface_loader, surface))) {
            Ok(ctx) => ctx,
            Err(failed) => {
                let (e, core) = *failed;
                // Поверхность принадлежит экземпляру: убираем её до того, как уйдёт core.
                // SAFETY: поверхность ещё ни для чего не использовалась.
                unsafe { surface_loader.destroy_surface(surface, None) };
                drop(core);
                return Err(e);
            }
        };
        let swapchain_loader = khr::swapchain::Device::new(ctx.instance(), &ctx.device);

        let mut renderer = Self {
            frames: Vec::new(),
            frame_index: 0,
            render_finished: Vec::new(),
            images: Vec::new(),
            views: Vec::new(),
            swapchain: vk::SwapchainKHR::null(),
            format: vk::SurfaceFormatKHR::default(),
            extent: vk::Extent2D::default(),
            present_mode: vk::PresentModeKHR::FIFO,
            vsync: options.vsync,
            needs_recreate: true,
            resources: None,
            demo: None,
            shaders: ShaderLibrary::new(options.hot_reload),
            frame_number: 0,
            swapchain_loader,
            surface_loader,
            surface,
            ctx,
            _window: window,
        };
        // Дальше при любой ошибке Drop аккуратно уберёт всё созданное.
        renderer.create_frames()?;
        renderer.resources = Some(GpuResources::new(&renderer.ctx)?);
        renderer.recreate_swapchain(size)?;
        tracing::info!(
            "Вывод в окно: {}×{}, формат {:?}, режим показа {:?}",
            renderer.extent.width,
            renderer.extent.height,
            renderer.format.format,
            renderer.present_mode
        );
        Ok(renderer)
    }

    /// Видеокарта, на которой идёт рисование.
    pub fn adapter(&self) -> &AdapterInfo {
        &self.ctx.adapter
    }

    /// Режим показа кадров словами: с вертикальной синхронизацией или без.
    pub fn present_mode_name(&self) -> &'static str {
        match self.present_mode {
            vk::PresentModeKHR::FIFO => "VSync",
            vk::PresentModeKHR::FIFO_RELAXED => "VSync (relaxed)",
            vk::PresentModeKHR::MAILBOX => "без VSync (mailbox)",
            vk::PresentModeKHR::IMMEDIATE => "без VSync",
            _ => "другой",
        }
    }

    /// Текст ошибки шейдера, если сохранённый шейдер не собрался (работает прошлая версия).
    pub fn shader_error(&self) -> Option<String> {
        self.shaders.error()
    }

    /// Следит ли движок за файлами шейдеров.
    pub fn shaders_hot_reload(&self) -> bool {
        self.shaders.is_watching()
    }

    /// Сообщить, что окно поменяло размер: swapchain пересоздастся перед следующим кадром.
    pub fn resize(&mut self) {
        self.needs_recreate = true;
    }

    /// Рисует один кадр. `size`: текущий размер окна в пикселях, `time`: секунды с запуска.
    pub fn render(&mut self, size: [u32; 2], time: f32) -> Result<FrameStatus, GpuError> {
        if size[0] == 0 || size[1] == 0 {
            return Ok(FrameStatus::Skipped);
        }
        if self.needs_recreate && !self.recreate_swapchain(size)? {
            return Ok(FrameStatus::Skipped);
        }
        let changed = self.shaders.poll();
        if !changed.is_empty()
            && let (Some(demo), Some(res)) = (self.demo.as_mut(), self.resources.as_ref())
        {
            // SAFETY: ждём, пока видеокарта перестанет использовать старые конвейеры.
            unsafe { self.ctx.device.device_wait_idle() }.ctx("ожидание видеокарты")?;
            demo.reload(&self.ctx, res, &mut self.shaders, &changed);
        }
        let shader_error = self.shaders.error().is_some();

        let device = &self.ctx.device;
        let frame = &self.frames[self.frame_index];

        // SAFETY: забор принадлежит этому устройству.
        unsafe { device.wait_for_fences(&[frame.in_flight], true, u64::MAX) }
            .ctx("ожидание кадра")?;
        // Забор этой ячейки дождались, значит кадр, что был в ней раньше, закончен.
        let completed = self.frame_number.checked_sub(FRAMES_IN_FLIGHT as u64);
        if let Some(res) = self.resources.as_mut() {
            res.begin_frame(&self.ctx, self.frame_number, completed);
            if let Some(demo) = self.demo.as_mut() {
                demo.update(&self.ctx, res, time)?;
            }
        }

        // SAFETY: swapchain и семафор живы; семафор не ждёт ни одна отправка.
        let acquired = unsafe {
            self.swapchain_loader.acquire_next_image(
                self.swapchain,
                u64::MAX,
                frame.image_available,
                vk::Fence::null(),
            )
        };
        let image_index = match acquired {
            Ok((index, suboptimal)) => {
                if suboptimal {
                    self.needs_recreate = true;
                }
                index as usize
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.needs_recreate = true;
                return Ok(FrameStatus::Skipped);
            }
            Err(e) => {
                return Err(GpuError::Vulkan {
                    what: "получение изображения",
                    result: e,
                });
            }
        };

        let image = self.images[image_index];
        let view = self.views[image_index];
        let render_finished = self.render_finished[image_index];
        let extent = self.extent;

        // SAFETY: забор кадра дождались, значит его командный буфер свободен.
        // Все объекты принадлежат этому устройству и живы до конца кадра.
        unsafe {
            device
                .reset_fences(&[frame.in_flight])
                .ctx("сброс забора")?;
            device
                .reset_command_pool(frame.pool, vk::CommandPoolResetFlags::empty())
                .ctx("сброс командного пула")?;
            let begin = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            device
                .begin_command_buffer(frame.cmd, &begin)
                .ctx("запись команд")?;

            let to_color = [image_barrier(
                image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::NONE,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            )];
            device.cmd_pipeline_barrier2(
                frame.cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_color),
            );

            if let (Some(demo), Some(res)) = (&self.demo, self.resources.as_mut()) {
                demo.record(
                    &self.ctx,
                    res,
                    frame.cmd,
                    self.frame_index,
                    view,
                    extent,
                    time,
                    shader_error,
                );
            }

            let to_present = [image_barrier(
                image,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::PRESENT_SRC_KHR,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                vk::PipelineStageFlags2::NONE,
                vk::AccessFlags2::NONE,
            )];
            device.cmd_pipeline_barrier2(
                frame.cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_present),
            );
            device
                .end_command_buffer(frame.cmd)
                .ctx("конец записи команд")?;

            let wait = [vk::SemaphoreSubmitInfo::default()
                .semaphore(frame.image_available)
                .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
            let signal = [vk::SemaphoreSubmitInfo::default()
                .semaphore(render_finished)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            let cmds = [vk::CommandBufferSubmitInfo::default().command_buffer(frame.cmd)];
            let submit = vk::SubmitInfo2::default()
                .wait_semaphore_infos(&wait)
                .command_buffer_infos(&cmds)
                .signal_semaphore_infos(&signal);
            device
                .queue_submit2(self.ctx.queue, &[submit], frame.in_flight)
                .ctx("отправка кадра")?;
        }

        let wait_present = [render_finished];
        let swapchains = [self.swapchain];
        let indices = [image_index as u32];
        let present = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_present)
            .swapchains(&swapchains)
            .image_indices(&indices);
        // SAFETY: изображение получено из этого swapchain и отправлено на рисование выше.
        match unsafe {
            self.swapchain_loader
                .queue_present(self.ctx.queue, &present)
        } {
            Ok(false) => {}
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => self.needs_recreate = true,
            Err(e) => {
                return Err(GpuError::Vulkan {
                    what: "показ кадра",
                    result: e,
                });
            }
        }

        self.frame_number += 1;
        self.frame_index = (self.frame_number % FRAMES_IN_FLIGHT as u64) as usize;
        Ok(FrameStatus::Presented)
    }

    fn create_frames(&mut self) -> Result<(), GpuError> {
        let device = &self.ctx.device;
        for _ in 0..FRAMES_IN_FLIGHT {
            // Кладём кадр с пустыми дескрипторами сразу: если дальше будет ошибка,
            // Drop уберёт то, что успели создать.
            self.frames.push(FrameSync {
                pool: vk::CommandPool::null(),
                cmd: vk::CommandBuffer::null(),
                image_available: vk::Semaphore::null(),
                in_flight: vk::Fence::null(),
            });
            let frame = self.frames.last_mut().expect("кадр только что добавлен");
            let pool_info = vk::CommandPoolCreateInfo::default()
                .flags(vk::CommandPoolCreateFlags::TRANSIENT)
                .queue_family_index(self.ctx.queue_family);
            // SAFETY: простые объекты устройства; уничтожаются в Drop.
            unsafe {
                frame.pool = device
                    .create_command_pool(&pool_info, None)
                    .ctx("командный пул")?;
                let alloc = vk::CommandBufferAllocateInfo::default()
                    .command_pool(frame.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1);
                frame.cmd = device
                    .allocate_command_buffers(&alloc)
                    .ctx("командный буфер")?[0];
                frame.image_available = device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .ctx("семафор")?;
                frame.in_flight = device
                    .create_fence(
                        &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                        None,
                    )
                    .ctx("забор")?;
            }
        }
        Ok(())
    }

    /// Пересоздаёт swapchain под текущий размер окна. `false`, если окно свёрнуто.
    fn recreate_swapchain(&mut self, size: [u32; 2]) -> Result<bool, GpuError> {
        let pd = self.ctx.physical;
        // SAFETY: поверхность и видеокарта из одного экземпляра.
        let (caps, formats, modes) = unsafe {
            (
                self.surface_loader
                    .get_physical_device_surface_capabilities(pd, self.surface)
                    .ctx("свойства поверхности")?,
                self.surface_loader
                    .get_physical_device_surface_formats(pd, self.surface)
                    .ctx("форматы поверхности")?,
                self.surface_loader
                    .get_physical_device_surface_present_modes(pd, self.surface)
                    .ctx("режимы показа")?,
            )
        };
        let extent = choose_extent(&caps, size);
        if extent.width == 0 || extent.height == 0 {
            return Ok(false);
        }
        let format = choose_format(&formats);
        let present_mode = choose_present_mode(&modes, self.vsync);
        let mut image_count = caps.min_image_count + 1;
        if caps.max_image_count != 0 {
            image_count = image_count.min(caps.max_image_count);
        }

        // SAFETY: ждём, пока видеокарта закончит со старым swapchain, прежде чем трогать его.
        unsafe { self.ctx.device.device_wait_idle() }.ctx("ожидание видеокарты")?;

        let old = self.swapchain;
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old);
        // SAFETY: параметры взяты из возможностей поверхности.
        let swapchain = unsafe { self.swapchain_loader.create_swapchain(&info, None) }
            .ctx("создание swapchain")?;

        // SAFETY: видеокарта простаивает (device_wait_idle выше), старые объекты не используются.
        unsafe {
            self.destroy_swapchain_objects();
            if old != vk::SwapchainKHR::null() {
                self.swapchain_loader.destroy_swapchain(old, None);
            }
        }
        self.swapchain = swapchain;
        self.format = format;
        self.extent = extent;
        self.present_mode = present_mode;

        let device = &self.ctx.device;
        // SAFETY: swapchain только что создан.
        self.images = unsafe { self.swapchain_loader.get_swapchain_images(swapchain) }
            .ctx("изображения swapchain")?;
        for &image in &self.images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format.format)
                .subresource_range(color_range());
            // SAFETY: изображение принадлежит swapchain этого устройства.
            unsafe {
                self.views.push(
                    device
                        .create_image_view(&view_info, None)
                        .ctx("вид изображения")?,
                );
                self.render_finished.push(
                    device
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                        .ctx("семафор показа")?,
                );
            }
        }

        if let Some(res) = self.resources.as_mut() {
            match self.demo.as_mut() {
                // Видеокарта простаивает (device_wait_idle выше), конвейеры можно менять.
                Some(demo) => demo.set_format(&self.ctx, res, &self.shaders, format.format)?,
                None => {
                    self.demo = Some(Demo::new(
                        &self.ctx,
                        res,
                        &self.shaders,
                        format.format,
                        FRAMES_IN_FLIGHT,
                    )?)
                }
            }
        }

        self.needs_recreate = false;
        tracing::debug!("swapchain: {}×{}", extent.width, extent.height);
        Ok(true)
    }

    /// # Safety
    /// Видеокарта не использует виды изображений и семафоры показа.
    unsafe fn destroy_swapchain_objects(&mut self) {
        let device = &self.ctx.device;
        // SAFETY: условия описаны в контракте функции.
        unsafe {
            for view in self.views.drain(..) {
                device.destroy_image_view(view, None);
            }
            for sem in self.render_finished.drain(..) {
                device.destroy_semaphore(sem, None);
            }
        }
        self.images.clear();
    }
}

impl Drop for WindowRenderer {
    fn drop(&mut self) {
        // SAFETY: сначала ждём видеокарту, потом уничтожаем объекты в обратном порядке.
        // Пустые (null) дескрипторы Vulkan разрешает передавать в destroy-функции.
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
            self.destroy_swapchain_objects();
            let device = &self.ctx.device;
            if let Some(demo) = self.demo.take() {
                demo.destroy(&self.ctx);
            }
            if let Some(res) = self.resources.take() {
                res.destroy(&self.ctx);
            }
            for frame in self.frames.drain(..) {
                device.destroy_fence(frame.in_flight, None);
                device.destroy_semaphore(frame.image_available, None);
                device.destroy_command_pool(frame.pool, None);
            }
            self.swapchain_loader
                .destroy_swapchain(self.swapchain, None);
            self.surface_loader.destroy_surface(self.surface, None);
        }
        // Дальше Rust сам уничтожит ctx (устройство и экземпляр), а затем отпустит окно.
    }
}

/// sRGB-формат, чтобы видеокарта сама переводила линейный цвет в цвет экрана.
fn choose_format(formats: &[vk::SurfaceFormatKHR]) -> vk::SurfaceFormatKHR {
    let preferred = [vk::Format::B8G8R8A8_SRGB, vk::Format::R8G8B8A8_SRGB];
    for want in preferred {
        if let Some(f) = formats
            .iter()
            .find(|f| f.format == want && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        {
            return *f;
        }
    }
    formats.first().copied().unwrap_or_default()
}

fn choose_present_mode(modes: &[vk::PresentModeKHR], vsync: bool) -> vk::PresentModeKHR {
    if vsync {
        return vk::PresentModeKHR::FIFO;
    }
    [vk::PresentModeKHR::MAILBOX, vk::PresentModeKHR::IMMEDIATE]
        .into_iter()
        .find(|m| modes.contains(m))
        .unwrap_or(vk::PresentModeKHR::FIFO)
}

fn choose_extent(caps: &vk::SurfaceCapabilitiesKHR, size: [u32; 2]) -> vk::Extent2D {
    if caps.current_extent.width != u32::MAX {
        return caps.current_extent;
    }
    vk::Extent2D {
        width: size[0].clamp(caps.min_image_extent.width, caps.max_image_extent.width),
        height: size[1].clamp(caps.min_image_extent.height, caps.max_image_extent.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_srgb_format() {
        let formats = [
            vk::SurfaceFormatKHR {
                format: vk::Format::B8G8R8A8_UNORM,
                color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            },
            vk::SurfaceFormatKHR {
                format: vk::Format::B8G8R8A8_SRGB,
                color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            },
        ];
        assert_eq!(choose_format(&formats).format, vk::Format::B8G8R8A8_SRGB);
        assert_eq!(
            choose_format(&formats[..1]).format,
            vk::Format::B8G8R8A8_UNORM
        );
    }

    #[test]
    fn present_modes() {
        use vk::PresentModeKHR as M;
        let all = [M::FIFO, M::MAILBOX, M::IMMEDIATE];
        assert_eq!(choose_present_mode(&all, true), M::FIFO);
        assert_eq!(choose_present_mode(&all, false), M::MAILBOX);
        assert_eq!(
            choose_present_mode(&[M::FIFO, M::IMMEDIATE], false),
            M::IMMEDIATE
        );
        assert_eq!(choose_present_mode(&[M::FIFO], false), M::FIFO);
    }

    #[test]
    fn extent_follows_surface_or_window() {
        let mut caps = vk::SurfaceCapabilitiesKHR {
            current_extent: vk::Extent2D {
                width: 800,
                height: 600,
            },
            min_image_extent: vk::Extent2D {
                width: 1,
                height: 1,
            },
            max_image_extent: vk::Extent2D {
                width: 4096,
                height: 4096,
            },
            ..Default::default()
        };
        assert_eq!(choose_extent(&caps, [1, 1]).width, 800);
        caps.current_extent.width = u32::MAX;
        let e = choose_extent(&caps, [1920, 9000]);
        assert_eq!((e.width, e.height), (1920, 4096));
    }
}
