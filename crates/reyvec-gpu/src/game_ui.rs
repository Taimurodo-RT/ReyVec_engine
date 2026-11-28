//! Проход «интерфейс»: интерфейс игры из `reyvec-ui` (решение 0022) поверх кадра.
//!
//! Процессор отдаёт готовый список прямоугольников и букв в пикселях окна
//! ([`reyvec_ui::UiDraw`]). Здесь он ложится в буфер кадра (64 байта на прямоугольник,
//! `Quad` в `game_ui.slang`) и рисуется одной командой: шесть вершин на прямоугольник.
//! Атлас букв загружается, только когда поменялся файл экрана.

use ash::vk;
use reyvec_text::GlyphAtlas;
use reyvec_ui::{QuadKind, UiDraw};

use crate::GpuError;
use crate::bindless::Bindless;
use crate::context::GpuContext;
use crate::graph::{Access, ImageHandle, RenderGraph};
use crate::pipeline::{Blend, graphics_pipeline};
use crate::resources::{BufferId, BufferMemory, GpuResources, TextureId};
use crate::shaders::ShaderLibrary;

/// Байт на прямоугольник: четыре `float4`, как `Quad` в `game_ui.slang`.
pub(crate) const QUAD_BYTES: usize = 64;

/// Что проходу нужно в этом кадре. Готовится в [`GameUiRenderer::prepare`].
pub(crate) struct GameUiData {
    address: vk::DeviceAddress,
    count: u32,
    screen: [f32; 2],
    atlas: u32,
    atlas_size: [f32; 2],
}

struct Atlas {
    id: TextureId,
    size: [u32; 2],
    key: u64,
}

pub(crate) struct GameUiRenderer {
    pipeline: vk::Pipeline,
    format: vk::Format,
    atlas: Option<Atlas>,
    /// Буфер прямоугольников каждого кадра в работе и его размер в байтах.
    frames: Vec<Option<(BufferId, usize)>>,
    /// Прямоугольники, которые рисовать (уже в байтах для видеокарты).
    quads: Vec<u8>,
}

impl GameUiRenderer {
    pub fn new(
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &ShaderLibrary,
        format: vk::Format,
        frames_in_flight: usize,
    ) -> Result<Self, GpuError> {
        Ok(Self {
            pipeline: build(ctx, res, shaders, format)?,
            format,
            atlas: None,
            frames: (0..frames_in_flight).map(|_| None).collect(),
            quads: Vec::new(),
        })
    }

    /// Пересобирает конвейер под новый формат окна. Видеокарта не должна использовать старый.
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
        let new = build(ctx, res, shaders, format)?;
        // SAFETY: условие функции.
        unsafe { ctx.device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = new;
        self.format = format;
        Ok(())
    }

    /// Пересобирает конвейер после перезагрузки `game_ui.slang`. Видеокарта простаивает.
    pub fn reload(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &mut ShaderLibrary,
        names: &[String],
    ) {
        if !names.iter().any(|n| n == "game_ui") {
            return;
        }
        match build(ctx, res, shaders, self.format) {
            Ok(new) => {
                // SAFETY: видеокарта простаивает (условие функции).
                unsafe { ctx.device.destroy_pipeline(self.pipeline, None) };
                self.pipeline = new;
            }
            Err(e) => shaders.set_error("game_ui", e.to_string()),
        }
    }

    /// Запоминает, что рисовать. `key` меняется вместе с файлом экрана: тогда атлас букв
    /// загружается заново (ждёт загрузку — это бывает только при правке файла).
    /// `None` — интерфейса нет.
    pub fn set(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        ui: Option<(&UiDraw, &GlyphAtlas, u64)>,
    ) -> Result<(), GpuError> {
        self.quads.clear();
        let Some((draw, atlas, key)) = ui else {
            return Ok(());
        };
        let (aw, ah) = atlas.size();
        if self.atlas.as_ref().is_none_or(|a| a.key != key) && aw > 0 && ah > 0 {
            let id = res.create_texture_rgba8(
                ctx,
                aw,
                ah,
                atlas.pixels(),
                false,
                "атлас букв интерфейса",
            )?;
            // Старый атлас может читать кадр в работе: удаление отложенное.
            if let Some(old) = self.atlas.replace(Atlas {
                id,
                size: [aw, ah],
                key,
            }) {
                res.destroy_texture(old.id);
            }
        }
        let size = self
            .atlas
            .as_ref()
            .map_or([1.0, 1.0], |a| [a.size[0] as f32, a.size[1] as f32]);
        self.quads = quad_bytes(draw, size);
        Ok(())
    }

    /// Пишет прямоугольники в буфер кадра `slot`. `None`, если рисовать нечего.
    pub fn prepare(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        slot: usize,
        extent: vk::Extent2D,
    ) -> Result<Option<GameUiData>, GpuError> {
        if self.quads.is_empty() {
            return Ok(None);
        }
        let bytes = self.quads.len();
        let buffer = match self.frames[slot] {
            Some((id, capacity)) if capacity >= bytes => id,
            _ => {
                if let Some((old, _)) = self.frames[slot].take() {
                    res.destroy_buffer(old);
                }
                let capacity = bytes.next_power_of_two().max(16 * 1024);
                let id = res.create_buffer(
                    ctx,
                    capacity as u64,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    BufferMemory::CpuToGpu,
                    &format!("интерфейс, кадр {slot}"),
                )?;
                self.frames[slot] = Some((id, capacity));
                id
            }
        };
        res.write_buffer(buffer, 0, &self.quads);
        let (atlas, atlas_size) = self.atlas.as_ref().map_or((0, [1.0, 1.0]), |a| {
            (a.id.0, [a.size[0] as f32, a.size[1] as f32])
        });
        Ok(Some(GameUiData {
            address: res.buffer_address(buffer),
            count: (bytes / QUAD_BYTES) as u32,
            screen: [extent.width as f32, extent.height as f32],
            atlas,
            atlas_size,
        }))
    }

    /// Добавляет проход «интерфейс»: рисует поверх `target` (содержимое сохраняется).
    pub fn add_pass<'a>(
        &'a self,
        graph: &mut RenderGraph<'a>,
        bindless: &'a Bindless,
        data: GameUiData,
        target: ImageHandle,
    ) {
        let layout = bindless.pipeline_layout;
        let pipeline = self.pipeline;
        graph.add_pass(
            "интерфейс",
            &[(target, Access::ColorAttachment)],
            move |p| {
                let (d, cmd) = (p.device, p.cmd);
                let extent = p.extent(target);
                let push = push_constants(&data);
                // SAFETY: буфер кадра удаляется только после окончания кадра; граф перевёл
                // цель в раскладку рисования; конвейер жив, пока кадр рисуется.
                unsafe {
                    let attachments = [vk::RenderingAttachmentInfo::default()
                        .image_view(p.view(target))
                        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                        .load_op(vk::AttachmentLoadOp::LOAD)
                        .store_op(vk::AttachmentStoreOp::STORE)];
                    let rendering = vk::RenderingInfo::default()
                        .render_area(vk::Rect2D::default().extent(extent))
                        .layer_count(1)
                        .color_attachments(&attachments);
                    d.cmd_begin_rendering(cmd, &rendering);
                    d.cmd_set_viewport(
                        cmd,
                        0,
                        &[vk::Viewport::default()
                            .width(extent.width as f32)
                            .height(extent.height as f32)
                            .max_depth(1.0)],
                    );
                    d.cmd_set_scissor(cmd, 0, &[vk::Rect2D::default().extent(extent)]);
                    bindless.bind(d, cmd);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                    d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                    d.cmd_draw(cmd, 6, data.count, 0, 0);
                    d.cmd_end_rendering(cmd);
                }
            },
        );
    }

    /// Уничтожает конвейер. Атлас и буферы уходят вместе с [`GpuResources`].
    pub fn destroy(self, ctx: &GpuContext) {
        // SAFETY: вызывается, когда видеокарта простаивает.
        unsafe { ctx.device.destroy_pipeline(self.pipeline, None) };
    }
}

/// Прямоугольники в байтах `Quad` из `game_ui.slang`. `atlas`: размер атласа в пикселях
/// (места букв переводятся в доли атласа).
pub(crate) fn quad_bytes(draw: &UiDraw, atlas: [f32; 2]) -> Vec<u8> {
    let mut out = Vec::with_capacity(draw.quads.len() * QUAD_BYTES);
    for q in &draw.quads {
        let (extra, params) = match q.kind {
            QuadKind::Block {
                radius,
                border,
                border_color,
            } => (border_color, [radius, border, 0.0, 0.0]),
            QuadKind::Glyph { uv } => (
                [
                    uv[0] / atlas[0],
                    uv[1] / atlas[1],
                    uv[2] / atlas[0],
                    uv[3] / atlas[1],
                ],
                [0.0, 0.0, 1.0, 0.0],
            ),
        };
        for f in q.rect.iter().chain(&q.color).chain(&extra).chain(&params) {
            out.extend_from_slice(&f.to_ne_bytes());
        }
    }
    out
}

/// Push-константы, как `Push` в `game_ui.slang`.
fn push_constants(data: &GameUiData) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0..8].copy_from_slice(&data.address.to_ne_bytes());
    out[8..12].copy_from_slice(&data.screen[0].to_ne_bytes());
    out[12..16].copy_from_slice(&data.screen[1].to_ne_bytes());
    out[16..20].copy_from_slice(&data.atlas.to_ne_bytes());
    out[20..24].copy_from_slice(&reyvec_text::RANGE.to_ne_bytes());
    out[24..28].copy_from_slice(&data.atlas_size[0].to_ne_bytes());
    out[28..32].copy_from_slice(&data.atlas_size[1].to_ne_bytes());
    out
}

fn build(
    ctx: &GpuContext,
    res: &GpuResources,
    shaders: &ShaderLibrary,
    format: vk::Format,
) -> Result<vk::Pipeline, GpuError> {
    let pair = shaders.get("game_ui");
    graphics_pipeline(
        &ctx.device,
        res.bindless.pipeline_layout,
        &pair.vs,
        &pair.fs,
        format,
        Blend::Premultiplied,
        vk::SampleCountFlags::TYPE_1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_ui::UiQuad;

    #[test]
    fn quads_pack_like_the_shader() {
        let draw = UiDraw {
            quads: vec![
                UiQuad {
                    rect: [1.0, 2.0, 3.0, 4.0],
                    color: [0.1, 0.2, 0.3, 0.4],
                    kind: QuadKind::Block {
                        radius: 5.0,
                        border: 6.0,
                        border_color: [0.5, 0.6, 0.7, 0.8],
                    },
                },
                UiQuad {
                    rect: [0.0; 4],
                    color: [1.0; 4],
                    kind: QuadKind::Glyph {
                        uv: [64.0, 32.0, 128.0, 64.0],
                    },
                },
            ],
            targets: Vec::new(),
            solid: Vec::new(),
            viewport: [10.0, 10.0],
        };
        let bytes = quad_bytes(&draw, [256.0, 128.0]);
        assert_eq!(bytes.len(), 2 * QUAD_BYTES);
        let f = |i: usize| f32::from_ne_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!(
            (f(0), f(3), f(7), f(8), f(12), f(13), f(14)),
            (1.0, 4.0, 0.4, 0.5, 5.0, 6.0, 0.0)
        );
        assert_eq!(
            (f(16 + 8), f(16 + 9), f(16 + 10), f(16 + 11), f(16 + 14)),
            (0.25, 0.25, 0.5, 0.5, 1.0)
        );
    }
}
