//! Сцена песочницы этапа 2: треугольник этапа 1 плюс спрайты с текстурами.
//!
//! Показывает, что работают память видеокарты, загрузка текстур, общий список
//! текстур (bindless) и чтение данных спрайтов из буфера по адресу.
//! На этапе 2 эту сцену заменит граф рендера, а спрайты станут частью векторного слоя.

use ash::vk;

use crate::GpuError;
use crate::bindless::SamplerKind;
use crate::context::GpuContext;
use crate::pipeline::{Blend, SPRITE_FRAG, SPRITE_VERT, TRIANGLE_FRAG, TRIANGLE_VERT};
use crate::pipeline::{begin_color_pass, graphics_pipeline};
use crate::resources::{BufferId, BufferMemory, GpuResources, TextureId};

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

pub(crate) struct Demo {
    triangle: vk::Pipeline,
    sprites: vk::Pipeline,
    format: vk::Format,
    checker: TextureId,
    /// Какой вариант шахматки сейчас загружен (меняется каждые 2 секунды).
    checker_phase: u32,
    disk: TextureId,
    /// Свой буфер спрайтов на каждый кадр в работе: пока видеокарта читает один,
    /// процессор пишет в другой.
    instance_buffers: Vec<BufferId>,
}

impl Demo {
    pub fn new(
        ctx: &GpuContext,
        res: &mut GpuResources,
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
            format: vk::Format::UNDEFINED,
            checker,
            checker_phase: 0,
            disk,
            instance_buffers,
        };
        demo.set_format(ctx, res, format)?;
        Ok(demo)
    }

    /// Пересобирает конвейеры, если поменялся формат картинки, в которую рисуем.
    pub fn set_format(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        format: vk::Format,
    ) -> Result<(), GpuError> {
        if format == self.format {
            return Ok(());
        }
        let layout = res.bindless.pipeline_layout;
        let d = &ctx.device;
        let triangle = graphics_pipeline(
            d,
            layout,
            TRIANGLE_VERT,
            TRIANGLE_FRAG,
            format,
            Blend::Opaque,
        )?;
        let sprites =
            match graphics_pipeline(d, layout, SPRITE_VERT, SPRITE_FRAG, format, Blend::Alpha) {
                Ok(p) => p,
                Err(e) => {
                    // SAFETY: конвейер только что создан и нигде не используется.
                    unsafe { d.destroy_pipeline(triangle, None) };
                    return Err(e);
                }
            };
        self.destroy_pipelines(ctx);
        self.triangle = triangle;
        self.sprites = sprites;
        self.format = format;
        Ok(())
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

    /// Спрайты сцены в момент `time` (секунды).
    pub fn sprites_at(&self, time: f32) -> Vec<Sprite> {
        let mut out = Vec::with_capacity(2 + ORBITERS);
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
        out
    }

    /// Записывает кадр: фон, треугольник, спрайты. `slot`: номер кадра в работе.
    ///
    /// # Safety
    /// `cmd` в состоянии записи; `view` в раскладке `COLOR_ATTACHMENT_OPTIMAL` и жив до
    /// конца выполнения; буфер спрайтов `slot` видеокарта сейчас не читает.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn record(
        &self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        cmd: vk::CommandBuffer,
        slot: usize,
        view: vk::ImageView,
        extent: vk::Extent2D,
        time: f32,
    ) {
        let d = &ctx.device;
        let aspect = extent.width as f32 / extent.height.max(1) as f32;
        let sprites = self.sprites_at(time);
        let count = sprites.len().min(MAX_SPRITES);
        let bytes: Vec<u8> = sprites[..count].iter().flat_map(|s| s.to_bytes()).collect();
        let buffer = self.instance_buffers[slot];
        res.write_buffer(buffer, 0, &bytes);
        let address = res.buffer_address(buffer);

        let mut triangle_push = [0u8; 8];
        triangle_push[..4].copy_from_slice(&time.to_ne_bytes());
        triangle_push[4..].copy_from_slice(&aspect.to_ne_bytes());
        let mut sprite_push = [0u8; 16];
        sprite_push[..8].copy_from_slice(&address.to_ne_bytes());
        sprite_push[8..12].copy_from_slice(&aspect.to_ne_bytes());

        let layout = res.bindless.pipeline_layout;
        // SAFETY: условия описаны в контракте функции.
        unsafe {
            begin_color_pass(d, cmd, view, extent);
            res.bindless.bind(d, cmd);
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.triangle);
            d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &triangle_push);
            d.cmd_draw(cmd, 3, 1, 0, 0);
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.sprites);
            d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &sprite_push);
            d.cmd_draw(cmd, 6, count as u32, 0, 0);
            d.cmd_end_rendering(cmd);
        }
    }

    fn destroy_pipelines(&mut self, ctx: &GpuContext) {
        // SAFETY: вызывается, когда видеокарта не использует конвейеры; null допустим.
        unsafe {
            ctx.device.destroy_pipeline(self.triangle, None);
            ctx.device.destroy_pipeline(self.sprites, None);
        }
        self.triangle = vk::Pipeline::null();
        self.sprites = vk::Pipeline::null();
    }

    /// Уничтожает конвейеры. Текстуры и буферы уходят вместе с [`GpuResources`].
    pub fn destroy(mut self, ctx: &GpuContext) {
        self.destroy_pipelines(ctx);
    }
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
