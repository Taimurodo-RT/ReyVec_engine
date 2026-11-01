//! Векторный слой на видеокарте. Два способа рисовать заливки ([`FillMethod`], выбирается
//! для каждой фигуры, а для сравнения можно переключить на всю сцену: [`FillMode`]):
//!
//! 1. **Треугольники** (по умолчанию): фигуры заранее разбиты на треугольники (lyon).
//!    Треугольники всех фигур [`ShapeLibrary`] лежат в двух общих буферах на видеокарте и
//!    загружаются только когда набор фигур меняется. Каждый кадр процессор пишет список
//!    экземпляров (48 байт на фигуру) и список команд: подряд идущие экземпляры одной
//!    фигуры дают одну команду, и всё рисуется одним вызовом `vkCmdDrawIndirect`.
//! 2. **Трафарет** (Loop–Blinn + трафарет, см. `curves.rs` в reyvec-vector): для каждой
//!    залитой фигуры два прохода. Первый считает в трафарете обороты контура (кривые
//!    считаются точной формулой в шейдере), второй красит, где трафарет не ноль. Изгибы
//!    гладкие при любом приближении, и процессору не нужно разбивать фигуру. Цена: две
//!    команды на каждую залитую фигуру в кадре. Так же рисуются перетекающие фигуры:
//!    точки всех ключевых контуров лежат в буфере подряд, вершинный шейдер смешивает
//!    два соседних по числу `morph` из экземпляра.
//!
//! Обводки в обоих способах рисуются треугольниками. Порядок экземпляров сохраняется:
//! следующие рисуются поверх предыдущих.

use ash::vk;
use reyvec_vector::{
    Color, FillMethod, FillRule, Gradient, GradientKind, Instance, Paint, ShapeLibrary, Spread,
    Style,
};

use crate::GpuError;
use crate::context::GpuContext;
use crate::demo::{Msaa, SCENE_FORMAT};
use crate::pipeline::{Blend, Stencil, StencilUse, graphics_pipeline_stencil};
use crate::resources::{BufferId, BufferMemory, GpuResources};
use crate::shaders::ShaderLibrary;

/// Сколько экземпляров фигур помещается в кадр. Больше пока не нужно (стресс-тест этапа
/// 3 рассчитан на 10 000); лишние не рисуются, а в лог пишется предупреждение.
pub(crate) const MAX_INSTANCES: usize = 16 * 1024;
/// Байт на экземпляр: раскладка `Instance` в `vector.slang` (std430).
const INSTANCE_SIZE: usize = 64;
/// Байт на команду рисования (`VkDrawIndirectCommand`).
const COMMAND_SIZE: usize = 16;
/// Байт на градиент (`Paint` в `include/paint.slang`) и на его точку цвета.
const PAINT_SIZE: usize = 64;
const STOP_SIZE: usize = 32;
/// Номер закраски у фигур одного цвета (`NO_PAINT` в шейдере).
const NO_PAINT: u32 = u32::MAX;
/// Байт констант векторных конвейеров.
const PUSH_SIZE: usize = 48;

/// Как рисовать залитые фигуры во всей сцене.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillMode {
    /// Каждая фигура своим способом ([`reyvec_vector::ShapeLibrary::add_with`]).
    #[default]
    PerShape,
    /// Все треугольниками (кроме перетекающих: они только трафаретом). Для сравнения.
    Triangles,
    /// Все трафаретом. Для сравнения.
    Stencil,
}

impl FillMode {
    /// Название для панели и лога.
    pub fn label(self) -> &'static str {
        match self {
            FillMode::PerShape => "у каждой фигуры свой способ",
            FillMode::Triangles => "все треугольниками",
            FillMode::Stencil => "все трафаретом",
        }
    }

    /// Следующий вариант по кругу.
    pub fn next(self) -> Self {
        match self {
            FillMode::PerShape => FillMode::Triangles,
            FillMode::Triangles => FillMode::Stencil,
            FillMode::Stencil => FillMode::PerShape,
        }
    }

    /// Вариант по имени из командной строки: `shape`, `triangles` или `stencil`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "shape" | "per-shape" => Some(FillMode::PerShape),
            "triangles" => Some(FillMode::Triangles),
            "stencil" => Some(FillMode::Stencil),
            _ => None,
        }
    }

    /// Рисовать ли трафаретом фигуру со способом `method`.
    fn stencil(self, method: FillMethod) -> bool {
        match self {
            FillMode::PerShape => method == FillMethod::Stencil,
            FillMode::Triangles => false,
            FillMode::Stencil => true,
        }
    }
}

/// Где в общих буферах лежат треугольники одной фигуры.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MeshRange {
    first_index: u32,
    index_count: u32,
    /// Веер и кривые для трафарета (0 номеров у обводок).
    curve_first_index: u32,
    curve_index_count: u32,
    even_odd: bool,
    color: Color,
    method: FillMethod,
    /// Номер градиента в буфере закрасок или [`NO_PAINT`].
    paint: u32,
    /// Перетекание: сколько точек между ключевыми контурами в буфере и номер последнего
    /// ключевого контура (0 у обычных фигур).
    morph_stride: u32,
    morph_last: u32,
}

/// Треугольники всех фигур на видеокарте.
struct Meshes {
    vertices: BufferId,
    indices: BufferId,
    curve_vertices: BufferId,
    curve_indices: BufferId,
    paints: BufferId,
    stops: BufferId,
    ranges: Vec<MeshRange>,
    revision: u64,
}

/// Что рисовать в кадре, по порядку.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// Команды `first..first + count` из буфера команд (способ 1 и обводки).
    Triangles { first: u32, count: u32 },
    /// Одна залитая фигура через трафарет: экземпляр и число номеров веера и кривых.
    Stencil {
        instance: u32,
        index_count: u32,
        even_odd: bool,
    },
}

/// Буферы одного кадра в работе: пока видеокарта читает одни, процессор пишет в другие.
struct FrameBuffers {
    instances: BufferId,
    commands: BufferId,
    ops: Vec<Op>,
}

/// Что нужно команде рисования в этом кадре. Готовится в [`VectorRenderer::prepare`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct VectorData {
    indices: vk::DeviceAddress,
    vertices: vk::DeviceAddress,
    curve_indices: vk::DeviceAddress,
    curve_vertices: vk::DeviceAddress,
    paints: vk::DeviceAddress,
    stops: vk::DeviceAddress,
    instances: vk::DeviceAddress,
    commands: vk::Buffer,
    slot: usize,
    aspect: f32,
}

/// Конвейеры векторного слоя под одно сглаживание.
#[derive(Clone, Copy)]
struct Pipelines {
    triangles: vk::Pipeline,
    count_nonzero: vk::Pipeline,
    count_even_odd: vk::Pipeline,
    cover: vk::Pipeline,
}

impl Pipelines {
    const NULL: Self = Self {
        triangles: vk::Pipeline::null(),
        count_nonzero: vk::Pipeline::null(),
        count_even_odd: vk::Pipeline::null(),
        cover: vk::Pipeline::null(),
    };

    fn destroy(self, ctx: &GpuContext) {
        for p in [
            self.triangles,
            self.count_nonzero,
            self.count_even_odd,
            self.cover,
        ] {
            // SAFETY: вызывающий гарантирует, что видеокарта конвейеры не использует.
            unsafe { ctx.device.destroy_pipeline(p, None) };
        }
    }
}

pub(crate) struct VectorRenderer {
    pipelines: Pipelines,
    /// Сглаживание, под которое собраны конвейеры.
    msaa: Msaa,
    /// Формат трафарета прохода сцены.
    stencil_format: vk::Format,
    fill: FillMode,
    meshes: Option<Meshes>,
    frames: Vec<FrameBuffers>,
    /// Предупреждение о переполнении уже было в логе.
    warned_overflow: bool,
}

/// Шейдеры векторного слоя: при их перезагрузке конвейеры пересобираются.
const SHADERS: [&str; 3] = ["vector", "vector_stencil", "vector_cover"];

impl VectorRenderer {
    pub fn new(
        ctx: &GpuContext,
        res: &mut GpuResources,
        shaders: &ShaderLibrary,
        frames_in_flight: usize,
        stencil_format: vk::Format,
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
                ops: Vec::new(),
            });
        }
        Ok(Self {
            pipelines: build(ctx, res, shaders, Msaa::Off, stencil_format)?,
            msaa: Msaa::Off,
            stencil_format,
            fill: FillMode::default(),
            meshes: None,
            frames,
            warned_overflow: false,
        })
    }

    /// Как рисуются заливки.
    pub fn fill(&self) -> FillMode {
        self.fill
    }

    /// Способ рисования заливок со следующего кадра.
    pub fn set_fill(&mut self, fill: FillMode) {
        self.fill = fill;
    }

    /// Пересобирает конвейеры после перезагрузки шейдеров слоя. Видеокарта простаивает.
    pub fn reload(
        &mut self,
        ctx: &GpuContext,
        res: &GpuResources,
        shaders: &mut ShaderLibrary,
        names: &[String],
    ) {
        if !names.iter().any(|n| SHADERS.contains(&n.as_str())) {
            return;
        }
        match build(ctx, res, shaders, self.msaa, self.stencil_format) {
            Ok(new) => {
                // Видеокарта простаивает (условие функции).
                self.pipelines.destroy(ctx);
                self.pipelines = new;
            }
            Err(e) => {
                for n in names.iter().filter(|n| SHADERS.contains(&n.as_str())) {
                    shaders.set_error(n, e.to_string());
                }
            }
        }
    }

    /// Пересобирает конвейеры под другое сглаживание. Видеокарта простаивает.
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
        let new = build(ctx, res, shaders, msaa, self.stencil_format)?;
        // Видеокарта простаивает (условие функции).
        self.pipelines.destroy(ctx);
        self.pipelines = new;
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
        let mut curve_vertices: Vec<u8> = Vec::new();
        let mut curve_indices: Vec<u8> = Vec::new();
        let mut paints: Vec<u8> = Vec::new();
        let mut stops: Vec<u8> = Vec::new();
        let mut paint_count = 0u32;
        let mut stop_count = 0u32;
        let mut ranges = Vec::with_capacity(library.len());
        let mut vertex_count = 0u32;
        let mut index_count = 0u32;
        let mut curve_vertex_count = 0u32;
        let mut curve_index_count = 0u32;
        for (id, shape, mesh) in library.iter() {
            for (v, e) in mesh.vertices.iter().zip(&mesh.edges) {
                pack_vertex(&mut vertices, *v, e);
            }
            // Номера точек сразу общие для всех фигур: шейдеру не нужно знать, где
            // начинаются точки фигуры.
            for &i in &mesh.indices {
                indices.extend_from_slice(&(i + vertex_count).to_ne_bytes());
            }
            let curves = library.curves(id);
            let curve_first = curve_index_count;
            let keyframes = library.morph_keyframes(id);
            let mut morph_stride = 0;
            if let Some(c) = curves {
                // Точки первого ключевого контура, за ними остальных: номер точки в
                // контуре k = номер в первом + k · stride.
                for positions in std::iter::once(&c.vertices).chain(keyframes) {
                    for (p, uv) in positions.iter().zip(&c.uv) {
                        for f in [p[0], p[1], uv[0], uv[1]] {
                            curve_vertices.extend_from_slice(&f.to_ne_bytes());
                        }
                    }
                }
                for &i in &c.indices {
                    curve_indices.extend_from_slice(&(i + curve_vertex_count).to_ne_bytes());
                }
                morph_stride = c.vertices.len() as u32;
                curve_vertex_count += morph_stride * (1 + keyframes.len() as u32);
                curve_index_count += c.indices.len() as u32;
            }
            let (color, paint) = match &shape.paint {
                Paint::Solid(c) => (*c, NO_PAINT),
                Paint::Gradient(g) => {
                    pack_gradient(&mut paints, &mut stops, &mut stop_count, g);
                    paint_count += 1;
                    (Color::WHITE, paint_count - 1)
                }
            };
            ranges.push(MeshRange {
                first_index: index_count,
                index_count: mesh.indices.len() as u32,
                curve_first_index: curve_first,
                curve_index_count: curves.map_or(0, |c| c.indices.len() as u32),
                even_odd: matches!(shape.style, Style::Fill(FillRule::EvenOdd)),
                color,
                method: library.method(id).unwrap_or_default(),
                paint,
                morph_stride,
                morph_last: keyframes.len() as u32,
            });
            vertex_count += mesh.vertices.len() as u32;
            index_count += mesh.indices.len() as u32;
        }
        let usage = vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST;
        let mut created: Vec<BufferId> = Vec::new();
        let mut make = |res: &mut GpuResources, bytes: &mut Vec<u8>, name: &str| {
            // Пустой буфер создать нельзя: оставляем место хотя бы под одну запись.
            if bytes.is_empty() {
                bytes.resize(PAINT_SIZE, 0);
            }
            let id =
                res.create_buffer(ctx, bytes.len() as u64, usage, BufferMemory::GpuOnly, name)?;
            created.push(id);
            res.upload_buffer(ctx, id, bytes)?;
            Ok::<BufferId, GpuError>(id)
        };
        let made = (|| {
            Ok::<_, GpuError>((
                make(res, &mut vertices, "точки векторных фигур")?,
                make(res, &mut indices, "треугольники векторных фигур")?,
                make(res, &mut curve_vertices, "точки веера и кривых")?,
                make(res, &mut curve_indices, "треугольники веера и кривых")?,
                make(res, &mut paints, "градиенты векторных фигур")?,
                make(res, &mut stops, "точки цвета градиентов")?,
            ))
        })();
        let (vertices_id, indices_id, curve_vertices_id, curve_indices_id, paints_id, stops_id) =
            match made {
                Ok(ids) => ids,
                Err(e) => {
                    for id in created {
                        res.destroy_buffer(id);
                    }
                    return Err(e);
                }
            };
        // Старые буферы может ещё читать кадр в работе: они уходят в очередь отложенного удаления.
        if let Some(old) = self.meshes.take() {
            for id in [
                old.vertices,
                old.indices,
                old.curve_vertices,
                old.curve_indices,
                old.paints,
                old.stops,
            ] {
                res.destroy_buffer(id);
            }
        }
        tracing::info!(
            "Векторные фигуры на видеокарте: {} фигур, {} треугольников, для трафарета {} треугольников, градиентов {}, {} КБ",
            library.len(),
            index_count / 3,
            curve_index_count / 3,
            paint_count,
            (vertices.len()
                + indices.len()
                + curve_vertices.len()
                + curve_indices.len()
                + paints.len()
                + stops.len())
                / 1024
        );
        self.meshes = Some(Meshes {
            vertices: vertices_id,
            indices: indices_id,
            curve_vertices: curve_vertices_id,
            curve_indices: curve_indices_id,
            paints: paints_id,
            stops: stops_id,
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
        let packed = pack(
            &meshes.ranges,
            &instances[..instances.len().min(MAX_INSTANCES)],
            self.fill,
        );
        if packed.ops.is_empty() {
            return None;
        }
        let frame = &mut self.frames[slot];
        res.write_buffer(frame.instances, 0, &packed.instances);
        res.write_buffer(frame.commands, 0, &packed.commands);
        frame.ops = packed.ops;
        Some(VectorData {
            indices: res.buffer_address(meshes.indices),
            vertices: res.buffer_address(meshes.vertices),
            curve_indices: res.buffer_address(meshes.curve_indices),
            curve_vertices: res.buffer_address(meshes.curve_vertices),
            paints: res.buffer_address(meshes.paints),
            stops: res.buffer_address(meshes.stops),
            instances: res.buffer_address(frame.instances),
            commands: res.buffer_raw(frame.commands)?,
            slot,
            aspect,
        })
    }

    /// Записывает рисование фигур в открытый проход рисования в кадр сцены.
    ///
    /// # Safety
    /// `cmd` записывается внутри `vkCmdBeginRendering` с картинкой формата
    /// [`SCENE_FORMAT`] и трафаретом формата, переданного в [`Self::new`], общая раскладка
    /// конвейеров уже привязана; буферы `data` живы, пока видеокарта рисует кадр.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        data: VectorData,
    ) {
        let push = |indices: vk::DeviceAddress, vertices: vk::DeviceAddress| {
            let mut push = [0u8; PUSH_SIZE];
            push[..8].copy_from_slice(&indices.to_ne_bytes());
            push[8..16].copy_from_slice(&vertices.to_ne_bytes());
            push[16..24].copy_from_slice(&data.instances.to_ne_bytes());
            push[24..28].copy_from_slice(&data.aspect.to_ne_bytes());
            push[28..32].copy_from_slice(&self.msaa.samples().as_raw().to_ne_bytes());
            push[32..40].copy_from_slice(&data.paints.to_ne_bytes());
            push[40..48].copy_from_slice(&data.stops.to_ne_bytes());
            push
        };
        let triangles_push = push(data.indices, data.vertices);
        let curves_push = push(data.curve_indices, data.curve_vertices);
        let p = &self.pipelines;
        let bind = |pipeline: vk::Pipeline, push: &[u8; PUSH_SIZE]| {
            // SAFETY: условия функции.
            unsafe {
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                device.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, push);
            }
        };
        for &op in &self.frames[data.slot].ops {
            match op {
                Op::Triangles { first, count } => {
                    bind(p.triangles, &triangles_push);
                    // SAFETY: условия функции; команд не больше MAX_INSTANCES, буфер их вмещает.
                    unsafe {
                        device.cmd_draw_indirect(
                            cmd,
                            data.commands,
                            u64::from(first) * COMMAND_SIZE as u64,
                            count,
                            COMMAND_SIZE as u32,
                        );
                    }
                }
                Op::Stencil {
                    instance,
                    index_count,
                    even_odd,
                } => {
                    let count = if even_odd {
                        p.count_even_odd
                    } else {
                        p.count_nonzero
                    };
                    bind(count, &curves_push);
                    // SAFETY: условия функции.
                    unsafe { device.cmd_draw(cmd, index_count, 1, 0, instance) };
                    bind(p.cover, &curves_push);
                    // SAFETY: условия функции.
                    unsafe { device.cmd_draw(cmd, index_count, 1, 0, instance) };
                }
            }
        }
    }

    /// Уничтожает конвейеры, когда видеокарта их не использует. Буферы уходят вместе с
    /// [`GpuResources`].
    pub fn destroy(&mut self, ctx: &GpuContext) {
        self.pipelines.destroy(ctx);
        self.pipelines = Pipelines::NULL;
    }
}

/// Кадр, разложенный в байты для видеокарты.
struct Packed {
    instances: Vec<u8>,
    commands: Vec<u8>,
    ops: Vec<Op>,
}

/// Раскладывает экземпляры в байты для видеокарты: экземпляры (std430), команды
/// рисования треугольников и порядок рисования. Подряд идущие экземпляры одной фигуры,
/// которые рисуются треугольниками, становятся одной командой. Экземпляры неизвестных и
/// пустых фигур пропускаются. Фигуры без треугольников (перетекающие) рисуются трафаретом
/// при любом `fill`.
fn pack(ranges: &[MeshRange], instances: &[Instance], fill: FillMode) -> Packed {
    let mut out = Vec::with_capacity(instances.len() * INSTANCE_SIZE);
    let mut commands: Vec<[u32; 4]> = Vec::new();
    let mut ops: Vec<Op> = Vec::new();
    let mut last_shape = None;
    let mut written = 0u32;
    for inst in instances {
        let Some(range) = ranges.get(inst.shape.0 as usize) else {
            continue;
        };
        let stencil =
            range.curve_index_count > 0 && (range.index_count == 0 || fill.stencil(range.method));
        if range.index_count == 0 && !stencil {
            continue;
        }
        let [a, b, c, d, e, f] = inst.transform.as_coeffs().map(|v| v as f32);
        let color = range.color.multiply(inst.tint).to_array();
        for v in [a, b, c, d, e, f] {
            out.extend_from_slice(&v.to_ne_bytes());
        }
        out.extend_from_slice(&range.first_index.to_ne_bytes());
        out.extend_from_slice(&range.curve_first_index.to_ne_bytes());
        for v in color {
            out.extend_from_slice(&v.to_ne_bytes());
        }
        out.extend_from_slice(&range.morph_stride.to_ne_bytes());
        out.extend_from_slice(&range.morph_last.to_ne_bytes());
        out.extend_from_slice(&inst.morph.to_ne_bytes());
        out.extend_from_slice(&range.paint.to_ne_bytes());
        if stencil {
            ops.push(Op::Stencil {
                instance: written,
                index_count: range.curve_index_count,
                even_odd: range.even_odd,
            });
            last_shape = None;
        } else {
            match (commands.last_mut(), ops.last_mut()) {
                (Some(cmd), Some(Op::Triangles { .. })) if last_shape == Some(inst.shape) => {
                    cmd[1] += 1;
                }
                _ => {
                    commands.push([range.index_count, 1, 0, written]);
                    match ops.last_mut() {
                        Some(Op::Triangles { count, .. }) => *count += 1,
                        _ => ops.push(Op::Triangles {
                            first: commands.len() as u32 - 1,
                            count: 1,
                        }),
                    }
                }
            }
            last_shape = Some(inst.shape);
        }
        written += 1;
    }
    Packed {
        instances: out,
        commands: commands
            .iter()
            .flatten()
            .flat_map(|w| w.to_ne_bytes())
            .collect(),
        ops,
    }
}

fn build(
    ctx: &GpuContext,
    res: &GpuResources,
    shaders: &ShaderLibrary,
    msaa: Msaa,
    stencil_format: vk::Format,
) -> Result<Pipelines, GpuError> {
    let one = |name: &str, blend: Blend, usage: StencilUse| {
        let pair = shaders.get(name);
        graphics_pipeline_stencil(
            &ctx.device,
            res.bindless.pipeline_layout,
            &pair.vs,
            &pair.fs,
            SCENE_FORMAT,
            blend,
            msaa.samples(),
            Some(Stencil {
                format: stencil_format,
                usage,
            }),
        )
    };
    let mut made = Pipelines::NULL;
    let result = (|| {
        made.triangles = one("vector", Blend::Alpha, StencilUse::Ignore)?;
        made.count_nonzero = one("vector_stencil", Blend::Opaque, StencilUse::CountNonZero)?;
        made.count_even_odd = one("vector_stencil", Blend::Opaque, StencilUse::CountEvenOdd)?;
        made.cover = one("vector_cover", Blend::Alpha, StencilUse::Cover)?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(made),
        Err(e) => {
            made.destroy(ctx);
            Err(e)
        }
    }
}

/// Пишет градиент в формате шейдера (`Paint` и `GradientStop` в `include/paint.slang`).
fn pack_gradient(paints: &mut Vec<u8>, stops: &mut Vec<u8>, stop_count: &mut u32, g: &Gradient) {
    let start = paints.len();
    let [a, b, c, d, e, f] = g.transform.inverse().as_coeffs().map(|v| v as f32);
    let normalized = g.normalized_stops();
    let (kind, geometry, radius) = match g.kind {
        GradientKind::Linear { start, end } => (0u32, [start.x, start.y, end.x, end.y], 0.0),
        GradientKind::Radial { center, radius, .. } => {
            let focus = g.clamped_focus().unwrap_or(center);
            (1, [center.x, center.y, focus.x, focus.y], radius)
        }
    };
    let spread: u32 = match g.spread {
        Spread::Pad => 0,
        Spread::Reflect => 1,
        Spread::Repeat => 2,
    };
    let mut put = |v: [u8; 4]| paints.extend_from_slice(&v);
    for v in [a, b, c, d, e, f] {
        put(v.to_ne_bytes());
    }
    put(kind.to_ne_bytes());
    put(spread.to_ne_bytes());
    for v in geometry {
        put((v as f32).to_ne_bytes());
    }
    put((radius as f32).to_ne_bytes());
    put(stop_count.to_ne_bytes());
    put((normalized.len() as u32).to_ne_bytes());
    put(0u32.to_ne_bytes());
    debug_assert_eq!(paints.len() - start, PAINT_SIZE);
    for (offset, color) in normalized {
        let s = stops.len();
        for v in color {
            stops.extend_from_slice(&v.to_ne_bytes());
        }
        stops.extend_from_slice(&offset.to_ne_bytes());
        stops.extend_from_slice(&[0u8; 12]);
        debug_assert_eq!(stops.len() - s, STOP_SIZE);
        *stop_count += 1;
    }
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

    const PLAIN: MeshRange = MeshRange {
        first_index: 0,
        index_count: 0,
        curve_first_index: 0,
        curve_index_count: 0,
        even_odd: false,
        color: Color::WHITE,
        method: FillMethod::Triangles,
        paint: NO_PAINT,
        morph_stride: 0,
        morph_last: 0,
    };

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
                curve_first_index: 0,
                curve_index_count: 0,
                even_odd: false,
                color: red,
                ..PLAIN
            },
            MeshRange {
                first_index: 6,
                index_count: 3,
                curve_first_index: 0,
                curve_index_count: 0,
                even_odd: false,
                color: Color::WHITE,
                ..PLAIN
            },
            MeshRange {
                first_index: 9,
                index_count: 0,
                curve_first_index: 0,
                curve_index_count: 0,
                even_odd: false,
                color: Color::WHITE,
                ..PLAIN
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
        let Packed {
            instances: inst,
            commands: cmds,
            ops,
        } = pack(&ranges, &instances, FillMode::Triangles);
        assert_eq!(inst.len(), 5 * INSTANCE_SIZE);
        assert_eq!(ops, [Op::Triangles { first: 0, count: 3 }]);
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
        assert_eq!(words(&last[48..64]), [0, 0, 0, NO_PAINT]);
    }

    #[test]
    fn gradient_layout_matches_shader() {
        let g = Gradient::radial((1.0, 2.0), 3.0)
            .with_focus((9.0, 2.0))
            .with_spread(Spread::Reflect)
            .with_transform(Affine::scale(2.0))
            .stop(0.0, Color::WHITE)
            .stop(1.0, Color::linear(0.0, 0.0, 0.0, 0.5));
        let (mut paints, mut stops, mut count) = (Vec::new(), Vec::new(), 5u32);
        pack_gradient(&mut paints, &mut stops, &mut count, &g);
        assert_eq!(
            (paints.len(), stops.len(), count),
            (PAINT_SIZE, 2 * STOP_SIZE, 7)
        );
        let f = |b: &[u8], i: usize| f32::from_ne_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        // Обратное преобразование: масштаб 0.5.
        assert_eq!([f(&paints, 0), f(&paints, 3)], [0.5, 0.5]);
        assert_eq!(words(&paints[24..32]), [1, 1], "радиальный, отражать");
        // Фокус вне круга подтянут к краю.
        assert_eq!([f(&paints, 8), f(&paints, 9)], [1.0, 2.0]);
        assert!(f(&paints, 10) < 4.0 && f(&paints, 10) > 3.99);
        assert_eq!(f(&paints, 12), 3.0);
        assert_eq!(
            words(&paints[52..60]),
            [5, 2],
            "первая точка цвета и сколько их"
        );
        // Вторая точка: чёрный с прозрачностью 0.5 на доле 1.
        assert_eq!([f(&stops, 8 + 3), f(&stops, 8 + 4)], [0.5, 1.0]);
    }

    #[test]
    fn per_shape_methods_and_morphs() {
        let ranges = [
            // 0: заливка треугольниками, 1: заливка трафаретом, 2: перетекающая (нет
            // треугольников, три ключевых контура по 12 точек).
            MeshRange {
                index_count: 3,
                curve_index_count: 6,
                ..PLAIN
            },
            MeshRange {
                index_count: 3,
                curve_index_count: 6,
                method: FillMethod::Stencil,
                ..PLAIN
            },
            MeshRange {
                curve_index_count: 12,
                method: FillMethod::Stencil,
                morph_stride: 12,
                morph_last: 2,
                ..PLAIN
            },
        ];
        let at = |shape: u32| Instance::new(ShapeId(shape), Affine::IDENTITY);
        let instances = [at(0), at(1), at(2).with_morph(1.5)];
        let stencil = |instance, index_count| Op::Stencil {
            instance,
            index_count,
            even_odd: false,
        };
        let per_shape = pack(&ranges, &instances, FillMode::PerShape);
        assert_eq!(
            per_shape.ops,
            [
                Op::Triangles { first: 0, count: 1 },
                stencil(1, 6),
                stencil(2, 12)
            ]
        );
        let morph = &per_shape.instances[2 * INSTANCE_SIZE + 48..3 * INSTANCE_SIZE];
        assert_eq!(words(&morph[..8]), [12, 2]);
        assert_eq!(f32::from_ne_bytes(morph[8..12].try_into().unwrap()), 1.5);
        // «Все треугольниками»: перетекающая всё равно трафаретом.
        let triangles = pack(&ranges, &instances, FillMode::Triangles);
        assert_eq!(
            triangles.ops,
            [Op::Triangles { first: 0, count: 2 }, stencil(2, 12)]
        );
        let all = pack(&ranges, &instances, FillMode::Stencil);
        assert_eq!(all.ops, [stencil(0, 6), stencil(1, 6), stencil(2, 12)]);
    }

    #[test]
    fn stencil_fills_split_triangle_runs_and_keep_order() {
        let range = |first_index, curve_index_count, even_odd| MeshRange {
            first_index,
            index_count: 3,
            curve_first_index: 30,
            curve_index_count,
            even_odd,
            method: FillMethod::Triangles,
            ..PLAIN
        };
        // 0: заливка, 1: обводка (у обводок нет веера), 2: заливка EvenOdd.
        let ranges = [range(0, 12, false), range(3, 0, false), range(6, 9, true)];
        let at = |shape: u32| Instance::new(ShapeId(shape), Affine::IDENTITY);
        let instances = [at(1), at(1), at(0), at(1), at(2), at(2)];
        let packed = pack(&ranges, &instances, FillMode::Stencil);
        assert_eq!(
            packed.ops,
            [
                Op::Triangles { first: 0, count: 1 },
                Op::Stencil {
                    instance: 2,
                    index_count: 12,
                    even_odd: false
                },
                Op::Triangles { first: 1, count: 1 },
                Op::Stencil {
                    instance: 4,
                    index_count: 9,
                    even_odd: true
                },
                Op::Stencil {
                    instance: 5,
                    index_count: 9,
                    even_odd: true
                },
            ]
        );
        assert_eq!(words(&packed.commands), [3, 2, 0, 0, 3, 1, 0, 3]);
        // Номер первого треугольника веера лежит в экземпляре рядом с номером способа 1.
        assert_eq!(words(&packed.instances[24..32]), [3, 30]);
        // Тот же кадр треугольниками: все шесть экземпляров в командах.
        let triangles = pack(&ranges, &instances, FillMode::Triangles);
        assert_eq!(triangles.ops, [Op::Triangles { first: 0, count: 4 }]);
    }
}
