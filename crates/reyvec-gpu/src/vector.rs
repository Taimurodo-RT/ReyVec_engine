//! Векторный слой на видеокарте. Способ 1: фигуры, заранее разбитые на треугольники.
//!
//! Треугольники всех фигур [`ShapeLibrary`] лежат в двух общих буферах на видеокарте и
//! загружаются только когда набор фигур меняется. Каждый кадр процессор пишет:
//!
//! - список экземпляров (48 байт на фигуру: матрица, сдвиг, цвет, где её треугольники);
//! - список команд рисования: подряд идущие экземпляры одной фигуры дают одну команду.
//!
//! Видеокарта получает всё это одним вызовом `vkCmdDrawIndirect`, поэтому число фигур
//! почти не влияет на работу процессора при записи кадра. Порядок экземпляров сохраняется:
//! следующие рисуются поверх предыдущих.

use ash::vk;
use reyvec_vector::{Color, Instance, ShapeLibrary};

use crate::GpuError;
use crate::context::GpuContext;
use crate::demo::{Msaa, SCENE_FORMAT};
use crate::pipeline::{Blend, graphics_pipeline};
use crate::resources::{BufferId, BufferMemory, GpuResources};
use crate::shaders::ShaderLibrary;

/// Сколько экземпляров фигур помещается в кадр. Больше пока не нужно (стресс-тест этапа
/// 3 рассчитан на 10 000); лишние не рисуются, а в лог пишется предупреждение.
pub(crate) const MAX_INSTANCES: usize = 16 * 1024;
/// Байт на экземпляр: раскладка `Instance` в `vector.slang` (std430).
const INSTANCE_SIZE: usize = 48;
/// Байт на команду рисования (`VkDrawIndirectCommand`).
const COMMAND_SIZE: usize = 16;

/// Где в общих буферах лежат треугольники одной фигуры.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MeshRange {
    first_index: u32,
    index_count: u32,
    color: Color,
}

/// Треугольники всех фигур на видеокарте.
struct Meshes {
    vertices: BufferId,
    indices: BufferId,
    ranges: Vec<MeshRange>,
    revision: u64,
}

/// Буферы одного кадра в работе: пока видеокарта читает одни, процессор пишет в другие.
struct FrameBuffers {
    instances: BufferId,
    commands: BufferId,
}

/// Что нужно команде рисования в этом кадре. Готовится в [`VectorRenderer::prepare`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct VectorData {
    indices: vk::DeviceAddress,
    vertices: vk::DeviceAddress,
    instances: vk::DeviceAddress,
    commands: vk::Buffer,
    draw_count: u32,
    aspect: f32,
}

pub(crate) struct VectorRenderer {
    pipeline: vk::Pipeline,
    /// Сглаживание, под которое собран конвейер.
    msaa: Msaa,
    meshes: Option<Meshes>,
    frames: Vec<FrameBuffers>,
    /// Предупреждение о переполнении уже было в логе.
    warned_overflow: bool,
}

impl VectorRenderer {
    pub fn new(
        ctx: &GpuContext,
        res: &mut GpuResources,
        shaders: &ShaderLibrary,
        frames_in_flight: usize,
    ) -> Result<Self, GpuError> {
        let mut frames = Vec::with_capacity(frames_in_flight);
        for i in 0..frames_in_flight {
            frames.push(FrameBuffers {
                instances: res.create_buffer(
                    ctx,
                    (MAX_INSTANCES * INSTANCE_SIZE) as u64,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    BufferMemory::CpuToGpu,
                    &format!("векторные экземпляры кадра {i}"),
                )?,
                commands: res.create_buffer(
                    ctx,
                    (MAX_INSTANCES * COMMAND_SIZE) as u64,
                    vk::BufferUsageFlags::INDIRECT_BUFFER,
                    BufferMemory::CpuToGpu,
                    &format!("векторные команды кадра {i}"),
                )?,
            });
        }
        Ok(Self {
            pipeline: build(ctx, res, shaders, Msaa::Off)?,
            msaa: Msaa::Off,
            meshes: None,
            frames,
            warned_overflow: false,
        })
    }

    /// Пересобирает конвейер после перезагрузки `vector.slang`. Видеокарта простаивает.
    pub fn reload(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &mut ShaderLibrary,
        names: &[String],
    ) {
        if !names.iter().any(|n| n == "vector") {
            return;
        }
        match build(ctx, res, shaders, self.msaa) {
            Ok(new) => {
                // SAFETY: видеокарта простаивает (условие функции).
                unsafe { ctx.device.destroy_pipeline(self.pipeline, None) };
                self.pipeline = new;
            }
            Err(e) => shaders.set_error("vector", e.to_string()),
        }
    }

    /// Пересобирает конвейер под другое сглаживание. Видеокарта простаивает.
    pub fn set_msaa(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &ShaderLibrary,
        msaa: Msaa,
    ) -> Result<(), GpuError> {
        if msaa == self.msaa {
            return Ok(());
        }
        let new = build(ctx, res, shaders, msaa)?;
        // SAFETY: видеокарта простаивает (условие функции).
        unsafe { ctx.device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = new;
        self.msaa = msaa;
        Ok(())
    }

    /// Загружает треугольники фигур на видеокарту, если набор фигур изменился с прошлой
    /// загрузки. Ждёт окончания загрузки: вызывать при загрузке сцены, а не каждый кадр.
    pub fn upload(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        library: &ShapeLibrary,
    ) -> Result<(), GpuError> {
        if self
            .meshes
            .as_ref()
            .is_some_and(|m| m.revision == library.revision())
        {
            return Ok(());
        }
        let mut vertices: Vec<u8> = Vec::new();
        let mut indices: Vec<u8> = Vec::new();
        let mut ranges = Vec::with_capacity(library.len());
        let mut vertex_count = 0u32;
        let mut index_count = 0u32;
        for (_, shape, mesh) in library.iter() {
            for (v, e) in mesh.vertices.iter().zip(&mesh.edges) {
                pack_vertex(&mut vertices, *v, e);
            }
            // Номера точек сразу общие для всех фигур: шейдеру не нужно знать, где
            // начинаются точки фигуры.
            for &i in &mesh.indices {
                indices.extend_from_slice(&(i + vertex_count).to_ne_bytes());
            }
            ranges.push(MeshRange {
                first_index: index_count,
                index_count: mesh.indices.len() as u32,
                color: shape.color,
            });
            vertex_count += mesh.vertices.len() as u32;
            index_count += mesh.indices.len() as u32;
        }
        let usage = vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST;
        let vertices_id = res.create_buffer(
            ctx,
            vertices.len() as u64,
            usage,
            BufferMemory::GpuOnly,
            "точки векторных фигур",
        )?;
        let indices_id = match res.create_buffer(
            ctx,
            indices.len() as u64,
            usage,
            BufferMemory::GpuOnly,
            "треугольники векторных фигур",
        ) {
            Ok(id) => id,
            Err(e) => {
                res.destroy_buffer(vertices_id);
                return Err(e);
            }
        };
        let uploaded = res
            .upload_buffer(ctx, vertices_id, &vertices)
            .and_then(|()| res.upload_buffer(ctx, indices_id, &indices));
        if let Err(e) = uploaded {
            res.destroy_buffer(vertices_id);
            res.destroy_buffer(indices_id);
            return Err(e);
        }
        // Старые буферы может ещё читать кадр в работе: они уходят в очередь отложенного удаления.
        if let Some(old) = self.meshes.take() {
            res.destroy_buffer(old.vertices);
            res.destroy_buffer(old.indices);
        }
        tracing::info!(
            "Векторные фигуры на видеокарте: {} фигур, {} треугольников, {} КБ",
            library.len(),
            index_count / 3,
            (vertices.len() + indices.len()) / 1024
        );
        self.meshes = Some(Meshes {
            vertices: vertices_id,
            indices: indices_id,
            ranges,
            revision: library.revision(),
        });
        Ok(())
    }

    /// Пишет экземпляры и команды рисования кадра в буферы ячейки `slot`. Видеокарта не
    /// должна в этот момент читать буферы `slot`. `None`, если рисовать нечего.
    pub fn prepare(
        &mut self,
        res: &mut GpuResources,
        slot: usize,
        instances: &[Instance],
        aspect: f32,
    ) -> Option<VectorData> {
        let meshes = self.meshes.as_ref()?;
        if instances.len() > MAX_INSTANCES && !self.warned_overflow {
            tracing::warn!(
                "Векторных фигур в кадре {}, а помещается {MAX_INSTANCES}: лишние не рисуются",
                instances.len()
            );
            self.warned_overflow = true;
        }
        let (instance_bytes, command_bytes, draw_count) = pack(
            &meshes.ranges,
            &instances[..instances.len().min(MAX_INSTANCES)],
        );
        if draw_count == 0 {
            return None;
        }
        let frame = &self.frames[slot];
        res.write_buffer(frame.instances, 0, &instance_bytes);
        res.write_buffer(frame.commands, 0, &command_bytes);
        Some(VectorData {
            indices: res.buffer_address(meshes.indices),
            vertices: res.buffer_address(meshes.vertices),
            instances: res.buffer_address(frame.instances),
            commands: res.buffer_raw(frame.commands)?,
            draw_count,
            aspect,
        })
    }

    /// Записывает рисование фигур в открытый проход рисования в кадр сцены.
    ///
    /// # Safety
    /// `cmd` записывается внутри `vkCmdBeginRendering` с картинкой формата
    /// [`SCENE_FORMAT`], общая раскладка конвейеров уже привязана; буферы `data` живы, пока
    /// видеокарта рисует кадр.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        data: VectorData,
    ) {
        let mut push = [0u8; 28];
        push[..8].copy_from_slice(&data.indices.to_ne_bytes());
        push[8..16].copy_from_slice(&data.vertices.to_ne_bytes());
        push[16..24].copy_from_slice(&data.instances.to_ne_bytes());
        push[24..].copy_from_slice(&data.aspect.to_ne_bytes());
        // SAFETY: условия функции; число команд не больше MAX_INSTANCES, буфер команд их вмещает.
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
            device.cmd_draw_indirect(cmd, data.commands, 0, data.draw_count, COMMAND_SIZE as u32);
        }
    }

    /// Уничтожает конвейер, когда видеокарта его не использует. Буферы уходят вместе с
    /// [`GpuResources`].
    pub fn destroy(&mut self, ctx: &GpuContext) {
        // SAFETY: условие функции; null допустим.
        unsafe { ctx.device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = vk::Pipeline::null();
    }
}

/// Раскладывает экземпляры в байты для видеокарты: экземпляры (std430) и команды
/// рисования. Подряд идущие экземпляры одной фигуры становятся одной командой.
/// Экземпляры неизвестных и пустых фигур пропускаются.
fn pack(ranges: &[MeshRange], instances: &[Instance]) -> (Vec<u8>, Vec<u8>, u32) {
    let mut out = Vec::with_capacity(instances.len() * INSTANCE_SIZE);
    let mut commands: Vec<[u32; 4]> = Vec::new();
    let mut last_shape = None;
    let mut written = 0u32;
    for inst in instances {
        let Some(range) = ranges.get(inst.shape.0 as usize) else {
            continue;
        };
        if range.index_count == 0 {
            continue;
        }
        let [a, b, c, d, e, f] = inst.transform.as_coeffs().map(|v| v as f32);
        let color = range.color.multiply(inst.tint).to_array();
        for v in [a, b, c, d, e, f] {
            out.extend_from_slice(&v.to_ne_bytes());
        }
        out.extend_from_slice(&range.first_index.to_ne_bytes());
        out.extend_from_slice(&0u32.to_ne_bytes());
        for v in color {
            out.extend_from_slice(&v.to_ne_bytes());
        }
        match commands.last_mut() {
            Some(cmd) if last_shape == Some(inst.shape) => cmd[1] += 1,
            _ => commands.push([range.index_count, 1, 0, written]),
        }
        last_shape = Some(inst.shape);
        written += 1;
    }
    let command_bytes = commands
        .iter()
        .flatten()
        .flat_map(|w| w.to_ne_bytes())
        .collect();
    (out, command_bytes, commands.len() as u32)
}

fn build(
    ctx: &GpuContext,
    res: &GpuResources,
    shaders: &ShaderLibrary,
    msaa: Msaa,
) -> Result<vk::Pipeline, GpuError> {
    let pair = shaders.get("vector");
    graphics_pipeline(
        &ctx.device,
        res.bindless.pipeline_layout,
        &pair.vs,
        &pair.fs,
        SCENE_FORMAT,
        Blend::Alpha,
        msaa.samples(),
    )
}

/// Размер точки на видеокарте: положение, смещение от контура, данные края (см. `vector.slang`).
const VERTEX_SIZE: usize = 32;

/// Пишет точку фигуры в формате шейдера: `float2 position; float2 offset; float4 edge`
/// (поперёк, полутолщина, вид края, 0).
fn pack_vertex(out: &mut Vec<u8>, position: [f32; 2], edge: &reyvec_vector::EdgeData) {
    let start = out.len();
    for f in [
        position[0],
        position[1],
        edge.offset[0],
        edge.offset[1],
        edge.across,
        edge.half_width,
        edge.round,
        0.0,
    ] {
        out.extend_from_slice(&f.to_ne_bytes());
    }
    debug_assert_eq!(out.len() - start, VERTEX_SIZE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_vector::{Affine, ShapeId};

    fn words(bytes: &[u8]) -> Vec<u32> {
        bytes
            .chunks_exact(4)
            .map(|c| u32::from_ne_bytes(c.try_into().unwrap()))
            .collect()
    }

    #[test]
    fn runs_of_same_shape_become_one_command() {
        let red = Color::linear(1.0, 0.0, 0.0, 1.0);
        let ranges = [
            MeshRange {
                first_index: 0,
                index_count: 6,
                color: red,
            },
            MeshRange {
                first_index: 6,
                index_count: 3,
                color: Color::WHITE,
            },
            MeshRange {
                first_index: 9,
                index_count: 0,
                color: Color::WHITE,
            },
        ];
        let at = |shape: u32, x: f64| Instance::new(ShapeId(shape), Affine::translate((x, 2.0)));
        let instances = [
            at(0, 1.0),
            at(0, 2.0),
            at(2, 0.0), // пустая фигура: пропускается и не рвёт отрезок
            at(0, 3.0),
            at(1, 4.0),
            at(7, 0.0), // неизвестная фигура
            at(0, 5.0).with_tint(Color::linear(0.5, 1.0, 1.0, 0.25)),
        ];
        let (inst, cmds, count) = pack(&ranges, &instances);
        assert_eq!(inst.len(), 5 * INSTANCE_SIZE);
        assert_eq!(count, 3);
        assert_eq!(
            words(&cmds),
            [6, 3, 0, 0, 3, 1, 0, 3, 6, 1, 0, 4],
            "команды: число номеров, экземпляров, первая точка, первый экземпляр"
        );
        // Последний экземпляр: сдвиг (5, 2), первый номер 0, цвет красный × оттенок.
        let last = &inst[4 * INSTANCE_SIZE..];
        let f = |i: usize| f32::from_ne_bytes(last[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!([f(0), f(1), f(2), f(3)], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!([f(4), f(5)], [5.0, 2.0]);
        assert_eq!(words(&last[24..32]), [0, 0]);
        assert_eq!([f(8), f(9), f(10), f(11)], [0.5, 0.0, 0.0, 0.25]);
    }
}
