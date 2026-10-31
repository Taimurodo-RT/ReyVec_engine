//! Рисование панели egui поверх кадра.
//!
//! egui сам раскладывает панель и превращает её в треугольники (`ClippedPrimitive`) и
//! изменения текстур (`TexturesDelta`). Здесь они попадают на видеокарту: треугольники в
//! буфер кадра (шейдер читает их по адресу), текстуры (атлас шрифта и картинки) в общий
//! список текстур. Загрузка текстур записывается в команды того же кадра, поэтому кадр
//! не ждёт видеокарту, а барьеры не дают затереть атлас, пока его читает прошлый кадр.

use std::collections::HashMap;

use ash::vk;

use crate::GpuError;
use crate::bindless::{Bindless, SamplerKind};
use crate::context::GpuContext;
use crate::graph::{Access, ImageHandle, RenderGraph};
use crate::pipeline::{Blend, graphics_pipeline};
use crate::resources::{BufferId, BufferMemory, GpuResources, TextureId, record_texture_upload};
use crate::shaders::ShaderLibrary;

/// Слов (u32) на одну вершину в буфере: x, y, u, v, цвет.
const VERTEX_WORDS: usize = 5;

/// Что egui нарисовал за кадр: передаётся в [`crate::WindowRenderer::render`].
#[derive(Clone, Debug, Default)]
pub struct UiFrame {
    /// Треугольники панели с прямоугольниками обрезки.
    pub primitives: Vec<egui::ClippedPrimitive>,
    /// Новые и изменённые текстуры egui (атлас шрифта и т. п.).
    pub textures_delta: egui::TexturesDelta,
    /// Сколько пикселей окна в одной точке egui.
    pub pixels_per_point: f32,
}

struct UiTexture {
    id: TextureId,
    size: [u32; 2],
    /// Текстура уже заполнялась (раскладка чтения), иначе она в UNDEFINED.
    filled: bool,
    sampler: SamplerKind,
}

/// Загрузка в текстуру: записывается в начале прохода панели.
#[derive(Clone, Copy)]
struct Upload {
    image: vk::Image,
    staging: vk::Buffer,
    offset: [u32; 2],
    extent: vk::Extent2D,
    first: bool,
}

#[derive(Clone, Copy)]
struct Draw {
    scissor: vk::Rect2D,
    first_index: u32,
    index_count: u32,
    base_vertex: u32,
    texture: u32,
    sampler: u32,
}

/// Всё, что проходу панели нужно в этом кадре. Готовится в [`UiRenderer::prepare`].
pub(crate) struct UiData {
    uploads: Vec<Upload>,
    draws: Vec<Draw>,
    indices_address: vk::DeviceAddress,
    vertices_address: vk::DeviceAddress,
    screen: [f32; 2],
}

/// Буфер вершин и индексов одного кадра в работе.
struct FrameBuffer {
    id: BufferId,
    capacity: usize,
}

pub(crate) struct UiRenderer {
    pipeline: vk::Pipeline,
    format: vk::Format,
    textures: HashMap<egui::TextureId, UiTexture>,
    frames: Vec<Option<FrameBuffer>>,
}

impl UiRenderer {
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
            textures: HashMap::new(),
            frames: (0..frames_in_flight).map(|_| None).collect(),
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

    /// Пересобирает конвейер после перезагрузки `ui.slang`. Видеокарта простаивает.
    pub fn reload(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &mut ShaderLibrary,
        names: &[String],
    ) {
        if !names.iter().any(|n| n == "ui") {
            return;
        }
        match build(ctx, res, shaders, self.format) {
            Ok(new) => {
                // SAFETY: видеокарта простаивает (условие функции).
                unsafe { ctx.device.destroy_pipeline(self.pipeline, None) };
                self.pipeline = new;
            }
            Err(e) => shaders.set_error("ui", e.to_string()),
        }
    }

    /// Готовит кадр панели: текстуры, вершины и список рисований. `slot`: номер кадра в
    /// работе, его буфер видеокарта сейчас не читает. `extent`: размер окна в пикселях.
    /// `textures`: изменения текстур, накопленные с прошлого нарисованного кадра.
    pub fn prepare(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        slot: usize,
        frame: &UiFrame,
        textures: &egui::TexturesDelta,
        extent: vk::Extent2D,
    ) -> Result<UiData, GpuError> {
        let uploads = self.update_textures(ctx, res, textures)?;

        let ppp = frame.pixels_per_point.max(0.01);
        let mut indices: Vec<u32> = Vec::new();
        let mut vertices: Vec<u32> = Vec::new();
        let mut draws = Vec::new();
        for prim in &frame.primitives {
            let egui::epaint::Primitive::Mesh(mesh) = &prim.primitive else {
                continue;
            };
            if mesh.indices.is_empty() {
                continue;
            }
            let Some(texture) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            let Some(scissor) = scissor(prim.clip_rect, ppp, extent) else {
                continue;
            };
            draws.push(Draw {
                scissor,
                first_index: indices.len() as u32,
                index_count: mesh.indices.len() as u32,
                base_vertex: (vertices.len() / VERTEX_WORDS) as u32,
                texture: texture.id.0,
                sampler: texture.sampler as u32,
            });
            indices.extend_from_slice(&mesh.indices);
            for v in &mesh.vertices {
                vertices.extend_from_slice(&[
                    v.pos.x.to_bits(),
                    v.pos.y.to_bits(),
                    v.uv.x.to_bits(),
                    v.uv.y.to_bits(),
                    u32::from_le_bytes(v.color.to_array()),
                ]);
            }
        }

        // Индексы и вершины лежат в одном буфере: сначала индексы, потом вершины.
        let index_bytes = indices.len() * 4;
        let total = index_bytes + vertices.len() * 4;
        let (indices_address, vertices_address) = if total == 0 {
            (0, 0)
        } else {
            let buffer = self.frame_buffer(ctx, res, slot, total)?;
            let mut bytes = Vec::with_capacity(total);
            for w in indices.iter().chain(&vertices) {
                bytes.extend_from_slice(&w.to_ne_bytes());
            }
            res.write_buffer(buffer, 0, &bytes);
            let base = res.buffer_address(buffer);
            (base, base + index_bytes as u64)
        };

        // Освобождаем текстуры, которые egui больше не нужны. Настоящее удаление случится,
        // когда видеокарта закончит этот кадр (отложенное удаление ресурсов).
        for id in &textures.free {
            if let Some(t) = self.textures.remove(id) {
                res.destroy_texture(t.id);
            }
        }

        Ok(UiData {
            uploads,
            draws,
            indices_address,
            vertices_address,
            screen: [extent.width as f32 / ppp, extent.height as f32 / ppp],
        })
    }

    fn update_textures(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        delta: &egui::TexturesDelta,
    ) -> Result<Vec<Upload>, GpuError> {
        let mut uploads = Vec::new();
        for (id, changes) in &delta.set {
            for change in changes {
                let egui::ImageData::Color(image) = &change.image;
                let size = [image.size[0] as u32, image.size[1] as u32];
                if size[0] == 0 || size[1] == 0 {
                    continue;
                }
                let sampler = match change.options.magnification {
                    egui::TextureFilter::Nearest => SamplerKind::Nearest,
                    egui::TextureFilter::Linear => SamplerKind::Linear,
                };
                let offset = match change.pos {
                    Some(pos) => [pos[0] as u32, pos[1] as u32],
                    None => {
                        // Вся текстура заново: если размер тот же, пишем в старую, иначе новая.
                        let reuse = self.textures.get(id).is_some_and(|t| t.size == size);
                        if !reuse {
                            if let Some(old) = self.textures.remove(id) {
                                res.destroy_texture(old.id);
                            }
                            let tex = res.create_texture_empty(
                                ctx,
                                size[0],
                                size[1],
                                true,
                                "текстура панели",
                            )?;
                            self.textures.insert(
                                *id,
                                UiTexture {
                                    id: tex,
                                    size,
                                    filled: false,
                                    sampler,
                                },
                            );
                        }
                        [0, 0]
                    }
                };
                let Some(texture) = self.textures.get_mut(id) else {
                    tracing::warn!("egui обновляет текстуру {id:?}, которой ещё нет");
                    continue;
                };
                texture.sampler = sampler;
                if offset[0] + size[0] > texture.size[0] || offset[1] + size[1] > texture.size[1] {
                    tracing::warn!("egui: кусок текстуры {id:?} выходит за её край");
                    continue;
                }
                let pixels: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                // Временный буфер с пикселями удаляется, когда видеокарта закончит этот кадр.
                let staging = res.create_buffer(
                    ctx,
                    pixels.len() as u64,
                    vk::BufferUsageFlags::TRANSFER_SRC,
                    BufferMemory::CpuToGpu,
                    "загрузка текстуры панели",
                )?;
                res.write_buffer(staging, 0, &pixels);
                let raw = res.buffer_raw(staging).expect("буфер только что создан");
                res.destroy_buffer(staging);
                uploads.push(Upload {
                    image: res.texture_image(texture.id).expect("текстура жива"),
                    staging: raw,
                    offset,
                    extent: vk::Extent2D {
                        width: size[0],
                        height: size[1],
                    },
                    first: !texture.filled,
                });
                texture.filled = true;
            }
        }
        Ok(uploads)
    }

    /// Буфер кадра `slot` не меньше `bytes`. Старый буфер, если мал, уходит в отложенное удаление.
    fn frame_buffer(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        slot: usize,
        bytes: usize,
    ) -> Result<BufferId, GpuError> {
        if let Some(fb) = &self.frames[slot]
            && fb.capacity >= bytes
        {
            return Ok(fb.id);
        }
        if let Some(old) = self.frames[slot].take() {
            res.destroy_buffer(old.id);
        }
        // С запасом, чтобы не пересоздавать буфер каждый раз, когда панель чуть подрастёт.
        let capacity = bytes.next_power_of_two().max(64 * 1024);
        let id = res.create_buffer(
            ctx,
            capacity as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            BufferMemory::CpuToGpu,
            &format!("панель, кадр {slot}"),
        )?;
        self.frames[slot] = Some(FrameBuffer { id, capacity });
        Ok(id)
    }

    /// Добавляет проход «панель»: рисует поверх `target` (содержимое сохраняется).
    pub fn add_pass<'a>(
        &'a self,
        graph: &mut RenderGraph<'a>,
        bindless: &'a Bindless,
        data: UiData,
        target: ImageHandle,
    ) {
        let layout = bindless.pipeline_layout;
        let pipeline = self.pipeline;
        graph.add_pass(
            "панель",
            &[(target, Access::ColorAttachment)],
            move |p| {
                let (d, cmd) = (p.device, p.cmd);
                let extent = p.extent(target);
                // SAFETY: временные буферы и текстуры удаляются только после окончания кадра;
                // граф перевёл цель в раскладку рисования; конвейер жив, пока кадр рисуется.
                unsafe {
                    for u in &data.uploads {
                        record_texture_upload(
                            d, cmd, u.image, u.staging, u.offset, u.extent, u.first,
                        );
                    }
                    if data.draws.is_empty() {
                        return;
                    }
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
                    bindless.bind(d, cmd);
                    d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                    for draw in &data.draws {
                        let push = push_constants(&data, draw);
                        d.cmd_set_scissor(cmd, 0, &[draw.scissor]);
                        d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                        d.cmd_draw(cmd, draw.index_count, 1, draw.first_index, 0);
                    }
                    d.cmd_end_rendering(cmd);
                }
            },
        );
    }

    /// Уничтожает конвейер. Текстуры и буферы уходят вместе с [`GpuResources`].
    pub fn destroy(self, ctx: &GpuContext) {
        // SAFETY: вызывается, когда видеокарта простаивает.
        unsafe { ctx.device.destroy_pipeline(self.pipeline, None) };
    }
}

/// Push-константы, как `Push` в `ui.slang`: два адреса, размер экрана, три числа.
fn push_constants(data: &UiData, draw: &Draw) -> [u8; 40] {
    let mut out = [0u8; 40];
    out[0..8].copy_from_slice(&data.indices_address.to_ne_bytes());
    out[8..16].copy_from_slice(&data.vertices_address.to_ne_bytes());
    out[16..20].copy_from_slice(&data.screen[0].to_ne_bytes());
    out[20..24].copy_from_slice(&data.screen[1].to_ne_bytes());
    out[24..28].copy_from_slice(&draw.texture.to_ne_bytes());
    out[28..32].copy_from_slice(&draw.sampler.to_ne_bytes());
    out[32..36].copy_from_slice(&draw.base_vertex.to_ne_bytes());
    out
}

/// Прямоугольник обрезки egui (в точках) в пикселях окна. `None`, если он пустой.
fn scissor(clip: egui::Rect, ppp: f32, extent: vk::Extent2D) -> Option<vk::Rect2D> {
    let x0 = (clip.min.x * ppp).round().clamp(0.0, extent.width as f32) as u32;
    let y0 = (clip.min.y * ppp).round().clamp(0.0, extent.height as f32) as u32;
    let x1 = (clip.max.x * ppp).round().clamp(0.0, extent.width as f32) as u32;
    let y1 = (clip.max.y * ppp).round().clamp(0.0, extent.height as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(vk::Rect2D {
        offset: vk::Offset2D {
            x: x0 as i32,
            y: y0 as i32,
        },
        extent: vk::Extent2D {
            width: x1 - x0,
            height: y1 - y0,
        },
    })
}

fn build(
    ctx: &GpuContext,
    res: &GpuResources,
    shaders: &ShaderLibrary,
    format: vk::Format,
) -> Result<vk::Pipeline, GpuError> {
    let pair = shaders.get("ui");
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

    #[test]
    fn scissor_is_clamped_to_window() {
        let extent = vk::Extent2D {
            width: 100,
            height: 50,
        };
        let r = egui::Rect::from_min_max(egui::pos2(-10.0, 5.0), egui::pos2(40.0, 100.0));
        let s = scissor(r, 2.0, extent).unwrap();
        assert_eq!((s.offset.x, s.offset.y), (0, 10));
        assert_eq!((s.extent.width, s.extent.height), (80, 40));
        let outside = egui::Rect::from_min_max(egui::pos2(60.0, 0.0), egui::pos2(70.0, 10.0));
        assert!(scissor(outside, 2.0, extent).is_none());
    }
}
