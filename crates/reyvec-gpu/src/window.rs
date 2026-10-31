//! Вывод кадров в окно: поверхность, swapchain, синхронизация кадров.

use std::sync::Arc;

use ash::{khr, vk};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::context::{Core, GpuContext, GpuOptions};
use crate::demo::Demo;
use crate::graph::{Access, ImageState, RenderGraph, TransientPool};
use crate::pipeline::color_range;
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

/// Если старый swapchain так долго не удаётся удалить (забор показа не сработал),
/// останавливаем видеокарту и удаляем его наверняка.
const RETIRE_TIMEOUT_FRAMES: u64 = 120;

/// Старый swapchain после изменения размера окна. Его последние кадры могут ещё
/// показываться, поэтому он удаляется не сразу, а когда сработают заборы всех его показов.
struct Retired {
    swapchain: vk::SwapchainKHR,
    views: Vec<vk::ImageView>,
    semaphores: Vec<vk::Semaphore>,
    /// Номер кадра, когда swapchain стал старым.
    frame: u64,
}

/// Заборы показа (VK_EXT_swapchain_maintenance1): забор срабатывает, когда показ кадра
/// закончил пользоваться картинкой и семафором. Только по ним можно точно сказать,
/// что старый swapchain больше не нужен.
#[derive(Default)]
struct PresentFences {
    /// Показы в работе: в какой swapchain и какой забор.
    pending: Vec<(vk::SwapchainKHR, vk::Fence)>,
    free: Vec<vk::Fence>,
}

impl PresentFences {
    fn take(&mut self, device: &ash::Device) -> Result<vk::Fence, GpuError> {
        if let Some(f) = self.free.pop() {
            return Ok(f);
        }
        // SAFETY: простой объект устройства, уничтожается в destroy().
        unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }.ctx("забор показа")
    }

    /// Возвращает в оборот заборы показов, которые уже закончились.
    fn collect(&mut self, device: &ash::Device) -> Result<(), GpuError> {
        let mut i = 0;
        while i < self.pending.len() {
            let fence = self.pending[i].1;
            // SAFETY: забор принадлежит этому устройству.
            if unsafe { device.get_fence_status(fence) }.ctx("состояние забора показа")?
            {
                // SAFETY: забор сработал, его больше никто не ждёт.
                unsafe { device.reset_fences(&[fence]) }.ctx("сброс забора показа")?;
                self.free.push(fence);
                self.pending.swap_remove(i);
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// Есть ли незаконченные показы в `swapchain`.
    fn busy(&self, swapchain: vk::SwapchainKHR) -> bool {
        self.pending.iter().any(|(s, _)| *s == swapchain)
    }

    /// Уничтожает все заборы. Вызывать, когда видеокарта простаивает.
    fn destroy(&mut self, device: &ash::Device) {
        for f in self
            .free
            .drain(..)
            .chain(self.pending.drain(..).map(|(_, f)| f))
        {
            // SAFETY: видеокарта простаивает (условие функции).
            unsafe { device.destroy_fence(f, None) };
        }
    }
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
    /// Временные картинки графа кадра.
    transients: TransientPool,
    retired: Vec<Retired>,
    present_fences: PresentFences,
    /// Устройство графа уже записано в лог (пишем один раз).
    graph_logged: bool,
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
            transients: TransientPool::new(),
            retired: Vec::new(),
            present_fences: PresentFences::default(),
            graph_logged: false,
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
        if renderer.ctx.present_fences {
            tracing::info!(
                "Заборы показа есть (VK_EXT_swapchain_maintenance1): при изменении размера окна видеокарта не останавливается"
            );
        } else {
            tracing::info!(
                "Заборов показа нет: при изменении размера окна видеокарта на миг останавливается"
            );
        }
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
        let in_flight = self.frames[self.frame_index].in_flight;
        // SAFETY: забор принадлежит этому устройству.
        unsafe {
            self.ctx
                .device
                .wait_for_fences(&[in_flight], true, u64::MAX)
        }
        .ctx("ожидание кадра")?;
        // Забор этой ячейки дождались, значит кадр, что был в ней раньше, закончен.
        let completed = self.frame_number.checked_sub(FRAMES_IN_FLIGHT as u64);
        self.transients
            .begin_frame(&self.ctx, self.frame_number, completed);
        self.collect_retired(completed)?;

        let device = &self.ctx.device;
        let frame = &self.frames[self.frame_index];
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

            if let (Some(demo), Some(res)) = (&self.demo, self.resources.as_mut()) {
                let data = demo.prepare(res, self.frame_index, extent, time, shader_error);
                let res = &*res;
                let mut graph = RenderGraph::new();
                // Картинку окна только что выдал acquire: содержимого нет, рисовать в неё
                // можно после семафора image_available (он ждёт на стадии вывода цвета).
                let target = graph.import(
                    "окно",
                    image,
                    view,
                    extent,
                    self.format.format,
                    ImageState {
                        layout: vk::ImageLayout::UNDEFINED,
                        stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                        access: vk::AccessFlags2::NONE,
                    },
                    Some(Access::Present),
                );
                demo.add_passes(&mut graph, &res.bindless, data, target);
                let report = graph.execute(
                    &self.ctx,
                    frame.cmd,
                    &mut self.transients,
                    &res.bindless,
                    self.frame_number,
                )?;
                if !self.graph_logged {
                    tracing::info!("Граф кадра: {}", report.describe());
                    self.graph_logged = true;
                }
            }

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
        let mut present = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_present)
            .swapchains(&swapchains)
            .image_indices(&indices);
        let present_fence = if self.ctx.present_fences {
            let f = self.present_fences.take(&self.ctx.device)?;
            // Забор числится в работе сразу: даже если показ вернёт ошибку, мы его не
            // переиспользуем, пока он не сработает (или пока видеокарта не остановится).
            self.present_fences.pending.push((self.swapchain, f));
            Some([f])
        } else {
            None
        };
        let mut fence_info = vk::SwapchainPresentFenceInfoEXT::default();
        if let Some(f) = &present_fence {
            fence_info = fence_info.fences(f);
            present = present.push_next(&mut fence_info);
        }
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

        // Видеокарту не останавливаем: старый swapchain отдаём в новый как old_swapchain,
        // а удаляем его, когда сработают заборы всех его показов (см. collect_retired).
        // Так при растягивании окна кадры не ждут полной остановки видеокарты.
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

        if old != vk::SwapchainKHR::null() {
            self.retired.push(Retired {
                swapchain: old,
                views: std::mem::take(&mut self.views),
                semaphores: std::mem::take(&mut self.render_finished),
                frame: self.frame_number,
            });
            if !self.ctx.present_fences {
                // Без заборов показа нельзя узнать, что старые кадры показаны: ждём видеокарту.
                // SAFETY: простое ожидание устройства.
                unsafe { self.ctx.device.device_wait_idle() }.ctx("ожидание видеокарты")?;
                self.destroy_all_retired();
            }
        }
        self.images.clear();
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
                Some(demo) => {
                    if demo.output_format() != format.format {
                        // Редкий случай (окно перенесли на другой монитор): конвейер вывода
                        // пересобирается, старый может ещё использоваться, поэтому ждём.
                        // SAFETY: простое ожидание устройства.
                        unsafe { self.ctx.device.device_wait_idle() }.ctx("ожидание видеокарты")?;
                        demo.set_format(&self.ctx, res, &self.shaders, format.format)?;
                    }
                }
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

    /// Удаляет старые swapchain, все показы которых закончились. `completed`: последний
    /// кадр, который видеокарта точно закончила.
    fn collect_retired(&mut self, completed: Option<u64>) -> Result<(), GpuError> {
        // Заборы собираем каждый кадр, иначе они копились бы, пока окно не меняет размер.
        self.present_fences.collect(&self.ctx.device)?;
        if self.retired.is_empty() {
            return Ok(());
        }
        let oldest = self.retired.iter().map(|r| r.frame).min().unwrap_or(0);
        if self.frame_number.saturating_sub(oldest) > RETIRE_TIMEOUT_FRAMES {
            tracing::warn!("Старый swapchain долго не освобождается, жду видеокарту");
            // SAFETY: простое ожидание устройства.
            unsafe { self.ctx.device.device_wait_idle() }.ctx("ожидание видеокарты")?;
            self.destroy_all_retired();
            return Ok(());
        }
        let mut i = 0;
        while i < self.retired.len() {
            let r = &self.retired[i];
            // Нужны оба условия: показы его картинок закончены (сработали их заборы), и
            // закончен кадр r.frame, первый после него. Кадры, рисовавшие в старые картинки,
            // все раньше r.frame; ждём ещё один кадр сверху, чтобы и показ последнего из них,
            // стоявший в очереди перед кадром r.frame, точно считался завершённым.
            let frames_done = completed.is_some_and(|c| c >= r.frame);
            if !frames_done || self.present_fences.busy(r.swapchain) {
                i += 1;
                continue;
            }
            let r = self.retired.swap_remove(i);
            // SAFETY: кадры и показы со старыми объектами закончены (проверено выше), а новые
            // кадры в этот swapchain больше не рисуются.
            unsafe { destroy_retired(&self.ctx.device, &self.swapchain_loader, r) };
        }
        Ok(())
    }

    /// Удаляет все старые swapchain. Вызывать, когда видеокарта простаивает.
    fn destroy_all_retired(&mut self) {
        let device = &self.ctx.device;
        for r in self.retired.drain(..) {
            let fences = &mut self.present_fences;
            fences.pending.retain(|&(s, f)| {
                if s != r.swapchain {
                    return true;
                }
                // Видеокарта простаивает: забор либо сработал, либо уже не сработает
                // никогда. Сбрасываем и возвращаем в оборот.
                // SAFETY: забор этого устройства, его никто не ждёт.
                if unsafe { device.reset_fences(&[f]) }.is_ok() {
                    fences.free.push(f);
                } else {
                    // SAFETY: см. выше.
                    unsafe { device.destroy_fence(f, None) };
                }
                false
            });
            // SAFETY: видеокарта простаивает (условие функции).
            unsafe { destroy_retired(device, &self.swapchain_loader, r) };
        }
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
            self.destroy_all_retired();
            self.present_fences.destroy(&self.ctx.device);
            self.destroy_swapchain_objects();
            self.transients.destroy(&self.ctx);
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

/// Уничтожает объекты старого swapchain.
///
/// # Safety
/// Ни видеокарта, ни показ кадров их больше не используют.
unsafe fn destroy_retired(device: &ash::Device, loader: &khr::swapchain::Device, r: Retired) {
    // SAFETY: условие функции.
    unsafe {
        for v in r.views {
            device.destroy_image_view(v, None);
        }
        for s in r.semaphores {
            device.destroy_semaphore(s, None);
        }
        loader.destroy_swapchain(r.swapchain, None);
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
