//! Сцена песочницы этапа 2: треугольник этапа 1 плюс спрайты с текстурами.
//!
//! Показывает, что работают память видеокарты, загрузка текстур, общий список
//! текстур (bindless), чтение данных спрайтов из буфера по адресу и граф кадра:
//! сцена рисуется во временную картинку (проход «сцена»), а потом копируется в окно
//! (проход «вывод»). Позже спрайты станут частью векторного слоя.

use ash::vk;

use crate::GpuError;
use crate::bindless::{Bindless, SamplerKind};
use crate::context::GpuContext;
use crate::graph::{Access, ImageHandle, RenderGraph};
use crate::pipeline::Blend;
use crate::pipeline::{begin_color_pass, graphics_pipeline};
use crate::resources::{BufferId, BufferMemory, GpuResources, TextureId};
use crate::shaders::ShaderLibrary;

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
}

impl Demo {
    pub fn new(
        ctx: &GpuContext,
        res: &mut GpuResources,
        shaders: &ShaderLibrary,
        format: vk::Format,
        frames_in_flight: usize,
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
        let mut demo = Self {
            triangle: vk::Pipeline::null(),
            sprites: vk::Pipeline::null(),
            present: vk::Pipeline::null(),
            format: vk::Format::UNDEFINED,
            checker,
            checker_phase: 0,
            disk,
            white,
            instance_buffers,
        };
        // Если что-то не соберётся, Drop не нужен: destroy() убирает null-конвейеры спокойно.
        let built = (|| {
            demo.triangle = build(ctx, res, shaders, "triangle", SCENE_FORMAT)?;
            demo.sprites = build(ctx, res, shaders, "sprite", SCENE_FORMAT)?;
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
        let present = build(ctx, res, shaders, "present", format)?;
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
        for name in names {
            let (slot, format) = match name.as_str() {
                "triangle" => (&mut self.triangle, SCENE_FORMAT),
                "sprite" => (&mut self.sprites, SCENE_FORMAT),
                "present" => (&mut self.present, self.format),
                _ => continue,
            };
            match build(ctx, res, shaders, name, format) {
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
    pub fn sprites_at(&self, time: f32, aspect: f32, shader_error: bool) -> Vec<Sprite> {
        let mut out = Vec::with_capacity(6 + ORBITERS);
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
        if shader_error {
            // Рамка по краю окна: слева, справа, сверху, снизу.
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
                    texture: self.white.0,
                    sampler: SamplerKind::Nearest as u32,
                    pad: 0,
                });
            }
        }
        out
    }

    /// Готовит данные кадра: пишет спрайты в буфер `slot` (номер кадра в работе).
    /// Буфер `slot` видеокарта в этот момент читать не должна.
    pub fn prepare(
        &self,
        res: &mut GpuResources,
        slot: usize,
        extent: vk::Extent2D,
        time: f32,
        shader_error: bool,
    ) -> FrameData {
        let aspect = extent.width as f32 / extent.height.max(1) as f32;
        let sprites = self.sprites_at(time, aspect, shader_error);
        let count = sprites.len().min(MAX_SPRITES);
        let bytes: Vec<u8> = sprites[..count].iter().flat_map(|s| s.to_bytes()).collect();
        let buffer = self.instance_buffers[slot];
        res.write_buffer(buffer, 0, &bytes);
        FrameData {
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
        let scene = graph.create("кадр сцены", frame.extent, SCENE_FORMAT);
        let layout = bindless.pipeline_layout;

        graph.add_pass(
            "сцена",
            &[(scene, Access::ColorAttachment)],
            move |p| {
                let mut triangle_push = [0u8; 8];
                triangle_push[..4].copy_from_slice(&frame.time.to_ne_bytes());
                triangle_push[4..].copy_from_slice(&frame.aspect.to_ne_bytes());
                let mut sprite_push = [0u8; 16];
                sprite_push[..8].copy_from_slice(&frame.sprites_address.to_ne_bytes());
                sprite_push[8..12].copy_from_slice(&frame.aspect.to_ne_bytes());
                let (d, cmd) = (p.device, p.cmd);
                // SAFETY: граф перевёл картинку сцены в раскладку цели рисования; конвейеры
                // и буфер спрайтов живы, пока видеокарта рисует этот кадр.
                unsafe {
                    begin_color_pass(d, cmd, p.view(scene), p.extent(scene));
                    bindless.bind(d, cmd);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.triangle);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &triangle_push);
                    d.cmd_draw(cmd, 3, 1, 0, 0);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.sprites);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &sprite_push);
                    d.cmd_draw(cmd, 6, frame.sprite_count, 0, 0);
                    d.cmd_end_rendering(cmd);
                }
            },
        );

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
        }
        self.triangle = vk::Pipeline::null();
        self.sprites = vk::Pipeline::null();
        self.present = vk::Pipeline::null();
    }

    /// Уничтожает конвейеры. Текстуры и буферы уходят вместе с [`GpuResources`].
    pub fn destroy(mut self, ctx: &GpuContext) {
        self.destroy_pipelines(ctx);
    }
}

fn build(
    ctx: &GpuContext,
    res: &GpuResources,
    shaders: &ShaderLibrary,
    name: &str,
    format: vk::Format,
) -> Result<vk::Pipeline, GpuError> {
    let pair = shaders.get(name);
    let blend = if name == "sprite" {
        Blend::Alpha
    } else {
        Blend::Opaque
    };
    graphics_pipeline(
        &ctx.device,
        res.bindless.pipeline_layout,
        &pair.vs,
        &pair.fs,
        format,
        blend,
    )
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
