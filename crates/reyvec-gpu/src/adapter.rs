//! Сведения о видеокартах и выбор той, на которой работает движок.

use std::fmt;

use ash::vk;

/// Тип видеокарты.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterKind {
    /// Отдельная видеокарта, например RTX 5080.
    Discrete,
    /// Встроенная в процессор, например Radeon в Ryzen.
    Integrated,
    /// Виртуальная (в виртуальной машине).
    Virtual,
    /// Программная, считает на процессоре (например, lavapipe).
    Cpu,
    /// Что-то другое.
    Other,
}

impl AdapterKind {
    pub(crate) fn from_vk(t: vk::PhysicalDeviceType) -> Self {
        match t {
            vk::PhysicalDeviceType::DISCRETE_GPU => Self::Discrete,
            vk::PhysicalDeviceType::INTEGRATED_GPU => Self::Integrated,
            vk::PhysicalDeviceType::VIRTUAL_GPU => Self::Virtual,
            vk::PhysicalDeviceType::CPU => Self::Cpu,
            _ => Self::Other,
        }
    }

    /// Чем меньше, тем охотнее выбираем по умолчанию.
    fn rank(self) -> u8 {
        match self {
            Self::Discrete => 0,
            Self::Integrated => 1,
            Self::Virtual => 2,
            Self::Cpu => 3,
            Self::Other => 4,
        }
    }
}

impl fmt::Display for AdapterKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Discrete => "дискретная",
            Self::Integrated => "встроенная",
            Self::Virtual => "виртуальная",
            Self::Cpu => "программная (CPU)",
            Self::Other => "другая",
        })
    }
}

/// Сведения об одной видеокарте.
#[derive(Clone, Debug)]
pub struct AdapterInfo {
    /// Номер в списке Vulkan. Его можно передать в `--gpu`.
    pub index: usize,
    /// Название, например «NVIDIA GeForce RTX 5080».
    pub name: String,
    /// Тип видеокарты.
    pub kind: AdapterKind,
    /// Версия Vulkan, которую поддерживает драйвер: (мажорная, минорная, патч).
    pub api_version: (u32, u32, u32),
    /// Драйвер и его версия, если драйвер их сообщает.
    pub driver: String,
    /// Почему видеокарта не подходит движку. `None`, если подходит.
    pub problem: Option<String>,
}

impl AdapterInfo {
    /// Подходит ли видеокарта движку.
    pub fn is_suitable(&self) -> bool {
        self.problem.is_none()
    }
}

impl fmt::Display for AdapterInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (major, minor, patch) = self.api_version;
        write!(
            f,
            "[{}] {} ({}, Vulkan {major}.{minor}.{patch}",
            self.index, self.name, self.kind
        )?;
        if !self.driver.is_empty() {
            write!(f, ", драйвер {}", self.driver)?;
        }
        write!(f, ")")?;
        match &self.problem {
            None => write!(f, ": подходит"),
            Some(p) => write!(f, ": не подходит, {p}"),
        }
    }
}

/// Выбирает видеокарту.
///
/// `preference` берётся из флага `--gpu`: номер из списка или часть названия
/// без учёта регистра (например `radeon`). Без него берём лучшую подходящую:
/// дискретная, потом встроенная, потом остальные.
pub fn pick_adapter(adapters: &[AdapterInfo], preference: Option<&str>) -> Result<usize, String> {
    if adapters.is_empty() {
        return Err("Vulkan не нашёл ни одной видеокарты. Обновите драйвер".into());
    }
    let list = || {
        adapters
            .iter()
            .map(|a| format!("  {a}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    if let Some(pref) = preference.map(str::trim).filter(|p| !p.is_empty()) {
        let found = match pref.parse::<usize>() {
            Ok(i) => adapters.iter().position(|a| a.index == i),
            Err(_) => {
                let needle = pref.to_lowercase();
                adapters
                    .iter()
                    .position(|a| a.name.to_lowercase().contains(&needle))
            }
        };
        return match found {
            None => Err(format!(
                "видеокарта «{pref}» не найдена. Есть такие:\n{}",
                list()
            )),
            Some(i) => match &adapters[i].problem {
                None => Ok(i),
                Some(p) => Err(format!(
                    "видеокарта «{}» не подходит: {p}",
                    adapters[i].name
                )),
            },
        };
    }

    adapters
        .iter()
        .enumerate()
        .filter(|(_, a)| a.is_suitable())
        .min_by_key(|(_, a)| (a.kind.rank(), a.index))
        .map(|(i, _)| i)
        .ok_or_else(|| {
            format!(
                "ни одна видеокарта не подходит движку (нужен Vulkan 1.3). Обновите драйвер. Найдены:\n{}",
                list()
            )
        })
}

/// Сведения, которые нужны для проверки видеокарты, уже прочитанные из Vulkan.
pub(crate) struct AdapterProbe {
    pub api_version: u32,
    pub dynamic_rendering: bool,
    pub synchronization2: bool,
    pub descriptor_indexing: bool,
    pub buffer_device_address: bool,
    /// Много команд рисования одной командой (векторный слой).
    pub multi_draw_indirect: bool,
    pub has_graphics_queue: bool,
    pub can_present: Option<bool>,
    pub has_swapchain_ext: bool,
}

/// Объясняет, чего не хватает видеокарте. `None`, если всего хватает.
pub(crate) fn find_problem(p: &AdapterProbe) -> Option<String> {
    let (major, minor) = (
        vk::api_version_major(p.api_version),
        vk::api_version_minor(p.api_version),
    );
    if (major, minor) < (1, 3) {
        return Some(format!(
            "драйвер даёт Vulkan {major}.{minor}, а нужен 1.3. Обновите драйвер"
        ));
    }
    let mut missing = Vec::new();
    if !p.dynamic_rendering {
        missing.push("dynamicRendering");
    }
    if !p.synchronization2 {
        missing.push("synchronization2");
    }
    if !p.descriptor_indexing {
        missing.push("descriptorIndexing");
    }
    if !p.buffer_device_address {
        missing.push("bufferDeviceAddress");
    }
    if !p.multi_draw_indirect {
        missing.push("multiDrawIndirect");
    }
    if !p.has_graphics_queue {
        missing.push("очередь графики");
    }
    if p.can_present == Some(false) {
        missing.push("вывод в окно");
    }
    if p.can_present.is_some() && !p.has_swapchain_ext {
        missing.push("VK_KHR_swapchain");
    }
    if missing.is_empty() {
        None
    } else {
        Some(format!("нет нужных возможностей: {}", missing.join(", ")))
    }
}

/// Строки Vulkan хранятся в массивах фиксированной длины и кончаются нулём.
pub(crate) fn c_chars_to_string(chars: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(index: usize, name: &str, kind: AdapterKind, ok: bool) -> AdapterInfo {
        AdapterInfo {
            index,
            name: name.into(),
            kind,
            api_version: (1, 3, 0),
            driver: String::new(),
            problem: (!ok).then(|| "нет Vulkan 1.3".to_string()),
        }
    }

    fn roman_pc() -> Vec<AdapterInfo> {
        vec![
            adapter(0, "AMD Radeon(TM) Graphics", AdapterKind::Integrated, true),
            adapter(1, "NVIDIA GeForce RTX 5080", AdapterKind::Discrete, true),
        ]
    }

    #[test]
    fn prefers_discrete_by_default() {
        assert_eq!(pick_adapter(&roman_pc(), None), Ok(1));
    }

    #[test]
    fn picks_by_name_ignoring_case() {
        assert_eq!(pick_adapter(&roman_pc(), Some("radeon")), Ok(0));
        assert_eq!(pick_adapter(&roman_pc(), Some("RTX")), Ok(1));
    }

    #[test]
    fn picks_by_index() {
        assert_eq!(pick_adapter(&roman_pc(), Some("0")), Ok(0));
    }

    #[test]
    fn unknown_name_lists_adapters() {
        let err = pick_adapter(&roman_pc(), Some("intel")).unwrap_err();
        assert!(err.contains("RTX 5080"), "{err}");
    }

    #[test]
    fn skips_unsuitable_discrete() {
        let mut list = roman_pc();
        list[1].problem = Some("старый драйвер".into());
        assert_eq!(pick_adapter(&list, None), Ok(0));
        assert!(pick_adapter(&list, Some("rtx")).is_err());
    }

    #[test]
    fn falls_back_to_cpu_renderer() {
        let list = vec![adapter(0, "llvmpipe", AdapterKind::Cpu, true)];
        assert_eq!(pick_adapter(&list, None), Ok(0));
    }

    #[test]
    fn nothing_suitable_is_an_error() {
        let list = vec![adapter(0, "old", AdapterKind::Discrete, false)];
        assert!(pick_adapter(&list, None).is_err());
        assert!(pick_adapter(&[], None).is_err());
    }

    fn good_probe() -> AdapterProbe {
        AdapterProbe {
            api_version: vk::make_api_version(0, 1, 4, 0),
            dynamic_rendering: true,
            synchronization2: true,
            descriptor_indexing: true,
            buffer_device_address: true,
            multi_draw_indirect: true,
            has_graphics_queue: true,
            can_present: Some(true),
            has_swapchain_ext: true,
        }
    }

    #[test]
    fn problem_detection() {
        assert_eq!(find_problem(&good_probe()), None);

        let old = AdapterProbe {
            api_version: vk::make_api_version(0, 1, 2, 0),
            ..good_probe()
        };
        assert!(find_problem(&old).unwrap().contains("1.2"));

        let no_present = AdapterProbe {
            can_present: Some(false),
            ..good_probe()
        };
        assert!(find_problem(&no_present).unwrap().contains("вывод в окно"));

        let headless = AdapterProbe {
            can_present: None,
            has_swapchain_ext: false,
            ..good_probe()
        };
        assert_eq!(find_problem(&headless), None);
    }

    #[test]
    fn display_is_readable() {
        let text = roman_pc()[1].to_string();
        assert!(text.contains("[1] NVIDIA GeForce RTX 5080"), "{text}");
        assert!(text.contains("дискретная"), "{text}");
    }

    #[test]
    fn vk_strings() {
        let raw: Vec<std::ffi::c_char> = b"RTX\0garbage"
            .iter()
            .map(|&b| b as std::ffi::c_char)
            .collect();
        assert_eq!(c_chars_to_string(&raw), "RTX");
    }
}
