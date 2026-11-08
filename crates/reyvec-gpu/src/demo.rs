//! Сцены песочницы. Их восемь, переключаются клавишами 1–8 ([`DemoScene`]):
//!
//! - «GPU-фундамент» (этап 2): треугольник этапа 1 плюс спрайты с текстурами;
//! - «Векторы» (этап 3): векторные фигуры, см. `demo_vector.rs`;
//! - «Перетекание» (этап 3): фигуры, которые плавно меняют форму, там же;
//! - «Слои» (этап 3): прозрачность группы фигур и режимы смешивания, там же;
//! - «Маски» (этап 3): обрезка по силуэту, маска по яркости и обратная, там же;
//! - «SVG» (этап 3): тестовые рисунки SVG или свой файл (`--svg`), там же;
//! - «Motif» (этап 3): перетекающие фигуры из файла Motif (`--motif`), там же;
//! - «поток» (этап 3): новые фигуры появляются во время работы, там же.
//!
//! Про сцену этапа 2.
//!
//! Показывает, что работают память видеокарты, загрузка текстур, общий список
//! текстур (bindless), чтение данных спрайтов из буфера по адресу и граф кадра:
//! сцена рисуется во временную картинку (проход «сцена»), а потом копируется в окно
//! (проход «вывод»). Позже спрайты станут частью векторного слоя.

use ash::vk;

use crate::GpuError;
use crate::adapter::AdapterKind;
use crate::bindless::{Bindless, SamplerKind};
use crate::blobs::{BLOB_SIZE, MAX_BLOBS, MAX_OBJECTS, OBJECT_SIZE};
use crate::context::GpuContext;
use crate::demo_vector::VectorDemo;
use crate::graph::{Access, ImageHandle, PassContext, RenderGraph};
use crate::lights::{LIGHT_SIZE, MAX_LIGHTS};
use crate::pipeline::Blend;
use crate::pipeline::{
    CLEAR_COLOR, Stencil, StencilUse, begin_color_pass, begin_color_pass_with, begin_depth_pass,
    begin_scene_pass, graphics_pipeline_stencil,
};
use crate::resources::{BufferId, BufferMemory, GpuResources, TextureId};
use crate::shaders::ShaderLibrary;
use crate::vector::{FillMode, LayerTexture, Uploads, VectorData, VectorRenderer};

/// Время процессора на векторный кадр (сглаженное по кадрам), раздельно: сколько
/// заняла сама сцена песочницы (расчёт движения фигур — то, что в игре делает её
/// логика) и сколько движок (раскладка фигур, отсечение, пачки, запись для видеокарты).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VectorCpu {
    /// Сцена песочницы: построить список фигур кадра, мс.
    pub scene_ms: f64,
    /// Движок: подготовить векторный кадр из списка фигур, мс.
    pub engine_ms: f64,
    /// Фигур в кадре.
    pub shapes: usize,
    /// Из них нарисовано после отсечения.
    pub drawn: usize,
}

/// Счётчики сцены «3D» и 2.5D-вида для панели (7.18б): до поля слоя ни одна цифра,
/// определяющая цену кадра, не измерялась (перепроверка разбора 2026-11-07).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpaceStats {
    /// Процессор: собрать формы кадра (уровень из снимка мира и лампы или предметы файла
    /// сцены «3D»), мс, сглажено по кадрам.
    pub build_ms: f64,
    /// Процессор: отсев по взгляду, программы форм, сетка отсева, запись в буфер (и
    /// подготовка поля 7.10 в сцене «3D»), мс, сглажено по кадрам.
    pub pack_ms: f64,
    /// Объектов в списке сцены (предметы, заменители моделей, светящиеся шарики ламп).
    pub objects: u32,
    /// Слов сцены в буфере кадра (по 4 байта).
    pub words: usize,
    /// Объектов сверх предела шейдера ([`MAX_SCENE_OBJECTS`]): они не рисуются.
    pub dropped: usize,
    /// Сцена не поместилась в буфер ([`MAX_SCENE_WORDS`] слов): сколько было нужно;
    /// рисуется пустая.
    pub overflow: Option<usize>,
    /// Пикселей края в последнем кадре этой ячейки со сглаживанием (счётчик прохода
    /// «края 3D: поиск», прочитан с видеокарты; в первом кадре ячейки — 0); `None` —
    /// сглаживание краёв выключено или список не создан.
    pub edge_pixels: Option<u32>,
    /// Пикселей в кадре 3D (с учётом «Разрешения 3D»): для доли краёв.
    pub pixels: u32,
}

/// Больше объектов шейдер сцены «3D» не читает (`MAX_OBJECTS` в `include/space.slang`).
pub const MAX_SCENE_OBJECTS: usize = reyvec_sdf::shape3d::MAX_OBJECTS;
/// Больше слов в буфер сцены кадра не помещается.
pub const MAX_SCENE_WORDS: usize = crate::space::MAX_WORDS;
/// Больше пикселей края в список не входит (`MAX_EDGES` в `raymarch_edge_find.slang`);
/// остальные рисуются одним лучом.
pub const MAX_EDGE_PIXELS: u32 = 65535 * 16;
/// Полоса `B` поля слоя у выдавливания через поле (опыт 7.18б), м: как у поля уровня в
/// `docs/decisions/0020-level-field.md`. На собственной сетке профиля значения до неё не
/// доходят; в заголовке примитива она нужна порогу «только оценка».
const EXTRUDE_FIELD_BAND: f32 = 1.0;

/// Сколько времени кадра можно тратить на постройку недостающих уровней детализации, мс.
const DETAIL_BUDGET_MS: f64 = 1.0;

/// Какую сцену песочницы рисовать.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DemoScene {
    /// Этап 2: треугольник и спрайты с текстурами.
    Gpu,
    /// Этап 3: векторные фигуры.
    #[default]
    Vector,
    /// Этап 3: перетекающие фигуры (большая в центре и сетка маленьких).
    Morph,
    /// Этап 3: слои с общей прозрачностью и режимами смешивания.
    Layers,
    /// Этап 3: маски и обрезка.
    Masks,
    /// Этап 3: рисунки SVG (тестовые или свой файл).
    Svg,
    /// Этап 3: перетекающие фигуры из файла Motif (пример или свой файл).
    Motif,
    /// Этап 3: новые фигуры появляются во время работы и загружаются внутри кадра.
    Stream,
    /// Этап 3: текст (кириллица и латиница, переносы, выравнивание, слои и маски).
    Text,
    /// Этап 3: 10 000 движущихся фигур (замер нагрузки на процессор и видеокарту).
    Stress,
    /// Этап 3: 10 000 перетекающих фигур, все меняют форму каждый кадр.
    StressMorph,
    /// Этап 4: свет на полях расстояний (SDF). Пока фигуры библиотеки SDF и их поле.
    Light,
    /// Этап 4: те же фигуры, освещённые лампами, с мягкими тенями по полю (4.3).
    Lamps,
    /// Этап 4: объекты целиком из SDF (слизь, капли, магический шар) рядом с векторными,
    /// со светом и тенями (4.5).
    Blobs,
    /// Этап 4: демо-сцена «лаборатория мага» (4.6): всё вместе — векторные предметы с
    /// тенями, светящиеся зелья, неон, свеча, котёл и магический шар.
    Lab,
    /// Этап 5: сцена из файла (`scenes/first-room.ron` или свой файл): правка файла
    /// сразу видна в окне (5.1).
    File,
    /// Этап 7: объёмный мир реймарчингом (7.1 — камера 3D, пол с сеткой, простые формы).
    Space,
}

/// Свои файлы для сцен песочницы (содержимое файлов). Без них сцены показывают примеры.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserFiles {
    /// Рисунок SVG для сцены «SVG».
    pub svg: Option<std::sync::Arc<[u8]>>,
    /// Перетекающие фигуры от Motif (`*.reyvec.json`) для сцены «Motif».
    pub motif: Option<std::sync::Arc<[u8]>>,
    /// Текст файла сцены (RON) для сцены «из файла». Без него — пример «первая комната».
    pub scene: Option<std::sync::Arc<str>>,
    /// Папка файла сцены: от неё считаются пути к рисункам SVG и файлам Motif в сцене.
    /// Для встроенного примера не нужна (берётся `reyvec_scene::EXAMPLE_DIR`).
    pub scene_dir: Option<std::path::PathBuf>,
    /// Текст файла объёмной сцены «3D» (RON, этап 7.5). Без него — пример `scenes/3d/space.ron`.
    pub space: Option<std::sync::Arc<str>>,
}

impl UserFiles {
    /// Текст сцены «из файла» и папка, от которой считать пути в нём.
    pub fn scene_text(&self) -> (&str, Option<&std::path::Path>) {
        match &self.scene {
            Some(text) => (text, self.scene_dir.as_deref()),
            None => (
                reyvec_scene::EXAMPLE,
                Some(std::path::Path::new(reyvec_scene::EXAMPLE_DIR)),
            ),
        }
    }
}

impl DemoScene {
    /// Название для панели и лога.
    pub fn title(self) -> &'static str {
        match self {
            Self::Gpu => "GPU-фундамент",
            Self::Vector => "векторы",
            Self::Morph => "перетекание",
            Self::Layers => "слои",
            Self::Masks => "маски",
            Self::Svg => "SVG",
            Self::Motif => "Motif",
            Self::Stream => "поток",
            Self::Text => "текст",
            Self::Stress => "стресс: движение",
            Self::StressMorph => "стресс: перетекание",
            Self::Light => "свет (SDF)",
            Self::Lamps => "лампы и тени",
            Self::Blobs => "слизь и магия",
            Self::Lab => "лаборатория мага",
            Self::File => "из файла",
            Self::Space => "3D",
        }
    }

    /// Сцена по имени из командной строки: `gpu`, `vector`, `morph`, `layers`, `masks`,
    /// `svg`, `motif`, `stream`, `text`, `stress`, `stress-morph`, `light`, `lamps`,
    /// `blobs`, `lab`, `file` или `3d`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "gpu" => Some(Self::Gpu),
            "vector" => Some(Self::Vector),
            "morph" => Some(Self::Morph),
            "layers" => Some(Self::Layers),
            "masks" => Some(Self::Masks),
            "svg" => Some(Self::Svg),
            "motif" => Some(Self::Motif),
            "stream" => Some(Self::Stream),
            "text" => Some(Self::Text),
            "stress" => Some(Self::Stress),
            "stress-morph" => Some(Self::StressMorph),
            "light" => Some(Self::Light),
            "lamps" => Some(Self::Lamps),
            "blobs" => Some(Self::Blobs),
            "lab" => Some(Self::Lab),
            "file" => Some(Self::File),
            "3d" | "space" => Some(Self::Space),
            _ => None,
        }
    }

    /// Сцены этапа 4 с SDF-фигурами и полем расстояний.
    pub fn uses_field(self) -> bool {
        matches!(
            self,
            Self::Light | Self::Lamps | Self::Blobs | Self::Lab | Self::File
        )
    }

    /// Сцены с SDF-фигурами библиотеки на весь экран (`sdf_demo.slang`); у остальных
    /// сцен этапа 4 шейдер рисует только фон.
    fn has_demo_shapes(self) -> bool {
        matches!(self, Self::Light | Self::Lamps)
    }

    /// Сцены этапа 4, освещённые лампами (свет, тени, свечение).
    pub fn uses_lights(self) -> bool {
        matches!(self, Self::Lamps | Self::Blobs | Self::Lab | Self::File)
    }

    /// Другой вариант стресс-теста (движение ↔ перетекание). У остальных сцен `None`.
    pub fn stress_variant(self) -> Option<Self> {
        match self {
            Self::Stress => Some(Self::StressMorph),
            Self::StressMorph => Some(Self::Stress),
            _ => None,
        }
    }
}

/// Сглаживание краёв сцены через MSAA: видеокарта берёт несколько точек на пиксель, но
/// пиксельный шейдер считает один раз на пиксель, поэтому дорого только на краях фигур.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Msaa {
    /// Без сглаживания: края «лесенкой».
    Off,
    /// 2 точки на пиксель: по умолчанию на встроенных видеокартах.
    X2,
    /// 4 точки на пиксель.
    #[default]
    X4,
    /// 8 точек на пиксель.
    X8,
}

impl Msaa {
    /// Название для панели и лога.
    pub fn title(self) -> &'static str {
        match self {
            Self::Off => "выкл",
            Self::X2 => "MSAA 2×",
            Self::X4 => "MSAA 4×",
            Self::X8 => "MSAA 8×",
        }
    }

    /// Следующий вариант по кругу: выкл → 2× → 4× → 8× → выкл.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::X2,
            Self::X2 => Self::X4,
            Self::X4 => Self::X8,
            Self::X8 => Self::Off,
        }
    }

    /// Сглаживание по умолчанию для видеокарты этого типа (задача 4.0). Встроенная делит
    /// память с процессором, и MSAA 4× съедает на ней около 1,6 мс кадра (узкие места 9 и
    /// 40): берём 2×, а края обводок и текста и так сглаживает шейдер. Остальным 4×.
    pub fn for_adapter(kind: AdapterKind) -> Self {
        match kind {
            AdapterKind::Integrated => Self::X2,
            _ => Self::X4,
        }
    }

    /// Вариант по имени из командной строки: `off`, `2`, `4` или `8`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "off" | "0" | "1" => Some(Self::Off),
            "2" => Some(Self::X2),
            "4" => Some(Self::X4),
            "8" => Some(Self::X8),
            _ => None,
        }
    }

    pub(crate) fn samples(self) -> vk::SampleCountFlags {
        match self {
            Self::Off => vk::SampleCountFlags::TYPE_1,
            Self::X2 => vk::SampleCountFlags::TYPE_2,
            Self::X4 => vk::SampleCountFlags::TYPE_4,
            Self::X8 => vk::SampleCountFlags::TYPE_8,
        }
    }

    /// Самый близкий вариант не больше `self`, который видеокарта умеет для кадра сцены.
    pub(crate) fn supported(self, counts: vk::SampleCountFlags) -> Self {
        [Self::X8, Self::X4, Self::X2, Self::Off]
            .into_iter()
            .find(|m| *m <= self && counts.contains(m.samples()))
            .unwrap_or(Self::Off)
    }
}

/// Разрешение сцены «3D» (7.14): в какой доле кадра по каждой стороне шагают лучи,
/// считаются тени, края и освещение. Меньше — вчетверо меньше лучей на половине, а
/// картинка растягивается на весь кадр одним проходом без накопления по кадрам (без
/// сглаживания по времени). Встроенной видеокарте иначе не хватает на 60 кадров.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpaceScale {
    /// Каждый пиксель кадра — свой луч.
    #[default]
    Full,
    /// Три четверти по каждой стороне: около 56 % лучей.
    ThreeQuarters,
    /// Половина по каждой стороне: четверть лучей. По умолчанию на встроенных.
    Half,
}

impl SpaceScale {
    /// Все варианты для панели.
    pub const ALL: [Self; 3] = [Self::Full, Self::ThreeQuarters, Self::Half];

    /// Название для панели и лога.
    pub fn title(self) -> &'static str {
        match self {
            Self::Full => "100 %",
            Self::ThreeQuarters => "75 %",
            Self::Half => "50 %",
        }
    }

    /// Разрешение по умолчанию для видеокарты этого типа. Встроенная Radeon на полном
    /// разрешении рисует сцену «3D» за 43 мс (23 кадра, замер 2026-11-07), а лучей и
    /// теней на половине вчетверо меньше.
    pub fn for_adapter(kind: AdapterKind) -> Self {
        match kind {
            AdapterKind::Integrated => Self::Half,
            _ => Self::Full,
        }
    }

    /// Размер картинок сцены «3D» для кадра размера `extent` (не меньше 1×1).
    pub fn apply(self, extent: vk::Extent2D) -> vk::Extent2D {
        let (num, den) = match self {
            Self::Full => return extent,
            Self::ThreeQuarters => (3, 4),
            Self::Half => (1, 2),
        };
        vk::Extent2D {
            width: (extent.width * num).div_ceil(den).max(1),
            height: (extent.height * num).div_ceil(den).max(1),
        }
    }
}

/// Данные одного спрайта. Раскладка совпадает со структурой `Sprite` в `sprite.vert` (std430).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sprite {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub tint: [f32; 4],
    pub rotation: f32,
    pub texture: u32,
    pub sampler: u32,
    pub pad: u32,
}

const SPRITE_SIZE: usize = std::mem::size_of::<Sprite>();
/// Формат картинок слоёв векторов: линейный цвет с прозрачностью и запасом точности.
pub(crate) const LAYER_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
/// Формат кадра сцены, если видеокарта его умеет: линейный цвет с запасом яркости (HDR),
/// но без прозрачности и вдвое компактнее [`LAYER_FORMAT`] (4 байта на точку вместо 8).
/// Кадр сцены очищается, рисуется и сводится из MSAA каждый кадр во весь экран, и на
/// встроенной видеокарте с общей памятью это главная постоянная цена кадра (узкое место
/// 40 в `docs/vector-bottlenecks.md`). Прозрачность кадру сцены не нужна: слои с
/// прозрачностью рисуются в свои картинки.
const COMPACT_SCENE_FORMAT: vk::Format = vk::Format::B10G11R11_UFLOAT_PACK32;
const MAX_SPRITES: usize = 64;
/// Размеры клеток шахматки по очереди, в пикселях текстуры.
const CHECKER_CELLS: [u32; 3] = [16, 32, 8];
/// Маленькие круги вокруг треугольника.
const ORBITERS: usize = 12;

impl Sprite {
    fn to_bytes(self) -> [u8; SPRITE_SIZE] {
        let mut out = [0u8; SPRITE_SIZE];
        let words: [u32; 12] = [
            self.pos[0].to_bits(),
            self.pos[1].to_bits(),
            self.size[0].to_bits(),
            self.size[1].to_bits(),
            self.tint[0].to_bits(),
            self.tint[1].to_bits(),
            self.tint[2].to_bits(),
            self.tint[3].to_bits(),
            self.rotation.to_bits(),
            self.texture,
            self.sampler,
            self.pad,
        ];
        for (i, w) in words.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_ne_bytes());
        }
        out
    }
}

/// Всё, что проходам кадра нужно знать о сцене. Готовится до сборки графа.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameData {
    scene: DemoScene,
    msaa: Msaa,
    perceptual_edges: bool,
    vector: Option<VectorData>,
    sprites_address: vk::DeviceAddress,
    sprite_count: u32,
    /// Сцена «3D» (7.2): объекты, материалы и программы форм в буфере кадра.
    space_address: vk::DeviceAddress,
    /// Вершин полигональных моделей сцены «3D» (7.6ж; 0 — моделей нет, проход
    /// «полигоны 3D» не нужен).
    space_vertices: u32,
    /// Карт теней ламп от треугольников (7.9б; 0 — тени ламп только по формам SDF).
    space_shadow_maps: u32,
    /// Опыт 7.6м: лучи вычислительным шейдером, он пишет буфер геометрии прямо в
    /// картинку. `false` — пиксельным шейдером.
    space_compute: bool,
    /// Сглаживание краёв форм (7.6о): список пикселей краёв (адрес и сам буфер) и буфер
    /// для чтения его счётчика процессором (7.18б). `None` — выключено.
    space_edges: Option<(vk::DeviceAddress, vk::Buffer, vk::Buffer)>,
    /// Запечённое поле расстояний (7.10б): что пересчитать в проходе «поле». `None` —
    /// поле выключено.
    space_field: Option<crate::field::FieldFrame>,
    /// Источники света сцены «лампы и тени» в буфере кадра (адрес и сколько их).
    lights_address: vk::DeviceAddress,
    light_count: u32,
    /// Объекты из SDF (4.5): записи объектов и их частей.
    objects_address: vk::DeviceAddress,
    blobs_address: vk::DeviceAddress,
    object_count: u32,
    /// Общий тусклый свет (линейный цвет) и фон «лаборатории» (каменная стена и пол).
    ambient: [f32; 3],
    lab_backdrop: bool,
    time: f32,
    aspect: f32,
    extent: vk::Extent2D,
    /// Размер картинок сцены «3D» (7.14): `extent` или меньше (см. [`SpaceScale`]).
    space_extent: vk::Extent2D,
    /// Камера объёма этого кадра: сцены «3D» или 2.5D-вида уровня (7.9).
    space_camera: reyvec_sdf::Camera3D,
}

pub(crate) struct Demo {
    triangle: vk::Pipeline,
    /// Сцена «свет»: SDF-фигуры одним треугольником на весь экран (`sdf_demo.slang`).
    sdf: vk::Pipeline,
    /// Сцена «3D» (7.3): шагание лучей в буфер геометрии (`raymarch.slang`)…
    space_march: vk::Pipeline,
    /// Полигональные модели сцены «3D» (7.6ж, `mesh3d.slang`) в свой буфер геометрии.
    space_mesh: vk::Pipeline,
    /// Карты теней ламп от треугольников (7.9б, `mesh_shadow.slang`).
    space_mesh_shadow: vk::Pipeline,
    space_cone: vk::Pipeline,
    /// …тени и затенение в углах по нему (7.4, `raymarch_shadow.slang`)…
    space_shadow: vk::Pipeline,
    /// Опыт 7.6м: те же лучи вычислительным шейдером (`cs_main` в `raymarch.slang`),
    /// если в файле сцены нет `compute: false` и видеокарта умеет писать в картинки через
    /// общий список (иначе конвейера нет, null).
    space_march_compute: vk::Pipeline,
    /// Шли ли лучи последнего кадра «3D» вычислительным шейдером.
    space_compute_running: bool,
    /// Сглаживание краёв форм сцены «3D» (7.6о): поиск краёв и лучи внутри их пикселей
    /// (`raymarch_edge_find.slang`, `raymarch_edges.slang`). Null — видеокарта не умеет
    /// писать в картинки через общий список.
    space_edge_find: vk::Pipeline,
    space_edges: vk::Pipeline,
    /// Список пикселей краёв, свой у каждого кадра в полёте (буфер и его размер).
    space_edge_lists: Vec<Option<(BufferId, u64)>>,
    /// Начало списка краёв (16 байт, в том числе число пикселей), скопированное для
    /// процессора; свой у каждого кадра в полёте (7.18б).
    space_edge_counts: Vec<Option<BufferId>>,
    /// Включено ли сглаживание краёв (настройка, X).
    space_smooth: bool,
    /// Выдавливание через поле слоя (опыт 7.18б): подменять ли в сцене «3D» выдавленные
    /// фигуры верхнего уровня примитивом «поле слоя».
    space_extrude_field: bool,
    /// Поля слоя опытных фигур по контуру и шагу сетки профиля: картинка и само поле.
    field_textures:
        std::collections::HashMap<(String, u32), (TextureId, reyvec_sdf::shape3d::FieldRef)>,
    /// Сбросить поля слоя при следующей подготовке кадра (сменился файл сцены).
    field_textures_stale: bool,
    /// Сколько фигур последнего кадра сцены «3D» нарисовано через поле слоя и сколько
    /// выдавленных фигур верхнего уровня в кадре было.
    field_shapes: (usize, usize),
    /// Запечённое поле расстояний (7.10б) и его проход «поле» (`field_bake.slang`).
    space_field: crate::field::SpaceField,
    space_field_bake: vk::Pipeline,
    /// …и его освещение в проходе «сцена» (`raymarch_shade.slang`).
    space_shade: vk::Pipeline,
    /// Разрешение сцены «3D» (7.14). Если меньше полного, освещение идёт своим проходом
    /// в картинку [`SPACE_COLOR_FORMAT`] (`space_shade_small`), а «сцена» растягивает её
    /// на весь кадр (`raymarch_upscale.slang`).
    space_scale: SpaceScale,
    /// 2.5D-вид уровня из файла (7.9): та же игра, нарисованная объёмом.
    level_view: bool,
    /// Облёт 2.5D-вида: влево-вправо и вверх-вниз от обычного вида, градусы.
    level_orbit: [f32; 2],
    /// Как рисуется 2.5D-вид: формами SDF или треугольниками (7.9б).
    level_render: crate::level3d::LevelRender,
    level: crate::level3d::Level3d,
    space_shade_small: vk::Pipeline,
    space_upscale: vk::Pipeline,
    /// Камера сцены «3D».
    space_camera: reyvec_sdf::Camera3D,
    /// Отсев по взгляду в последнем кадре сцены «3D» (7.6к).
    space_view: reyvec_sdf::view3d::ViewCullStats,
    /// Сетка отсева последнего кадра сцены «3D» (`None` — сетки не было).
    space_grid: Option<crate::space::GridStats>,
    /// Был ли в последнем кадре сцены «3D» отсев по взгляду (у уровня — всегда).
    space_view_culled: bool,
    /// Была ли в последнем кадре сцены «3D» сетка отсева (у уровня — всегда).
    space_culled: bool,
    /// Счётчики последнего кадра сцены «3D» или 2.5D-вида (7.18б); `None` — кадр был
    /// не объёмный.
    space_stats: Option<SpaceStats>,
    /// Поле расстояний сцены «свет» (4.2): маска теней и проходы jump flooding.
    field: FieldPipelines,
    /// Линии фона сцены «свет» по полю из картинки (jump flooding), а не точному.
    light_field: bool,
    /// Во сколько раз поле расстояний меньше кадра по каждой стороне (по видеокарте).
    field_downscale: u32,
    /// Свет сцены «лампы и тени» (4.3): проход «свет» и источники каждого кадра.
    light: vk::Pipeline,
    light_buffers: Vec<BufferId>,
    /// Объекты из SDF (4.5): в кадр сцены и в маску теней; буферы объектов и частей.
    object: vk::Pipeline,
    object_mask: vk::Pipeline,
    object_buffers: Vec<BufferId>,
    /// Сцена «3D» (7.2): список слов сцены на каждый кадр в работе.
    space_buffers: Vec<BufferId>,
    blob_buffers: Vec<BufferId>,
    /// Свечение ярких мест (4.4): шаги вниз и вверх по цепочке уменьшенных картинок.
    bloom_down: vk::Pipeline,
    bloom_up: vk::Pipeline,
    /// Опыт 4.6: свет каскадами лучей вместо лучей к каждому источнику.
    rc_cascade: vk::Pipeline,
    rc_gather: vk::Pipeline,
    cascades: bool,
    /// Свечение и мягкий переход к белому в сцене «лампы и тени».
    glow: bool,
    /// Лёгкий шум при выводе против полос на плавных переходах.
    dither: bool,
    sprites: vk::Pipeline,
    /// Частицы (6.6): кольцо на видеокарте, расчёт и рисование, шаги расчёта этого кадра.
    particles: crate::particles::ParticleSystem,
    particle_update: vk::Pipeline,
    particle_draw: vk::Pipeline,
    /// Светящиеся частицы (искры) поверх освещённого кадра, в проходе «вывод».
    particle_glow: vk::Pipeline,
    particle_steps: Vec<crate::particles::ParticleFrame>,
    /// Новые частицы по шагам логики, пока мир шагает по сценарию без окна.
    particle_queue: Vec<(f64, f32, Vec<crate::particles::Particle>)>,
    /// Последний шаг логики, события которого уже дали частицы.
    seen_event_step: u64,
    /// Новые события игры, которые ещё не забрал звук (6.7).
    fresh_events: Vec<reyvec_scene::GameEvent>,
    /// Фонтан для замера скорости частиц (около 100 000 живых).
    fountain: bool,
    /// Время прошлого кадра с частицами (в окне шаг расчёта — время между кадрами).
    particle_time: Option<f64>,
    /// Копирует кадр сцены в окно; зависит от формата окна.
    present: vk::Pipeline,
    /// Сводит картинку MSAA в обычный кадр сцены.
    resolve: vk::Pipeline,
    /// Формат картинки, в которую выводится кадр (окно или картинка теста).
    format: vk::Format,
    checker: TextureId,
    /// Какой вариант шахматки сейчас загружен (меняется каждые 2 секунды).
    checker_phase: u32,
    disk: TextureId,
    /// Белая точка 1×1: из неё рисуются одноцветные прямоугольники (рамка ошибки шейдера).
    white: TextureId,
    /// Свой буфер спрайтов на каждый кадр в работе: пока видеокарта читает один,
    /// процессор пишет в другой.
    instance_buffers: Vec<BufferId>,
    vector: VectorRenderer,
    vector_demo: VectorDemo,
    /// Копирования новых фигур на видеокарту в начале этого кадра.
    uploads: Uploads,
    /// Сглаживание, под которое собраны конвейеры сцены.
    msaa: Msaa,
    /// Какие числа точек на пиксель видеокарта умеет для кадра сцены.
    msaa_counts: vk::SampleCountFlags,
    /// Сводить MSAA «как в редакторах» (в цвете экрана), а не в линейном цвете.
    perceptual_edges: bool,
    /// Формат трафарета прохода сцены (нужен заливке через трафарет).
    stencil_format: vk::Format,
    /// Формат кадра сцены: компактный ([`COMPACT_SCENE_FORMAT`]) или [`LAYER_FORMAT`].
    scene_format: vk::Format,
    /// Видеокарта умеет компактный кадр сцены ([`pick_scene_format`]).
    compact_supported: bool,
    /// Камера векторных сцен (сдвиг и приближение). Лупа — одно её положение.
    camera: reyvec_vector::Camera2D,
    /// Время процессора на подготовку векторного кадра, мс (сглаженное).
    vector_cpu: VectorCpu,
    /// Векторных фигур и блоков текста в последнем кадре и сколько из них нарисовано
    /// (остальные за краем экрана).
    vector_items: usize,
    vector_drawn: usize,
    /// Сцена «из файла» (5.1): последняя сцена, которая прочиталась без ошибок, готовые
    /// формы её фигур и ошибка последнего сохранения файла (если была).
    file_scene: reyvec_scene::Scene,
    /// Отсечение света и объектов в прошлом кадре (5.5в).
    cull: crate::CullStats,
    file_shapes: reyvec_scene::ShapeCache,
    scene_error: Option<String>,
    /// Сцена «3D» из файла (7.5): последняя, что прочиталась без ошибок, и ошибка
    /// последнего сохранения файла (если была).
    space: reyvec_sdf::SpaceScene,
    space_error: Option<String>,
    /// Мир сцены из файла (5.3): в окне он шагает в своём потоке логики, без окна
    /// (снимки, тесты) — прямо в кадре в заданный момент, чтобы кадр был повторяемым.
    file_world: Option<reyvec_scene::SceneWorld>,
    file_logic: Option<reyvec_scene::SceneLogic>,
    /// Следующий принятый мир — другой файл сцены: персонаж не переносится (6.13).
    fresh_scene: bool,
    logic_thread: bool,
    /// Загрузка в фоне (5.4): файл сцены, уровни детализации, проверочная картина.
    loading: crate::demo_load::DemoLoading,
    /// Камера идёт за персонажем игрока (6.1). Мышь выключает, R включает снова.
    follow: bool,
    /// Время прошлого кадра, когда камера шла за персонажем.
    follow_time: Option<f32>,
    /// Сдвиг камеры от тряски в этом кадре (6.5): перед следующим кадром снимается.
    shake_offset: [f64; 2],
    /// Сценарий игры для кадров без окна (проверка в движении): мир шагает по 1/60 с
    /// с этими нажатиями до момента кадра.
    play: Option<PlayState>,
    /// Последнее нажатие, ответ на которое уже был в кадре, и сколько в среднем от
    /// нажатия до такого кадра, мс (6.3).
    seen_press: u64,
    press_to_frame_ms: Option<f64>,
}

/// Персонаж игрока для панели (6.1, 6.3).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerStats {
    pub view: reyvec_scene::PlayerView,
    /// Сколько нажатие в среднем ждёт шага логики, мс.
    pub input_ms: Option<f64>,
    /// Сколько от нажатия до кадра, в котором уже виден ответ (до отправки кадра
    /// видеокарте; ещё до одного кадра добавляет очередь показа с VSync), мс.
    pub frame_ms: Option<f64>,
}

/// Рамка камеры за персонажем (в высотах экрана от середины, экран от -1 до 1): по
/// бокам, снизу и сверху. Персонаж обычно ниже середины: впереди видно больше.
const FOLLOW_X: f64 = 0.3;
/// Облёт 2.5D-вида (7.9): влево-вправо до этого угла, вниз до вида прямо сбоку, вверх
/// почти до вида сверху (к обычным 12° сверху).
const LEVEL_ORBIT_YAW: f32 = 75.0;
const LEVEL_ORBIT_DOWN: f32 = 12.0;
const LEVEL_ORBIT_UP: f32 = 63.0;
const FOLLOW_BELOW: f64 = 0.75;
const FOLLOW_ABOVE: f64 = 0.3;
/// Как быстро камера догоняет (за 0,1 с проходит 63% пути).
const FOLLOW_RATE: f64 = 10.0;

/// Во сколько раз увеличивает лупа векторной сцены.
pub(crate) const ZOOM: f64 = 8.0;
/// Точка сцены, которая под лупой оказывается в центре окна: край левой половинки сердца.
const ZOOM_CENTER: (f64, f64) = (0.95, 0.6);

/// Камера в положении лупы.
fn lens() -> reyvec_vector::Camera2D {
    reyvec_vector::Camera2D::new(ZOOM_CENTER, ZOOM)
}

impl Demo {
    pub fn new(
        ctx: &GpuContext,
        res: &mut GpuResources,
        shaders: &ShaderLibrary,
        format: vk::Format,
        frames_in_flight: usize,
        files: &UserFiles,
    ) -> Result<Self, GpuError> {
        let checker = res.create_texture_rgba8(
            ctx,
            128,
            128,
            &checker_pixels(128, CHECKER_CELLS[0]),
            true,
            "шахматка",
        )?;
        let disk = res.create_texture_rgba8(ctx, 128, 128, &disk_pixels(128), true, "круг")?;
        let white = res.create_texture_rgba8(ctx, 1, 1, &[255; 4], true, "белый")?;
        let mut instance_buffers = Vec::new();
        for i in 0..frames_in_flight {
            instance_buffers.push(res.create_buffer(
                ctx,
                (MAX_SPRITES * SPRITE_SIZE) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                BufferMemory::CpuToGpu,
                &format!("спрайты кадра {i}"),
            )?);
        }
        let particles = crate::particles::ParticleSystem::new(ctx, res, frames_in_flight)?;
        let mut light_buffers = Vec::new();
        for i in 0..frames_in_flight {
            light_buffers.push(res.create_buffer(
                ctx,
                (MAX_LIGHTS * LIGHT_SIZE) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                BufferMemory::CpuToGpu,
                &format!("свет кадра {i}"),
            )?);
        }
        let mut space_buffers = Vec::new();
        for i in 0..frames_in_flight {
            space_buffers.push(res.create_buffer(
                ctx,
                (crate::space::MAX_WORDS * 4) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                BufferMemory::CpuToGpu,
                &format!("сцена 3D кадра {i}"),
            )?);
        }
        let mut object_buffers = Vec::new();
        let mut blob_buffers = Vec::new();
        for i in 0..frames_in_flight {
            object_buffers.push(res.create_buffer(
                ctx,
                (MAX_OBJECTS * OBJECT_SIZE) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                BufferMemory::CpuToGpu,
                &format!("SDF-объекты кадра {i}"),
            )?);
            blob_buffers.push(res.create_buffer(
                ctx,
                (MAX_BLOBS * BLOB_SIZE) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                BufferMemory::CpuToGpu,
                &format!("части SDF-объектов кадра {i}"),
            )?);
        }
        let vector_demo = VectorDemo::new(files);
        let stencil_format = pick_stencil_format(ctx)?;
        // SAFETY: physical получен из этого экземпляра.
        let limits = unsafe {
            ctx.instance()
                .get_physical_device_properties(ctx.physical)
                .limits
        };
        // Картинку MSAA и рисуют, и читают в шейдере сведения; трафарет того же размера.
        let msaa_counts = limits.framebuffer_color_sample_counts
            & limits.sampled_image_color_sample_counts
            & limits.framebuffer_stencil_sample_counts;
        let scene_format = pick_scene_format(ctx, msaa_counts);
        tracing::info!("Формат кадра сцены: {scene_format:?}");
        tracing::info!(
            "Поле расстояний света: в {} раза меньше кадра по каждой стороне",
            field_downscale_for(ctx.adapter.kind)
        );
        let mut vector = VectorRenderer::new(
            ctx,
            res,
            shaders,
            frames_in_flight,
            stencil_format,
            scene_format,
        )?;
        if let Err(e) = vector
            .upload(ctx, res, &vector_demo.library)
            .and_then(|()| vector.upload_text(ctx, res, &vector_demo.text))
        {
            vector.destroy(ctx);
            return Err(e);
        }
        let mut demo = Self {
            triangle: vk::Pipeline::null(),
            sdf: vk::Pipeline::null(),
            space_march: vk::Pipeline::null(),
            space_mesh: vk::Pipeline::null(),
            space_mesh_shadow: vk::Pipeline::null(),
            space_cone: vk::Pipeline::null(),
            space_shadow: vk::Pipeline::null(),
            space_march_compute: vk::Pipeline::null(),
            space_compute_running: false,
            space_edge_find: vk::Pipeline::null(),
            space_edges: vk::Pipeline::null(),
            space_edge_lists: vec![None; frames_in_flight],
            space_edge_counts: vec![None; frames_in_flight],
            space_smooth: true,
            space_extrude_field: false,
            field_textures: std::collections::HashMap::new(),
            field_textures_stale: false,
            field_shapes: (0, 0),
            space_field: crate::field::SpaceField::new(
                frames_in_flight,
                reyvec_sdf::field3d::FIELD_SIZE,
            ),
            space_field_bake: vk::Pipeline::null(),
            space_shade: vk::Pipeline::null(),
            space_scale: SpaceScale::for_adapter(ctx.adapter.kind),
            level_view: false,
            level_orbit: [0.0, 0.0],
            level_render: crate::level3d::LevelRender::for_adapter(ctx.adapter.kind),
            level: Default::default(),
            space_shade_small: vk::Pipeline::null(),
            space_upscale: vk::Pipeline::null(),
            space_camera: reyvec_sdf::Camera3D::default(),
            space_view: Default::default(),
            space_grid: None,
            space_view_culled: false,
            space_culled: true,
            space_stats: None,
            field: FieldPipelines::default(),
            light_field: false,
            file_scene: reyvec_scene::Scene::default(),
            cull: crate::CullStats::default(),
            file_shapes: reyvec_scene::ShapeCache::new(),
            scene_error: None,
            space: reyvec_sdf::SpaceScene::default(),
            space_error: None,
            file_world: None,
            file_logic: None,
            fresh_scene: false,
            logic_thread: false,
            loading: crate::demo_load::DemoLoading::default(),
            follow: true,
            follow_time: None,
            shake_offset: [0.0, 0.0],
            play: None,
            seen_press: 0,
            press_to_frame_ms: None,
            field_downscale: field_downscale_for(ctx.adapter.kind),
            light: vk::Pipeline::null(),
            light_buffers,
            object: vk::Pipeline::null(),
            object_mask: vk::Pipeline::null(),
            object_buffers,
            space_buffers,
            blob_buffers,
            bloom_down: vk::Pipeline::null(),
            bloom_up: vk::Pipeline::null(),
            rc_cascade: vk::Pipeline::null(),
            rc_gather: vk::Pipeline::null(),
            cascades: false,
            glow: true,
            dither: true,
            sprites: vk::Pipeline::null(),
            particles,
            particle_update: vk::Pipeline::null(),
            particle_draw: vk::Pipeline::null(),
            particle_glow: vk::Pipeline::null(),
            particle_steps: Vec::new(),
            particle_queue: Vec::new(),
            seen_event_step: 0,
            fresh_events: Vec::new(),
            fountain: false,
            particle_time: None,
            present: vk::Pipeline::null(),
            resolve: vk::Pipeline::null(),
            format: vk::Format::UNDEFINED,
            checker,
            checker_phase: 0,
            disk,
            white,
            instance_buffers,
            vector,
            vector_demo,
            uploads: Uploads::default(),
            msaa: Msaa::Off,
            msaa_counts,
            perceptual_edges: false,
            stencil_format,
            scene_format,
            compact_supported: scene_format == COMPACT_SCENE_FORMAT,
            camera: reyvec_vector::Camera2D::default(),
            vector_cpu: VectorCpu::default(),
            vector_items: 0,
            vector_drawn: 0,
        };
        let (text, dir) = files.scene_text();
        demo.set_scene_file(text, dir);
        demo.set_space_file(
            files
                .space
                .as_deref()
                .unwrap_or(reyvec_sdf::space_file::EXAMPLE),
        );
        // Если что-то не соберётся, Drop не нужен: destroy() убирает null-конвейеры спокойно.
        let built = (|| {
            demo.triangle = build(
                ctx,
                res,
                shaders,
                "triangle",
                scene_format,
                Msaa::Off,
                Some(stencil_format),
            )?;
            demo.sprites = build(
                ctx,
                res,
                shaders,
                "sprite",
                scene_format,
                Msaa::Off,
                Some(stencil_format),
            )?;
            demo.sdf = build(
                ctx,
                res,
                shaders,
                "sdf_demo",
                scene_format,
                Msaa::Off,
                Some(stencil_format),
            )?;
            demo.space_shade = build(
                ctx,
                res,
                shaders,
                "raymarch_shade",
                scene_format,
                Msaa::Off,
                Some(stencil_format),
            )?;
            demo.space_shade_small = build(
                ctx,
                res,
                shaders,
                "raymarch_shade",
                SPACE_COLOR_FORMAT,
                Msaa::Off,
                None,
            )?;
            demo.space_upscale = build(
                ctx,
                res,
                shaders,
                "raymarch_upscale",
                scene_format,
                Msaa::Off,
                Some(stencil_format),
            )?;
            demo.space_march = build(
                ctx,
                res,
                shaders,
                "raymarch",
                GBUFFER_FORMAT,
                Msaa::Off,
                None,
            )?;
            demo.space_mesh = build(
                ctx,
                res,
                shaders,
                "mesh3d",
                GBUFFER_FORMAT,
                Msaa::Off,
                Some(MESH_DEPTH_FORMAT),
            )?;
            demo.space_mesh_shadow = build(
                ctx,
                res,
                shaders,
                "mesh_shadow",
                SHADOW_MAP_FORMAT,
                Msaa::Off,
                Some(MESH_DEPTH_FORMAT),
            )?;
            demo.space_cone = build(
                ctx,
                res,
                shaders,
                "raymarch_cone",
                CONE_FORMAT,
                Msaa::Off,
                None,
            )?;
            demo.space_shadow = build(
                ctx,
                res,
                shaders,
                "raymarch_shadow",
                SHADOW3D_FORMAT,
                Msaa::Off,
                None,
            )?;
            demo.resolve = build(ctx, res, shaders, "resolve", scene_format, Msaa::Off, None)?;
            for name in FIELD_SHADERS {
                *demo.field.slot(name) =
                    build(ctx, res, shaders, name, field_format(name), Msaa::Off, None)?;
            }
            demo.light = build(ctx, res, shaders, "light", LIGHT_FORMAT, Msaa::Off, None)?;
            demo.object = build(
                ctx,
                res,
                shaders,
                "sdf_object",
                scene_format,
                Msaa::Off,
                Some(stencil_format),
            )?;
            demo.object_mask = build_object_mask(ctx, res, shaders)?;
            demo.particle_draw = build(
                ctx,
                res,
                shaders,
                "particles",
                scene_format,
                Msaa::Off,
                Some(stencil_format),
            )?;
            demo.particle_update = crate::pipeline::compute_pipeline(
                &ctx.device,
                res.bindless.pipeline_layout,
                shaders.compute("particles"),
            )?;
            demo.space_field_bake = crate::pipeline::compute_pipeline(
                &ctx.device,
                res.bindless.pipeline_layout,
                shaders.compute("field_bake"),
            )?;
            if res.bindless.storage_images {
                let layout = res.bindless.pipeline_layout;
                demo.space_march_compute = crate::pipeline::compute_pipeline(
                    &ctx.device,
                    layout,
                    shaders.compute("raymarch"),
                )?;
                demo.space_edge_find = crate::pipeline::compute_pipeline(
                    &ctx.device,
                    layout,
                    shaders.compute("raymarch_edge_find"),
                )?;
                demo.space_edges = crate::pipeline::compute_pipeline(
                    &ctx.device,
                    layout,
                    shaders.compute("raymarch_edges"),
                )?;
            }
            demo.bloom_down = build(
                ctx,
                res,
                shaders,
                "bloom_down",
                BLOOM_FORMAT,
                Msaa::Off,
                None,
            )?;
            demo.bloom_up = build(ctx, res, shaders, "bloom_up", BLOOM_FORMAT, Msaa::Off, None)?;
            demo.rc_cascade = build(ctx, res, shaders, "rc_cascade", RC_FORMAT, Msaa::Off, None)?;
            demo.rc_gather = build(
                ctx,
                res,
                shaders,
                "rc_gather",
                LIGHT_FORMAT,
                Msaa::Off,
                None,
            )?;
            demo.set_format(ctx, res, shaders, format)
        })();
        match built {
            Ok(()) => Ok(demo),
            Err(e) => {
                demo.destroy_pipelines(ctx);
                Err(e)
            }
        }
    }

    /// Читает текст файла сцены для сцены «из файла». Если в нём ошибка, остаётся прошлая
    /// сцена, а ошибка видна через [`Demo::scene_error`]. Новые формы фигур нарезаются
    /// здесь же и едут на видеокарту в следующем кадре.
    pub fn set_scene_file(&mut self, text: &str, dir: Option<&std::path::Path>) {
        // В окне файл разбирается и режется в фоне, а мир придёт в одном из следующих
        // кадров ([`Self::poll_loading`]).
        if self.loading.submit_scene(text, dir, &self.file_shapes) {
            return;
        }
        let _zone = crate::cpu_zone!("файл сцены");
        let started = std::time::Instant::now();
        let built = reyvec_scene::SceneFile::parse_in(text, dir).and_then(|f| {
            reyvec_scene::SceneWorld::new(&f, &mut self.vector_demo.library, &mut self.file_shapes)
        });
        match built {
            Ok(world) => {
                tracing::info!(
                    "Сцена из файла: сущностей {}, разных форм {} ({:.2} мс)",
                    world.entity_count(),
                    self.file_shapes.len(),
                    started.elapsed().as_secs_f64() * 1000.0
                );
                self.accept_world(world);
            }
            Err(e) => self.reject_scene(e.to_string()),
        }
    }

    /// Читает текст файла сцены «3D» (7.5). Если в нём ошибка, остаётся прошлая сцена,
    /// а ошибка видна через [`Demo::space_error`].
    pub fn set_space_file(&mut self, text: &str) {
        match reyvec_sdf::SpaceScene::parse(text) {
            Ok(space) => {
                tracing::info!("Сцена 3D из файла: предметов {}", space.object_count());
                // Камера из файла ставится, когда её запись поменялась (или появилась):
                // правка остального файла не сбивает полёт.
                if space.camera != self.space.camera {
                    self.space_camera = space.home_camera();
                }
                self.space = space;
                self.space_error = None;
                self.field_textures_stale = true;
            }
            Err(e) => {
                tracing::warn!("Файл сцены 3D не принят: {e}");
                self.space_error = Some(e);
            }
        }
    }

    /// Ошибка в последнем сохранении файла сцены «3D» (рисуется прошлая сцена).
    pub fn space_error(&self) -> Option<&str> {
        self.space_error.as_deref()
    }

    /// Сколько предметов в сцене «3D».
    pub fn space_objects(&self) -> usize {
        self.space.object_count()
    }

    /// Включена ли сетка отсева в последнем объёмном кадре: в сцене «3D» её выключает
    /// `culling: false` в файле, у 2.5D-вида она есть всегда.
    pub fn space_culling(&self) -> bool {
        self.space_culled
    }

    /// Список пикселей краёв (7.6о) кадра `slot` для кадра размера `extent`: создаётся
    /// при первой надобности и пересоздаётся, когда кадр стал больше. Вместе с ним —
    /// буфер на 16 байт, куда проход «края 3D: поиск» копирует начало списка со счётчиком
    /// пикселей для панели (7.18б). `None` — не вышло (тогда края не сглаживаются).
    fn edge_list(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        slot: usize,
        extent: vk::Extent2D,
    ) -> Option<(vk::DeviceAddress, vk::Buffer, vk::Buffer)> {
        let size = 16 + u64::from(extent.width) * u64::from(extent.height) * 4;
        let entry = self.space_edge_lists.get_mut(slot)?;
        if entry.is_some_and(|(_, have)| have < size)
            && let Some((id, _)) = entry.take()
        {
            res.destroy_buffer(id);
        }
        if entry.is_none() {
            match res.create_buffer(
                ctx,
                size,
                vk::BufferUsageFlags::STORAGE_BUFFER
                    | vk::BufferUsageFlags::INDIRECT_BUFFER
                    | vk::BufferUsageFlags::TRANSFER_DST
                    | vk::BufferUsageFlags::TRANSFER_SRC,
                BufferMemory::GpuOnly,
                &format!("края 3D, кадр {slot}"),
            ) {
                Ok(id) => *entry = Some((id, size)),
                Err(e) => {
                    tracing::warn!("буфер краёв 3D: {e}");
                    return None;
                }
            }
        }
        let (id, _) = (*entry)?;
        let count = self.space_edge_counts.get_mut(slot)?;
        if count.is_none() {
            match res.create_buffer(
                ctx,
                16,
                vk::BufferUsageFlags::TRANSFER_DST,
                BufferMemory::GpuToCpu,
                &format!("число краёв 3D, кадр {slot}"),
            ) {
                Ok(id) => {
                    // Панель читает буфер до первого кадра ячейки: пусть там будет 0, а не
                    // старая память (память видна процессору сразу, HOST_COHERENT).
                    res.write_buffer(id, 0, &[0; 16]);
                    *count = Some(id);
                }
                Err(e) => {
                    tracing::warn!("буфер числа краёв 3D: {e}");
                    return None;
                }
            }
        }
        let count = (*count)?;
        Some((
            res.buffer_address(id),
            res.buffer_raw(id)?,
            res.buffer_raw(count)?,
        ))
    }

    /// Число пикселей края в последнем законченном кадре ячейки `slot` (счётчик списка
    /// краёв, скопированный проходом «края 3D: поиск»). Кадр ячейки должен быть закончен
    /// (его забор дождались). `None` — списка краёв у ячейки нет.
    pub fn edge_pixels(&self, res: &GpuResources, slot: usize) -> Option<u32> {
        let id = (*self.space_edge_counts.get(slot)?)?;
        let bytes = res.read_buffer(id, 16)?;
        Some(u32::from_ne_bytes(bytes.get(12..16)?.try_into().ok()?))
    }

    /// Счётчики последнего кадра сцены «3D» или 2.5D-вида для панели (7.18б); `None` —
    /// последний кадр был не объёмный.
    pub fn space_stats(&self) -> Option<SpaceStats> {
        self.space_stats
    }

    /// Запечённое поле расстояний сцены «3D» (7.10б): включено ли и умеет ли видеокарта.
    pub fn space_field(&self) -> (bool, bool) {
        (
            self.space_field.enabled,
            self.space_field_bake != vk::Pipeline::null(),
        )
    }

    /// Включает или выключает запечённое поле со следующего кадра (включённое
    /// считается заново целиком).
    pub fn set_space_field(&mut self, on: bool) {
        self.space_field.enabled = on;
        self.space_field.window.reset();
        self.space_field.skip();
    }

    /// Затенение в углах из поля (7.10г): включено ли и можно ли (поле включено).
    pub fn space_ao_field(&self) -> (bool, bool) {
        let (on, can) = self.space_field();
        (self.space_field.ao_from_field, on && can)
    }

    /// Дальние пробы затенения в углах берут число поля вместо расчёта форм.
    pub fn set_space_ao_field(&mut self, on: bool) {
        self.space_field.ao_from_field = on;
    }

    /// Буфер запечённого поля (для проверки в тестах: читается после кадра).
    pub(crate) fn space_field_buffer(
        &self,
    ) -> Option<(BufferId, reyvec_sdf::field3d::FieldWindow)> {
        Some((self.space_field.buffer()?, self.space_field.window.clone()))
    }

    /// Сглаживание краёв форм сцены «3D» (7.6о): включено ли и умеет ли видеокарта.
    pub fn space_smooth(&self) -> (bool, bool) {
        (self.space_smooth, self.space_edges != vk::Pipeline::null())
    }

    /// Выдавливание через поле слоя в сцене «3D» (опыт 7.18б): включено ли и есть ли в
    /// кадре выдавленные фигуры верхнего уровня, которые подменяются.
    pub fn space_extrude_field(&self) -> (bool, bool) {
        (self.space_extrude_field, self.field_shapes.1 > 0)
    }

    /// Включает или выключает выдавливание через поле слоя со следующего кадра.
    pub fn set_space_extrude_field(&mut self, on: bool) {
        self.space_extrude_field = on;
    }

    /// Сколько фигур последнего кадра сцены «3D» нарисовано через поле слоя (7.18б).
    pub fn field_shapes(&self) -> usize {
        self.field_shapes.0
    }

    /// Опыт 7.18б: выдавленные фигуры верхнего уровня сцены «3D» (`Paint{Move{Extrude}}`,
    /// как их отдаёт файл сцены) рисуются через примитив «поле слоя» с полем на сетке
    /// самого профиля — картинка та же, путь другой: поле читается из картинки, а не из
    /// слов сцены. Шар-граница у поля та же, что у выдавливания (по прямоугольнику
    /// профиля), так что отсев и пересчёт поля 7.10 не меняются. Выдавливания внутри
    /// объединений и вырезаний не трогаются: опыту хватает верхнего уровня, а
    /// материал-оценка поля в выборе ближайшей части не проверена.
    fn swap_extrudes(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        frame: &mut reyvec_sdf::SpaceFrame,
    ) {
        use reyvec_sdf::Shape3;
        if self.field_textures_stale || !self.space_extrude_field {
            for (id, _) in self.field_textures.drain().map(|(_, v)| v) {
                res.destroy_texture(id);
            }
            self.field_textures_stale = false;
        }
        let (mut swapped, mut candidates) = (0, 0);
        for shape in &mut frame.shapes {
            let Shape3::Paint { shape: moved, .. } = shape else {
                continue;
            };
            let Shape3::Move { shape: inner, .. } = moved.as_mut() else {
                continue;
            };
            let Shape3::Extrude {
                profile,
                depth,
                round,
            } = inner.as_ref()
            else {
                continue;
            };
            candidates += 1;
            if !self.space_extrude_field {
                continue;
            }
            let (depth, round) = (*depth, *round);
            let Some(field) = self.field_for(ctx, res, profile) else {
                break;
            };
            **inner = Shape3::Field {
                field,
                depth,
                round,
            };
            swapped += 1;
        }
        self.field_shapes = (swapped, candidates);
    }

    /// Поле слоя для профиля (7.18б): из кэша или новая картинка R16G16_SFLOAT на сетке
    /// профиля. Если видеокарта картинку не дала, опыт выключается с записью в журнал.
    fn field_for(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        profile: &reyvec_sdf::profile2d::Profile,
    ) -> Option<reyvec_sdf::shape3d::FieldRef> {
        use reyvec_sdf::layer_field::LayerField;
        use reyvec_sdf::shape3d::FieldRef;
        // Один контур с другой клеткой — другие слова: в ключе и контур, и шаг.
        let key = (profile.path().to_owned(), profile.words()[2]);
        if let Some((_, field)) = self.field_textures.get(&key) {
            return Some(field.clone());
        }
        let mut field = LayerField::from_profile(profile, EXTRUDE_FIELD_BAND);
        // Тест 7.18б проверяет и бесконечный путь поля уровня 7.18в: поле без
        // прямоугольника фигуры, без шара-границы.
        #[cfg(test)]
        if crate::headless::tests::FIELD_NO_RECT.get() {
            field = field.without_rect();
        }
        let (nx, ny) = field.size();
        let extent = vk::Extent2D {
            width: nx,
            height: ny,
        };
        let made = res
            .create_texture_format(
                ctx,
                nx,
                ny,
                vk::Format::R16G16_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                &format!("поле слоя {nx}×{ny}"),
            )
            .and_then(
                |id| match res.upload_texture_bytes(ctx, id, extent, &field.bytes()) {
                    Ok(()) => Ok(id),
                    Err(e) => {
                        res.destroy_texture(id);
                        Err(e)
                    }
                },
            );
        match made {
            Ok(id) => {
                field.set_texture(id.0);
                let field = FieldRef(std::sync::Arc::new(field));
                self.field_textures.insert(key, (id, field.clone()));
                Some(field)
            }
            Err(e) => {
                tracing::warn!("поле слоя не создано, выдавливание через поле выключено: {e}");
                self.space_extrude_field = false;
                None
            }
        }
    }

    /// Включает или выключает сглаживание краёв форм сцены «3D».
    pub fn set_space_smooth(&mut self, on: bool) {
        self.space_smooth = on;
    }

    /// 2.5D-вид уровня из файла (7.9): включён ли.
    pub fn level_view(&self) -> bool {
        self.level_view
    }

    /// Облёт 2.5D-вида для осмотра: повернуть ещё на `yaw` градусов влево-вправо и
    /// `pitch` вверх-вниз. Наклон держится между видом прямо сбоку снизу и почти сверху.
    pub fn orbit_level(&mut self, yaw: f32, pitch: f32) {
        let [y, p] = self.level_orbit;
        self.level_orbit = [
            (y + yaw).clamp(-LEVEL_ORBIT_YAW, LEVEL_ORBIT_YAW),
            (p + pitch).clamp(-LEVEL_ORBIT_DOWN, LEVEL_ORBIT_UP),
        ];
    }

    /// Как рисуется 2.5D-вид (7.9б).
    pub fn level_render(&self) -> crate::level3d::LevelRender {
        self.level_render
    }

    /// Меняет способ рисования 2.5D-вида со следующего кадра.
    pub fn set_level_render(&mut self, render: crate::level3d::LevelRender) {
        self.level_render = render;
    }

    /// Возвращает 2.5D-вид из облёта к обычному.
    pub fn reset_level_orbit(&mut self) {
        self.level_orbit = [0.0, 0.0];
    }

    /// Включает или выключает 2.5D-вид уровня со следующего кадра. Игра не меняется.
    pub fn set_level_view(&mut self, on: bool) {
        if on && !self.level_view {
            self.level.clear();
        }
        self.level_view = on;
    }

    /// Разрешение сцены «3D» (7.14).
    pub fn space_scale(&self) -> SpaceScale {
        self.space_scale
    }

    /// Меняет разрешение сцены «3D» со следующего кадра.
    pub fn set_space_scale(&mut self, scale: SpaceScale) {
        self.space_scale = scale;
    }

    /// Отсев по взгляду в последнем кадре сцены «3D»: сколько объектов осталось из
    /// скольких (`None` — выключен, `view_culling: false`).
    pub fn space_in_view(&self) -> Option<reyvec_sdf::view3d::ViewCullStats> {
        self.space_view_culled.then_some(self.space_view)
    }

    /// Сетка отсева последнего кадра сцены «3D»: клетка, размеры, длины списков
    /// (`None` — сетки не было).
    pub fn space_grid(&self) -> Option<crate::space::GridStats> {
        self.space_grid
    }

    /// Лучи сцены «3D» вычислительным шейдером (опыт 7.6м): попросил ли файл
    /// (нет `compute: false`) и шли ли так лучи последнего кадра.
    pub fn space_compute(&self) -> (bool, bool) {
        (self.space.compute, self.space_compute_running)
    }

    /// Другой файл сцены (N в песочнице, 6.13): как [`Self::set_scene_file`], но персонаж
    /// встаёт на место из нового файла, а не остаётся там, где был.
    pub fn switch_scene_file(&mut self, text: &str, dir: Option<&std::path::Path>) {
        self.fresh_scene = true;
        self.set_scene_file(text, dir);
    }

    fn accept_world(&mut self, world: reyvec_scene::SceneWorld) {
        // Новый мир принят: формы и рисунки, которые спрашивал только старый, больше не
        // нужны (5.5б). Снимок старого мира, если он ещё рисуется, удалённые фигуры
        // просто пропустит.
        self.file_shapes
            .release_unused(&mut self.vector_demo.library);
        let mut world = world;
        let fresh = std::mem::take(&mut self.fresh_scene);
        if !fresh && let Some(body) = self.file_world.as_mut().and_then(|w| w.player_body()) {
            world.set_player_body(body);
        }
        match &self.file_logic {
            Some(logic) if fresh => logic.switch(world),
            Some(logic) => logic.replace(world),
            None if self.logic_thread => {
                self.file_logic = Some(reyvec_scene::SceneLogic::start(world, 0.0));
            }
            None => self.file_world = Some(world),
        }
        self.scene_error = None;
    }

    fn reject_scene(&mut self, error: String) {
        self.fresh_scene = false;
        tracing::warn!("Файл сцены не принят: {error}");
        self.scene_error = Some(error);
    }

    /// Забирает готовое из потока загрузки: кладёт фигуры в набор, принимает новый мир
    /// сцены из файла.
    fn poll_loading(&mut self) {
        let loaded = self
            .loading
            .poll(&mut self.vector_demo.library, &mut self.file_shapes);
        match loaded {
            Some(crate::demo_load::SceneLoad::World(world)) => {
                tracing::info!(
                    "Сцена из файла: сущностей {}, разных форм {}",
                    world.entity_count(),
                    self.file_shapes.len()
                );
                self.accept_world(*world);
            }
            Some(crate::demo_load::SceneLoad::Error(e)) => self.reject_scene(e),
            None => {}
        }
        // Память удалённых фигур (старая картина, ненужные формы сцены) освобождается
        // в потоке загрузки.
        self.loading.discard(&mut self.vector_demo.library);
    }

    /// Окно: логика сцены из файла шагает в своём потоке 60 раз в секунду, а кадр
    /// сглаживает движение между шагами; файл сцены, картина и уровни детализации
    /// режутся в потоке загрузки. Без этого (снимки, тесты) всё делается прямо в кадре,
    /// чтобы кадр был повторяемым.
    pub fn use_threads(&mut self) {
        self.logic_thread = true;
        if let Some(world) = self.file_world.take() {
            self.file_logic = Some(reyvec_scene::SceneLogic::start(world, 0.0));
            tracing::info!("Логика сцены из файла: свой поток, 60 шагов в секунду");
        }
        self.loading.start(&mut self.vector_demo.library);
    }

    /// Проверка загрузки (5.4): новая тяжёлая картина на стене сцены «из файла». В фоне
    /// (`background`) или, для сравнения, прямо в кадре.
    pub fn load_picture(&mut self, background: bool) {
        self.loading
            .load_picture(&mut self.vector_demo.library, background);
    }

    /// Свет и SDF-объекты прошлого кадра: сколько в сцене и сколько ушло в кадр (5.5в).
    pub fn cull_stats(&self) -> crate::CullStats {
        self.cull
    }

    /// Что показывает строка «Загрузка» на панели.
    pub fn load_stats(&self) -> crate::demo_load::LoadStats {
        let library = &self.vector_demo.library;
        crate::demo_load::LoadStats {
            memory: crate::demo_load::ShapeMemory {
                live: library.live(),
                removed: library.removed_count(),
                gpu: self.vector.mesh_memory(),
            },
            ..self.loading.stats()
        }
    }

    /// Действие игрока нажато или отпущено (6.1): уходит в поток логики сцены из файла.
    pub fn game_action(&mut self, action: reyvec_scene::game::Action, down: bool) {
        if let Some(logic) = &self.file_logic {
            logic.set_action(action, down);
        }
    }

    /// Мышь для физики сцены из файла (6.13): точка в координатах сцены и держат ли
    /// кнопку захвата.
    pub fn physics_pointer(&mut self, at: [f64; 2], held: bool) {
        if let Some(logic) = &self.file_logic {
            logic.set_pointer(at, held);
        } else if let Some(w) = &mut self.file_world {
            w.set_pointer(reyvec_scene::Pointer {
                at,
                held,
                blast: false,
            });
        }
    }

    /// Взрыв в точке `at` (координаты сцены).
    pub fn physics_blast(&mut self, at: [f64; 2]) {
        if let Some(logic) = &self.file_logic {
            logic.blast(at);
        } else if let Some(w) = &mut self.file_world {
            w.set_pointer(reyvec_scene::Pointer {
                at,
                held: false,
                blast: true,
            });
        }
    }

    /// Физика сцены из файла для панели (6.13), если в сцене есть тела.
    pub fn physics_stats(&self) -> Option<reyvec_scene::game::physics::PhysicsStats> {
        self.file_scene.physics
    }

    /// Всё отпущено (окно потеряло фокус).
    pub fn release_game_input(&mut self) {
        if let Some(logic) = &self.file_logic {
            logic.release_all();
        }
    }

    /// Пауза игры (6.10б): мир сцены из файла не шагает.
    pub fn set_game_paused(&mut self, on: bool) {
        if let Some(logic) = &self.file_logic
            && logic.paused() != on
        {
            logic.set_paused(on);
        }
    }

    /// Камера идёт за персонажем игрока.
    pub fn follow(&self) -> bool {
        self.follow
    }

    /// Включает или выключает камеру за персонажем.
    pub fn set_follow(&mut self, on: bool) {
        self.follow = on;
        self.follow_time = None;
    }

    /// Персонаж игрока сцены из файла и задержки ввода.
    pub fn player_stats(&self) -> Option<PlayerStats> {
        Some(PlayerStats {
            view: self.file_scene.player?,
            input_ms: self.file_logic.as_ref().and_then(|l| l.input_ms()),
            frame_ms: self.press_to_frame_ms,
        })
    }

    /// Сценарий игры для кадров без окна: герой встаёт в `play.start`, нажатия идут по
    /// времени, мир шагает шагами логики до момента кадра.
    pub fn set_play(&mut self, play: crate::HeadlessPlay) {
        self.fountain = play.fountain;
        self.play = Some(PlayState {
            script: play,
            time: 0.0,
            held: Vec::new(),
            started: false,
        });
    }

    /// Мир по сценарию до момента `time` (только вперёд).
    fn advance_play(&mut self, time: f32) {
        let (Some(play), Some(world)) = (self.play.as_mut(), self.file_world.as_mut()) else {
            return;
        };
        if !play.started {
            play.started = true;
            if let (Some(at), Some(_)) = (play.script.start, world.player_body()) {
                world.set_player_body(reyvec_scene::game::Body::new(at));
            }
        }
        let end = f64::from(time);
        let step = reyvec_scene::STEP.as_secs_f64();
        while play.time + step <= end + 1e-9 {
            let next = play.time + step;
            let mut pressed = Vec::new();
            for input in &play.script.inputs {
                if input.at > play.time - 1e-9 && input.at <= next - 1e-9 {
                    play.held.retain(|a| *a != input.action);
                    if input.down {
                        play.held.push(input.action);
                        pressed.push(input.action);
                    }
                }
            }
            world.set_input(reyvec_scene::game::InputFrame::new(&play.held, &pressed));
            world.update_at(next);
            play.time = next;
            // Частицы шагают вместе с логикой: каждый шаг — свой шаг расчёта на
            // видеокарте, иначе в кадре через секунду не было бы ни полёта, ни отскока.
            let mut emitted = event_particles(
                &world.events(),
                &mut self.seen_event_step,
                &mut self.fresh_events,
            );
            if self.fountain {
                let at = fountain_at(world.player_body().map(|b| b.pos), self.camera);
                let seed = (next * 1000.0) as u64;
                emitted.extend(crate::particles::fountain(at, step as f32, seed));
            }
            self.particle_queue.push((next, step as f32, emitted));
        }
        self.file_scene = world.snapshot();
    }

    /// Шаги расчёта частиц этого кадра (6.6): новые частицы из событий игры и фонтана,
    /// твёрдое уровня на экране. Частицы есть только в сцене «из файла».
    fn prepare_particles(
        &mut self,
        res: &mut GpuResources,
        slot: usize,
        time: f32,
        scene: DemoScene,
        aspect: f32,
    ) {
        if scene != DemoScene::File {
            self.particle_steps.clear();
            self.particle_queue.clear();
            self.particle_time = None;
            return;
        }
        let now = f64::from(time);
        let steps = if self.play.is_some() {
            std::mem::take(&mut self.particle_queue)
        } else {
            // В окне: один шаг на кадр длиной в время между кадрами.
            let dt = self
                .particle_time
                .map_or(0.0, |t| (now - t).clamp(0.0, 0.1)) as f32;
            let mut emitted = event_particles(
                &self.file_scene.events,
                &mut self.seen_event_step,
                &mut self.fresh_events,
            );
            if self.fountain {
                let at = fountain_at(
                    self.file_scene
                        .player
                        .map(|p| [f64::from(p.at[0]), f64::from(p.at[1])]),
                    self.camera,
                );
                emitted.extend(crate::particles::fountain(at, dt, (now * 1000.0) as u64));
            }
            vec![(now, dt, emitted)]
        };
        self.particle_time = Some(now);
        // Твёрдое для отскока: только то, что рядом с экраном (с запасом на полёт).
        let view = crate::cull::ViewRect::new(
            [self.camera.center.x as f32, self.camera.center.y as f32],
            self.camera.zoom as f32,
            aspect,
        );
        let margin = 0.5;
        let solids: Vec<crate::particles::ParticleSolid> = self
            .file_scene
            .solids
            .iter()
            .filter(|c| {
                let (lo, hi) = (c.aabb.min, c.aabb.max);
                hi[0] as f32 >= view.lo[0] - margin
                    && lo[0] as f32 <= view.hi[0] + margin
                    && hi[1] as f32 >= view.lo[1] - margin
                    && lo[1] as f32 <= view.hi[1] + margin
            })
            .map(|c| crate::particles::ParticleSolid {
                lo: [c.aabb.min[0] as f32, c.aabb.min[1] as f32],
                hi: [c.aabb.max[0] as f32, c.aabb.max[1] as f32],
                slope: c.slope as f32,
                oneway: c.oneway,
            })
            .collect();
        self.particle_steps =
            self.particles
                .prepare(res, slot, steps, &solids, crate::particles::GRAVITY);
    }

    /// Частицы в работе (для панели и проверок).
    pub fn particle_stats(&self) -> crate::ParticleStats {
        crate::ParticleStats {
            live: self.particle_steps.last().map_or(0, |f| f.span.count),
            dropped: self.particles.dropped,
            fountain: self.fountain,
        }
    }

    /// Новые события игры с прошлого вызова (для звука).
    pub fn take_game_events(&mut self) -> Vec<reyvec_scene::GameEvent> {
        std::mem::take(&mut self.fresh_events)
    }

    /// Включает фонтан частиц со следующего кадра.
    pub fn set_fountain(&mut self, on: bool) {
        self.fountain = on;
    }

    /// Где камера: середина и приближение (для проверок кадров без окна).
    pub fn camera_view(&self) -> ([f64; 2], f64) {
        (
            [self.camera.center.x, self.camera.center.y],
            self.camera.zoom,
        )
    }

    /// Скрипты сцены из файла (6.9), если они есть.
    pub fn script_info(&self) -> Option<reyvec_scene::ScriptInfo> {
        let s = &self.file_scene.scripts;
        (s.files > 0).then(|| s.clone())
    }

    /// Поток логики сцены из файла (для консоли разработчика, 6.11).
    pub fn scene_logic(&self) -> Option<&reyvec_scene::SceneLogic> {
        self.file_logic.as_ref()
    }

    /// Твёрдое сцены из файла в рисуемом кадре (показ столкновений в консоли).
    pub fn file_solids(&self) -> &[reyvec_scene::game::Collider] {
        &self.file_scene.solids
    }

    /// Снимок сцены из файла, который рисуется сейчас.
    pub fn file_player(&self) -> Option<reyvec_scene::PlayerView> {
        self.file_scene.player
    }

    /// Камера за персонажем: пока он в рамке посередине экрана, камера стоит; вышел —
    /// камера плавно догоняет, чтобы он снова был у края рамки.
    fn follow_player(&mut self, time: f32) {
        // Тряска прошлого кадра снимается: камера догоняет героя от своего места.
        self.camera.center.x -= self.shake_offset[0];
        self.camera.center.y -= self.shake_offset[1];
        self.shake_offset = [0.0, 0.0];
        let Some(p) = self.file_scene.player.filter(|_| self.follow) else {
            self.follow_time = None;
            return;
        };
        // В первом кадре камера сразу встаёт на место.
        let dt = self
            .follow_time
            .map_or(1.0, |t| f64::from((time - t).clamp(0.0, 0.1)));
        self.follow_time = Some(time);
        let z = self.camera.zoom;
        let c = self.camera.center;
        let (px, py) = (f64::from(p.at[0]), f64::from(p.at[1]));
        let target = |c: f64, p: f64, below: f64, above: f64| {
            if p < c - below / z {
                p + below / z
            } else if p > c + above / z {
                p - above / z
            } else {
                c
            }
        };
        let tx = target(c.x, px, FOLLOW_X, FOLLOW_X);
        let ty = target(c.y, py, FOLLOW_BELOW, FOLLOW_ABOVE);
        let k = 1.0 - (-FOLLOW_RATE * dt).exp();
        self.camera.center.x += (tx - c.x) * k;
        self.camera.center.y += (ty - c.y) * k;
        // Тряска от попадания (6.5): быстрые колебания с затухающим размахом.
        let amp = f64::from(self.file_scene.shake);
        if amp > 0.0 {
            let t = f64::from(time);
            self.shake_offset = [amp * (t * 71.0).sin(), amp * (t * 53.0 + 1.3).cos()];
            self.camera.center.x += self.shake_offset[0];
            self.camera.center.y += self.shake_offset[1];
        }
    }

    /// Логика сцены из файла: сколько шагов сделано и среднее время шага, мс.
    pub fn logic_stats(&self) -> Option<(u64, f64)> {
        self.file_logic.as_ref().map(|l| (l.steps(), l.step_ms()))
    }

    /// Ошибка в последнем сохранении файла сцены (рисуется прошлая сцена).
    pub fn scene_error(&self) -> Option<&str> {
        self.scene_error.as_deref()
    }

    /// Сглаживание, с которым рисуется сцена.
    pub fn msaa(&self) -> Msaa {
        self.msaa
    }

    /// Как рисуются залитые векторные фигуры.
    pub fn fill(&self) -> FillMode {
        self.vector.fill()
    }

    /// Способ рисования залитых фигур со следующего кадра.
    pub fn set_fill(&mut self, fill: FillMode) {
        self.vector.set_fill(fill);
    }

    /// Камера стоит в положении лупы.
    pub fn zoom(&self) -> bool {
        self.camera == lens()
    }

    /// Лупа векторной сцены: камера с увеличением в [`ZOOM`] раз у края сердца, чтобы были
    /// видны изгибы кривых. Выключение возвращает камеру в начало.
    pub fn set_zoom(&mut self, on: bool) {
        self.camera = if on {
            lens()
        } else {
            reyvec_vector::Camera2D::default()
        };
    }

    /// Линии фона сцены «свет» по полю из картинки (jump flooding), а не точному.
    pub fn light_field(&self) -> bool {
        self.light_field
    }

    /// Включает линии фона сцены «свет» по полю из картинки.
    pub fn set_light_field(&mut self, on: bool) {
        self.light_field = on;
    }

    /// Свечение ярких мест и мягкий переход к белому в сцене «лампы и тени» (4.4).
    pub fn glow(&self) -> bool {
        self.glow
    }

    /// Включает свечение и мягкий переход к белому со следующего кадра.
    pub fn set_glow(&mut self, on: bool) {
        self.glow = on;
    }

    /// Опыт 4.6: свет каскадами лучей (radiance cascades) вместо лучей к источникам.
    pub fn cascades(&self) -> bool {
        self.cascades
    }

    /// Включает свет каскадами лучей со следующего кадра.
    pub fn set_cascades(&mut self, on: bool) {
        self.cascades = on;
    }

    /// Лёгкий шум при выводе против полос на плавных переходах (4.4).
    pub fn dither(&self) -> bool {
        self.dither
    }

    /// Включает шум против полос со следующего кадра.
    pub fn set_dither(&mut self, on: bool) {
        self.dither = on;
    }

    /// Камера векторных сцен.
    pub fn camera(&self) -> reyvec_vector::Camera2D {
        self.camera
    }

    /// Ставит камеру векторных сцен со следующего кадра.
    pub fn set_camera(&mut self, camera: reyvec_vector::Camera2D) {
        self.camera = camera;
    }

    /// Камера сцены «3D».
    pub fn space_camera(&self) -> reyvec_sdf::Camera3D {
        self.space_camera
    }

    /// Камера сцены «3D» по R: из файла сцены или вид по умолчанию.
    pub fn space_home_camera(&self) -> reyvec_sdf::Camera3D {
        self.space.home_camera()
    }

    /// Ставит камеру сцены «3D» со следующего кадра.
    pub fn set_space_camera(&mut self, camera: reyvec_sdf::Camera3D) {
        self.space_camera = camera;
    }

    /// Сколько процессор тратит на подготовку векторного кадра (раскладка фигур, запись
    /// экземпляров и команд), мс, сглажено за последние кадры, сколько в кадре фигур и
    /// сколько из них нарисовано (остальные целиком за краем экрана).
    pub fn vector_cpu(&self) -> VectorCpu {
        VectorCpu {
            shapes: self.vector_items,
            drawn: self.vector_drawn,
            ..self.vector_cpu
        }
    }

    /// Края сводятся «как в редакторах» (`true`) или физически точно.
    pub fn perceptual_edges(&self) -> bool {
        self.perceptual_edges
    }

    /// Как сводить края при MSAA, см. `resolve.slang`.
    pub fn set_perceptual_edges(&mut self, on: bool) {
        self.perceptual_edges = on;
    }

    /// Что получится, если попросить `wanted`: видеокарта может уметь меньше.
    pub fn msaa_supported(&self, wanted: Msaa) -> Msaa {
        wanted.supported(self.msaa_counts)
    }

    /// Пересобирает конвейеры сцены под сглаживание `wanted` (или ближайшее, которое
    /// умеет видеокарта). Вызывать, когда видеокарта простаивает. Возвращает, что вышло.
    pub fn set_msaa(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &ShaderLibrary,
        wanted: Msaa,
    ) -> Result<Msaa, GpuError> {
        let msaa = self.msaa_supported(wanted);
        if msaa == self.msaa {
            return Ok(msaa);
        }
        self.set_targets(ctx, res, shaders, msaa, self.scene_format)?;
        if msaa != wanted {
            tracing::warn!(
                "Видеокарта не умеет {}, сглаживание: {}",
                wanted.title(),
                msaa.title()
            );
        }
        tracing::info!("Сглаживание сцены: {}", msaa.title());
        Ok(msaa)
    }

    /// Компактный ли кадр сцены (4 байта на точку, без прозрачности) или точный
    /// (16 бит на канал, как у слоёв).
    pub fn compact_scene(&self) -> bool {
        self.scene_format != LAYER_FORMAT
    }

    /// Умеет ли видеокарта компактный кадр сцены.
    pub fn compact_scene_supported(&self) -> bool {
        self.compact_supported
    }

    /// Компактный или точный кадр сцены (для сравнения скорости и вида). Если видеокарта
    /// компактный не умеет, остаётся точный. Вызывать, когда видеокарта простаивает.
    pub fn set_compact_scene(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &ShaderLibrary,
        compact: bool,
    ) -> Result<(), GpuError> {
        let format = if compact && self.compact_supported {
            COMPACT_SCENE_FORMAT
        } else {
            LAYER_FORMAT
        };
        if format == self.scene_format {
            return Ok(());
        }
        self.set_targets(ctx, res, shaders, self.msaa, format)?;
        tracing::info!("Кадр сцены: {format:?}");
        Ok(())
    }

    /// Пересобирает конвейеры, которые рисуют в кадр сцены, под сглаживание `msaa` и
    /// формат `format`. Видеокарта простаивает.
    fn set_targets(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &ShaderLibrary,
        msaa: Msaa,
        format: vk::Format,
    ) -> Result<(), GpuError> {
        let stencil = Some(self.stencil_format);
        let mut made: Vec<vk::Pipeline> = Vec::new();
        let result = (|| {
            for (name, samples, stencil) in [
                ("triangle", msaa, stencil),
                ("sprite", msaa, stencil),
                ("resolve", Msaa::Off, None),
                ("sdf_demo", msaa, stencil),
                ("sdf_object", msaa, stencil),
                ("particles", msaa, stencil),
                ("raymarch_shade", msaa, stencil),
                ("raymarch_upscale", msaa, stencil),
            ] {
                made.push(build(ctx, res, shaders, name, format, samples, stencil)?);
            }
            self.vector.set_targets(ctx, res, shaders, msaa, format)
        })();
        if let Err(e) = result {
            // SAFETY: конвейеры только что созданы и нигде не используются.
            for p in made {
                unsafe { ctx.device.destroy_pipeline(p, None) };
            }
            return Err(e);
        }
        // SAFETY: видеокарта простаивает (условие функции).
        unsafe {
            ctx.device.destroy_pipeline(self.triangle, None);
            ctx.device.destroy_pipeline(self.sprites, None);
            ctx.device.destroy_pipeline(self.resolve, None);
            ctx.device.destroy_pipeline(self.sdf, None);
            ctx.device.destroy_pipeline(self.object, None);
            ctx.device.destroy_pipeline(self.particle_draw, None);
            ctx.device.destroy_pipeline(self.space_shade, None);
            ctx.device.destroy_pipeline(self.space_upscale, None);
        }
        self.triangle = made[0];
        self.sprites = made[1];
        self.resolve = made[2];
        self.sdf = made[3];
        self.object = made[4];
        self.particle_draw = made[5];
        self.space_shade = made[6];
        self.space_upscale = made[7];
        self.msaa = msaa;
        self.scene_format = format;
        Ok(())
    }

    /// Формат картинки, в которую выводится кадр.
    pub fn output_format(&self) -> vk::Format {
        self.format
    }

    /// Пересобирает конвейер вывода, если поменялся формат окна. Вызывать, когда
    /// видеокарта не использует старый конвейер вывода.
    pub fn set_format(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &ShaderLibrary,
        format: vk::Format,
    ) -> Result<(), GpuError> {
        if format == self.format {
            return Ok(());
        }
        let present = build(ctx, res, shaders, "present", format, Msaa::Off, None)?;
        let glow = match build(ctx, res, shaders, "particles", format, Msaa::Off, None) {
            Ok(p) => p,
            Err(e) => {
                // SAFETY: конвейер только что создан и нигде не используется.
                unsafe { ctx.device.destroy_pipeline(present, None) };
                return Err(e);
            }
        };
        // SAFETY: условие функции; null допустим.
        unsafe {
            ctx.device.destroy_pipeline(self.present, None);
            ctx.device.destroy_pipeline(self.particle_glow, None);
        }
        self.present = present;
        self.particle_glow = glow;
        self.format = format;
        Ok(())
    }

    /// Пересобирает конвейеры шейдеров `names` после их перезагрузки. Если драйвер не
    /// принял новый шейдер, остаётся прошлый конвейер, а ошибка записывается в библиотеку.
    /// Вызывать, когда видеокарта простаивает.
    pub fn reload(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &mut ShaderLibrary,
        names: &[String],
    ) {
        self.vector.reload(ctx, res, shaders, names);
        for name in names {
            let stencil = Some(self.stencil_format);
            let (slot, format, msaa, stencil) = match name.as_str() {
                "triangle" => (&mut self.triangle, self.scene_format, self.msaa, stencil),
                "sprite" => (&mut self.sprites, self.scene_format, self.msaa, stencil),
                "present" => (&mut self.present, self.format, Msaa::Off, None),
                "resolve" => (&mut self.resolve, self.scene_format, Msaa::Off, None),
                "sdf_demo" => (&mut self.sdf, self.scene_format, self.msaa, stencil),
                "raymarch_shade" => {
                    match build(ctx, res, shaders, name, SPACE_COLOR_FORMAT, Msaa::Off, None) {
                        Ok(new) => {
                            let old = std::mem::replace(&mut self.space_shade_small, new);
                            // SAFETY: видеокарта простаивает (условие функции).
                            unsafe { ctx.device.destroy_pipeline(old, None) };
                        }
                        Err(e) => shaders.set_error(name, e.to_string()),
                    }
                    (&mut self.space_shade, self.scene_format, self.msaa, stencil)
                }
                "raymarch_upscale" => (
                    &mut self.space_upscale,
                    self.scene_format,
                    self.msaa,
                    stencil,
                ),
                "raymarch" => (&mut self.space_march, GBUFFER_FORMAT, Msaa::Off, None),
                "raymarch_cone" => (&mut self.space_cone, CONE_FORMAT, Msaa::Off, None),
                "mesh3d" => (
                    &mut self.space_mesh,
                    GBUFFER_FORMAT,
                    Msaa::Off,
                    Some(MESH_DEPTH_FORMAT),
                ),
                "mesh_shadow" => (
                    &mut self.space_mesh_shadow,
                    SHADOW_MAP_FORMAT,
                    Msaa::Off,
                    Some(MESH_DEPTH_FORMAT),
                ),
                "raymarch_shadow" => (&mut self.space_shadow, SHADOW3D_FORMAT, Msaa::Off, None),
                "particles" => {
                    match build(ctx, res, shaders, name, self.format, Msaa::Off, None) {
                        Ok(new) => {
                            let old = std::mem::replace(&mut self.particle_glow, new);
                            // SAFETY: видеокарта простаивает (условие функции).
                            unsafe { ctx.device.destroy_pipeline(old, None) };
                        }
                        Err(e) => shaders.set_error(name, e.to_string()),
                    }
                    (
                        &mut self.particle_draw,
                        self.scene_format,
                        self.msaa,
                        stencil,
                    )
                }
                "light" => (&mut self.light, LIGHT_FORMAT, Msaa::Off, None),
                "sdf_object" => {
                    match build_object_mask(ctx, res, shaders) {
                        Ok(new) => {
                            let old = std::mem::replace(&mut self.object_mask, new);
                            // SAFETY: видеокарта простаивает (условие функции).
                            unsafe { ctx.device.destroy_pipeline(old, None) };
                        }
                        Err(e) => shaders.set_error(name, e.to_string()),
                    }
                    (&mut self.object, self.scene_format, self.msaa, stencil)
                }
                "bloom_down" => (&mut self.bloom_down, BLOOM_FORMAT, Msaa::Off, None),
                "bloom_up" => (&mut self.bloom_up, BLOOM_FORMAT, Msaa::Off, None),
                "rc_cascade" => (&mut self.rc_cascade, RC_FORMAT, Msaa::Off, None),
                "rc_gather" => (&mut self.rc_gather, LIGHT_FORMAT, Msaa::Off, None),
                n if FIELD_SHADERS.contains(&n) => {
                    (self.field.slot(n), field_format(n), Msaa::Off, None)
                }
                _ => continue,
            };
            match build(ctx, res, shaders, name, format, msaa, stencil) {
                Ok(new) => {
                    let old = std::mem::replace(slot, new);
                    // SAFETY: видеокарта простаивает (условие функции).
                    unsafe { ctx.device.destroy_pipeline(old, None) };
                }
                Err(e) => shaders.set_error(name, e.to_string()),
            }
        }
    }

    /// Обновляет сцену перед записью кадра. Каждые 2 секунды загружает новую шахматку
    /// (другой размер клеток), а старую отдаёт в очередь отложенного удаления: так в
    /// песочнице всё время работают загрузка текстур и безопасное удаление.
    pub fn update(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        time: f32,
    ) -> Result<(), GpuError> {
        let phase = (time / 2.0) as u32 % CHECKER_CELLS.len() as u32;
        if phase == self.checker_phase {
            return Ok(());
        }
        let cell = CHECKER_CELLS[phase as usize];
        let new =
            res.create_texture_rgba8(ctx, 128, 128, &checker_pixels(128, cell), true, "шахматка")?;
        res.destroy_texture(self.checker);
        self.checker = new;
        self.checker_phase = phase;
        Ok(())
    }

    /// Спрайты сцены в момент `time` (секунды). `shader_error`: обвести окно красной рамкой.
    /// В векторной сцене спрайты нужны только для рамки.
    pub fn sprites_at(
        &self,
        scene: DemoScene,
        time: f32,
        aspect: f32,
        shader_error: bool,
    ) -> Vec<Sprite> {
        let mut out = Vec::with_capacity(6 + ORBITERS);
        if scene == DemoScene::Gpu {
            self.stage2_sprites(time, &mut out);
        }
        if shader_error {
            error_frame(aspect, self.white, &mut out);
        }
        out
    }

    fn stage2_sprites(&self, time: f32, out: &mut Vec<Sprite>) {
        out.push(Sprite {
            pos: [-1.15, 0.0],
            size: [0.7, 0.7],
            tint: [1.0, 1.0, 1.0, 1.0],
            rotation: -time * 0.3,
            texture: self.checker.0,
            sampler: SamplerKind::Nearest as u32,
            pad: 0,
        });
        let pulse = 0.85 + 0.15 * (time * 2.0).sin();
        out.push(Sprite {
            pos: [1.15, 0.0],
            size: [0.7 * pulse, 0.7 * pulse],
            tint: [1.0, 0.75, 0.2, 1.0],
            rotation: 0.0,
            texture: self.disk.0,
            sampler: SamplerKind::Linear as u32,
            pad: 0,
        });
        for i in 0..ORBITERS {
            let a = time * 0.8 + i as f32 * std::f32::consts::TAU / ORBITERS as f32;
            let hue = i as f32 / ORBITERS as f32;
            let [r, g, b] = hue_to_rgb(hue);
            out.push(Sprite {
                pos: [a.cos() * 0.8, a.sin() * 0.8],
                size: [0.12, 0.12],
                tint: [r, g, b, 0.9],
                rotation: 0.0,
                texture: self.disk.0,
                sampler: SamplerKind::Linear as u32,
                pad: 0,
            });
        }
    }

    /// Список векторных фигур кадра сцены `scene` в момент `time`.
    fn scene_items(
        &mut self,
        scene: DemoScene,
        time: f32,
        aspect: f32,
    ) -> Vec<reyvec_vector::SceneItem> {
        match scene {
            DemoScene::Morph => self
                .vector_demo
                .morph_instances(time)
                .into_iter()
                .map(Into::into)
                .collect(),
            DemoScene::Layers => self.vector_demo.layer_items(time),
            DemoScene::Masks => self.vector_demo.mask_items(time),
            DemoScene::Svg => self.vector_demo.svg_items(aspect),
            DemoScene::Motif => self.vector_demo.motif_items(time, aspect),
            DemoScene::Stream => self.vector_demo.stream_items(time, aspect),
            DemoScene::Text => self.vector_demo.text_items(time, aspect),
            DemoScene::Light | DemoScene::Lamps | DemoScene::Blobs => {
                self.vector_demo.light_items(time)
            }
            DemoScene::Lab => self.vector_demo.lab.items(time),
            DemoScene::File => {
                let mut items = self.file_scene.items.clone();
                items.extend(self.loading.picture_items());
                items
            }
            DemoScene::Stress | DemoScene::StressMorph => self
                .vector_demo
                .stress_instances(time, aspect, scene == DemoScene::StressMorph)
                .into_iter()
                .map(Into::into)
                .collect(),
            _ => self
                .vector_demo
                .instances(time)
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }

    /// Строит уровни детализации, которых не хватило последнему кадру, пока не потрачено
    /// `budget_ms` (хотя бы один). Построенные уровни загрузит следующий кадр; до тех пор
    /// фигура рисуется ближайшим готовым уровнем.
    fn build_details(&mut self, budget_ms: f64) {
        let requests = self.vector.detail_requests().to_vec();
        if requests.is_empty()
            || self
                .loading
                .submit_details(&self.vector_demo.library, &requests)
        {
            return;
        }
        let _zone = crate::cpu_zone!("уровни детализации");
        let started = std::time::Instant::now();
        for (id, level) in requests {
            if let Err(e) = self.vector_demo.library.build_detail(id, level) {
                tracing::warn!("уровень детализации {level} фигуры {}: {e}", id.0);
            }
            if started.elapsed().as_secs_f64() * 1000.0 > budget_ms {
                break;
            }
        }
    }

    /// Для картинок тестов: строит и загружает все уровни детализации, нужные кадру
    /// сцены `scene` в момент `time`, до первого кадра (в окне они строятся по ходу).
    pub fn settle_details(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        extent: vk::Extent2D,
        time: f32,
        scene: DemoScene,
    ) -> Result<(), GpuError> {
        let aspect = extent.width as f32 / extent.height.max(1) as f32;
        if scene == DemoScene::File && self.play.is_some() {
            // По сценарию: детали нужны там, где герой будет в момент кадра.
            self.advance_play(time);
            self.follow_player(time);
        }
        let items = self.scene_items(scene, time, aspect);
        self.vector.upload(ctx, res, &self.vector_demo.library)?;
        let requests = self.vector.plan_details(&items, extent, self.camera.view());
        for (id, level) in requests {
            if let Err(e) = self.vector_demo.library.build_detail(id, level) {
                tracing::warn!("уровень детализации {level} фигуры {}: {e}", id.0);
            }
        }
        self.vector.upload(ctx, res, &self.vector_demo.library)
    }

    /// Готовит данные кадра: пишет спрайты в буфер `slot` (номер кадра в работе).
    /// Буфер `slot` видеокарта в этот момент читать не должна.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        slot: usize,
        extent: vk::Extent2D,
        time: f32,
        shader_error: bool,
        scene: DemoScene,
    ) -> FrameData {
        let aspect = extent.width as f32 / extent.height.max(1) as f32;
        if self.loading.is_background() {
            let _zone = crate::cpu_zone!("готовое из загрузки");
            self.poll_loading();
        }
        if scene == DemoScene::File {
            if let Some(logic) = &self.file_logic {
                self.file_scene = logic.frame(std::time::Instant::now());
                // Первый кадр со снимком, где логика уже взяла нажатие: замер задержки.
                let stamp = self.file_scene.input_stamp;
                if stamp != 0 && stamp != self.seen_press {
                    self.seen_press = stamp;
                    let ms = logic.press_age_ms(stamp);
                    self.press_to_frame_ms = Some(
                        self.press_to_frame_ms
                            .map_or(ms, |old| old * 0.8 + ms * 0.2),
                    );
                }
            } else if self.play.is_some() {
                self.advance_play(time);
            } else if let Some(world) = self.file_world.as_mut() {
                world.update_at(f64::from(time));
                self.file_scene = world.snapshot();
            }
            self.follow_player(time);
        }
        // 2.5D-вид (7.9): мир уровня обновлён как обычно, а рисуется он объёмом — дальше
        // кадр готовится как кадр сцены «3D».
        let level = scene == DemoScene::File && self.level_view;
        let scene = if level { DemoScene::Space } else { scene };
        self.prepare_particles(res, slot, time, scene, aspect);
        let vector = match scene {
            DemoScene::Gpu | DemoScene::Space => {
                self.vector_items = 0;
                None
            }
            _ => {
                let _zone = crate::cpu_zone!("векторные фигуры");
                let started = std::time::Instant::now();
                let items = self.scene_items(scene, time, aspect);
                let built = std::time::Instant::now();
                // Фигуры, добавленные с прошлого кадра, едут на видеокарту в этом же
                // кадре (копирование в начале кадра, без ожидания).
                self.uploads = match self
                    .vector
                    .stream(ctx, res, slot, &self.vector_demo.library)
                {
                    Ok(u) => u,
                    Err(e) => {
                        tracing::error!("загрузка новых векторных фигур: {e}");
                        Uploads::default()
                    }
                };
                if !self.uploads.is_empty() {
                    tracing::debug!(
                        "внутри кадра загружено фигур: {} ({} КБ), всего на видеокарте {}",
                        self.uploads.shapes,
                        self.uploads.bytes / 1024,
                        self.vector.uploaded_shapes()
                    );
                }
                let data = self
                    .vector
                    .prepare(res, slot, &items, extent, self.camera.view());
                self.build_details(DETAIL_BUDGET_MS);
                let scene_ms = (built - started).as_secs_f64() * 1000.0;
                let engine_ms = built.elapsed().as_secs_f64() * 1000.0;
                // Сглаживание: число на панели не прыгает от кадра к кадру.
                let smooth = |old: f64, new: f64| {
                    if self.vector_items == 0 {
                        new
                    } else {
                        old * 0.9 + new * 0.1
                    }
                };
                self.vector_cpu.scene_ms = smooth(self.vector_cpu.scene_ms, scene_ms);
                self.vector_cpu.engine_ms = smooth(self.vector_cpu.engine_ms, engine_ms);
                self.vector_items = items.len();
                self.vector_drawn = self.vector.drawn() as usize;
                data
            }
        };
        let sprites = self.sprites_at(scene, time, aspect, shader_error);
        let count = sprites.len().min(MAX_SPRITES);
        let bytes: Vec<u8> = sprites[..count].iter().flat_map(|s| s.to_bytes()).collect();
        let buffer = self.instance_buffers[slot];
        res.write_buffer(buffer, 0, &bytes);
        let lights = match scene {
            DemoScene::Lamps => crate::lights::demo_lights(time),
            DemoScene::Blobs => crate::blobs::demo_lights(time),
            DemoScene::Lab => crate::lab::lights(time),
            DemoScene::File => self.file_scene.lights.iter().map(Into::into).collect(),
            _ => Vec::new(),
        };
        // Отсечение (5.5в): в кадр только свет и объекты, которые что-то меняют на экране.
        let view = crate::cull::ViewRect::new(
            [self.camera.center.x as f32, self.camera.center.y as f32],
            self.camera.zoom as f32,
            aspect,
        );
        let mut cull = crate::cull::CullStats::default();
        let lights = crate::cull::lights(lights, &view, &mut cull);
        let light_count = lights.len().min(MAX_LIGHTS);
        let light_bytes: Vec<u8> = lights[..light_count]
            .iter()
            .flat_map(|l| l.to_bytes())
            .collect();
        let light_buffer = self.light_buffers[slot];
        res.write_buffer(light_buffer, 0, &light_bytes);
        let objects = match scene {
            DemoScene::Blobs => crate::blobs::demo_objects(time),
            DemoScene::Lab => crate::lab::objects(time),
            DemoScene::File => self.file_scene.objects.iter().map(Into::into).collect(),
            _ => Vec::new(),
        };
        let objects = crate::cull::objects(objects, &view, &mut cull);
        self.cull = cull;
        let (object_bytes, blob_bytes, object_count) = crate::blobs::pack(&objects);
        let (object_buffer, blob_buffer) = (self.object_buffers[slot], self.blob_buffers[slot]);
        res.write_buffer(object_buffer, 0, &object_bytes);
        res.write_buffer(blob_buffer, 0, &blob_bytes);
        let space_buffer = self.space_buffers[slot];
        let mut space_vertices = 0;
        let mut space_shadow_maps = 0;
        let space_compute =
            (level || self.space.compute) && self.space_march_compute != vk::Pipeline::null();
        if scene == DemoScene::Space {
            self.space_compute_running = space_compute;
        }
        let space_edges = if scene == DemoScene::Space
            && self.space_smooth
            && self.space_edges != vk::Pipeline::null()
        {
            self.edge_list(ctx, res, slot, self.space_scale.apply(extent))
        } else {
            None
        };
        let mut space_field = None;
        let mut space_camera = self.space_camera;
        if scene == DemoScene::Space {
            // Счётчики для панели (7.18б): число краёв — из прошлого кадра этой ячейки
            // (он закончен: его забор дождались перед подготовкой), времена — отсюда.
            let edge_pixels = space_edges.and_then(|_| self.edge_pixels(res, slot));
            let started = std::time::Instant::now();
            let (mut frame, level_materials) = if level {
                let _zone = crate::cpu_zone!("уровень в объёме");
                let center = [self.camera.center.x, self.camera.center.y];
                space_camera = crate::level3d::camera(center, self.camera.zoom, self.level_orbit);
                let made = self.level.frame(
                    &self.file_scene,
                    &self.vector_demo.library,
                    self.level_render,
                );
                let mut frame = made.frame;
                frame.lamps =
                    crate::level3d::lamps(&self.file_scene, [center[0] as f32, center[1] as f32]);
                (frame, Some(made.materials))
            } else {
                (self.space.frame(time), None)
            };
            if level {
                self.field_shapes = (0, 0);
            } else {
                self.swap_extrudes(ctx, res, &mut frame);
            }
            let built = std::time::Instant::now();
            // Поле расстояний — только у сцены «3D»: у уровня почти всё движется вместе
            // с камерой и героем, пересчёт стоил бы дороже, чем экономит (7.12).
            if level {
                self.space_field.skip();
            } else if self.space_field_bake != vk::Pipeline::null() {
                // Поле — по всем предметам, до отсева по взгляду.
                let programs = crate::space::field_programs(&frame);
                space_field = self
                    .space_field
                    .prepare(ctx, res, slot, space_camera.eye, programs);
            }
            let view_proj = space_camera.view_proj(aspect);
            self.space_view_culled = level || !self.space.view_culling_off;
            if self.space_view_culled {
                // У уровня солнца нет: его заслонки не держим, иначе в кадр комнаты
                // тянулся бы коридор (разбор 2026-11-07).
                self.space_view = reyvec_sdf::view3d::cull_frame(&mut frame, view_proj, !level);
            }
            let materials = level_materials.as_deref().unwrap_or(&self.space.materials);
            let culling = level || !self.space.culling_off;
            self.space_culled = culling;
            let mut packed = crate::space::pack(&frame, materials, culling);
            self.space_grid = packed.grid;
            crate::space::set_view_proj(&mut packed.words, view_proj);
            crate::space::set_field(
                &mut packed.words,
                space_field.map(|f| (f.field, &self.space_field.window)),
                self.space_field.ao_from_field,
            );
            space_vertices = packed.vertices;
            if level {
                crate::space::set_no_sun(&mut packed.words);
                // Треугольники (7.9б): карты теней двух первых ламп, если есть чем рисовать.
                if space_vertices > 0 && self.space_mesh_shadow != vk::Pipeline::null() {
                    let maps: Vec<glam::Mat4> = frame
                        .lamps
                        .iter()
                        .take(crate::space::MAX_SHADOW_MAPS)
                        .map(crate::level3d::lamp_view_proj)
                        .collect();
                    crate::space::set_shadow_maps(&mut packed.words, &maps);
                    space_shadow_maps = maps.len() as u32;
                }
            }
            let bytes: Vec<u8> = packed.words.iter().flat_map(|w| w.to_ne_bytes()).collect();
            res.write_buffer(space_buffer, 0, &bytes);
            let build_ms = (built - started).as_secs_f64() * 1000.0;
            let pack_ms = built.elapsed().as_secs_f64() * 1000.0;
            // Сглаживание, как у векторного кадра: числа на панели не прыгают.
            let smooth = |old: Option<f64>, new: f64| old.map_or(new, |o| o * 0.9 + new * 0.1);
            let prev = self.space_stats;
            let space_extent = self.space_scale.apply(extent);
            self.space_stats = Some(SpaceStats {
                build_ms: smooth(prev.map(|p| p.build_ms), build_ms),
                pack_ms: smooth(prev.map(|p| p.pack_ms), pack_ms),
                objects: packed.objects,
                words: packed.words.len(),
                dropped: packed.dropped,
                overflow: packed.overflow,
                edge_pixels,
                pixels: space_extent.width * space_extent.height,
            });
        } else {
            self.space_field.skip();
            self.space_stats = None;
        }
        FrameData {
            scene,
            msaa: self.msaa,
            perceptual_edges: self.perceptual_edges,
            vector,
            sprites_address: res.buffer_address(buffer),
            sprite_count: count as u32,
            space_address: res.buffer_address(space_buffer),
            space_vertices,
            space_shadow_maps,
            space_compute,
            space_edges,
            space_field,
            lights_address: res.buffer_address(light_buffer),
            light_count: light_count as u32,
            objects_address: res.buffer_address(object_buffer),
            blobs_address: res.buffer_address(blob_buffer),
            object_count,
            ambient: if scene == DemoScene::File {
                self.file_scene.ambient
            } else {
                crate::lights::AMBIENT
            },
            lab_backdrop: scene == DemoScene::Lab
                || (scene == DemoScene::File
                    && self.file_scene.background == reyvec_scene::Background::Lab),
            time,
            aspect,
            extent,
            space_extent: if scene == DemoScene::Space {
                self.space_scale.apply(extent)
            } else {
                extent
            },
            space_camera,
        }
    }

    /// Добавляет в граф проходы сцены: «сцена» рисует фон, треугольник и спрайты во
    /// временную картинку, «вывод» копирует её в `target`.
    pub fn add_passes<'a>(
        &'a self,
        graph: &mut RenderGraph<'a>,
        bindless: &'a Bindless,
        frame: FrameData,
        target: ImageHandle,
    ) {
        // Новые фигуры попадают на видеокарту первым делом (своим проходом, чтобы его
        // время было видно на панели).
        if !self.uploads.is_empty() {
            graph.add_pass("загрузка фигур", &[], move |p| {
                // SAFETY: проход вне рисования; буферы живы, пока выполняется кадр
                // (старые удаляются отложенно).
                unsafe { VectorRenderer::record_uploads(p.device, p.cmd, &self.uploads) };
            });
        }
        let scene = graph.create("кадр сцены", frame.extent, self.scene_format);
        // Со сглаживанием сцена рисуется в картинку с несколькими точками на пиксель. Точки
        // усредняет в обычный кадр сцены сама видеокарта в конце прохода «сцена» (быстро:
        // картинка MSAA не пишется в память). Для краёв «как в редакторах» усредняет свой
        // проход «сведение MSAA» (resolve.slang): так дороже, особенно на встроенной видеокарте.
        let msaa = (frame.msaa != Msaa::Off).then(|| {
            graph.create_multisampled(
                "кадр сцены MSAA",
                frame.extent,
                self.scene_format,
                frame.msaa.samples(),
            )
        });
        let layout = bindless.pipeline_layout;
        let target_image = msaa.unwrap_or(scene);
        let own_resolve = msaa.filter(|_| frame.perceptual_edges);
        let hardware_resolve = msaa.filter(|_| !frame.perceptual_edges);
        // Трафарет нужен заливке через трафарет. Он есть у прохода всегда, чтобы все
        // конвейеры сцены были собраны под один и тот же проход.
        let stencil = graph.create_multisampled(
            "трафарет сцены",
            frame.extent,
            self.stencil_format,
            frame.msaa.samples(),
        );
        // Слои векторных фигур: каждый рисуется своим проходом в свою картинку
        // (прозрачный фон, то же сглаживание и тот же трафарет), а потом накладывается
        // в проходе, где он стоит. Вложенные слои идут раньше внешних. Картинка слоя
        // размером с прямоугольник, который занимают его фигуры (а не с кадр): её
        // очистка и сведение MSAA стоят пропорционально площади, а на встроенной
        // видеокарте это главная цена слоя.
        let mut layers: Vec<LayerImage> = Vec::new();
        if let Some(vector) = frame.vector {
            for &layer in self.vector.layer_order(&vector) {
                let rect = self.vector.layer_rect(&vector, layer, frame.extent);
                let size = layer_image_size(rect.extent, frame.extent);
                let image = graph.create("слой", size, LAYER_FORMAT);
                let ms = (frame.msaa != Msaa::Off).then(|| {
                    graph.create_multisampled("слой MSAA", size, LAYER_FORMAT, frame.msaa.samples())
                });
                let mut uses = vec![
                    (ms.unwrap_or(image), Access::ColorAttachment),
                    (stencil, Access::StencilAttachment),
                ];
                if ms.is_some() {
                    uses.push((image, Access::ColorAttachment));
                }
                let children = layer_images(
                    &layers,
                    &self.vector.layer_children(&vector, layer as usize),
                );
                uses.extend(children.iter().map(|l| (l.image, Access::Sampled)));
                graph.add_pass("слой", &uses, move |p| {
                    let textures = layer_textures(p, &children);
                    let (d, cmd) = (p.device, p.cmd);
                    // SAFETY: граф перевёл картинки слоя в раскладку цели рисования, а
                    // вложенные слои в чтение; буферы кадра живы, пока он рисуется.
                    unsafe {
                        begin_scene_pass(
                            d,
                            cmd,
                            p.view(ms.unwrap_or(image)),
                            ms.map(|_| p.view(image)),
                            Some(p.view(stencil)),
                            frame.extent,
                            rect.offset,
                            Some(vk::Rect2D::default().extent(rect.extent)),
                            [0.0; 4],
                        );
                        bindless.bind(d, cmd);
                        self.vector.record(
                            d,
                            cmd,
                            layout,
                            vector,
                            layer as usize,
                            &textures,
                            rect,
                            rect.offset,
                        );
                        d.cmd_end_rendering(cmd);
                    }
                });
                layers.push(LayerImage { layer, image, rect });
            }
        }
        let scene_children = match frame.vector {
            Some(vector) => layer_images(&layers, &self.vector.layer_children(&vector, 0)),
            None => Vec::new(),
        };

        // Слои, фигуры которых отбрасывают тень: их силуэты читает маска теней.
        let shadow_images = match frame.vector {
            Some(vector) => layer_images(&layers, self.vector.shadow_layers(&vector)),
            None => Vec::new(),
        };
        let field = frame
            .scene
            .uses_field()
            .then(|| self.add_field_passes(graph, bindless, frame, shadow_images));
        let light = field.filter(|_| frame.scene.uses_lights()).map(|f| {
            if self.cascades {
                self.add_cascade_passes(graph, bindless, frame, f)
            } else {
                self.add_light_pass(graph, bindless, frame, f)
            }
        });

        // Частицы (6.6): расчёт до прохода «сцена» (по шагу на каждый шаг логики), рисование
        // в нём поверх фигур.
        let center = [self.camera.center.x as f32, self.camera.center.y as f32];
        let zoom = self.camera.zoom as f32;
        let particle_draw = |glow: bool| {
            self.particle_steps
                .last()
                .filter(|f| f.span.count > 0)
                .map(|f| {
                    (
                        crate::particles::push(f, center, frame.aspect, zoom, glow),
                        self.particles.draw_buffer,
                    )
                })
        };
        let particle_glow = particle_draw(true);
        let particle_draw = particle_draw(false);
        if self.particle_steps.iter().any(|f| f.span.count > 0) {
            graph.add_pass("частицы", &[], move |p| {
                for f in &self.particle_steps {
                    let push = crate::particles::push(f, center, frame.aspect, zoom, false);
                    // SAFETY: проход вне рисования; кольцо и буфер загрузки кадра живы,
                    // пока видеокарта выполняет кадр.
                    unsafe {
                        crate::particles::record_update(
                            p.device,
                            p.cmd,
                            layout,
                            self.particle_update,
                            &push,
                            f.span.count,
                            f.build_list.then_some(self.particles.draw_buffer),
                        );
                    }
                }
            });
        }

        // Сцена «3D» (7.3): лучи шагают своим проходом в буфер геометрии (по точке на
        // пиксель и без MSAA: шагание — самое дорогое), а «сцена» его освещает.
        let gbuffer = (frame.scene == DemoScene::Space).then(|| {
            let camera = frame.space_camera;
            let ext = frame.space_extent;
            // Запечённое поле расстояний (7.10б): кубики у сдвинутых предметов и вошедшие
            // в слои при полёте камеры — до всех лучей кадра.
            if let Some(field) = frame.space_field {
                let bake = self.space_field_bake;
                let copies = self.space_field.copies.clone();
                graph.add_pass("поле", &[], move |p| {
                    // SAFETY: проход вне рисования; конвейер жив, пока жива сцена; буферы
                    // поля и задания кадра живы, пока кадр выполняется.
                    unsafe {
                        crate::field::SpaceField::record(
                            p.device, p.cmd, bindless, layout, bake, &field, &copies,
                        );
                    }
                });
            }
            // Предварительное шагание конусами (7.8): точка на квадрат 8×8 пикселей.
            let tiles = vk::Extent2D {
                width: ext.width.div_ceil(CONE_TILE),
                height: ext.height.div_ceil(CONE_TILE),
            };
            let cone = graph.create("конусы 3D", tiles, CONE_FORMAT);
            graph.add_pass(
                "лучи 3D грубо",
                &[(cone, Access::ColorAttachment)],
                move |p| {
                    let push = space_push(
                        &camera,
                        ext,
                        frame.time,
                        frame.space_address,
                        SpaceSlots::default(),
                        0,
                    );
                    let (d, cmd) = (p.device, p.cmd);
                    // SAFETY: граф перевёл картинку конусов в цель рисования; буфер сцены
                    // этого кадра жив, пока кадр рисуется.
                    unsafe {
                        begin_color_pass(d, cmd, p.view(cone), tiles);
                        bindless.bind(d, cmd);
                        d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.space_cone);
                        d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                        d.cmd_draw(cmd, 3, 1, 0, 0);
                        d.cmd_end_rendering(cmd);
                    }
                },
            );
            // Полигональные модели (7.6ж): растеризация с глубиной в свой буфер геометрии
            // той же раскладки. «Реймарчинг» не шагает дальше них и берёт их точку, если
            // луч ни во что не попал раньше.
            let meshes = (frame.space_vertices > 0).then(|| {
                let meshes = graph.create("полигоны 3D", ext, GBUFFER_FORMAT);
                let depth = graph.create("глубина полигонов 3D", ext, MESH_DEPTH_FORMAT);
                graph.add_pass(
                    "полигоны 3D",
                    &[
                        (meshes, Access::ColorAttachment),
                        (depth, Access::StencilAttachment),
                    ],
                    move |p| {
                        let push = space_push(
                            &camera,
                            ext,
                            frame.time,
                            frame.space_address,
                            SpaceSlots::default(),
                            0,
                        );
                        let (d, cmd) = (p.device, p.cmd);
                        // SAFETY: граф перевёл картинки в цели рисования; буфер сцены
                        // этого кадра жив, пока кадр рисуется.
                        unsafe {
                            begin_depth_pass(
                                d,
                                cmd,
                                p.view(meshes),
                                p.view(depth),
                                ext,
                                [-1.0, 0.0, 0.0, 0.0],
                            );
                            bindless.bind(d, cmd);
                            d.cmd_bind_pipeline(
                                cmd,
                                vk::PipelineBindPoint::GRAPHICS,
                                self.space_mesh,
                            );
                            d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                            d.cmd_draw(cmd, frame.space_vertices, 1, 0, 0);
                            d.cmd_end_rendering(cmd);
                        }
                    },
                );
                meshes
            });
            let gbuffer = graph.create("буфер геометрии 3D", ext, GBUFFER_FORMAT);
            if frame.space_compute {
                // Опыт 7.6м: те же лучи вычислительным шейдером, прямо в картинку буфера
                // геометрии (её читают тени и освещение как обычно).
                let mut uses = vec![
                    (cone, Access::ComputeSampled),
                    (gbuffer, Access::ComputeWrite),
                ];
                if let Some(m) = meshes {
                    uses.push((m, Access::ComputeSampled));
                }
                graph.add_pass("реймарчинг", &uses, move |p| {
                    let push = space_push(
                        &camera,
                        ext,
                        frame.time,
                        frame.space_address,
                        SpaceSlots {
                            target: p.storage(gbuffer),
                            cone: p.texture(cone),
                            meshes: meshes.map_or(0, |m| p.texture(m)),
                            ..SpaceSlots::default()
                        },
                        u32::from(meshes.is_some()) * SPACE_FLAG_MESHES,
                    );
                    let (d, cmd) = (p.device, p.cmd);
                    // SAFETY: граф перевёл конусы (и полигоны) в чтение, буфер геометрии в
                    // запись; буфер сцены этого кадра жив, пока кадр рисуется.
                    unsafe {
                        bindless.bind_compute(d, cmd);
                        d.cmd_bind_pipeline(
                            cmd,
                            vk::PipelineBindPoint::COMPUTE,
                            self.space_march_compute,
                        );
                        d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                        d.cmd_dispatch(cmd, ext.width.div_ceil(8), ext.height.div_ceil(8), 1);
                    }
                });
            } else {
                let mut march_uses =
                    vec![(cone, Access::Sampled), (gbuffer, Access::ColorAttachment)];
                if let Some(m) = meshes {
                    march_uses.push((m, Access::Sampled));
                }
                graph.add_pass("реймарчинг", &march_uses, move |p| {
                    let push = space_push(
                        &camera,
                        ext,
                        frame.time,
                        frame.space_address,
                        SpaceSlots {
                            cone: p.texture(cone),
                            meshes: meshes.map_or(0, |m| p.texture(m)),
                            ..SpaceSlots::default()
                        },
                        u32::from(meshes.is_some()) * SPACE_FLAG_MESHES,
                    );
                    let (d, cmd) = (p.device, p.cmd);
                    // SAFETY: граф перевёл буфер геометрии в цель рисования; буфер сцены
                    // этого кадра жив, пока кадр рисуется.
                    unsafe {
                        begin_color_pass(d, cmd, p.view(gbuffer), ext);
                        bindless.bind(d, cmd);
                        d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.space_march);
                        d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                        d.cmd_draw(cmd, 3, 1, 0, 0);
                        d.cmd_end_rendering(cmd);
                    }
                });
            }
            // Тени и затенение в углах (7.4): мягкие, поэтому в половинном разрешении —
            // точка на квадрат 2×2 пикселя, вчетверо меньше лучей (7.6б). «Сцена»
            // растягивает их обратно с оглядкой на глубину и нормаль.
            let half = vk::Extent2D {
                width: ext.width.div_ceil(2),
                height: ext.height.div_ceil(2),
            };
            // Карты теней ламп от треугольников (7.9б): квадраты рядом в одной картинке.
            let shadow_maps = (frame.space_shadow_maps > 0).then(|| {
                let count = frame.space_shadow_maps;
                let size = vk::Extent2D {
                    width: SHADOW_MAP_SIZE * count,
                    height: SHADOW_MAP_SIZE,
                };
                let maps = graph.create("карты теней ламп", size, SHADOW_MAP_FORMAT);
                let depth = graph.create("глубина карт теней", size, MESH_DEPTH_FORMAT);
                graph.add_pass(
                    "карты теней ламп",
                    &[
                        (maps, Access::ColorAttachment),
                        (depth, Access::StencilAttachment),
                    ],
                    move |p| {
                        let push = space_push(
                            &camera,
                            ext,
                            frame.time,
                            frame.space_address,
                            SpaceSlots::default(),
                            0,
                        );
                        let (d, cmd) = (p.device, p.cmd);
                        // SAFETY: граф перевёл картинки в цели рисования; буфер сцены
                        // этого кадра жив, пока кадр рисуется.
                        unsafe {
                            begin_depth_pass(
                                d,
                                cmd,
                                p.view(maps),
                                p.view(depth),
                                size,
                                [1e9, 0.0, 0.0, 0.0],
                            );
                            bindless.bind(d, cmd);
                            d.cmd_bind_pipeline(
                                cmd,
                                vk::PipelineBindPoint::GRAPHICS,
                                self.space_mesh_shadow,
                            );
                            d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                            for k in 0..count {
                                let viewport = vk::Viewport::default()
                                    .x((k * SHADOW_MAP_SIZE) as f32)
                                    .width(SHADOW_MAP_SIZE as f32)
                                    .height(SHADOW_MAP_SIZE as f32)
                                    .max_depth(1.0);
                                d.cmd_set_viewport(cmd, 0, &[viewport]);
                                d.cmd_draw(cmd, frame.space_vertices, 1, 0, k);
                            }
                            d.cmd_end_rendering(cmd);
                        }
                    },
                );
                maps
            });
            // Флаг и номер картинки карт теней для «теней 3D» и «краёв 3D».
            let map_flags = move |p: &PassContext| {
                shadow_maps.map_or(0, |m| SPACE_FLAG_SHADOW_MAP | (p.texture(m) << 16))
            };
            let shadows = graph.create("тени 3D", half, SHADOW3D_FORMAT);
            let mut uses = vec![
                (gbuffer, Access::Sampled),
                (shadows, Access::ColorAttachment),
            ];
            if let Some(m) = shadow_maps {
                uses.push((m, Access::Sampled));
            }
            graph.add_pass("тени 3D", &uses, move |p| {
                let push = space_push(
                    &camera,
                    ext,
                    frame.time,
                    frame.space_address,
                    SpaceSlots {
                        gbuffer: p.texture(gbuffer),
                        ..SpaceSlots::default()
                    },
                    map_flags(p),
                );
                let (d, cmd) = (p.device, p.cmd);
                // SAFETY: граф перевёл буфер геометрии в чтение, а тени в цель
                // рисования; буфер сцены этого кадра жив, пока кадр рисуется.
                unsafe {
                    begin_color_pass(d, cmd, p.view(shadows), half);
                    bindless.bind(d, cmd);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.space_shadow);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                    d.cmd_end_rendering(cmd);
                }
            });
            // Сглаживание краёв форм (7.6о): поиск пикселей краёв, потом по четыре луча
            // внутри каждого. «Сцена» берёт готовый цвет краёв из картинки краёв.
            let edges = frame.space_edges.map(|(list, list_buffer, count_buffer)| {
                let pairs = if ext == frame.extent {
                    0
                } else {
                    SPACE_FLAG_EDGE_PAIRS
                };
                let edges = graph.create("края 3D", ext, EDGE_FORMAT);
                graph.add_pass(
                    "края 3D: поиск",
                    &[
                        (gbuffer, Access::ComputeSampled),
                        (edges, Access::ComputeWrite),
                    ],
                    move |p| {
                        let push = space_push(
                            &camera,
                            ext,
                            frame.time,
                            frame.space_address,
                            SpaceSlots {
                                gbuffer: p.texture(gbuffer),
                                target: p.storage(edges),
                                edges: list,
                                ..SpaceSlots::default()
                            },
                            pairs,
                        );
                        // Начало списка: 0 групп, размеры запуска 1 и 1, 0 пикселей.
                        let start: Vec<u8> = [0u32, 1, 1, 0]
                            .iter()
                            .flat_map(|w| w.to_ne_bytes())
                            .collect();
                        // Список этого кадра в прошлый раз читали проход «края 3D» и копия
                        // счётчика для процессора.
                        let before_reset = [vk::MemoryBarrier2::default()
                            .src_stage_mask(
                                vk::PipelineStageFlags2::COMPUTE_SHADER
                                    | vk::PipelineStageFlags2::DRAW_INDIRECT
                                    | vk::PipelineStageFlags2::COPY,
                            )
                            .src_access_mask(
                                vk::AccessFlags2::SHADER_STORAGE_READ
                                    | vk::AccessFlags2::INDIRECT_COMMAND_READ
                                    | vk::AccessFlags2::TRANSFER_READ,
                            )
                            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)];
                        let after_reset = [vk::MemoryBarrier2::default()
                            .src_stage_mask(vk::PipelineStageFlags2::COPY)
                            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                            .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                            .dst_access_mask(
                                vk::AccessFlags2::SHADER_STORAGE_READ
                                    | vk::AccessFlags2::SHADER_STORAGE_WRITE,
                            )];
                        let after = [vk::MemoryBarrier2::default()
                            .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                            .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
                            .dst_stage_mask(
                                vk::PipelineStageFlags2::COMPUTE_SHADER
                                    | vk::PipelineStageFlags2::DRAW_INDIRECT
                                    | vk::PipelineStageFlags2::COPY,
                            )
                            .dst_access_mask(
                                vk::AccessFlags2::SHADER_STORAGE_READ
                                    | vk::AccessFlags2::INDIRECT_COMMAND_READ
                                    | vk::AccessFlags2::TRANSFER_READ,
                            )];
                        // Начало списка (со счётчиком пикселей) — процессору, для панели
                        // (7.18б); читается, когда кадр закончен.
                        let count_copy = [vk::BufferCopy::default().size(16)];
                        let to_host = [vk::BufferMemoryBarrier2::default()
                            .buffer(count_buffer)
                            .size(vk::WHOLE_SIZE)
                            .src_stage_mask(vk::PipelineStageFlags2::COPY)
                            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                            .dst_access_mask(vk::AccessFlags2::HOST_READ)
                            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)];
                        let (d, cmd) = (p.device, p.cmd);
                        // SAFETY: граф перевёл буфер геометрии в чтение, а картинку краёв в
                        // запись; список краёв и буфер сцены этого кадра живы, пока кадр
                        // рисуется; барьеры выше упорядочивают список с прошлым кадром и
                        // со следующим проходом.
                        unsafe {
                            d.cmd_pipeline_barrier2(
                                cmd,
                                &vk::DependencyInfo::default().memory_barriers(&before_reset),
                            );
                            d.cmd_update_buffer(cmd, list_buffer, 0, &start);
                            d.cmd_pipeline_barrier2(
                                cmd,
                                &vk::DependencyInfo::default().memory_barriers(&after_reset),
                            );
                            bindless.bind_compute(d, cmd);
                            d.cmd_bind_pipeline(
                                cmd,
                                vk::PipelineBindPoint::COMPUTE,
                                self.space_edge_find,
                            );
                            d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                            d.cmd_dispatch(cmd, ext.width.div_ceil(8), ext.height.div_ceil(8), 1);
                            d.cmd_pipeline_barrier2(
                                cmd,
                                &vk::DependencyInfo::default().memory_barriers(&after),
                            );
                            d.cmd_copy_buffer(cmd, list_buffer, count_buffer, &count_copy);
                            d.cmd_pipeline_barrier2(
                                cmd,
                                &vk::DependencyInfo::default().buffer_memory_barriers(&to_host),
                            );
                        }
                    },
                );
                let mut uses = vec![
                    (cone, Access::ComputeSampled),
                    (gbuffer, Access::ComputeSampled),
                    (shadows, Access::ComputeSampled),
                    (edges, Access::ComputeWrite),
                ];
                if let Some(m) = meshes {
                    uses.push((m, Access::ComputeSampled));
                }
                if let Some(m) = shadow_maps {
                    uses.push((m, Access::ComputeSampled));
                }
                graph.add_pass("края 3D", &uses, move |p| {
                    let push = space_push(
                        &camera,
                        ext,
                        frame.time,
                        frame.space_address,
                        SpaceSlots {
                            gbuffer: p.texture(gbuffer),
                            shadows: p.texture(shadows),
                            target: p.storage(edges),
                            cone: p.texture(cone),
                            meshes: meshes.map_or(0, |m| p.texture(m)),
                            edges: list,
                        },
                        (u32::from(meshes.is_some()) * SPACE_FLAG_MESHES) | pairs | map_flags(p),
                    );
                    let (d, cmd) = (p.device, p.cmd);
                    // SAFETY: граф перевёл картинки в чтение, а картинку краёв в запись;
                    // список краёв записан (барьер в прошлом проходе) и жив весь кадр.
                    unsafe {
                        bindless.bind_compute(d, cmd);
                        d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, self.space_edges);
                        d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                        d.cmd_dispatch_indirect(cmd, list_buffer, 0);
                    }
                });
                edges
            });
            // Уменьшенное разрешение (7.14): освещение здесь же, в своей картинке того же
            // размера; «сцена» растянет её на весь кадр.
            let color = (ext != frame.extent).then(|| {
                let color = graph.create("цвет 3D", ext, SPACE_COLOR_FORMAT);
                let mut uses = vec![
                    (gbuffer, Access::Sampled),
                    (shadows, Access::Sampled),
                    (color, Access::ColorAttachment),
                ];
                if let Some(e) = edges {
                    uses.push((e, Access::Sampled));
                }
                graph.add_pass("освещение 3D", &uses, move |p| {
                    let push = space_push(
                        &camera,
                        ext,
                        frame.time,
                        frame.space_address,
                        SpaceSlots {
                            gbuffer: p.texture(gbuffer),
                            shadows: p.texture(shadows),
                            target: edges.map_or(0, |e| p.texture(e)),
                            ..SpaceSlots::default()
                        },
                        u32::from(edges.is_some()) * SPACE_FLAG_EDGES,
                    );
                    let (d, cmd) = (p.device, p.cmd);
                    // SAFETY: граф перевёл картинки 3D в чтение, а цвет в цель рисования;
                    // буфер сцены этого кадра жив, пока кадр рисуется.
                    unsafe {
                        begin_color_pass(d, cmd, p.view(color), ext);
                        bindless.bind(d, cmd);
                        d.cmd_bind_pipeline(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            self.space_shade_small,
                        );
                        d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                        d.cmd_draw(cmd, 3, 1, 0, 0);
                        d.cmd_end_rendering(cmd);
                    }
                });
                color
            });
            (gbuffer, shadows, edges, color)
        });

        let mut uses = vec![
            (target_image, Access::ColorAttachment),
            (stencil, Access::StencilAttachment),
        ];
        if let Some((g, s, e, color)) = gbuffer {
            if let Some(c) = color {
                uses.push((c, Access::Sampled));
            } else {
                uses.push((g, Access::Sampled));
                uses.push((s, Access::Sampled));
                if let Some(e) = e {
                    uses.push((e, Access::Sampled));
                }
            }
        }
        if let Some(f) = field {
            uses.push((f, Access::Sampled));
        }
        if hardware_resolve.is_some() {
            uses.push((scene, Access::ColorAttachment));
        }
        uses.extend(scene_children.iter().map(|l| (l.image, Access::Sampled)));

        graph.add_pass("сцена", &uses, move |p| {
            let mut triangle_push = [0u8; 8];
            triangle_push[..4].copy_from_slice(&frame.time.to_ne_bytes());
            triangle_push[4..].copy_from_slice(&frame.aspect.to_ne_bytes());
            let mut sprite_push = [0u8; 16];
            sprite_push[..8].copy_from_slice(&frame.sprites_address.to_ne_bytes());
            sprite_push[8..12].copy_from_slice(&frame.aspect.to_ne_bytes());
            let textures = layer_textures(p, &scene_children);
            let (d, cmd) = (p.device, p.cmd);
            // SAFETY: граф перевёл картинку сцены в раскладку цели рисования, а слои в
            // чтение; конвейеры и буфер спрайтов живы, пока видеокарта рисует этот кадр.
            unsafe {
                begin_scene_pass(
                    d,
                    cmd,
                    p.view(target_image),
                    hardware_resolve.map(|_| p.view(scene)),
                    Some(p.view(stencil)),
                    p.extent(scene),
                    vk::Offset2D::default(),
                    None,
                    CLEAR_COLOR,
                );
                bindless.bind(d, cmd);
                if frame.scene == DemoScene::Gpu {
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.triangle);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &triangle_push);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                }
                if frame.scene.uses_field() {
                    let field_texture = field.filter(|_| self.light_field).map(|f| p.texture(f));
                    let push = sdf_push(
                        frame.time,
                        frame.extent,
                        self.camera,
                        field_texture,
                        frame.scene.uses_lights(),
                        !frame.scene.has_demo_shapes(),
                        frame.lab_backdrop,
                    );
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.sdf);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                }
                if let Some((_, _, _, Some(c))) = gbuffer {
                    // Растяжение уменьшенной сцены «3D» на весь кадр (7.14).
                    let push = space_push(
                        &frame.space_camera,
                        frame.extent,
                        frame.time,
                        frame.space_address,
                        SpaceSlots {
                            target: p.texture(c),
                            ..SpaceSlots::default()
                        },
                        0,
                    );
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.space_upscale);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                } else if let Some((g, s, e, None)) = gbuffer {
                    let push = space_push(
                        &frame.space_camera,
                        frame.extent,
                        frame.time,
                        frame.space_address,
                        SpaceSlots {
                            gbuffer: p.texture(g),
                            shadows: p.texture(s),
                            target: e.map_or(0, |e| p.texture(e)),
                            ..SpaceSlots::default()
                        },
                        u32::from(e.is_some()) * SPACE_FLAG_EDGES,
                    );
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.space_shade);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                }
                if frame.object_count > 0 {
                    let push = object_push(frame, self.camera, 0);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.object);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_draw(cmd, 6, frame.object_count, 0, 0);
                }
                if let Some(vector) = frame.vector {
                    let full = vk::Rect2D::default().extent(p.extent(scene));
                    self.vector.record(
                        d,
                        cmd,
                        layout,
                        vector,
                        0,
                        &textures,
                        full,
                        vk::Offset2D::default(),
                    );
                }
                if let Some((push, list)) = particle_draw {
                    // Сколько рисовать, видеокарта записала сама (только живые обычные).
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.particle_draw);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    let args = crate::particles::DRAW_ARGS_NORMAL;
                    d.cmd_draw_indirect(cmd, list, args, 1, 16);
                }
                if frame.sprite_count > 0 {
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.sprites);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &sprite_push);
                    d.cmd_draw(cmd, 6, frame.sprite_count, 0, 0);
                }
                d.cmd_end_rendering(cmd);
            }
        });

        if let Some(m) = own_resolve {
            graph.add_pass(
                "сведение MSAA",
                &[(m, Access::Sampled), (scene, Access::ColorAttachment)],
                move |p| {
                    let mut push = [0u8; 12];
                    push[..4].copy_from_slice(&p.texture(m).to_ne_bytes());
                    push[4..8].copy_from_slice(&frame.msaa.samples().as_raw().to_ne_bytes());
                    push[8..].copy_from_slice(&u32::from(frame.perceptual_edges).to_ne_bytes());
                    let (d, cmd) = (p.device, p.cmd);
                    // SAFETY: граф перевёл картинку MSAA в чтение, а кадр сцены в цель рисования.
                    unsafe {
                        begin_color_pass(d, cmd, p.view(scene), p.extent(scene));
                        bindless.bind(d, cmd);
                        d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.resolve);
                        d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                        d.cmd_draw(cmd, 3, 1, 0, 0);
                        d.cmd_end_rendering(cmd);
                    }
                },
            );
        }

        let light = light.zip(field);
        // Свечение читает готовый кадр сцены, поэтому идёт после его сведения.
        let bloom = light
            .filter(|_| self.glow)
            .map(|(l, _)| self.add_bloom_passes(graph, bindless, frame, scene, l));
        let mut uses = vec![(scene, Access::Sampled), (target, Access::ColorAttachment)];
        if let Some((l, f)) = light {
            uses.push((l, Access::Sampled));
            uses.push((f, Access::Sampled));
        }
        if let Some(b) = bloom {
            uses.push((b, Access::Sampled));
        }
        let dither = self.dither;
        graph.add_pass("вывод", &uses, move |p| {
            let mut push = [0u8; 32];
            push[..4].copy_from_slice(&p.texture(scene).to_ne_bytes());
            if let Some((l, f)) = light {
                push[4..8].copy_from_slice(&p.texture(l).to_ne_bytes());
                push[8..12].copy_from_slice(&1u32.to_ne_bytes());
                push[12..16].copy_from_slice(&p.texture(f).to_ne_bytes());
            }
            let mut flags = 0u32;
            if let Some(b) = bloom {
                push[16..20].copy_from_slice(&p.texture(b).to_ne_bytes());
                flags |= PRESENT_GLOW;
            }
            if dither {
                flags |= PRESENT_DITHER;
            }
            push[20..24].copy_from_slice(&flags.to_ne_bytes());
            push[24..28].copy_from_slice(&GLOW_STRENGTH.to_ne_bytes());
            let (d, cmd) = (p.device, p.cmd);
            // SAFETY: граф перевёл сцену в чтение, а цель в раскладку рисования.
            unsafe {
                begin_color_pass(d, cmd, p.view(target), p.extent(target));
                bindless.bind(d, cmd);
                d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.present);
                d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                d.cmd_draw(cmd, 3, 1, 0, 0);
                // Искры светятся сами: поверх освещённого кадра, уровень их не затемняет.
                if let Some((push, list)) = particle_glow {
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.particle_glow);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    let args = crate::particles::DRAW_ARGS_GLOW;
                    d.cmd_draw_indirect(cmd, list, args, 1, 16);
                }
                d.cmd_end_rendering(cmd);
            }
        });
    }

    /// Проходы поля расстояний сцены «свет» (4.2): «маска» рисует фигуры, отбрасывающие
    /// тень, в уменьшенном разрешении (SDF-фигуры сцены и силуэты слоёв `shadows` из их
    /// готовых картинок); «поле» строит из неё расстояния jump flooding.
    /// Возвращает картинку поля (расстояние со знаком в единицах сцены).
    fn add_field_passes<'a>(
        &'a self,
        graph: &mut RenderGraph<'a>,
        bindless: &'a Bindless,
        frame: FrameData,
        shadows: Vec<LayerImage>,
    ) -> ImageHandle {
        let layout = bindless.pipeline_layout;
        let size = field_size(frame.extent, self.field_downscale);
        let mask = graph.create("маска теней", size, field_format("light_mask"));
        let seeds = [
            graph.create("семена поля", size, field_format("jfa_seed")),
            graph.create("семена поля", size, field_format("jfa_seed")),
        ];
        let field = graph.create("поле расстояний", size, field_format("jfa_field"));
        let camera = self.camera;
        // Одна команда рисования на весь экран: треугольник, без буферов.
        let draw =
            move |p: &PassContext, target: ImageHandle, pipeline: vk::Pipeline, push: &[u8]| {
                let (d, cmd) = (p.device, p.cmd);
                // SAFETY: граф перевёл цель в раскладку рисования, а прочитанные картинки — в
                // чтение; конвейер жив, пока кадр рисуется.
                unsafe {
                    begin_color_pass(d, cmd, p.view(target), size);
                    bindless.bind(d, cmd);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, push);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                    d.cmd_end_rendering(cmd);
                }
            };
        let downscale = self.field_downscale;
        let mut uses = vec![(mask, Access::ColorAttachment)];
        uses.extend(shadows.iter().map(|l| (l.image, Access::Sampled)));
        graph.add_pass("маска", &uses, move |p| {
            let push = sdf_push(frame.time, frame.extent, camera, None, false, false, false);
            let (d, cmd) = (p.device, p.cmd);
            // SAFETY: граф перевёл маску в раскладку рисования, а картинки слоёв (уже
            // нарисованные) в чтение; конвейеры живы, пока кадр рисуется.
            unsafe {
                // Очистка нулём: «ничего не закрыто». Цвет фона кадра (не ноль) сделал бы
                // каждую точку маски чуть закрытой, то есть краем, и поле стало бы почти
                // нулевым везде (так было в сцене «слизь и магия», где маску не
                // перекрывают SDF-фигуры сцены «свет»).
                begin_color_pass_with(d, cmd, p.view(mask), size, [0.0; 4]);
                bindless.bind(d, cmd);
                // Сначала SDF-фигуры сцены на всю маску (в сценах «слизь и магия» и «лаборатория мага» их нет,
                // маска просто очищена), потом SDF-объекты и силуэты слоёв поверх.
                if frame.scene.has_demo_shapes() {
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.field.mask);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push[..24]);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                }
                if frame.object_count > 0 {
                    let push = object_push(frame, camera, 1);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.object_mask);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_draw(cmd, 6, frame.object_count, 0, 0);
                }
                if !shadows.is_empty() {
                    d.cmd_bind_pipeline(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.field.mask_layer,
                    );
                }
                for l in &shadows {
                    let mut push = [0u8; 24];
                    push[..4].copy_from_slice(&p.texture(l.image).to_ne_bytes());
                    push[4..8].copy_from_slice(&(downscale as f32).to_ne_bytes());
                    push[8..12].copy_from_slice(&l.rect.offset.x.to_ne_bytes());
                    push[12..16].copy_from_slice(&l.rect.offset.y.to_ne_bytes());
                    push[16..20].copy_from_slice(&(l.rect.extent.width as f32).to_ne_bytes());
                    push[20..].copy_from_slice(&(l.rect.extent.height as f32).to_ne_bytes());
                    d.cmd_set_scissor(cmd, 0, &[mask_rect(l.rect, downscale, size)]);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                }
                d.cmd_end_rendering(cmd);
            }
        });
        graph.add_pass(
            "поле",
            &[(mask, Access::Sampled), (seeds[0], Access::ColorAttachment)],
            move |p| {
                draw(p, seeds[0], self.field.seed, &p.texture(mask).to_ne_bytes());
            },
        );
        let mut current = 0;
        for step in jfa_steps(size) {
            let (src, dst) = (seeds[current], seeds[1 - current]);
            graph.add_pass(
                "поле",
                &[(src, Access::Sampled), (dst, Access::ColorAttachment)],
                move |p| {
                    let mut push = [0u8; 8];
                    push[..4].copy_from_slice(&p.texture(src).to_ne_bytes());
                    push[4..].copy_from_slice(&step.to_ne_bytes());
                    draw(p, dst, self.field.step, &push);
                },
            );
            current = 1 - current;
        }
        let last = seeds[current];
        // Сколько единиц сцены в пикселе поля: высота экрана — 2 единицы при приближении 1.
        let scene_per_pixel = 2.0 / (size.height as f32 * camera.zoom as f32);
        graph.add_pass(
            "поле",
            &[
                (last, Access::Sampled),
                (mask, Access::Sampled),
                (field, Access::ColorAttachment),
            ],
            move |p| {
                let mut push = [0u8; 12];
                push[..4].copy_from_slice(&p.texture(last).to_ne_bytes());
                push[4..8].copy_from_slice(&p.texture(mask).to_ne_bytes());
                push[8..].copy_from_slice(&scene_per_pixel.to_ne_bytes());
                draw(p, field, self.field.field, &push);
            },
        );
        field
    }

    /// Проход «свет» сцены «лампы и тени» (4.3): в разрешении поля для каждой точки
    /// складывает свет всех источников с мягкими тенями по полю `field`. Возвращает
    /// картинку света, на которую «вывод» умножает кадр.
    /// Проходы свечения (4.4): «свечение» уменьшает освещённый кадр цепочкой картинок
    /// (половина кадра, четверть, … до [`BLOOM_LEVELS`] ступеней), оставляя только то,
    /// что ярче [`GLOW_THRESHOLD`], и собирает цепочку обратно с размытием. Возвращает
    /// картинку в половину кадра, которую вывод добавляет к кадру.
    fn add_bloom_passes<'a>(
        &'a self,
        graph: &mut RenderGraph<'a>,
        bindless: &'a Bindless,
        frame: FrameData,
        scene: ImageHandle,
        light: ImageHandle,
    ) -> ImageHandle {
        let layout = bindless.pipeline_layout;
        let sizes = bloom_sizes(frame.extent);
        let down: Vec<ImageHandle> = sizes
            .iter()
            .map(|&size| graph.create("свечение вниз", size, BLOOM_FORMAT))
            .collect();
        let draw = move |p: &PassContext,
                         target: ImageHandle,
                         size: vk::Extent2D,
                         pipeline: vk::Pipeline,
                         push: &[u8]| {
            let (d, cmd) = (p.device, p.cmd);
            // SAFETY: граф перевёл цель в раскладку рисования, а прочитанные картинки — в
            // чтение; конвейер жив, пока кадр рисуется.
            unsafe {
                begin_color_pass(d, cmd, p.view(target), size);
                bindless.bind(d, cmd);
                d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, push);
                d.cmd_draw(cmd, 3, 1, 0, 0);
                d.cmd_end_rendering(cmd);
            }
        };
        let texel = |size: vk::Extent2D| [1.0 / size.width as f32, 1.0 / size.height as f32];
        for (i, &size) in sizes.iter().enumerate() {
            let target = down[i];
            let source = if i == 0 { scene } else { down[i - 1] };
            let mut uses = vec![(source, Access::Sampled), (target, Access::ColorAttachment)];
            if i == 0 {
                uses.push((light, Access::Sampled));
            }
            let pipeline = self.bloom_down;
            graph.add_pass("свечение", &uses, move |p| {
                let mut push = [0u8; 24];
                push[..4].copy_from_slice(&p.texture(source).to_ne_bytes());
                if i == 0 {
                    push[4..8].copy_from_slice(&p.texture(light).to_ne_bytes());
                }
                push[8..12].copy_from_slice(&u32::from(i > 0).to_ne_bytes());
                push[12..16].copy_from_slice(&GLOW_THRESHOLD.to_ne_bytes());
                let t = texel(size);
                push[16..20].copy_from_slice(&t[0].to_ne_bytes());
                push[20..24].copy_from_slice(&t[1].to_ne_bytes());
                draw(p, target, size, pipeline, &push);
            });
        }
        // Обратно: каждая ступень = своя картинка шага вниз + растянутая ступень меньше.
        let mut small = down[down.len() - 1];
        for i in (0..sizes.len() - 1).rev() {
            let size = sizes[i];
            let same = down[i];
            let target = graph.create("свечение вверх", size, BLOOM_FORMAT);
            let pipeline = self.bloom_up;
            let from = small;
            graph.add_pass(
                "свечение",
                &[
                    (from, Access::Sampled),
                    (same, Access::Sampled),
                    (target, Access::ColorAttachment),
                ],
                move |p| {
                    let mut push = [0u8; 16];
                    push[..4].copy_from_slice(&p.texture(from).to_ne_bytes());
                    push[4..8].copy_from_slice(&p.texture(same).to_ne_bytes());
                    let t = texel(size);
                    push[8..12].copy_from_slice(&t[0].to_ne_bytes());
                    push[12..16].copy_from_slice(&t[1].to_ne_bytes());
                    draw(p, target, size, pipeline, &push);
                },
            );
            small = target;
        }
        small
    }

    fn add_light_pass<'a>(
        &'a self,
        graph: &mut RenderGraph<'a>,
        bindless: &'a Bindless,
        frame: FrameData,
        field: ImageHandle,
    ) -> ImageHandle {
        let layout = bindless.pipeline_layout;
        // Свет мельче кадра вдвое на любой видеокарте, даже если поле мельче: при
        // растяжении из четверти кадра резкие края теней шли лесенкой (узкое место 5).
        let size = field_size(frame.extent, LIGHT_DOWNSCALE);
        let light = graph.create("свет", size, LIGHT_FORMAT);
        let camera = self.camera;
        graph.add_pass(
            "свет",
            &[(field, Access::Sampled), (light, Access::ColorAttachment)],
            move |p| {
                let mut push = [0u8; 48];
                push[..8].copy_from_slice(&frame.lights_address.to_ne_bytes());
                push[8..12].copy_from_slice(&frame.light_count.to_ne_bytes());
                push[12..16].copy_from_slice(&p.texture(field).to_ne_bytes());
                let values = [
                    camera.center.x as f32,
                    camera.center.y as f32,
                    frame.aspect,
                    camera.zoom as f32,
                    frame.ambient[0],
                    frame.ambient[1],
                    frame.ambient[2],
                ];
                for (i, v) in values.into_iter().enumerate() {
                    push[16 + 4 * i..20 + 4 * i].copy_from_slice(&v.to_ne_bytes());
                }
                let (d, cmd) = (p.device, p.cmd);
                // SAFETY: граф перевёл поле в чтение, а картинку света в цель рисования;
                // буфер источников этого кадра жив, пока кадр рисуется.
                unsafe {
                    begin_color_pass(d, cmd, p.view(light), size);
                    bindless.bind(d, cmd);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.light);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                    d.cmd_end_rendering(cmd);
                }
            },
        );
        light
    }

    /// Опыт 4.6: свет каскадами лучей (`rc_cascade.slang`, `rc_gather.slang`). Каскады
    /// считаются сверху вниз (каждый дополняет свои лучи лучами каскада выше), потом
    /// «свет» собирает каскад 0 в картинку света того же вида, что у [`Self::add_light_pass`].
    fn add_cascade_passes<'a>(
        &'a self,
        graph: &mut RenderGraph<'a>,
        bindless: &'a Bindless,
        frame: FrameData,
        field: ImageHandle,
    ) -> ImageHandle {
        let layout = bindless.pipeline_layout;
        let field_extent = field_size(frame.extent, self.field_downscale);
        let camera = self.camera;
        // Отрезок луча каскада 0 — одна точка поля; каждый следующий вчетверо длиннее.
        let r0 = 2.0 / (field_extent.height as f32 * camera.zoom as f32);
        let count = cascade_count(r0, frame.aspect, camera.zoom as f32);
        let sizes: Vec<(vk::Extent2D, [u32; 2])> =
            (0..count).map(|i| cascade_size(field_extent, i)).collect();
        let images: Vec<ImageHandle> = sizes
            .iter()
            .map(|&(size, _)| graph.create("каскад света", size, RC_FORMAT))
            .collect();
        let draw = move |p: &PassContext,
                         target: ImageHandle,
                         size: vk::Extent2D,
                         pipeline: vk::Pipeline,
                         push: &[u8]| {
            let (d, cmd) = (p.device, p.cmd);
            // SAFETY: граф перевёл цель в раскладку рисования, а прочитанные картинки — в
            // чтение; конвейер и буфер источников живы, пока кадр рисуется.
            unsafe {
                begin_color_pass(d, cmd, p.view(target), size);
                bindless.bind(d, cmd);
                d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, push);
                d.cmd_draw(cmd, 3, 1, 0, 0);
                d.cmd_end_rendering(cmd);
            }
        };
        for i in (0..count).rev() {
            let target = images[i as usize];
            let (size, probes) = sizes[i as usize];
            let upper = (i + 1 < count).then(|| (images[i as usize + 1], sizes[i as usize + 1].1));
            let mut uses = vec![(field, Access::Sampled), (target, Access::ColorAttachment)];
            if let Some((u, _)) = upper {
                uses.push((u, Access::Sampled));
            }
            let pipeline = self.rc_cascade;
            graph.add_pass("каскады света", &uses, move |p| {
                let mut push = [0u8; 80];
                push[..8].copy_from_slice(&frame.lights_address.to_ne_bytes());
                let words: [u32; 5] = [
                    frame.light_count,
                    p.texture(field),
                    upper.map_or(0, |(u, _)| p.texture(u)),
                    i,
                    u32::from(upper.is_none()),
                ];
                for (k, w) in words.into_iter().enumerate() {
                    push[8 + 4 * k..12 + 4 * k].copy_from_slice(&w.to_ne_bytes());
                }
                let floats = [
                    r0,
                    camera.center.x as f32,
                    camera.center.y as f32,
                    frame.aspect,
                    camera.zoom as f32,
                ];
                for (k, v) in floats.into_iter().enumerate() {
                    push[28 + 4 * k..32 + 4 * k].copy_from_slice(&v.to_ne_bytes());
                }
                let up = upper.map_or([1, 1], |(_, pr)| pr);
                let sizes = [
                    field_extent.width,
                    field_extent.height,
                    probes[0],
                    probes[1],
                    up[0],
                    up[1],
                ];
                for (k, v) in sizes.into_iter().enumerate() {
                    push[48 + 4 * k..52 + 4 * k].copy_from_slice(&v.to_ne_bytes());
                }
                push[72..76].copy_from_slice(&RC_EMIT.to_ne_bytes());
                draw(p, target, size, pipeline, &push);
            });
        }
        let size = field_size(frame.extent, LIGHT_DOWNSCALE);
        let light = graph.create("свет", size, LIGHT_FORMAT);
        let c0 = images[0];
        let probes = sizes[0].1;
        let pipeline = self.rc_gather;
        let ambient = frame.ambient;
        graph.add_pass(
            "свет",
            &[(c0, Access::Sampled), (light, Access::ColorAttachment)],
            move |p| {
                let mut push = [0u8; 32];
                push[..4].copy_from_slice(&p.texture(c0).to_ne_bytes());
                push[8..12].copy_from_slice(&probes[0].to_ne_bytes());
                push[12..16].copy_from_slice(&probes[1].to_ne_bytes());
                for (k, v) in ambient.into_iter().enumerate() {
                    push[16 + 4 * k..20 + 4 * k].copy_from_slice(&v.to_ne_bytes());
                }
                draw(p, light, size, pipeline, &push);
            },
        );
        light
    }

    fn destroy_pipelines(&mut self, ctx: &GpuContext) {
        // SAFETY: вызывается, когда видеокарта не использует конвейеры; null допустим.
        unsafe {
            ctx.device.destroy_pipeline(self.triangle, None);
            ctx.device.destroy_pipeline(self.sprites, None);
            ctx.device.destroy_pipeline(self.present, None);
            ctx.device.destroy_pipeline(self.resolve, None);
            ctx.device.destroy_pipeline(self.sdf, None);
            ctx.device.destroy_pipeline(self.space_march, None);
            ctx.device.destroy_pipeline(self.space_mesh, None);
            ctx.device.destroy_pipeline(self.space_mesh_shadow, None);
            ctx.device.destroy_pipeline(self.space_cone, None);
            ctx.device.destroy_pipeline(self.space_shadow, None);
            ctx.device.destroy_pipeline(self.space_march_compute, None);
            ctx.device.destroy_pipeline(self.space_edge_find, None);
            ctx.device.destroy_pipeline(self.space_edges, None);
            ctx.device.destroy_pipeline(self.space_field_bake, None);
            ctx.device.destroy_pipeline(self.space_shade, None);
            ctx.device.destroy_pipeline(self.space_shade_small, None);
            ctx.device.destroy_pipeline(self.space_upscale, None);
            ctx.device.destroy_pipeline(self.light, None);
            ctx.device.destroy_pipeline(self.object, None);
            ctx.device.destroy_pipeline(self.object_mask, None);
            ctx.device.destroy_pipeline(self.bloom_down, None);
            ctx.device.destroy_pipeline(self.bloom_up, None);
            ctx.device.destroy_pipeline(self.rc_cascade, None);
            ctx.device.destroy_pipeline(self.rc_gather, None);
            ctx.device.destroy_pipeline(self.particle_update, None);
            ctx.device.destroy_pipeline(self.particle_draw, None);
            ctx.device.destroy_pipeline(self.particle_glow, None);
            for name in FIELD_SHADERS {
                ctx.device.destroy_pipeline(*self.field.slot(name), None);
            }
        }
        self.triangle = vk::Pipeline::null();
        self.sdf = vk::Pipeline::null();
        self.space_march = vk::Pipeline::null();
        self.space_mesh = vk::Pipeline::null();
        self.space_mesh_shadow = vk::Pipeline::null();
        self.space_cone = vk::Pipeline::null();
        self.space_shadow = vk::Pipeline::null();
        self.space_march_compute = vk::Pipeline::null();
        self.space_edge_find = vk::Pipeline::null();
        self.space_edges = vk::Pipeline::null();
        self.space_field_bake = vk::Pipeline::null();
        self.space_shade = vk::Pipeline::null();
        self.space_shade_small = vk::Pipeline::null();
        self.space_upscale = vk::Pipeline::null();
        self.light = vk::Pipeline::null();
        self.object = vk::Pipeline::null();
        self.object_mask = vk::Pipeline::null();
        self.bloom_down = vk::Pipeline::null();
        self.bloom_up = vk::Pipeline::null();
        self.rc_cascade = vk::Pipeline::null();
        self.rc_gather = vk::Pipeline::null();
        self.particle_update = vk::Pipeline::null();
        self.particle_draw = vk::Pipeline::null();
        self.particle_glow = vk::Pipeline::null();
        self.field = FieldPipelines::default();
        self.sprites = vk::Pipeline::null();
        self.present = vk::Pipeline::null();
        self.resolve = vk::Pipeline::null();
        self.vector.destroy(ctx);
    }

    /// Уничтожает конвейеры. Текстуры и буферы уходят вместе с [`GpuResources`].
    pub fn destroy(mut self, ctx: &GpuContext) {
        self.destroy_pipelines(ctx);
    }
}

/// Картинка слоя векторных фигур в графе кадра.
#[derive(Clone, Copy)]
struct LayerImage {
    layer: u32,
    image: ImageHandle,
    /// Где в ней нарисованы фигуры слоя.
    rect: vk::Rect2D,
}

/// Размер картинки слоя с прямоугольником `rect`: с запасом до кратного 128 пикселям
/// (чтобы слой, который двигается и меняет размер, чаще брал из пула ту же картинку), но
/// не больше кадра `frame`.
fn layer_image_size(rect: vk::Extent2D, frame: vk::Extent2D) -> vk::Extent2D {
    let up = |v: u32, max: u32| v.div_ceil(128).saturating_mul(128).min(max).max(v);
    vk::Extent2D {
        width: up(rect.width, frame.width),
        height: up(rect.height, frame.height),
    }
}

/// Картинки слоёв `wanted` из уже нарисованных `layers`.
fn layer_images(layers: &[LayerImage], wanted: &[u32]) -> Vec<LayerImage> {
    layers
        .iter()
        .copied()
        .filter(|l| wanted.contains(&l.layer))
        .collect()
}

/// Картинки по номеру слоя (для [`VectorRenderer::record`]).
fn layer_textures(p: &PassContext, layers: &[LayerImage]) -> Vec<Option<LayerTexture>> {
    let len = layers
        .iter()
        .map(|l| l.layer as usize + 1)
        .max()
        .unwrap_or(0);
    let mut out = vec![None; len];
    for l in layers {
        out[l.layer as usize] = Some(LayerTexture {
            texture: p.texture(l.image),
            rect: l.rect,
        });
    }
    out
}

/// Постоянные сцены «свет» в раскладке `Push` из `sdf_demo.slang` (первые 24 байта такие же
/// у `light_mask.slang`): время, отношение сторон, центр и приближение камеры, высота кадра
/// в пикселях, затем картинка поля расстояний (если линии фона берутся из неё).
fn sdf_push(
    time: f32,
    extent: vk::Extent2D,
    camera: reyvec_vector::Camera2D,
    field: Option<u32>,
    lit: bool,
    no_shapes: bool,
    lab: bool,
) -> [u8; 32] {
    let aspect = extent.width as f32 / extent.height.max(1) as f32;
    let values = [
        time,
        aspect,
        camera.center.x as f32,
        camera.center.y as f32,
        camera.zoom as f32,
        extent.height.max(1) as f32,
    ];
    let mut push = [0u8; 32];
    for (i, v) in values.into_iter().enumerate() {
        push[4 * i..4 * i + 4].copy_from_slice(&v.to_ne_bytes());
    }
    push[24..28].copy_from_slice(&field.unwrap_or(0).to_ne_bytes());
    let flags = u32::from(field.is_some())
        | (u32::from(lit) << 1)
        | (u32::from(no_shapes) << 2)
        | (u32::from(lab) << 3);
    push[28..32].copy_from_slice(&flags.to_ne_bytes());
    push
}

/// Константы сцены «3D» в раскладке `Push` из `include/space.slang`: столбцы обратной
/// матрицы камеры, глаз, время, адрес сцены (7.2), соотношение сторон, высота кадра, флаги
/// и номера картинок в общем списке текстур: буфера геометрии (7.3) и теней (7.4).
/// Картинки и буферы, которые читают и пишут проходы сцены «3D» (поля `Push` в
/// `include/space.slang`).
#[derive(Clone, Copy, Default)]
struct SpaceSlots {
    gbuffer: u32,
    shadows: u32,
    target: u32,
    cone: u32,
    meshes: u32,
    edges: vk::DeviceAddress,
}

fn space_push(
    camera: &reyvec_sdf::Camera3D,
    extent: vk::Extent2D,
    time: f32,
    scene: vk::DeviceAddress,
    slots: SpaceSlots,
    flags: u32,
) -> [u8; 128] {
    let aspect = extent.width as f32 / extent.height.max(1) as f32;
    let inv = camera.inverse_view_proj(aspect).to_cols_array();
    let mut values = [0f32; 20];
    values[..16].copy_from_slice(&inv);
    values[16..19].copy_from_slice(&camera.eye.to_array());
    values[19] = time;
    let mut push = [0u8; 128];
    for (i, v) in values.into_iter().enumerate() {
        push[4 * i..4 * i + 4].copy_from_slice(&v.to_ne_bytes());
    }
    push[80..88].copy_from_slice(&scene.to_ne_bytes());
    push[88..92].copy_from_slice(&aspect.to_ne_bytes());
    push[92..96].copy_from_slice(&(extent.height.max(1) as f32).to_ne_bytes());
    let words = [
        flags,
        slots.gbuffer,
        slots.shadows,
        slots.target,
        slots.cone,
        slots.meshes,
    ];
    for (i, w) in words.into_iter().enumerate() {
        push[96 + 4 * i..100 + 4 * i].copy_from_slice(&w.to_ne_bytes());
    }
    push[120..128].copy_from_slice(&slots.edges.to_ne_bytes());
    push
}

/// Константы для `sdf_object.slang` (4.5): записи объектов, камера и режим (0 — цвет
/// в кадр сцены, 1 — доля закрытой точки в маску теней).
fn object_push(frame: FrameData, camera: reyvec_vector::Camera2D, mode: u32) -> [u8; 40] {
    let mut push = [0u8; 40];
    push[..8].copy_from_slice(&frame.objects_address.to_ne_bytes());
    push[8..16].copy_from_slice(&frame.blobs_address.to_ne_bytes());
    let values = [
        camera.center.x as f32,
        camera.center.y as f32,
        frame.aspect,
        camera.zoom as f32,
    ];
    for (i, v) in values.into_iter().enumerate() {
        push[16 + 4 * i..20 + 4 * i].copy_from_slice(&v.to_ne_bytes());
    }
    push[32..36].copy_from_slice(&mode.to_ne_bytes());
    push
}

/// Конвейер SDF-объектов для маски теней: доля закрытой точки объединяется с маской,
/// как силуэты слоёв (`a + b − a·b`).
fn build_object_mask(
    ctx: &GpuContext,
    res: &GpuResources,
    shaders: &ShaderLibrary,
) -> Result<vk::Pipeline, GpuError> {
    let pair = shaders.get("sdf_object");
    graphics_pipeline_stencil(
        &ctx.device,
        res.bindless.pipeline_layout,
        &pair.vs,
        &pair.fs,
        field_format("light_mask"),
        Blend::Screen,
        vk::SampleCountFlags::TYPE_1,
        None,
    )
}

/// Формат картинки света (4.3): свет может быть ярче 1, поэтому с плавающей точкой.
const LIGHT_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
/// Буфер геометрии сцены «3D» (7.3): расстояние вдоль луча, материал и нормаль (две
/// координаты) — по 32 бита, чтобы расстояние до дальних точек не теряло точность.
const GBUFFER_FORMAT: vk::Format = vk::Format::R32G32B32A32_SFLOAT;
/// Длины, с которых начинать лучи сцены «3D» (7.8): по точке на квадрат
/// [`CONE_TILE`]×[`CONE_TILE`] пикселей.
const CONE_FORMAT: vk::Format = vk::Format::R32_SFLOAT;
/// Глубина прохода «полигоны 3D» (7.6ж). D32 видеокарты умеют все.
const MESH_DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;
/// Флаг сцены «3D»: у «реймарчинга» есть буфер геометрии полигонов (как в raymarch.slang).
const SPACE_FLAG_MESHES: u32 = 1;
/// Флаг сцены «3D»: края форм сглажены (7.6о), «сцена» берёт их цвет из картинки краёв.
const SPACE_FLAG_EDGES: u32 = 2;
/// Флаг сцены «3D»: края по два луча на пиксель вместо четырёх (7.15) — при уменьшенном
/// разрешении 3D, где растяжение и так смягчает края (как в space.slang).
const SPACE_FLAG_EDGE_PAIRS: u32 = 4;
/// Флаг сцены «3D»: тени ламп от треугольников по картам теней (7.9б); номер картинки
/// карт — в старших 16 битах флагов (как в space.slang).
const SPACE_FLAG_SHADOW_MAP: u32 = 8;
/// Карта тени лампы (7.9б): расстояние от лампы до ближней поверхности.
const SHADOW_MAP_FORMAT: vk::Format = vk::Format::R32_SFLOAT;
/// Сторона карты тени лампы, точек.
const SHADOW_MAP_SIZE: u32 = 1024;
/// Цвет краёв форм сцены «3D» (7.6о); прозрачность 0 — пиксель не на краю.
const EDGE_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
/// Сторона квадрата пикселей, который накрывает один конус (как в raymarch_cone.slang).
const CONE_TILE: u32 = 8;
/// Освещённая сцена «3D» в уменьшенном разрешении (7.14), до растяжения на кадр.
const SPACE_COLOR_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
/// Тени сцены «3D» (7.4): солнце, затенение в углах и две лампы — доли от 0 до 1.
const SHADOW3D_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
/// Каскады лучей (опыт 4.6): цвет, принесённый лучом, и пропустил ли он свет дальше.
const RC_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
/// Яркость поверхности источника в каскадах на единицу его цвета и размера: свет на
/// расстоянии d от лампы около цвет · RC_EMIT / (π d). Подобрано по снимку лаборатории:
/// при 1,0 стена была белёсой целиком (свет каскадов гаснет с расстоянием медленнее
/// обычного, у которого есть дальность).
const RC_EMIT: f32 = 0.4;
/// Каскадов больше не бывает (на случай очень мелкого поля или сильного отдаления).
const MAX_CASCADES: u32 = 8;

/// Сколько каскадов нужно, чтобы лучи (отрезок каскада 0 длиной `r0`, каждый следующий
/// вчетверо длиннее) дотянулись через весь экран по диагонали.
fn cascade_count(r0: f32, aspect: f32, zoom: f32) -> u32 {
    let diagonal = 2.0 * (aspect * aspect + 1.0).sqrt() / zoom.max(1e-3);
    let mut count = 1;
    while count < MAX_CASCADES && r0 * (4f32.powi(count as i32) - 1.0) / 3.0 < diagonal {
        count += 1;
    }
    count
}

/// Размер картинки каскада `i` для поля `field` и число его зондов: зонды через 2^i
/// точек поля, у каждого квадрат 2^(i+1) × 2^(i+1) лучей.
fn cascade_size(field: vk::Extent2D, i: u32) -> (vk::Extent2D, [u32; 2]) {
    let spacing = 1u32 << i;
    let side = 2u32 << i;
    let probes = [
        field.width.div_ceil(spacing).max(1),
        field.height.div_ceil(spacing).max(1),
    ];
    (
        vk::Extent2D {
            width: probes[0] * side,
            height: probes[1] * side,
        },
        probes,
    )
}

/// Формат картинок свечения: яркость выше белого нужна, прозрачность нет.
const BLOOM_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

/// Сколько ступеней в цепочке свечения: от половины кадра до 1/32 (у 1920×1009 это
/// 60×32 точки, свечение расходится примерно на десятую часть экрана).
const BLOOM_LEVELS: usize = 5;

/// С какой яркости начинается свечение (1 — белый; плавно с 0,5 ниже).
const GLOW_THRESHOLD: f32 = 1.0;

/// Сила свечения при выводе. Цепочка складывает все ступени, поэтому их сумма примерно
/// в BLOOM_LEVELS раз ярче ярких мест.
const GLOW_STRENGTH: f32 = 0.08;

/// Флаги вывода (`present.slang`).
const PRESENT_GLOW: u32 = 1;
const PRESENT_DITHER: u32 = 2;

/// Размеры ступеней свечения для кадра `frame`: половина, четверть, … (не меньше 1).
fn bloom_sizes(frame: vk::Extent2D) -> Vec<vk::Extent2D> {
    (1..=BLOOM_LEVELS as u32)
        .map(|k| field_size(frame, 1 << k))
        .collect()
}

/// Во сколько раз картинка света меньше кадра по каждой стороне (поле может быть ещё
/// меньше, свет читает его со смешиванием соседних точек).
const LIGHT_DOWNSCALE: u32 = 2;

/// Шейдеры поля расстояний (4.2) по порядку проходов.
const FIELD_SHADERS: [&str; 5] = [
    "light_mask",
    "light_mask_layer",
    "jfa_seed",
    "jfa_step",
    "jfa_field",
];

/// Во сколько раз поле расстояний меньше кадра по каждой стороне на видеокарте этого
/// типа. Jump flooding упирается в память: на встроенной Radeon поле в половину кадра
/// стоило 4 мс (узкое место света 2), в четверть — вчетверо меньше точек. Мягким теням
/// такой точности хватает; резкие края рисует векторный слой.
pub(crate) fn field_downscale_for(kind: AdapterKind) -> u32 {
    match kind {
        AdapterKind::Integrated => 4,
        _ => 2,
    }
}

/// Формат картинки, в которую рисует шейдер поля `name`: маска — 8 бит, семена — две
/// координаты пикселя (половинные числа точны до 2048, а поле не шире), поле — одно
/// число со знаком.
fn field_format(name: &str) -> vk::Format {
    match name {
        "light_mask" | "light_mask_layer" => vk::Format::R8_UNORM,
        "jfa_seed" | "jfa_step" => vk::Format::R16G16_SFLOAT,
        _ => vk::Format::R16_SFLOAT,
    }
}

/// Прямоугольник кадра `rect` в точках маски, которая в `downscale` раз меньше кадра
/// (с округлением наружу), внутри маски размером `size`.
fn mask_rect(rect: vk::Rect2D, downscale: u32, size: vk::Extent2D) -> vk::Rect2D {
    let ds = downscale.max(1) as i32;
    let x0 = rect.offset.x.max(0) / ds;
    let y0 = rect.offset.y.max(0) / ds;
    let x1 = ((rect.offset.x + rect.extent.width as i32 + ds - 1) / ds).min(size.width as i32);
    let y1 = ((rect.offset.y + rect.extent.height as i32 + ds - 1) / ds).min(size.height as i32);
    vk::Rect2D {
        offset: vk::Offset2D { x: x0, y: y0 },
        extent: vk::Extent2D {
            width: (x1 - x0).max(1) as u32,
            height: (y1 - y0).max(1) as u32,
        },
    }
}

/// Размер поля расстояний для кадра `frame`, если поле в `downscale` раз меньше.
fn field_size(frame: vk::Extent2D, downscale: u32) -> vk::Extent2D {
    vk::Extent2D {
        width: frame.width.div_ceil(downscale).max(1),
        height: frame.height.div_ceil(downscale).max(1),
    }
}

/// Шаги jump flooding для картинки `size`: от половины большей стороны (степень двойки)
/// до 1, и ещё раз 1 (как `reyvec_sdf::jump_flood`).
fn jfa_steps(size: vk::Extent2D) -> Vec<i32> {
    let mut steps = Vec::new();
    let mut step = size.width.max(size.height).next_power_of_two() / 2;
    while step >= 1 {
        steps.push(step as i32);
        step /= 2;
    }
    steps.push(1);
    steps
}

/// Конвейеры поля расстояний.
#[derive(Default)]
struct FieldPipelines {
    mask: vk::Pipeline,
    /// Силуэты слоёв, отбрасывающих тень, поверх маски.
    mask_layer: vk::Pipeline,
    seed: vk::Pipeline,
    step: vk::Pipeline,
    field: vk::Pipeline,
}

impl FieldPipelines {
    fn slot(&mut self, name: &str) -> &mut vk::Pipeline {
        match name {
            "light_mask" => &mut self.mask,
            "light_mask_layer" => &mut self.mask_layer,
            "jfa_seed" => &mut self.seed,
            "jfa_step" => &mut self.step,
            _ => &mut self.field,
        }
    }
}

fn build(
    ctx: &GpuContext,
    res: &GpuResources,
    shaders: &ShaderLibrary,
    name: &str,
    format: vk::Format,
    msaa: Msaa,
    stencil: Option<vk::Format>,
) -> Result<vk::Pipeline, GpuError> {
    let pair = shaders.get(name);
    let blend = match name {
        "sprite" => Blend::Alpha,
        // Силуэты слоёв объединяются с маской: a + b − a·b.
        "light_mask_layer" => Blend::Screen,
        // SDF-объекты с мягким краем поверх фона.
        "sdf_object" => Blend::Alpha,
        // Частицы: мягкие кружки поверх сцены.
        "particles" => Blend::Alpha,
        _ => Blend::Opaque,
    };
    graphics_pipeline_stencil(
        &ctx.device,
        res.bindless.pipeline_layout,
        &pair.vs,
        &pair.fs,
        format,
        blend,
        msaa.samples(),
        stencil.map(|format| Stencil {
            format,
            // Полигоны сцены «3D» (7.6ж) — с глубиной, остальным трафарет не нужен.
            usage: if name == "mesh3d" || name == "mesh_shadow" {
                StencilUse::Depth
            } else {
                StencilUse::Ignore
            },
        }),
    )
}

/// Формат трафарета для прохода сцены: первый, который видеокарта умеет как цель рисования.
/// Только трафарет (S8) экономнее всего; AMD не умеет D24S8, поэтому есть и D32S8.
/// Формат кадра сцены: [`COMPACT_SCENE_FORMAT`], если видеокарта умеет в него рисовать
/// со смешиванием, читать его в шейдере и делать MSAA со всеми числами точек `msaa_counts`;
/// иначе [`LAYER_FORMAT`].
fn pick_scene_format(ctx: &GpuContext, msaa_counts: vk::SampleCountFlags) -> vk::Format {
    let format = COMPACT_SCENE_FORMAT;
    // SAFETY: physical получен из этого экземпляра.
    let features = unsafe {
        ctx.instance()
            .get_physical_device_format_properties(ctx.physical, format)
    }
    .optimal_tiling_features;
    let needed = vk::FormatFeatureFlags::COLOR_ATTACHMENT
        | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
        | vk::FormatFeatureFlags::SAMPLED_IMAGE;
    // SAFETY: physical получен из этого экземпляра.
    let samples = unsafe {
        ctx.instance().get_physical_device_image_format_properties(
            ctx.physical,
            format,
            vk::ImageType::TYPE_2D,
            vk::ImageTiling::OPTIMAL,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
            vk::ImageCreateFlags::empty(),
        )
    }
    .map_or(vk::SampleCountFlags::empty(), |p| p.sample_counts);
    if features.contains(needed) && samples.contains(msaa_counts) {
        format
    } else {
        LAYER_FORMAT
    }
}

pub(crate) fn pick_stencil_format(ctx: &GpuContext) -> Result<vk::Format, GpuError> {
    let candidates = [
        vk::Format::S8_UINT,
        vk::Format::D24_UNORM_S8_UINT,
        vk::Format::D32_SFLOAT_S8_UINT,
    ];
    candidates
        .into_iter()
        .find(|&f| {
            // SAFETY: physical получен из этого экземпляра.
            let props = unsafe {
                ctx.instance()
                    .get_physical_device_format_properties(ctx.physical, f)
            };
            props
                .optimal_tiling_features
                .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
        })
        .ok_or_else(|| {
            GpuError::NoSuitableAdapter(
                "видеокарта не умеет ни один формат трафарета (S8, D24S8, D32S8)".into(),
            )
        })
}

/// Шахматная доска `size × size` с клетками по `cell` пикселей, RGBA8 sRGB.
pub(crate) fn checker_pixels(size: u32, cell: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let light = ((x / cell) + (y / cell)).is_multiple_of(2);
            let c: [u8; 4] = if light {
                [235, 225, 250, 255]
            } else {
                [90, 60, 150, 255]
            };
            out.extend_from_slice(&c);
        }
    }
    out
}

/// Белый круг с мягким краем, прозрачный снаружи, RGBA8.
pub(crate) fn disk_pixels(size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    let r = size as f32 * 0.5;
    let edge = 1.5;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - r;
            let dy = y as f32 + 0.5 - r;
            // Расстояние до границы круга: SDF круга. Плавный переход на краю = сглаживание.
            let sdf = (dx * dx + dy * dy).sqrt() - (r - edge);
            let alpha = (0.5 - sdf / edge).clamp(0.0, 1.0);
            out.extend_from_slice(&[255, 255, 255, (alpha * 255.0).round() as u8]);
        }
    }
    out
}

/// Красная рамка по краю окна (ошибка шейдера): слева, справа, сверху, снизу.
fn error_frame(aspect: f32, white: TextureId, out: &mut Vec<Sprite>) {
    let w = 0.03;
    let red = [1.0, 0.05, 0.05, 1.0];
    let bars = [
        ([-aspect + w / 2.0, 0.0], [w, 2.0]),
        ([aspect - w / 2.0, 0.0], [w, 2.0]),
        ([0.0, 1.0 - w / 2.0], [2.0 * aspect, w]),
        ([0.0, -1.0 + w / 2.0], [2.0 * aspect, w]),
    ];
    for (pos, size) in bars {
        out.push(Sprite {
            pos,
            size,
            tint: red,
            rotation: 0.0,
            texture: white.0,
            sampler: SamplerKind::Nearest as u32,
            pad: 0,
        });
    }
}

fn hue_to_rgb(h: f32) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        1.0 - (k.min(4.0 - k).clamp(0.0, 1.0))
    };
    [f(5.0), f(3.0), f(1.0)]
}

/// Частицы новых событий игры: события с шагом после `seen` (одно событие лежит в
/// нескольких снимках подряд, а частицы даёт один раз). Если мир начался заново (номера
/// шагов меньше прошлых), счёт идёт сначала.
fn event_particles(
    events: &[(u64, reyvec_scene::GameEvent)],
    seen: &mut u64,
    fresh: &mut Vec<reyvec_scene::GameEvent>,
) -> Vec<crate::particles::Particle> {
    if events.last().is_some_and(|&(s, _)| s + 64 < *seen) {
        *seen = 0;
    }
    let mut out = Vec::new();
    for (i, &(step, event)) in events.iter().enumerate() {
        if step > *seen {
            out.extend(crate::particles::burst(&event, step * 31 + i as u64));
            // Для звука (6.7); если их никто не забирает (кадры без окна), не копятся.
            if fresh.len() < MAX_FRESH_EVENTS {
                fresh.push(event);
            }
        }
    }
    if let Some(&(s, _)) = events.last() {
        *seen = (*seen).max(s);
    }
    out
}

/// Сколько новых событий игры ждут, пока их заберут (звук).
const MAX_FRESH_EVENTS: usize = 256;

/// Откуда бьёт фонтан для замера: чуть выше героя или из середины экрана.
fn fountain_at(player: Option<[f64; 2]>, camera: reyvec_vector::Camera2D) -> [f32; 2] {
    match player {
        Some(p) => [p[0] as f32, p[1] as f32 + 0.3],
        None => [camera.center.x as f32, camera.center.y as f32],
    }
}

/// Сценарий игры для кадров без окна и докуда мир уже прошёл.
struct PlayState {
    script: crate::HeadlessPlay,
    time: f64,
    held: Vec<reyvec_scene::game::Action>,
    started: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_rect_in_mask_points_rounds_outward() {
        let r = |x, y, width, height| vk::Rect2D {
            offset: vk::Offset2D { x, y },
            extent: vk::Extent2D { width, height },
        };
        let size = vk::Extent2D {
            width: 480,
            height: 253,
        };
        assert_eq!(mask_rect(r(10, 21, 7, 4), 4, size), r(2, 5, 3, 2));
        assert_eq!(mask_rect(r(8, 20, 8, 4), 4, size), r(2, 5, 2, 1));
        // У края кадра не выходит за маску.
        assert_eq!(mask_rect(r(1900, 1000, 20, 9), 4, size), r(475, 250, 5, 3));
    }

    #[test]
    fn cascades_reach_across_the_screen() {
        // Поле 960×540 (кадр 1920×1080, поле вдвое меньше): точка поля 2/540 единицы.
        let field = vk::Extent2D {
            width: 960,
            height: 540,
        };
        let r0 = 2.0 / 540.0;
        let n = cascade_count(r0, 16.0 / 9.0, 1.0);
        let reach = r0 * (4f32.powi(n as i32) - 1.0) / 3.0;
        assert!(
            reach >= 2.0 * (16.0f32 / 9.0).hypot(1.0),
            "{n} каскадов, {reach}"
        );
        assert_eq!(n, 6);
        // Каждый каскад — картинка примерно вдвое больше поля по каждой стороне.
        for i in 0..n {
            let (size, probes) = cascade_size(field, i);
            assert_eq!(size.width, probes[0] * (2 << i));
            assert!(size.width >= 2 * field.width && size.width < 2 * field.width + (4 << i));
        }
    }

    #[test]
    fn bloom_levels_halve_down_to_a_thirty_second() {
        let frame = vk::Extent2D {
            width: 1920,
            height: 1009,
        };
        let sizes: Vec<(u32, u32)> = bloom_sizes(frame)
            .iter()
            .map(|s| (s.width, s.height))
            .collect();
        assert_eq!(
            sizes,
            [(960, 505), (480, 253), (240, 127), (120, 64), (60, 32)]
        );
        // Даже у крошечного кадра ступени не пропадают до нуля.
        let tiny = bloom_sizes(vk::Extent2D {
            width: 3,
            height: 1,
        });
        assert!(tiny.iter().all(|s| s.width >= 1 && s.height >= 1));
    }

    #[test]
    fn field_is_half_size_and_steps_reach_one() {
        let frame = vk::Extent2D {
            width: 1920,
            height: 1009,
        };
        let size = field_size(frame, field_downscale_for(AdapterKind::Discrete));
        assert_eq!((size.width, size.height), (960, 505));
        let small = field_size(frame, field_downscale_for(AdapterKind::Integrated));
        assert_eq!((small.width, small.height), (480, 253));
        assert_eq!(jfa_steps(small).len(), 10);
        assert_eq!(jfa_steps(size), [512, 256, 128, 64, 32, 16, 8, 4, 2, 1, 1]);
        let tiny = field_size(
            vk::Extent2D {
                width: 1,
                height: 1,
            },
            4,
        );
        assert_eq!((tiny.width, tiny.height), (1, 1));
        assert_eq!(jfa_steps(tiny), [1]);
    }

    #[test]
    fn msaa_defaults_and_cycle() {
        assert_eq!(Msaa::for_adapter(AdapterKind::Integrated), Msaa::X2);
        assert_eq!(Msaa::for_adapter(AdapterKind::Discrete), Msaa::X4);
        assert_eq!(Msaa::for_adapter(AdapterKind::Cpu), Msaa::X4);
        let mut m = Msaa::Off;
        let mut seen = Vec::new();
        for _ in 0..4 {
            m = m.next();
            seen.push(m);
        }

        assert_eq!(seen, [Msaa::X2, Msaa::X4, Msaa::X8, Msaa::Off]);
        // Видеокарта без 2×: берётся ближайшее меньшее.
        let no_x2 = vk::SampleCountFlags::TYPE_1 | vk::SampleCountFlags::TYPE_4;
        assert_eq!(Msaa::X2.supported(no_x2), Msaa::Off);
        assert_eq!(Msaa::X8.supported(no_x2), Msaa::X4);
        assert_eq!(Msaa::from_name("2"), Some(Msaa::X2));
    }

    /// Разрешение 3D (7.14): на встроенной половина, у нечётных сторон с запасом.
    #[test]
    fn space_scale_by_adapter() {
        assert_eq!(
            SpaceScale::for_adapter(AdapterKind::Integrated),
            SpaceScale::Half
        );
        assert_eq!(
            SpaceScale::for_adapter(AdapterKind::Discrete),
            SpaceScale::Full
        );
        let e = vk::Extent2D {
            width: 1921,
            height: 1081,
        };
        assert_eq!(SpaceScale::Full.apply(e), e);
        let half = SpaceScale::Half.apply(e);
        assert_eq!((half.width, half.height), (961, 541));
        let three = SpaceScale::ThreeQuarters.apply(e);
        assert_eq!((three.width, three.height), (1441, 811));
    }

    #[test]
    fn layer_images_are_rect_sized_with_headroom() {
        let e = |width, height| vk::Extent2D { width, height };
        let frame = e(1920, 1009);
        assert_eq!(layer_image_size(e(1, 1), frame), e(128, 128));
        assert_eq!(layer_image_size(e(130, 128), frame), e(256, 128));
        // Не больше кадра, но и не меньше прямоугольника.
        assert_eq!(layer_image_size(e(1900, 1000), frame), e(1920, 1009));
        assert_eq!(layer_image_size(e(100, 50), e(90, 40)), e(100, 50));
    }

    #[test]
    fn sprite_layout_matches_std430() {
        assert_eq!(SPRITE_SIZE, 48);
        let s = Sprite {
            pos: [1.0, 2.0],
            size: [3.0, 4.0],
            tint: [0.5; 4],
            rotation: 0.25,
            texture: 7,
            sampler: 1,
            pad: 0,
        };
        let b = s.to_bytes();
        assert_eq!(&b[0..4], &1.0f32.to_ne_bytes());
        assert_eq!(&b[36..40], &7u32.to_ne_bytes());
        assert_eq!(&b[40..44], &1u32.to_ne_bytes());
    }

    #[test]
    fn checker_alternates() {
        let px = checker_pixels(32, 16);
        let at = |x: usize, y: usize| px[(y * 32 + x) * 4];
        assert_ne!(at(0, 0), at(16, 0));
        assert_eq!(at(0, 0), at(16, 16));
    }

    #[test]
    fn disk_is_opaque_inside_and_clear_outside() {
        let px = disk_pixels(64);
        let alpha = |x: usize, y: usize| px[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(32, 32), 255);
        assert_eq!(alpha(0, 0), 0);
    }

    #[test]
    fn hues_are_bright() {
        assert_eq!(hue_to_rgb(0.0), [1.0, 0.0, 0.0]);
        let [r, g, b] = hue_to_rgb(1.0 / 3.0);
        assert!(g > 0.99 && r < 0.01 && b < 0.01);
    }
}
