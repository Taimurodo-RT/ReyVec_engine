//! Экземпляр Vulkan, слои валидации, выбор видеокарты и логическое устройство.

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicU32, Ordering};

use ash::{ext, khr, vk};
use raw_window_handle::RawDisplayHandle;

use crate::adapter::{AdapterInfo, AdapterKind, AdapterProbe, c_chars_to_string, find_problem};
use crate::{GpuError, VkResultExt};

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

static VALIDATION_ERRORS: AtomicU32 = AtomicU32::new(0);

/// Сколько ошибок сообщили слои валидации с момента запуска программы.
///
/// Цель каждого этапа: ноль. Песочница завершается с ошибкой, если число больше нуля.
pub fn validation_error_count() -> u32 {
    VALIDATION_ERRORS.load(Ordering::Relaxed)
}

/// Общие настройки GPU-слоя.
#[derive(Clone, Debug, Default)]
pub struct GpuOptions {
    /// Включить слои валидации Vulkan. Они ловят ошибки использования API,
    /// но замедляют работу. Нужен установленный Vulkan SDK.
    pub validation: bool,
    /// Какую видеокарту взять: номер или часть названия. `None`: выбрать самим.
    pub gpu: Option<String>,
}

/// Уровень экземпляра Vulkan: загрузчик, экземпляр и приёмник сообщений валидации.
pub(crate) struct Core {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    debug: Option<(ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    /// Включено расширение экземпляра VK_EXT_surface_maintenance1 (нужно для заборов показа).
    surface_maintenance1: bool,
}

impl Core {
    pub fn new(validation: bool, display: Option<RawDisplayHandle>) -> Result<Self, GpuError> {
        // SAFETY: загрузка системной библиотеки Vulkan; дальше пользуемся только её функциями.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| GpuError::Loading(e.to_string()))?;

        // SAFETY: вызов без аргументов-указателей.
        let instance_version = unsafe { entry.try_enumerate_instance_version() }
            .ctx("версия Vulkan")?
            .unwrap_or(vk::API_VERSION_1_0);
        if instance_version < vk::API_VERSION_1_3 {
            return Err(GpuError::Loading(format!(
                "загрузчик Vulkan версии {}.{}, нужен 1.3",
                vk::api_version_major(instance_version),
                vk::api_version_minor(instance_version)
            )));
        }

        let mut extensions: Vec<*const c_char> = Vec::new();
        let mut surface_maintenance1 = false;
        if let Some(display) = display {
            let required =
                ash_window::enumerate_required_extensions(display).ctx("расширения для окна")?;
            extensions.extend_from_slice(required);
            // Необязательные: с ними видеокарта сообщает, когда кадр показан (см. window.rs).
            // SAFETY: вызов без слоя.
            let available = unsafe { entry.enumerate_instance_extension_properties(None) }
                .ctx("расширения экземпляра")?;
            let has = |name: &CStr| {
                available
                    .iter()
                    .any(|e| e.extension_name_as_c_str() == Ok(name))
            };
            if has(ext::surface_maintenance1::NAME) && has(khr::get_surface_capabilities2::NAME) {
                extensions.push(ext::surface_maintenance1::NAME.as_ptr());
                extensions.push(khr::get_surface_capabilities2::NAME.as_ptr());
                surface_maintenance1 = true;
            }
        }

        let mut layers: Vec<*const c_char> = Vec::new();
        let mut use_debug = false;
        if validation {
            // SAFETY: вызов без аргументов-указателей.
            let available =
                unsafe { entry.enumerate_instance_layer_properties() }.ctx("список слоёв")?;
            let has_layer = available
                .iter()
                .any(|l| l.layer_name_as_c_str() == Ok(VALIDATION_LAYER));
            if has_layer {
                layers.push(VALIDATION_LAYER.as_ptr());
                extensions.push(ext::debug_utils::NAME.as_ptr());
                use_debug = true;
            } else {
                tracing::warn!(
                    "Слои валидации не найдены: установите Vulkan SDK. Продолжаю без валидации"
                );
            }
        }

        let app_info = vk::ApplicationInfo::default()
            .application_name(c"ReyVec")
            .engine_name(c"ReyVec")
            .engine_version(vk::make_api_version(0, 0, 1, 0))
            .api_version(vk::API_VERSION_1_3);
        let mut debug_info = debug_messenger_info();
        let mut create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&layers)
            .enabled_extension_names(&extensions);
        if use_debug {
            // Так сообщения валидации ловятся и во время создания самого экземпляра.
            create_info = create_info.push_next(&mut debug_info);
        }

        // SAFETY: все указатели в create_info живут до конца вызова.
        let instance = unsafe { entry.create_instance(&create_info, None) }
            .ctx("создание экземпляра Vulkan")?;

        let debug = if use_debug {
            let loader = ext::debug_utils::Instance::new(&entry, &instance);
            // SAFETY: экземпляр создан с расширением debug_utils.
            match unsafe { loader.create_debug_utils_messenger(&debug_messenger_info(), None) } {
                Ok(m) => Some((loader, m)),
                Err(e) => {
                    tracing::warn!("Не удалось подключить сообщения валидации: {e:?}");
                    None
                }
            }
        } else {
            None
        };
        if debug.is_some() {
            tracing::info!("Слои валидации Vulkan включены");
        }

        Ok(Self {
            entry,
            instance,
            debug,
            surface_maintenance1,
        })
    }

    /// Перечисляет видеокарты. Если передана поверхность окна, проверяет и вывод в неё.
    pub fn adapters(
        &self,
        surface: Option<(&khr::surface::Instance, vk::SurfaceKHR)>,
    ) -> Result<Vec<(vk::PhysicalDevice, AdapterInfo, u32)>, GpuError> {
        // SAFETY: экземпляр жив.
        let devices =
            unsafe { self.instance.enumerate_physical_devices() }.ctx("список видеокарт")?;
        devices
            .into_iter()
            .enumerate()
            .map(|(index, pd)| {
                let (info, family) = self.describe(index, pd, surface)?;
                Ok((pd, info, family))
            })
            .collect()
    }

    fn describe(
        &self,
        index: usize,
        pd: vk::PhysicalDevice,
        surface: Option<(&khr::surface::Instance, vk::SurfaceKHR)>,
    ) -> Result<(AdapterInfo, u32), GpuError> {
        let instance = &self.instance;
        // SAFETY: pd получен из этого экземпляра.
        let props = unsafe { instance.get_physical_device_properties(pd) };
        let api = props.api_version;

        let mut probe = AdapterProbe {
            api_version: api,
            dynamic_rendering: false,
            synchronization2: false,
            descriptor_indexing: false,
            buffer_device_address: false,
            multi_draw_indirect: false,
            has_graphics_queue: false,
            can_present: surface.map(|_| false),
            has_swapchain_ext: false,
        };
        let mut driver = String::new();

        if api >= vk::API_VERSION_1_2 {
            let mut driver_props = vk::PhysicalDeviceDriverProperties::default();
            let mut props2 = vk::PhysicalDeviceProperties2::default().push_next(&mut driver_props);
            // SAFETY: цепочка структур корректна, устройство поддерживает Vulkan 1.2+.
            unsafe { instance.get_physical_device_properties2(pd, &mut props2) };
            let name = c_chars_to_string(&driver_props.driver_name);
            let info = c_chars_to_string(&driver_props.driver_info);
            driver = [name, info]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
        }

        if api >= vk::API_VERSION_1_3 {
            let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
            {
                let mut f2 = vk::PhysicalDeviceFeatures2::default()
                    .push_next(&mut f12)
                    .push_next(&mut f13);
                // SAFETY: цепочка структур корректна, устройство поддерживает Vulkan 1.3.
                unsafe { instance.get_physical_device_features2(pd, &mut f2) };
                probe.multi_draw_indirect = f2.features.multi_draw_indirect == vk::TRUE
                    && f2.features.draw_indirect_first_instance == vk::TRUE;
            }
            probe.dynamic_rendering = f13.dynamic_rendering == vk::TRUE;
            probe.synchronization2 = f13.synchronization2 == vk::TRUE;
            probe.descriptor_indexing = f12.descriptor_indexing == vk::TRUE
                && f12.runtime_descriptor_array == vk::TRUE
                && f12.descriptor_binding_partially_bound == vk::TRUE
                && f12.descriptor_binding_sampled_image_update_after_bind == vk::TRUE
                && f12.descriptor_binding_update_unused_while_pending == vk::TRUE
                && f12.shader_sampled_image_array_non_uniform_indexing == vk::TRUE;
            probe.buffer_device_address = f12.buffer_device_address == vk::TRUE;
        }

        // SAFETY: pd получен из этого экземпляра.
        let families = unsafe { instance.get_physical_device_queue_family_properties(pd) };
        let mut chosen_family = None;
        for (i, family) in families.iter().enumerate() {
            if !family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                continue;
            }
            probe.has_graphics_queue = true;
            let present_ok = match surface {
                None => true,
                Some((loader, s)) => {
                    // SAFETY: поверхность и устройство из одного экземпляра.
                    unsafe { loader.get_physical_device_surface_support(pd, i as u32, s) }
                        .unwrap_or(false)
                }
            };
            if present_ok {
                if surface.is_some() {
                    probe.can_present = Some(true);
                }
                chosen_family = Some(i as u32);
                break;
            }
        }

        if surface.is_some() {
            // SAFETY: pd получен из этого экземпляра.
            let exts = unsafe { instance.enumerate_device_extension_properties(pd) }
                .ctx("расширения видеокарты")?;
            probe.has_swapchain_ext = exts
                .iter()
                .any(|e| e.extension_name_as_c_str() == Ok(khr::swapchain::NAME));
        }

        let info = AdapterInfo {
            index,
            name: c_chars_to_string(&props.device_name),
            kind: AdapterKind::from_vk(props.device_type),
            api_version: (
                vk::api_version_major(api),
                vk::api_version_minor(api),
                vk::api_version_patch(api),
            ),
            driver,
            problem: find_problem(&probe),
        };
        Ok((info, chosen_family.unwrap_or(0)))
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        // SAFETY: все объекты, созданные из экземпляра, к этому моменту уничтожены
        // (Core всегда лежит последним полем у владельцев).
        unsafe {
            if let Some((loader, messenger)) = self.debug.take() {
                loader.destroy_debug_utils_messenger(messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

fn debug_messenger_info() -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::INFO,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(debug_callback))
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    kind: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user: *mut c_void,
) -> vk::Bool32 {
    let message = if data.is_null() {
        String::from("(без текста)")
    } else {
        // SAFETY: Vulkan передаёт корректную структуру на время вызова.
        let data = unsafe { &*data };
        if data.p_message.is_null() {
            String::from("(без текста)")
        } else {
            // SAFETY: p_message — строка с нулём на конце, живёт до конца вызова.
            unsafe { CStr::from_ptr(data.p_message) }
                .to_string_lossy()
                .into_owned()
        }
    };
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        let n = VALIDATION_ERRORS.fetch_add(1, Ordering::Relaxed);
        tracing::error!(target: "vulkan", "{kind:?}: {message}");
        // Первые ошибки ещё и в вывод ошибок: в тестах лог не настроен, а без текста
        // упавшую проверку валидации не разобрать. Прямо в stderr, мимо перехвата вывода
        // тестов: ошибка часто случается в тесте, который сам не падает (счётчик общий на
        // все тесты), и тогда её текст иначе пропал бы.
        if n < 8 {
            use std::io::Write as _;
            let _ = writeln!(
                std::io::stderr(),
                "Ошибка валидации Vulkan: {kind:?}: {message}"
            );
        }
    } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
        tracing::warn!(target: "vulkan", "{kind:?}: {message}");
    } else {
        tracing::debug!(target: "vulkan", "{kind:?}: {message}");
    }
    vk::FALSE
}

/// Видеокарта и логическое устройство с одной очередью графики.
pub(crate) struct GpuContext {
    /// Распределитель памяти видеокарты. Уничтожается раньше устройства (см. Drop).
    pub allocator: std::mem::ManuallyDrop<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>,
    pub device: ash::Device,
    pub queue: vk::Queue,
    pub queue_family: u32,
    pub physical: vk::PhysicalDevice,
    pub adapter: AdapterInfo,
    /// Видеокарта умеет сообщать забором, что кадр показан (VK_EXT_swapchain_maintenance1).
    pub present_fences: bool,
    /// Можно писать в картинки из шейдеров через общий список (binding 2 в bindless):
    /// так «реймарчинг» вычислительным шейдером пишет буфер геометрии (7.6м).
    pub storage_images: bool,
    /// Шейдеры могут складывать счётчики лучей в буфер (7.18д2): сложение по волне в
    /// пиксельном и вычислительном шейдере, запись в память из пиксельного, проверка
    /// вспомогательных потоков. Без этого флажок «Счётчики лучей 3D» неактивен.
    pub ray_counters: bool,
    /// RT-ядра доступны шейдерам (опыт «тени через RT-ядра», `rt.rs`): включены
    /// VK_KHR_acceleration_structure и VK_KHR_ray_query. Без них проход «тени 3D» идёт
    /// только по сетке отсева, флажок на панели неактивен.
    pub ray_query: bool,
    /// Свойства деревьев коробок (выравнивание памяти для постройки), если `ray_query`.
    pub accel_scratch_align: u64,
    /// Тензорные ядра доступны шейдерам (опыт Р4, `tensor_bench.rs`): сторона квадратных
    /// матриц half → float, которые видеокарта умножает на волну (16 у NVIDIA, 8 у
    /// lavapipe); 0 — тензорных ядер нет (VK_KHR_cooperative_matrix не включено).
    pub tensor: u32,
    // Последним: уничтожается после устройства.
    pub core: Core,
}

impl GpuContext {
    /// Выбирает видеокарту по `options.gpu` и создаёт на ней устройство.
    ///
    /// При ошибке возвращает `core` обратно, чтобы вызывающий успел уничтожить
    /// свои объекты экземпляра (например, поверхность окна).
    pub fn new(
        core: Core,
        options: &GpuOptions,
        surface: Option<(&khr::surface::Instance, vk::SurfaceKHR)>,
    ) -> Result<Self, Box<(GpuError, Core)>> {
        match Self::create_device(&core, options, surface) {
            Ok((
                device,
                queue,
                queue_family,
                physical,
                adapter,
                present_fences,
                storage_images,
                ray_counters,
                ray_query,
                accel_scratch_align,
                tensor,
            )) => {
                let allocator = gpu_allocator::vulkan::Allocator::new(
                    &gpu_allocator::vulkan::AllocatorCreateDesc {
                        instance: core.instance.clone(),
                        device: device.clone(),
                        physical_device: physical,
                        debug_settings: Default::default(),
                        buffer_device_address: true,
                        allocation_sizes: Default::default(),
                    },
                );
                let allocator = match allocator {
                    Ok(a) => a,
                    Err(e) => {
                        // SAFETY: устройство только что создано и ещё ничем не используется.
                        unsafe { device.destroy_device(None) };
                        return Err(Box::new((GpuError::Memory(e.to_string()), core)));
                    }
                };
                Ok(Self {
                    allocator: std::mem::ManuallyDrop::new(std::sync::Mutex::new(allocator)),
                    device,
                    queue,
                    queue_family,
                    physical,
                    adapter,
                    present_fences,
                    storage_images,
                    ray_counters,
                    ray_query,
                    accel_scratch_align,
                    tensor,
                    core,
                })
            }
            Err(e) => Err(Box::new((e, core))),
        }
    }

    #[allow(clippy::type_complexity)]
    fn create_device(
        core: &Core,
        options: &GpuOptions,
        surface: Option<(&khr::surface::Instance, vk::SurfaceKHR)>,
    ) -> Result<
        (
            ash::Device,
            vk::Queue,
            u32,
            vk::PhysicalDevice,
            AdapterInfo,
            bool,
            bool,
            bool,
            bool,
            u64,
            u32,
        ),
        GpuError,
    > {
        let adapters = core.adapters(surface)?;
        let infos: Vec<AdapterInfo> = adapters.iter().map(|(_, a, _)| a.clone()).collect();
        for a in &infos {
            tracing::info!("Видеокарта {a}");
        }
        let chosen = crate::adapter::pick_adapter(&infos, options.gpu.as_deref())
            .map_err(GpuError::NoSuitableAdapter)?;
        let (physical, adapter, queue_family) = adapters[chosen].clone();
        tracing::info!("Выбрана видеокарта: {} ({})", adapter.name, adapter.kind);

        let priorities = [1.0];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)];
        let storage_images = Self::storage_images(&core.instance, physical);
        let ray_counters = Self::ray_counters(&core.instance, physical);
        let (ray_query, accel_scratch_align) = Self::ray_query(&core.instance, physical)?;
        let tensor = Self::tensor(core, physical)?;
        let coop = tensor != 0;
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default()
            .descriptor_binding_storage_image_update_after_bind(storage_images)
            .descriptor_indexing(true)
            .runtime_descriptor_array(true)
            .descriptor_binding_partially_bound(true)
            .descriptor_binding_sampled_image_update_after_bind(true)
            .descriptor_binding_update_unused_while_pending(true)
            .shader_sampled_image_array_non_uniform_indexing(true)
            .buffer_device_address(true)
            .shader_float16(coop)
            .vulkan_memory_model(coop)
            .vulkan_memory_model_device_scope(coop);
        let mut f13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true)
            .shader_demote_to_helper_invocation(ray_counters);
        let mut extensions: Vec<*const c_char> = Vec::new();
        let mut present_fences = false;
        if surface.is_some() {
            extensions.push(khr::swapchain::NAME.as_ptr());
            if core.surface_maintenance1 {
                // SAFETY: physical получен из этого экземпляра.
                let available = unsafe {
                    core.instance
                        .enumerate_device_extension_properties(physical)
                }
                .ctx("расширения видеокарты")?;
                let has_ext = available
                    .iter()
                    .any(|e| e.extension_name_as_c_str() == Ok(ext::swapchain_maintenance1::NAME));
                let mut sm1 = vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default();
                let mut f2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut sm1);
                // SAFETY: структуры живут до конца вызова.
                unsafe {
                    core.instance
                        .get_physical_device_features2(physical, &mut f2)
                };
                present_fences = has_ext && sm1.swapchain_maintenance1 == vk::TRUE;
                // Для проверки запасного пути (видеокарты без заборов показа) в CI.
                if std::env::var_os("REYVEC_NO_PRESENT_FENCES").is_some() {
                    present_fences = false;
                }
            }
            if present_fences {
                extensions.push(ext::swapchain_maintenance1::NAME.as_ptr());
            }
        }
        if ray_query {
            extensions.push(khr::deferred_host_operations::NAME.as_ptr());
            extensions.push(khr::acceleration_structure::NAME.as_ptr());
            extensions.push(khr::ray_query::NAME.as_ptr());
        }
        let mut accel_features = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default()
            .acceleration_structure(true);
        let mut ray_query_features =
            vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(true);
        if coop {
            extensions.push(khr::cooperative_matrix::NAME.as_ptr());
        }
        let mut tensor_features =
            vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default().cooperative_matrix(true);
        let mut sm1 = vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default()
            .swapchain_maintenance1(true);
        // Векторный слой рисует много фигур одной командой (vkCmdDrawIndirect).
        let base_features = vk::PhysicalDeviceFeatures::default()
            .multi_draw_indirect(true)
            .draw_indirect_first_instance(true)
            .shader_storage_image_array_dynamic_indexing(storage_images)
            .fragment_stores_and_atomics(ray_counters);
        let mut create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_features(&base_features)
            .enabled_extension_names(&extensions)
            .push_next(&mut f12)
            .push_next(&mut f13);
        if present_fences {
            create_info = create_info.push_next(&mut sm1);
        }
        if ray_query {
            create_info = create_info
                .push_next(&mut accel_features)
                .push_next(&mut ray_query_features);
        }
        if coop {
            create_info = create_info.push_next(&mut tensor_features);
        }

        // SAFETY: видеокарта проверена в find_problem, все возможности поддерживаются.
        let device = unsafe { core.instance.create_device(physical, &create_info, None) }
            .ctx("создание устройства")?;
        // SAFETY: очередь 0 этого семейства запрошена выше.
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        Ok((
            device,
            queue,
            queue_family,
            physical,
            adapter,
            present_fences,
            storage_images,
            ray_counters,
            ray_query,
            accel_scratch_align,
            tensor,
        ))
    }

    /// Умеет ли видеокарта умножать матрицы на тензорных ядрах из шейдера (опыт Р4):
    /// VK_KHR_cooperative_matrix с матрицами 16×16×16 half → float на волну, числа half и
    /// модель памяти Vulkan. Переменная `REYVEC_NO_TENSOR` выключает.
    fn tensor(core: &Core, physical: vk::PhysicalDevice) -> Result<u32, GpuError> {
        if std::env::var_os("REYVEC_NO_TENSOR").is_some() {
            return Ok(0);
        }
        let instance = &core.instance;
        // SAFETY: physical получен из этого экземпляра.
        let available = unsafe { instance.enumerate_device_extension_properties(physical) }
            .ctx("расширения видеокарты")?;
        if !available
            .iter()
            .any(|e| e.extension_name_as_c_str() == Ok(khr::cooperative_matrix::NAME))
        {
            tracing::info!("тензорные ядра недоступны: нет VK_KHR_cooperative_matrix");
            return Ok(0);
        }
        let mut coop = vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default();
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut f2 = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut coop)
            .push_next(&mut f12);
        let loader = khr::cooperative_matrix::Instance::new(&core.entry, instance);
        // SAFETY: структуры живут до конца вызовов, расширение есть у видеокарты.
        let shapes = unsafe {
            instance.get_physical_device_features2(physical, &mut f2);
            loader.get_physical_device_cooperative_matrix_properties(physical)
        }
        .unwrap_or_default();
        let half_to_float = |side: u32| {
            shapes.iter().any(|p| {
                p.m_size == side
                    && p.n_size == side
                    && p.k_size == side
                    && p.a_type == vk::ComponentTypeKHR::FLOAT16
                    && p.b_type == vk::ComponentTypeKHR::FLOAT16
                    && p.c_type == vk::ComponentTypeKHR::FLOAT32
                    && p.result_type == vk::ComponentTypeKHR::FLOAT32
                    && p.scope == vk::ScopeKHR::SUBGROUP
            })
        };
        let side = [16, 8].into_iter().find(|&s| half_to_float(s)).unwrap_or(0);
        let ok = coop.cooperative_matrix == vk::TRUE
            && f12.shader_float16 == vk::TRUE
            && f12.vulkan_memory_model == vk::TRUE
            && f12.vulkan_memory_model_device_scope == vk::TRUE
            && side != 0;
        if !ok {
            tracing::info!(
                "тензорные ядра недоступны: нет умножения матриц 16×16 или 8×8 half → float на волну"
            );
        }
        Ok(if ok { side } else { 0 })
    }

    /// Умеет ли видеокарта искать коробки лучом из шейдера (RT-ядра): расширения
    /// VK_KHR_acceleration_structure, VK_KHR_ray_query и их возможности. Второе число —
    /// выравнивание памяти для постройки дерева. Переменная `REYVEC_NO_RAY_QUERY`
    /// выключает (проверка запасного пути).
    fn ray_query(
        instance: &ash::Instance,
        physical: vk::PhysicalDevice,
    ) -> Result<(bool, u64), GpuError> {
        if std::env::var_os("REYVEC_NO_RAY_QUERY").is_some() {
            return Ok((false, 0));
        }
        // SAFETY: physical получен из этого экземпляра.
        let available = unsafe { instance.enumerate_device_extension_properties(physical) }
            .ctx("расширения видеокарты")?;
        let has = |name: &CStr| {
            available
                .iter()
                .any(|e| e.extension_name_as_c_str() == Ok(name))
        };
        if !(has(khr::acceleration_structure::NAME)
            && has(khr::ray_query::NAME)
            && has(khr::deferred_host_operations::NAME))
        {
            tracing::info!("RT-ядра для теней недоступны: нет расширений ray query");
            return Ok((false, 0));
        }
        let mut accel = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
        let mut query = vk::PhysicalDeviceRayQueryFeaturesKHR::default();
        let mut f2 = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut accel)
            .push_next(&mut query);
        let mut accel_props = vk::PhysicalDeviceAccelerationStructurePropertiesKHR::default();
        let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut accel_props);
        // SAFETY: структуры живут до конца вызовов, расширения есть у видеокарты.
        unsafe {
            instance.get_physical_device_features2(physical, &mut f2);
            instance.get_physical_device_properties2(physical, &mut p2);
        }
        let ok = accel.acceleration_structure == vk::TRUE && query.ray_query == vk::TRUE;
        if !ok {
            tracing::info!("RT-ядра для теней недоступны: видеокарта не включает ray query");
        }
        Ok((
            ok,
            u64::from(
                accel_props
                    .min_acceleration_structure_scratch_offset_alignment
                    .max(1),
            ),
        ))
    }

    /// Умеет ли видеокарта то, что нужно шейдерам со счётчиками лучей (7.18д2): писать в
    /// память из пиксельного шейдера, узнавать вспомогательные потоки и складывать числа
    /// по волне в пиксельном и вычислительном шейдере.
    fn ray_counters(instance: &ash::Instance, physical: vk::PhysicalDevice) -> bool {
        let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut f2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut f13);
        let mut p11 = vk::PhysicalDeviceVulkan11Properties::default();
        let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut p11);
        // SAFETY: physical получен из этого экземпляра, структуры живут до конца вызовов.
        unsafe {
            instance.get_physical_device_features2(physical, &mut f2);
            instance.get_physical_device_properties2(physical, &mut p2);
        }
        let stages = vk::ShaderStageFlags::FRAGMENT | vk::ShaderStageFlags::COMPUTE;
        let operations = vk::SubgroupFeatureFlags::BASIC | vk::SubgroupFeatureFlags::ARITHMETIC;
        let ok = f2.features.fragment_stores_and_atomics == vk::TRUE
            && f13.shader_demote_to_helper_invocation == vk::TRUE
            && p11.subgroup_supported_stages.contains(stages)
            && p11.subgroup_supported_operations.contains(operations);
        if !ok {
            tracing::warn!(
                "счётчики лучей недоступны: видеокарта не умеет писать в память из \
                 пиксельного шейдера или складывать числа по волне"
            );
        }
        ok
    }

    /// Умеет ли видеокарта то, что нужно для записи в картинки через общий список:
    /// менять ячейки списка, пока кадр рисуется, выбирать ячейку номером из констант
    /// и держать столько ячеек. Без этого лучи «3D» идут пиксельным шейдером.
    fn storage_images(instance: &ash::Instance, physical: vk::PhysicalDevice) -> bool {
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut f2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut f12);
        let mut p12 = vk::PhysicalDeviceVulkan12Properties::default();
        let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut p12);
        // SAFETY: physical получен из этого экземпляра, структуры живут до конца вызовов.
        unsafe {
            instance.get_physical_device_features2(physical, &mut f2);
            instance.get_physical_device_properties2(physical, &mut p2);
        }
        let dynamic_indexing = f2.features.shader_storage_image_array_dynamic_indexing;
        let need = crate::graph::TRANSIENT_TEXTURES;
        let ok = f12.descriptor_binding_storage_image_update_after_bind == vk::TRUE
            && dynamic_indexing == vk::TRUE
            && p12.max_descriptor_set_update_after_bind_storage_images >= need
            && p12.max_per_stage_descriptor_update_after_bind_storage_images >= need;
        if !ok {
            tracing::warn!(
                "видеокарта не даёт писать в картинки через общий список: \
                 лучи «3D» пойдут пиксельным шейдером"
            );
        }
        ok
    }

    pub fn instance(&self) -> &ash::Instance {
        &self.core.instance
    }

    pub fn find_memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Option<u32> {
        // SAFETY: physical получен из этого экземпляра.
        let mem = unsafe {
            self.instance()
                .get_physical_device_memory_properties(self.physical)
        };
        (0..mem.memory_type_count).find(|&i| {
            bits & (1 << i) != 0 && mem.memory_types[i as usize].property_flags.contains(flags)
        })
    }
}

impl Drop for GpuContext {
    fn drop(&mut self) {
        // SAFETY: владельцы GpuContext уничтожают свои объекты раньше (в своём Drop),
        // здесь остаётся только дождаться видеокарты и закрыть устройство.
        unsafe {
            let _ = self.device.device_wait_idle();
            // Распределитель освобождает свои блоки памяти, поэтому уходит до устройства.
            std::mem::ManuallyDrop::drop(&mut self.allocator);
            self.device.destroy_device(None);
        }
    }
}

/// Список видеокарт с пометкой, подходит ли каждая движку. Окно не нужно.
pub fn list_adapters(validation: bool) -> Result<Vec<AdapterInfo>, GpuError> {
    let core = Core::new(validation, None)?;
    Ok(core
        .adapters(None)?
        .into_iter()
        .map(|(_, info, _)| info)
        .collect())
}
