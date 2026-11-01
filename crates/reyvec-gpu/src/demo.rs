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
use crate::bindless::{Bindless, SamplerKind};
use crate::context::GpuContext;
use crate::demo_vector::VectorDemo;
use crate::graph::{Access, ImageHandle, PassContext, RenderGraph};
use crate::pipeline::Blend;
use crate::pipeline::{
    CLEAR_COLOR, Stencil, StencilUse, begin_color_pass, begin_scene_pass, graphics_pipeline_stencil,
};
use crate::resources::{BufferId, BufferMemory, GpuResources, TextureId};
use crate::shaders::ShaderLibrary;
use crate::vector::{FillMode, LayerTexture, Uploads, VectorData, VectorRenderer};

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
}

/// Свои файлы для сцен песочницы (содержимое файлов). Без них сцены показывают примеры.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserFiles {
    /// Рисунок SVG для сцены «SVG».
    pub svg: Option<std::sync::Arc<[u8]>>,
    /// Перетекающие фигуры от Motif (`*.reyvec.json`) для сцены «Motif».
    pub motif: Option<std::sync::Arc<[u8]>>,
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
        }
    }

    /// Сцена по имени из командной строки: `gpu`, `vector`, `morph`, `layers`, `masks`,
    /// `svg`, `motif`, `stream`, `text`, `stress` или `stress-morph`.
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
            _ => None,
        }
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
            Self::X4 => "MSAA 4×",
            Self::X8 => "MSAA 8×",
        }
    }

    /// Следующий вариант по кругу: выкл → 4× → 8× → выкл.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::X4,
            Self::X4 => Self::X8,
            Self::X8 => Self::Off,
        }
    }

    /// Вариант по имени из командной строки: `off`, `4` или `8`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "off" | "0" | "1" => Some(Self::Off),
            "4" => Some(Self::X4),
            "8" => Some(Self::X8),
            _ => None,
        }
    }

    pub(crate) fn samples(self) -> vk::SampleCountFlags {
        match self {
            Self::Off => vk::SampleCountFlags::TYPE_1,
            Self::X4 => vk::SampleCountFlags::TYPE_4,
            Self::X8 => vk::SampleCountFlags::TYPE_8,
        }
    }

    /// Самый близкий вариант не больше `self`, который видеокарта умеет для кадра сцены.
    pub(crate) fn supported(self, counts: vk::SampleCountFlags) -> Self {
        [Self::X8, Self::X4, Self::Off]
            .into_iter()
            .find(|m| *m <= self && counts.contains(m.samples()))
            .unwrap_or(Self::Off)
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
/// Формат кадра сцены: линейный цвет с запасом точности (для будущих эффектов и HDR).
pub(crate) const SCENE_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
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
    time: f32,
    aspect: f32,
    extent: vk::Extent2D,
}

pub(crate) struct Demo {
    triangle: vk::Pipeline,
    sprites: vk::Pipeline,
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
    /// Камера векторных сцен (сдвиг и приближение). Лупа — одно её положение.
    camera: reyvec_vector::Camera2D,
    /// Время процессора на подготовку векторного кадра, мс (сглаженное).
    vector_cpu_ms: f64,
    /// Векторных фигур и блоков текста в последнем кадре.
    vector_items: usize,
}

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
        let vector_demo = VectorDemo::new(files);
        let stencil_format = pick_stencil_format(ctx)?;
        let mut vector = VectorRenderer::new(ctx, res, shaders, frames_in_flight, stencil_format)?;
        // SAFETY: physical получен из этого экземпляра.
        let limits = unsafe {
            ctx.instance()
                .get_physical_device_properties(ctx.physical)
                .limits
        };
        if let Err(e) = vector
            .upload(ctx, res, &vector_demo.library)
            .and_then(|()| vector.upload_text(ctx, res, &vector_demo.text))
        {
            vector.destroy(ctx);
            return Err(e);
        }
        let mut demo = Self {
            triangle: vk::Pipeline::null(),
            sprites: vk::Pipeline::null(),
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
            // Картинку MSAA и рисуют, и читают в шейдере сведения; трафарет того же размера.
            msaa_counts: limits.framebuffer_color_sample_counts
                & limits.sampled_image_color_sample_counts
                & limits.framebuffer_stencil_sample_counts,
            perceptual_edges: false,
            stencil_format,
            camera: reyvec_vector::Camera2D::default(),
            vector_cpu_ms: 0.0,
            vector_items: 0,
        };
        // Если что-то не соберётся, Drop не нужен: destroy() убирает null-конвейеры спокойно.
        let built = (|| {
            demo.triangle = build(
                ctx,
                res,
                shaders,
                "triangle",
                SCENE_FORMAT,
                Msaa::Off,
                Some(stencil_format),
            )?;
            demo.sprites = build(
                ctx,
                res,
                shaders,
                "sprite",
                SCENE_FORMAT,
                Msaa::Off,
                Some(stencil_format),
            )?;
            demo.resolve = build(ctx, res, shaders, "resolve", SCENE_FORMAT, Msaa::Off, None)?;
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

    /// Камера векторных сцен.
    pub fn camera(&self) -> reyvec_vector::Camera2D {
        self.camera
    }

    /// Ставит камеру векторных сцен со следующего кадра.
    pub fn set_camera(&mut self, camera: reyvec_vector::Camera2D) {
        self.camera = camera;
    }

    /// Сколько процессор тратит на подготовку векторного кадра (раскладка фигур, запись
    /// экземпляров и команд), мс, сглажено за последние кадры, и сколько в кадре фигур.
    pub fn vector_cpu(&self) -> (f64, usize) {
        (self.vector_cpu_ms, self.vector_items)
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
        let triangle = build(
            ctx,
            res,
            shaders,
            "triangle",
            SCENE_FORMAT,
            msaa,
            Some(self.stencil_format),
        )?;
        let sprites = match build(
            ctx,
            res,
            shaders,
            "sprite",
            SCENE_FORMAT,
            msaa,
            Some(self.stencil_format),
        ) {
            Ok(p) => p,
            Err(e) => {
                // SAFETY: конвейер только что создан и нигде не используется.
                unsafe { ctx.device.destroy_pipeline(triangle, None) };
                return Err(e);
            }
        };
        if let Err(e) = self.vector.set_msaa(ctx, res, shaders, msaa) {
            // SAFETY: конвейеры только что созданы и нигде не используются.
            unsafe {
                ctx.device.destroy_pipeline(triangle, None);
                ctx.device.destroy_pipeline(sprites, None);
            }
            return Err(e);
        }
        // SAFETY: видеокарта простаивает (условие функции).
        unsafe {
            ctx.device.destroy_pipeline(self.triangle, None);
            ctx.device.destroy_pipeline(self.sprites, None);
        }
        self.triangle = triangle;
        self.sprites = sprites;
        self.msaa = msaa;
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
        // SAFETY: условие функции; null допустим.
        unsafe { ctx.device.destroy_pipeline(self.present, None) };
        self.present = present;
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
                "triangle" => (&mut self.triangle, SCENE_FORMAT, self.msaa, stencil),
                "sprite" => (&mut self.sprites, SCENE_FORMAT, self.msaa, stencil),
                "present" => (&mut self.present, self.format, Msaa::Off, None),
                "resolve" => (&mut self.resolve, SCENE_FORMAT, Msaa::Off, None),
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
        let vector = match scene {
            DemoScene::Gpu => {
                self.vector_items = 0;
                None
            }
            _ => {
                let _zone = crate::cpu_zone!("векторные фигуры");
                let started = std::time::Instant::now();
                let items: Vec<reyvec_vector::SceneItem> = match scene {
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
                };
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
                    .prepare(res, slot, &items, aspect, self.camera.view());
                let ms = started.elapsed().as_secs_f64() * 1000.0;
                // Сглаживание: число на панели не прыгает от кадра к кадру.
                self.vector_cpu_ms = if self.vector_items == 0 {
                    ms
                } else {
                    self.vector_cpu_ms * 0.9 + ms * 0.1
                };
                self.vector_items = items.len();
                data
            }
        };
        let sprites = self.sprites_at(scene, time, aspect, shader_error);
        let count = sprites.len().min(MAX_SPRITES);
        let bytes: Vec<u8> = sprites[..count].iter().flat_map(|s| s.to_bytes()).collect();
        let buffer = self.instance_buffers[slot];
        res.write_buffer(buffer, 0, &bytes);
        FrameData {
            scene,
            msaa: self.msaa,
            perceptual_edges: self.perceptual_edges,
            vector,
            sprites_address: res.buffer_address(buffer),
            sprite_count: count as u32,
            time,
            aspect,
            extent,
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
        let scene = graph.create("кадр сцены", frame.extent, SCENE_FORMAT);
        // Со сглаживанием сцена рисуется в картинку с несколькими точками на пиксель. Точки
        // усредняет в обычный кадр сцены сама видеокарта в конце прохода «сцена» (быстро:
        // картинка MSAA не пишется в память). Для краёв «как в редакторах» усредняет свой
        // проход «сведение MSAA» (resolve.slang): так дороже, особенно на встроенной видеокарте.
        let msaa = (frame.msaa != Msaa::Off).then(|| {
            graph.create_multisampled(
                "кадр сцены MSAA",
                frame.extent,
                SCENE_FORMAT,
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
                let image = graph.create("слой", size, SCENE_FORMAT);
                let ms = (frame.msaa != Msaa::Off).then(|| {
                    graph.create_multisampled("слой MSAA", size, SCENE_FORMAT, frame.msaa.samples())
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

        let mut uses = vec![
            (target_image, Access::ColorAttachment),
            (stencil, Access::StencilAttachment),
        ];
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

        graph.add_pass(
            "вывод",
            &[(scene, Access::Sampled), (target, Access::ColorAttachment)],
            move |p| {
                let push = p.texture(scene).to_ne_bytes();
                let (d, cmd) = (p.device, p.cmd);
                // SAFETY: граф перевёл сцену в чтение, а цель в раскладку рисования.
                unsafe {
                    begin_color_pass(d, cmd, p.view(target), p.extent(target));
                    bindless.bind(d, cmd);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.present);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                    d.cmd_end_rendering(cmd);
                }
            },
        );
    }

    fn destroy_pipelines(&mut self, ctx: &GpuContext) {
        // SAFETY: вызывается, когда видеокарта не использует конвейеры; null допустим.
        unsafe {
            ctx.device.destroy_pipeline(self.triangle, None);
            ctx.device.destroy_pipeline(self.sprites, None);
            ctx.device.destroy_pipeline(self.present, None);
            ctx.device.destroy_pipeline(self.resolve, None);
        }
        self.triangle = vk::Pipeline::null();
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
    let blend = if name == "sprite" {
        Blend::Alpha
    } else {
        Blend::Opaque
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
            usage: StencilUse::Ignore,
        }),
    )
}

/// Формат трафарета для прохода сцены: первый, который видеокарта умеет как цель рисования.
/// Только трафарет (S8) экономнее всего; AMD не умеет D24S8, поэтому есть и D32S8.
fn pick_stencil_format(ctx: &GpuContext) -> Result<vk::Format, GpuError> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
