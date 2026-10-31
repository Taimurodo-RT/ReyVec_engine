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
        if let Some(display) = display {
            let required =
                ash_window::enumerate_required_extensions(display).ctx("расширения для окна")?;
            extensions.extend_from_slice(required);
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
        VALIDATION_ERRORS.fetch_add(1, Ordering::Relaxed);
        tracing::error!(target: "vulkan", "{kind:?}: {message}");
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
            Ok((device, queue, queue_family, physical, adapter)) => {
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
    ) -> Result<(ash::Device, vk::Queue, u32, vk::PhysicalDevice, AdapterInfo), GpuError> {
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
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default()
            .descriptor_indexing(true)
            .runtime_descriptor_array(true)
            .descriptor_binding_partially_bound(true)
            .descriptor_binding_sampled_image_update_after_bind(true)
            .descriptor_binding_update_unused_while_pending(true)
            .shader_sampled_image_array_non_uniform_indexing(true)
            .buffer_device_address(true);
        let mut f13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let extensions: Vec<*const c_char> = if surface.is_some() {
            vec![khr::swapchain::NAME.as_ptr()]
        } else {
            Vec::new()
        };
        let create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&extensions)
            .push_next(&mut f12)
            .push_next(&mut f13);

        // SAFETY: видеокарта проверена в find_problem, все возможности поддерживаются.
        let device = unsafe { core.instance.create_device(physical, &create_info, None) }
            .ctx("создание устройства")?;
        // SAFETY: очередь 0 этого семейства запрошена выше.
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        Ok((device, queue, queue_family, physical, adapter))
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
