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
//!
//! Текст (`reyvec-text`) рисуется в том же списке: буквы всех блоков лежат в общем буфере
//! прямоугольниками, формы букв — в атласе MSDF. Блок текста в кадре — один экземпляр и
//! один вызов рисования, сколько бы букв в нём ни было (шейдер `vector_text.slang`).

use ash::vk;
use reyvec_text::TextLibrary;
use reyvec_vector::{
    BlendMode, Color, FillMethod, FillRule, Gradient, GradientKind, MaskMode, Paint, SceneItem,
    ShapeId, ShapeLibrary, Spread, Style,
};

use crate::GpuError;
use crate::context::GpuContext;
use crate::demo::{Msaa, SCENE_FORMAT};
use crate::pipeline::{Blend, Stencil, StencilUse, graphics_pipeline_stencil};
use crate::resources::{BufferId, BufferMemory, GpuResources, TextureId};
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
/// Как используются буферы фигур: шейдеры читают, копирование пишет и читает (при росте).
const ARENA_USAGE: vk::BufferUsageFlags = vk::BufferUsageFlags::from_raw(
    vk::BufferUsageFlags::STORAGE_BUFFER.as_raw()
        | vk::BufferUsageFlags::TRANSFER_DST.as_raw()
        | vk::BufferUsageFlags::TRANSFER_SRC.as_raw(),
);

/// Размер буфера фигур под `used` байт: следующая степень двойки, не меньше [`MIN_ARENA`].
fn arena_capacity(used: u64) -> u64 {
    used.max(MIN_ARENA).next_power_of_two()
}
/// Байт констант векторных конвейеров. С 48-го байта у всех лежит вид камеры (24 байта).
const PUSH_SIZE: usize = 72;
const VIEW_OFFSET: usize = 48;
/// Байт на букву текста: прямоугольник и его место в атласе (`Glyph` в `vector_text.slang`).
const GLYPH_SIZE: usize = 32;

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
    /// Прямоугольник фигуры в её координатах (с запасом обводки и всеми ключевыми
    /// контурами перетекания): мин. x, мин. y, макс. x, макс. y. У пустой фигуры
    /// [`EMPTY_BOUNDS`].
    bounds: Bounds,
}

/// Прямоугольник: мин. x, мин. y, макс. x, макс. y.
type Bounds = [f32; 4];
const EMPTY_BOUNDS: Bounds = [
    f32::INFINITY,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NEG_INFINITY,
];

fn intersect_bounds(a: Bounds, b: Bounds) -> Bounds {
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    if r[0] > r[2] || r[1] > r[3] {
        EMPTY_BOUNDS
    } else {
        r
    }
}

fn union(a: Bounds, b: Bounds) -> Bounds {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn bounds_of<'p>(points: impl IntoIterator<Item = &'p [f32; 2]>) -> Bounds {
    points
        .into_iter()
        .fold(EMPTY_BOUNDS, |b, p| union(b, [p[0], p[1], p[0], p[1]]))
}

/// Прямоугольник `b` после преобразования `t`: центр переносится, полуширина и
/// полувысота растягиваются на модули коэффициентов. Это ровно рамка четырёх
/// повёрнутых углов, но без сравнений: функция вызывается для каждой фигуры в кадре.
fn transform_bounds(t: &reyvec_vector::Affine, b: Bounds) -> Bounds {
    if b[0] > b[2] {
        return EMPTY_BOUNDS;
    }
    let [a, bb, c, d, e, f] = t.as_coeffs();
    let (cx, cy) = (f64::from(b[0] + b[2]) * 0.5, f64::from(b[1] + b[3]) * 0.5);
    let (hw, hh) = (f64::from(b[2] - b[0]) * 0.5, f64::from(b[3] - b[1]) * 0.5);
    let (x, y) = (a * cx + c * cy + e, bb * cx + d * cy + f);
    let (rx, ry) = (a.abs() * hw + c.abs() * hh, bb.abs() * hw + d.abs() * hh);
    [
        (x - rx) as f32,
        (y - ry) as f32,
        (x + rx) as f32,
        (y + ry) as f32,
    ]
}

/// Один буфер фигур на видеокарте: новые фигуры дописываются в конец, старые не
/// двигаются. Когда место кончается, буфер заменяется вдвое большим (копию делает
/// видеокарта внутри кадра).
#[derive(Clone, Copy)]
struct Arena {
    id: BufferId,
    capacity: u64,
    used: u64,
}

/// Где в общем буфере букв лежит блок текста.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TextRange {
    first_glyph: u32,
    glyph_count: u32,
    color: Color,
    /// Рамка блока в его координатах (с запасом поля вокруг букв).
    bounds: Bounds,
}

/// Буквы всех блоков текста и атлас на видеокарте.
struct TextMeshes {
    glyphs: BufferId,
    atlas: TextureId,
    atlas_size: [f32; 2],
    /// Сколько разных букв было в атласе при загрузке.
    atlas_glyphs: usize,
    ranges: Vec<TextRange>,
    revision: u64,
}

/// Треугольники всех фигур на видеокарте: шесть буферов-копилок и где в них какая фигура.
struct Meshes {
    /// Точки, треугольники, точки веера и кривых, их треугольники, градиенты, точки цвета.
    arenas: [Arena; 6],
    ranges: Vec<MeshRange>,
    counters: Counters,
}

/// Сколько уже записано в буферы фигур (в штуках): номера у новых фигур идут дальше.
#[derive(Clone, Copy, Default)]
struct Counters {
    vertices: u32,
    indices: u32,
    curve_vertices: u32,
    curve_indices: u32,
    paints: u32,
    stops: u32,
}

/// Фигуры, разложенные в байты для шести буферов фигур.
#[derive(Default)]
struct PackedShapes {
    bytes: [Vec<u8>; 6],
    ranges: Vec<MeshRange>,
}

/// Названия буферов фигур для лога и отладки.
const ARENA_NAMES: [&str; 6] = [
    "точки векторных фигур",
    "треугольники векторных фигур",
    "точки веера и кривых",
    "треугольники веера и кривых",
    "градиенты векторных фигур",
    "точки цвета градиентов",
];
/// Сколько байт новых фигур загружать за кадр (не меньше одной фигуры): большая пачка
/// растягивается на несколько кадров, а не задерживает один.
const STREAM_BUDGET: usize = 2 * 1024 * 1024;
/// Самый маленький буфер фигур.
const MIN_ARENA: u64 = 64 * 1024;

/// Копирования, которые надо выполнить в начале кадра, чтобы новые фигуры попали на
/// видеокарту. Записывает их [`VectorRenderer::record_uploads`].
#[derive(Clone, Debug, Default)]
pub(crate) struct Uploads {
    copies: Vec<(vk::Buffer, vk::Buffer, vk::BufferCopy)>,
    /// Сколько фигур и байт в этом кадре (для лога и панели).
    pub shapes: usize,
    pub bytes: u64,
}

impl Uploads {
    pub fn is_empty(&self) -> bool {
        self.copies.is_empty()
    }
}

/// Раскладывает фигуры `ids` из набора в байты, продолжая номера с `counters`.
fn pack_shapes(
    library: &ShapeLibrary,
    ids: std::ops::Range<usize>,
    counters: &mut Counters,
) -> PackedShapes {
    let mut out = PackedShapes::default();
    let [
        vertices,
        indices,
        curve_vertices,
        curve_indices,
        paints,
        stops,
    ] = &mut out.bytes;
    for (id, shape, mesh) in library.iter().skip(ids.start).take(ids.len()) {
        for (v, e) in mesh.vertices.iter().zip(&mesh.edges) {
            pack_vertex(vertices, *v, e);
        }
        // Номера точек сразу общие для всех фигур: шейдеру не нужно знать, где
        // начинаются точки фигуры.
        for &i in &mesh.indices {
            indices.extend_from_slice(&(i + counters.vertices).to_ne_bytes());
        }
        let curves = library.curves(id);
        let curve_first = counters.curve_indices;
        let keyframes = library.morph_keyframes(id);
        let mut morph_stride = 0;
        let mut bounds = bounds_of(&mesh.vertices);
        if let Some(c) = curves {
            bounds = keyframes
                .iter()
                .fold(union(bounds, bounds_of(&c.vertices)), |b, k| {
                    union(b, bounds_of(k))
                });
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
                curve_indices.extend_from_slice(&(i + counters.curve_vertices).to_ne_bytes());
            }
            morph_stride = c.vertices.len() as u32;
            counters.curve_vertices += morph_stride * (1 + keyframes.len() as u32);
            counters.curve_indices += c.indices.len() as u32;
        }
        let (color, paint) = match &shape.paint {
            Paint::Solid(c) => (*c, NO_PAINT),
            Paint::Gradient(g) => {
                pack_gradient(paints, stops, &mut counters.stops, g);
                counters.paints += 1;
                (Color::WHITE, counters.paints - 1)
            }
        };
        out.ranges.push(MeshRange {
            first_index: counters.indices,
            index_count: mesh.indices.len() as u32,
            curve_first_index: curve_first,
            curve_index_count: curves.map_or(0, |c| c.indices.len() as u32),
            even_odd: matches!(shape.style, Style::Fill(FillRule::EvenOdd)),
            color,
            method: library.method(id).unwrap_or_default(),
            paint,
            morph_stride,
            morph_last: keyframes.len() as u32,
            bounds,
        });
        counters.vertices += mesh.vertices.len() as u32;
        counters.indices += mesh.indices.len() as u32;
    }
    out
}

/// Сколько фигур, начиная с `start`, загрузить за кадр: пока байт не больше
/// [`STREAM_BUDGET`], но хотя бы одну.
fn stream_count(library: &ShapeLibrary, start: usize) -> usize {
    let mut bytes = 0usize;
    let mut count = 0usize;
    for (id, _, mesh) in library.iter().skip(start) {
        let curves = library.curves(id).map_or(0, |c| {
            c.vertices.len() * 16 * (1 + library.morph_keyframes(id).len()) + c.indices.len() * 4
        });
        bytes += mesh.vertices.len() * VERTEX_SIZE + mesh.indices.len() * 4 + curves;
        if count > 0 && bytes > STREAM_BUDGET {
            break;
        }
        count += 1;
    }
    count
}

/// Что рисовать в кадре, по порядку.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    /// Команды `first..first + count` из буфера команд (способ 1 и обводки).
    Triangles { first: u32, count: u32 },
    /// Залитые фигуры через трафарет: экземпляры `instance..instance + count` одной фигуры,
    /// которые на экране не задевают друг друга (поэтому их можно рисовать одним вызовом),
    /// и число номеров веера и кривых.
    Stencil {
        instance: u32,
        count: u32,
        index_count: u32,
        even_odd: bool,
    },
    /// Блок текста: экземпляр и число букв (по шесть вершин на букву).
    Text { instance: u32, glyph_count: u32 },
    /// Наложить готовый слой `layer` (номер цели рисования) с прозрачностью, режимом и,
    /// если есть, маской.
    Composite {
        layer: u32,
        opacity: f32,
        blend: BlendMode,
        mask: Option<LayerMask>,
    },
}

/// Маска слоя: готовая картинка цели `target`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LayerMask {
    target: u32,
    mode: MaskMode,
    inverted: bool,
}

impl LayerMask {
    /// Режим для шейдера `vector_layer.slang`: 1 по прозрачности, 2 по яркости, +4 обратная.
    fn shader_mode(self) -> u32 {
        let mode = match self.mode {
            MaskMode::Alpha => 1,
            MaskMode::Luminance => 2,
        };
        mode | if self.inverted { 4 } else { 0 }
    }
}

/// Буферы одного кадра в работе: пока видеокарта читает одни, процессор пишет в другие.
struct FrameBuffers {
    instances: BufferId,
    commands: BufferId,
    /// Временный буфер для новых фигур ([`VectorRenderer::stream`]) и его размер.
    staging: Option<BufferId>,
    staging_size: u64,
    /// Что рисовать в каждую цель: 0 — кадр сцены, остальные — слои.
    targets: Vec<Vec<Op>>,
    /// Слои в порядке рисования: вложенные раньше тех, в которые они вложены.
    layer_order: Vec<u32>,
    /// Прямоугольник каждой цели в координатах сцены (у цели 0 не используется).
    bounds: Vec<Bounds>,
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
    /// Буквы текста и атлас (нули, если текста нет).
    glyphs: vk::DeviceAddress,
    atlas: u32,
    atlas_size: [f32; 2],
    slot: usize,
    aspect: f32,
    /// Вид камеры (a, b, c, d, e, f): сцена → экран.
    view: [f32; 6],
}

/// Конвейеры векторного слоя под одно сглаживание.
#[derive(Clone, Copy)]
struct Pipelines {
    triangles: vk::Pipeline,
    count_nonzero: vk::Pipeline,
    count_even_odd: vk::Pipeline,
    cover: vk::Pipeline,
    text: vk::Pipeline,
    /// Наложение слоя по режимам: обычное, умножение, экран, сложение.
    composite: [vk::Pipeline; 4],
}

impl Pipelines {
    const NULL: Self = Self {
        triangles: vk::Pipeline::null(),
        count_nonzero: vk::Pipeline::null(),
        count_even_odd: vk::Pipeline::null(),
        cover: vk::Pipeline::null(),
        text: vk::Pipeline::null(),
        composite: [vk::Pipeline::null(); 4],
    };

    fn destroy(self, ctx: &GpuContext) {
        for p in [
            self.triangles,
            self.count_nonzero,
            self.count_even_odd,
            self.cover,
            self.text,
        ]
        .into_iter()
        .chain(self.composite)
        {
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
    text: Option<TextMeshes>,
    frames: Vec<FrameBuffers>,
    /// Предупреждение о переполнении уже было в логе.
    warned_overflow: bool,
    /// Сколько фигур и блоков текста попало в последний кадр после отсечения.
    drawn: u32,
    /// Память раскладки фигур трафаретом по слоям, общая для всех кадров.
    stencil_run: StencilRun,
}

/// Шейдеры векторного слоя: при их перезагрузке конвейеры пересобираются.
const SHADERS: [&str; 5] = [
    "vector",
    "vector_stencil",
    "vector_cover",
    "vector_layer",
    "vector_text",
];

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
                staging: None,
                staging_size: 0,
                targets: Vec::new(),
                layer_order: Vec::new(),
                bounds: Vec::new(),
            });
        }
        Ok(Self {
            pipelines: build(ctx, res, shaders, Msaa::Off, stencil_format)?,
            msaa: Msaa::Off,
            stencil_format,
            fill: FillMode::default(),
            meshes: None,
            text: None,
            frames,
            warned_overflow: false,
            drawn: 0,
            stencil_run: StencilRun::default(),
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

    /// Загружает на видеокарту все фигуры набора, которых там ещё нет, и ждёт окончания
    /// загрузки. Для загрузки сцены; фигуры, добавленные во время игры, загружает
    /// [`Self::stream`] без ожидания.
    pub fn upload(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        library: &ShapeLibrary,
    ) -> Result<(), GpuError> {
        let done = self.meshes.as_ref().map_or(0, |m| m.ranges.len());
        if done == library.len() && self.meshes.is_some() {
            return Ok(());
        }
        let mut counters = self
            .meshes
            .as_ref()
            .map_or_else(Counters::default, |m| m.counters);
        let packed = pack_shapes(library, done..library.len(), &mut counters);
        // Новые буферы с запасом: старые (если были) копируются в начало.
        let old = self.meshes.take();
        let mut arenas = [Arena {
            id: BufferId(u32::MAX),
            capacity: 0,
            used: 0,
        }; 6];
        let mut created: Vec<BufferId> = Vec::new();
        let result = (|| {
            for (k, bytes) in packed.bytes.iter().enumerate() {
                let old_used = old.as_ref().map_or(0, |m| m.arenas[k].used);
                let used = old_used + bytes.len() as u64;
                let capacity = arena_capacity(used);
                let id = res.create_buffer(
                    ctx,
                    capacity,
                    ARENA_USAGE,
                    BufferMemory::GpuOnly,
                    ARENA_NAMES[k],
                )?;
                created.push(id);
                if let Some(m) = old.as_ref().filter(|_| old_used > 0) {
                    res.copy_buffer(ctx, m.arenas[k].id, id, old_used)?;
                }
                if !bytes.is_empty() {
                    res.upload_buffer_at(ctx, id, old_used, bytes)?;
                }
                arenas[k] = Arena { id, capacity, used };
            }
            Ok::<(), GpuError>(())
        })();
        if let Err(e) = result {
            for id in created {
                res.destroy_buffer(id);
            }
            self.meshes = old;
            return Err(e);
        }
        let mut ranges = old.as_ref().map(|m| m.ranges.clone()).unwrap_or_default();
        if let Some(m) = old {
            // Старые буферы может ещё читать кадр в работе: они уходят в очередь
            // отложенного удаления.
            for a in m.arenas {
                res.destroy_buffer(a.id);
            }
        }
        ranges.extend(packed.ranges);
        tracing::info!(
            "Векторные фигуры на видеокарте: {} фигур, {} треугольников, для трафарета {} треугольников, градиентов {}, {} КБ",
            ranges.len(),
            counters.indices / 3,
            counters.curve_indices / 3,
            counters.paints,
            arenas.iter().map(|a| a.used).sum::<u64>() / 1024
        );
        self.meshes = Some(Meshes {
            arenas,
            ranges,
            counters,
        });
        Ok(())
    }

    /// Готовит загрузку новых фигур набора (добавленных после прошлой загрузки) внутри
    /// кадра, без ожидания: байты пишутся во временный буфер кадра `slot`, а копирование в
    /// буферы фигур записывает [`Self::record_uploads`] в начало этого кадра. Новые
    /// фигуры видны с этого же кадра. Большая пачка растягивается на несколько кадров
    /// ([`STREAM_BUDGET`] байт за кадр); фигуры, которые ещё не загружены, просто не
    /// рисуются. Если место в буферах кончилось, они заменяются вдвое большими (старое
    /// содержимое копирует видеокарта в том же кадре).
    pub fn stream(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        slot: usize,
        library: &ShapeLibrary,
    ) -> Result<Uploads, GpuError> {
        let Some(meshes) = self.meshes.as_mut() else {
            return Ok(Uploads::default());
        };
        let start = meshes.ranges.len();
        if start >= library.len() {
            return Ok(Uploads::default());
        }
        let count = stream_count(library, start);
        let mut counters = meshes.counters;
        let packed = pack_shapes(library, start..start + count, &mut counters);
        let total: usize = packed.bytes.iter().map(Vec::len).sum();
        // Временный буфер кадра: растёт по необходимости.
        let frame = &mut self.frames[slot];
        if frame.staging_size < total as u64 {
            if let Some(old) = frame.staging.take() {
                res.destroy_buffer(old);
            }
            let size = arena_capacity(total as u64);
            frame.staging = Some(res.create_buffer(
                ctx,
                size,
                vk::BufferUsageFlags::TRANSFER_SRC,
                BufferMemory::CpuToGpu,
                &format!("загрузка векторных фигур кадра {slot}"),
            )?);
            frame.staging_size = size;
        }
        let staging = frame.staging.expect("создан выше");
        let staging_raw = res.buffer_raw(staging).expect("буфер жив");
        let mut uploads = Uploads {
            shapes: count,
            bytes: total as u64,
            ..Uploads::default()
        };
        let mut offset = 0u64;
        for (k, bytes) in packed.bytes.iter().enumerate() {
            if bytes.is_empty() {
                continue;
            }
            let arena = &mut meshes.arenas[k];
            let used = arena.used + bytes.len() as u64;
            if used > arena.capacity {
                // Место кончилось: новый буфер вдвое больше, старое содержимое
                // копирует видеокарта в начале этого кадра, старый буфер удалится, когда
                // его перестанут читать кадры в работе.
                let capacity = arena_capacity(used);
                let id = res.create_buffer(
                    ctx,
                    capacity,
                    ARENA_USAGE,
                    BufferMemory::GpuOnly,
                    ARENA_NAMES[k],
                )?;
                if arena.used > 0 {
                    uploads.copies.push((
                        res.buffer_raw(arena.id).expect("буфер жив"),
                        res.buffer_raw(id).expect("буфер жив"),
                        vk::BufferCopy::default().size(arena.used),
                    ));
                }
                res.destroy_buffer(arena.id);
                tracing::debug!("буфер «{}» вырос до {} КБ", ARENA_NAMES[k], capacity / 1024);
                arena.id = id;
                arena.capacity = capacity;
            }
            res.write_buffer(staging, offset as usize, bytes);
            uploads.copies.push((
                staging_raw,
                res.buffer_raw(arena.id).expect("буфер жив"),
                vk::BufferCopy::default()
                    .src_offset(offset)
                    .dst_offset(arena.used)
                    .size(bytes.len() as u64),
            ));
            offset += bytes.len() as u64;
            arena.used = used;
        }
        meshes.counters = counters;
        meshes.ranges.extend(packed.ranges);
        Ok(uploads)
    }

    /// Записывает копирования новых фигур из [`Self::stream`] и барьер: после него шейдеры
    /// читают уже новые данные. Вызывать в начале кадра, до проходов с векторами.
    ///
    /// # Safety
    /// `cmd` в состоянии записи вне прохода рисования; буферы из `uploads` живы, пока
    /// видеокарта выполняет этот кадр (удаление старых отложено до его конца).
    pub unsafe fn record_uploads(device: &ash::Device, cmd: vk::CommandBuffer, uploads: &Uploads) {
        if uploads.is_empty() {
            return;
        }
        // SAFETY: условия функции.
        unsafe {
            // Сначала копии старого содержимого в выросшие буферы, затем новые фигуры:
            // команды копирования в одном буфере команд к разным участкам не мешают друг
            // другу, кроме выросшего буфера, куда сначала пишется старое, потом новое.
            for (src, dst, region) in &uploads.copies {
                device.cmd_copy_buffer(cmd, *src, *dst, std::slice::from_ref(region));
            }
            let barrier = [vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(
                    vk::PipelineStageFlags2::VERTEX_SHADER
                        | vk::PipelineStageFlags2::FRAGMENT_SHADER,
                )
                .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_READ)];
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().memory_barriers(&barrier),
            );
        }
    }

    /// Сколько фигур и блоков текста нарисовано в последнем кадре: остальные целиком за
    /// краем экрана и отсечены.
    pub fn drawn(&self) -> u32 {
        self.drawn
    }

    /// Сколько фигур уже на видеокарте.
    pub fn uploaded_shapes(&self) -> usize {
        self.meshes.as_ref().map_or(0, |m| m.ranges.len())
    }

    /// Загружает буквы всех блоков текста на видеокарту, если текст изменился с прошлой
    /// загрузки, и атлас букв, если в нём появились новые буквы. Ждёт окончания загрузки,
    /// как [`Self::upload`]: вызывать, когда текст меняется, а не каждый кадр.
    pub fn upload_text(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        text: &TextLibrary,
    ) -> Result<(), GpuError> {
        if self
            .text
            .as_ref()
            .is_some_and(|t| t.revision == text.revision())
        {
            return Ok(());
        }
        let (aw, ah) = text.atlas().size();
        let mut glyphs: Vec<u8> = Vec::new();
        let mut ranges = Vec::with_capacity(text.len());
        let mut count = 0u32;
        for (_, laid) in text.iter() {
            let mut bounds = EMPTY_BOUNDS;
            for g in &laid.glyphs {
                let [l, t, r, b] = g.rect;
                bounds = union(bounds, [l, b, r, t]);
                let [u0, v0, u1, v1] = g.uv;
                for f in [
                    l,
                    t,
                    r,
                    b,
                    u0 / aw as f32,
                    v0 / ah as f32,
                    u1 / aw as f32,
                    v1 / ah as f32,
                ] {
                    glyphs.extend_from_slice(&f.to_ne_bytes());
                }
            }
            let n = laid.glyphs.len() as u32;
            ranges.push(TextRange {
                first_glyph: count,
                glyph_count: n,
                color: laid.color,
                bounds,
            });
            count += n;
        }
        debug_assert_eq!(glyphs.len(), count as usize * GLYPH_SIZE);
        if glyphs.is_empty() {
            glyphs.resize(GLYPH_SIZE, 0);
        }
        let buffer = res.create_buffer(
            ctx,
            glyphs.len() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            BufferMemory::GpuOnly,
            "буквы текста",
        )?;
        if let Err(e) = res.upload_buffer(ctx, buffer, &glyphs) {
            res.destroy_buffer(buffer);
            return Err(e);
        }
        // Атлас только растёт: если букв столько же и размер тот же, он не изменился.
        let atlas_glyphs = text.atlas().len();
        let size = [aw as f32, ah as f32];
        let old = self.text.take();
        let atlas = match &old {
            Some(o) if o.atlas_glyphs == atlas_glyphs && o.atlas_size == size => o.atlas,
            _ => {
                match res.create_texture_rgba8(
                    ctx,
                    aw,
                    ah,
                    text.atlas().pixels(),
                    false,
                    "атлас букв",
                ) {
                    Ok(t) => {
                        if let Some(o) = &old {
                            res.destroy_texture(o.atlas);
                        }
                        t
                    }
                    Err(e) => {
                        res.destroy_buffer(buffer);
                        self.text = old;
                        return Err(e);
                    }
                }
            }
        };
        // Старый буфер может ещё читать кадр в работе: он уходит в очередь отложенного удаления.
        if let Some(o) = old {
            res.destroy_buffer(o.glyphs);
        }
        tracing::info!(
            "Текст на видеокарте: {} блоков, {} букв, атлас {aw}×{ah} ({} разных букв)",
            ranges.len(),
            count,
            atlas_glyphs
        );
        if text.atlas().overflow() > 0 {
            tracing::warn!(
                "В атлас букв не поместилось {} букв: они не рисуются",
                text.atlas().overflow()
            );
        }
        self.text = Some(TextMeshes {
            glyphs: buffer,
            atlas,
            atlas_size: size,
            atlas_glyphs,
            ranges,
            revision: text.revision(),
        });
        Ok(())
    }

    /// Пишет экземпляры и команды рисования кадра в буферы ячейки `slot`. Видеокарта не
    /// должна в этот момент читать буферы `slot`. `None`, если рисовать нечего.
    pub fn prepare(
        &mut self,
        res: &mut GpuResources,
        slot: usize,
        items: &[SceneItem],
        aspect: f32,
        view: reyvec_vector::Affine,
    ) -> Option<VectorData> {
        let meshes = self.meshes.as_ref()?;
        let texts = self.text.as_ref().map_or(&[][..], |t| &t.ranges[..]);
        let packed = pack_frame(
            &meshes.ranges,
            texts,
            items,
            self.fill,
            view,
            visible_area(aspect),
            &mut self.stencil_run,
        );
        self.drawn = packed.drawn;
        if packed.overflow && !self.warned_overflow {
            tracing::warn!(
                "Векторных фигур в кадре больше, чем помещается ({MAX_INSTANCES}): лишние не рисуются"
            );
            self.warned_overflow = true;
        }
        if packed.targets.iter().all(Vec::is_empty) {
            return None;
        }
        let frame = &mut self.frames[slot];
        res.write_buffer(frame.instances, 0, &packed.instances);
        res.write_buffer(frame.commands, 0, &packed.commands);
        frame.targets = packed.targets;
        frame.layer_order = packed.layer_order;
        frame.bounds = packed.bounds;
        Some(VectorData {
            vertices: res.buffer_address(meshes.arenas[0].id),
            indices: res.buffer_address(meshes.arenas[1].id),
            curve_vertices: res.buffer_address(meshes.arenas[2].id),
            curve_indices: res.buffer_address(meshes.arenas[3].id),
            paints: res.buffer_address(meshes.arenas[4].id),
            stops: res.buffer_address(meshes.arenas[5].id),
            instances: res.buffer_address(frame.instances),
            commands: res.buffer_raw(frame.commands)?,
            glyphs: self
                .text
                .as_ref()
                .map_or(0, |t| res.buffer_address(t.glyphs)),
            atlas: self.text.as_ref().map_or(0, |t| t.atlas.0),
            atlas_size: self.text.as_ref().map_or([1.0; 2], |t| t.atlas_size),
            slot,
            aspect,
            view: view.as_coeffs().map(|v| v as f32),
        })
    }

    /// Слои кадра `data` в порядке рисования (номера целей, см. [`Self::record`]).
    /// Каждый рисуется своим проходом раньше прохода, в котором его накладывают.
    pub fn layer_order(&self, data: &VectorData) -> &[u32] {
        &self.frames[data.slot].layer_order
    }

    /// Прямоугольник пикселей кадра размером `extent`, который занимают фигуры слоя
    /// `layer`, с запасом в 2 пикселя на сглаживание. Слой рисуется и накладывается только
    /// в нём. Не меньше 1×1 (пустой проход видеокарта не принимает).
    pub fn layer_rect(&self, data: &VectorData, layer: u32, extent: vk::Extent2D) -> vk::Rect2D {
        let b = self.frames[data.slot]
            .bounds
            .get(layer as usize)
            .copied()
            .unwrap_or(EMPTY_BOUNDS);
        pixel_rect(b, data.aspect, extent)
    }

    /// Слои и их маски, которые накладываются прямо в цель `target`: их картинки проход
    /// этой цели читает.
    pub fn layer_children(&self, data: &VectorData, target: usize) -> Vec<u32> {
        self.frames[data.slot]
            .targets
            .get(target)
            .into_iter()
            .flatten()
            .filter_map(|op| match *op {
                Op::Composite { layer, mask, .. } => {
                    Some(std::iter::once(layer).chain(mask.map(|m| m.target)))
                }
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// Записывает рисование цели `target` (0 — кадр сцены, иначе слой из
    /// [`Self::layer_order`]) в открытый проход рисования. `layers[k]` — готовая
    /// картинка слоя `k` (для слоёв, которые здесь накладываются): номер в общем списке
    /// текстур и прямоугольник кадра, где она нарисована (с угла этого прямоугольника
    /// начинается картинка). `area` — область рисования прохода в пикселях кадра: после
    /// наложения слоя ножницы возвращаются на неё. `origin` — пиксель кадра, с которого
    /// начинается картинка цели (у кадра сцены ноль).
    ///
    /// # Safety
    /// `cmd` записывается внутри `vkCmdBeginRendering` с картинкой формата
    /// [`SCENE_FORMAT`] и трафаретом формата, переданного в [`Self::new`], общая раскладка
    /// конвейеров уже привязана; буферы `data` живы, пока видеокарта рисует кадр;
    /// картинки слоёв из `layers` уже нарисованы и доступны для чтения.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        data: VectorData,
        target: usize,
        layers: &[Option<LayerTexture>],
        area: vk::Rect2D,
        origin: vk::Offset2D,
    ) {
        let area_here = shift(area, origin);
        let push = |indices: vk::DeviceAddress, vertices: vk::DeviceAddress| {
            let mut push = [0u8; PUSH_SIZE];
            push[..8].copy_from_slice(&indices.to_ne_bytes());
            push[8..16].copy_from_slice(&vertices.to_ne_bytes());
            push[16..24].copy_from_slice(&data.instances.to_ne_bytes());
            push[24..28].copy_from_slice(&data.aspect.to_ne_bytes());
            push[28..32].copy_from_slice(&self.msaa.samples().as_raw().to_ne_bytes());
            push[32..40].copy_from_slice(&data.paints.to_ne_bytes());
            push[40..48].copy_from_slice(&data.stops.to_ne_bytes());
            write_view(&mut push, data.view);
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
        let Some(ops) = self.frames[data.slot].targets.get(target) else {
            return;
        };
        for &op in ops {
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
                    count: instances,
                    index_count,
                    even_odd,
                } => {
                    let count = if even_odd {
                        p.count_even_odd
                    } else {
                        p.count_nonzero
                    };
                    // Фигуры вызова не пересекаются на экране: их трафареты не смешиваются,
                    // а покрытие каждой очищает только свой трафарет.
                    bind(count, &curves_push);
                    // SAFETY: условия функции.
                    unsafe { device.cmd_draw(cmd, index_count, instances, 0, instance) };
                    bind(p.cover, &curves_push);
                    // SAFETY: условия функции.
                    unsafe { device.cmd_draw(cmd, index_count, instances, 0, instance) };
                }
                Op::Text {
                    instance,
                    glyph_count,
                } => {
                    let mut push = [0u8; PUSH_SIZE];
                    write_view(&mut push, data.view);
                    push[..8].copy_from_slice(&data.glyphs.to_ne_bytes());
                    push[8..16].copy_from_slice(&data.instances.to_ne_bytes());
                    push[16..20].copy_from_slice(&data.aspect.to_ne_bytes());
                    push[20..24].copy_from_slice(&data.atlas.to_ne_bytes());
                    push[24..28].copy_from_slice(&data.atlas_size[0].to_ne_bytes());
                    push[28..32].copy_from_slice(&data.atlas_size[1].to_ne_bytes());
                    push[32..36].copy_from_slice(&reyvec_text::RANGE.to_ne_bytes());
                    // SAFETY: условия функции; буквы блока лежат в буфере букв.
                    unsafe {
                        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, p.text);
                        device.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                        device.cmd_draw(cmd, glyph_count * 6, 1, 0, instance);
                    }
                }
                Op::Composite {
                    layer,
                    opacity,
                    blend,
                    mask,
                } => {
                    let Some(&Some(LayerTexture { texture, rect })) = layers.get(layer as usize)
                    else {
                        continue;
                    };
                    let (mask_texture, mask_mode, mask_origin) = match mask {
                        None => (u32::MAX, 0, rect.offset),
                        Some(m) => match layers.get(m.target as usize) {
                            Some(Some(t)) => (t.texture, m.shader_mode(), t.rect.offset),
                            _ => continue,
                        },
                    };
                    let Some(scissor) = intersect(rect, area) else {
                        continue;
                    };
                    // Пиксель цели + сдвиг = пиксель картинки слоя (и маски).
                    let mut push = [0u8; 32];
                    push[..4].copy_from_slice(&texture.to_ne_bytes());
                    push[4..8].copy_from_slice(&opacity.to_ne_bytes());
                    push[8..12].copy_from_slice(&mask_texture.to_ne_bytes());
                    push[12..16].copy_from_slice(&mask_mode.to_ne_bytes());
                    for (i, v) in [
                        origin.x - rect.offset.x,
                        origin.y - rect.offset.y,
                        origin.x - mask_origin.x,
                        origin.y - mask_origin.y,
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        push[16 + 4 * i..20 + 4 * i].copy_from_slice(&v.to_ne_bytes());
                    }
                    let scissor = shift(scissor, origin);
                    let pipeline = p.composite[blend_index(blend)];
                    // SAFETY: условия функции; картинка слоя готова к чтению.
                    unsafe {
                        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                        device.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, &push);
                        // Только прямоугольник слоя: остальное в картинке слоя не рисовалось.
                        device.cmd_set_scissor(cmd, 0, &[scissor]);
                        device.cmd_draw(cmd, 3, 1, 0, 0);
                        device.cmd_set_scissor(cmd, 0, &[area_here]);
                    }
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
    /// Что рисовать в каждую цель: 0 — кадр сцены, остальные — слои.
    targets: Vec<Vec<Op>>,
    /// Непустые слои в порядке рисования: вложенные раньше внешних.
    layer_order: Vec<u32>,
    /// Прямоугольник каждой цели на экране (после камеры): всё, что в неё рисуется, включая
    /// вложенные слои.
    bounds: Vec<Bounds>,
    /// Экземпляров больше [`MAX_INSTANCES`], лишние пропущены.
    overflow: bool,
    /// Сколько фигур и блоков текста попало в кадр (после отсечения).
    drawn: u32,
}

/// Номер конвейера наложения слоя.
fn blend_index(blend: BlendMode) -> usize {
    match blend {
        BlendMode::Normal => 0,
        BlendMode::Multiply => 1,
        BlendMode::Screen => 2,
        BlendMode::Add => 3,
    }
}

/// Раскладывает кадр в байты для видеокарты: экземпляры (std430), команды рисования
/// треугольников и порядок рисования каждой цели (кадр сцены и слои). Подряд идущие
/// экземпляры одной фигуры в одной цели, которые рисуются треугольниками, становятся одной
/// командой. Экземпляры неизвестных и пустых фигур пропускаются, пустые слои тоже. Фигуры
/// без треугольников (перетекающие) рисуются трафаретом при любом `fill`.
#[cfg(test)]
fn pack(ranges: &[MeshRange], items: &[SceneItem], fill: FillMode) -> Packed {
    pack_frame(
        ranges,
        &[],
        items,
        fill,
        reyvec_vector::Affine::IDENTITY,
        EVERYWHERE,
        &mut StencilRun::default(),
    )
}

/// Видимая область без отсечения (для тестов).
#[cfg(test)]
const EVERYWHERE: Bounds = [f32::MIN, f32::MIN, f32::MAX, f32::MAX];

/// Запас вокруг экрана при отсечении, в единицах экрана (высота кадра = 2): около 10
/// пикселей при высоте окна 1080. Покрывает сглаживание краёв и округления.
const CULL_MARGIN: f32 = 0.02;

/// Видимая область экрана с запасом: x от `-aspect` до `aspect`, y от -1 до 1.
fn visible_area(aspect: f32) -> Bounds {
    [
        -aspect - CULL_MARGIN,
        -1.0 - CULL_MARGIN,
        aspect + CULL_MARGIN,
        1.0 + CULL_MARGIN,
    ]
}

/// Запись экземпляра (16 слов по 4 байта) в байты для видеокарты одним куском: так
/// процессор пишет её одной записью, а не шестнадцатью маленькими.
fn instance_bytes(words: [u32; INSTANCE_SIZE / 4]) -> [u8; INSTANCE_SIZE] {
    let mut out = [0u8; INSTANCE_SIZE];
    for (chunk, w) in out.chunks_exact_mut(4).zip(words) {
        chunk.copy_from_slice(&w.to_ne_bytes());
    }
    out
}

/// Пустая ссылка в цепочках сетки [`StencilRun`].
const NONE: u32 = u32::MAX;

/// Корзина клетки (x, y) сетки [`StencilRun`]; `mask` = число корзин − 1, корзин 4ᵏ.
/// Сетка корзин квадратная и сворачивается по краям: соседние клетки лежат рядом в памяти,
/// а одну корзину делят только далёкие друг от друга клетки.
fn bucket_of(x: i64, y: i64, mask: usize) -> usize {
    let side_bits = (mask + 1).trailing_zeros() / 2;
    let side = (1usize << side_bits) - 1;
    ((x as usize) & side) | (((y as usize) & side) << side_bits)
}

/// Подряд идущие экземпляры одной фигуры, которые рисуются трафаретом (обычно
/// перетекающие). Их можно рисовать не по одному, а слоями: фигура попадает в слой на
/// единицу выше самого верхнего слоя фигур, которые она задевает на экране и которые шли
/// раньше. Фигуры одного слоя друг друга не задевают, поэтому слой рисуется одним вызовом
/// трафарета и одним покрытием, а порядок рисования пересекающихся фигур сохраняется.
/// Чтобы не сравнивать каждую фигуру со всеми, рамки разложены по клеткам сетки размером
/// с первую рамку.
#[derive(Default)]
struct StencilRun {
    target: usize,
    shape: Option<ShapeId>,
    index_count: u32,
    even_odd: bool,
    /// Слой каждого экземпляра и их данные подряд, в порядке сцены.
    levels_of: Vec<u32>,
    data: Vec<u8>,
    /// Сколько экземпляров в каждом слое (для раскладки по слоям при записи).
    per_level: Vec<u32>,
    cell: f32,
    /// Рамка, слой и следующая рамка той же корзины у каждой фигуры, разложенной по сетке.
    placed: Vec<(Bounds, u32, u32)>,
    /// Самые большие полуширина и полувысота рамок группы.
    max_half: [f32; 2],
    /// Сетка: клетка (x, y) попадает в корзину (см. [`bucket_of`]), у корзины цепочка
    /// рамок через `placed`. Совпадение корзин у далёких клеток только добавляет лишнюю
    /// проверку рамок. Без выделения памяти на каждую фигуру.
    /// Начало цепочки корзины и поколение, в котором оно записано: запись старого
    /// поколения значит «пусто», поэтому сброс сетки — это `generation += 1`.
    heads: Vec<(u32, u32)>,
    generation: u32,
    /// Сколько слоёв уже есть.
    levels: u32,
    /// Ниже этого слоя новые фигуры не кладутся (после фигуры, которую не разложить по
    /// сетке: она считается задевающей всё).
    floor: u32,
}

impl StencilRun {
    /// Рамка больше этого числа клеток (клетка — размер первой рамки) считается
    /// задевающей все: иначе пришлось бы смотреть слишком много клеток.
    const MAX_REACH: i64 = 3;
    /// Корзин сетки в начале (степень двойки); растёт вместе с числом фигур.
    const MIN_BUCKETS: usize = 256;

    fn len(&self) -> usize {
        self.levels_of.len()
    }

    /// Экземпляр фигуры `shape` в цель `target` продолжает накопленное.
    fn accepts(&self, target: usize, shape: ShapeId) -> bool {
        self.levels_of.is_empty() || (self.target == target && self.shape == Some(shape))
    }

    fn add(&mut self, target: usize, shape: ShapeId, range: &MeshRange, b: Bounds) -> &mut Vec<u8> {
        let usable = b.iter().all(|v| v.is_finite()) && b[0] <= b[2] && b[1] <= b[3];
        if self.levels_of.is_empty() {
            self.target = target;
            self.shape = Some(shape);
            self.index_count = range.curve_index_count;
            self.even_odd = range.even_odd;
            // Клетка в полтора раза больше первой рамки: в клетке в среднем одна-две
            // фигуры, а поиск соседей задевает 2×2–3×3 клетки.
            self.cell = if usable {
                1.5 * (b[2] - b[0]).max(b[3] - b[1]).max(1e-4)
            } else {
                1.0
            };
        }
        let level = self.place(b, usable);
        self.levels = self.levels.max(level + 1);
        if self.per_level.len() <= level as usize {
            self.per_level.resize(level as usize + 1, 0);
        }
        self.per_level[level as usize] += 1;
        self.levels_of.push(level);
        &mut self.data
    }

    /// Слой для рамки `b`, и рамка запоминается. Рамка кладётся в клетку своего центра.
    /// Центр задевающей рамки отстоит от нашего не больше чем на сумму полуширин (и
    /// полувысот), поэтому смотрим только клетки, куда такой центр может попасть: при
    /// клетке в полтора размера фигуры это обычно 2×2 или 2×3 клетки, а не 3×3.
    fn place(&mut self, b: Bounds, usable: bool) -> u32 {
        let (hw, hh) = ((b[2] - b[0]) * 0.5, (b[3] - b[1]) * 0.5);
        if !usable || 2.0 * hw.max(hh) > Self::MAX_REACH as f32 * self.cell {
            // Выше всех, и все следующие тоже выше неё.
            let level = self.levels;
            self.floor = level + 1;
            return level;
        }
        self.max_half = [self.max_half[0].max(hw), self.max_half[1].max(hh)];
        if self.heads.is_empty() {
            self.heads.resize(Self::MIN_BUCKETS, (0, NONE));
        } else if self.placed.len() > 2 * self.heads.len() {
            self.grow();
        }
        let (mx, my) = ((b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5);
        // Небольшой запас: округление не должно увести край поиска за клетку соседа.
        let slack = self.cell * 1e-3;
        let (rx, ry) = (hw + self.max_half[0] + slack, hh + self.max_half[1] + slack);
        let (x0, x1) = (self.cell_of(mx - rx), self.cell_of(mx + rx));
        let (y0, y1) = (self.cell_of(my - ry), self.cell_of(my + ry));
        let mask = self.heads.len() - 1;
        let mut level = self.floor;
        // Полуоткрытые диапазоны: `..=` в горячем цикле заметно дороже.
        for y in y0..y1 + 1 {
            for x in x0..x1 + 1 {
                let (generation, head) = self.heads[bucket_of(x, y, mask)];
                let mut e = if generation == self.generation {
                    head
                } else {
                    NONE
                };
                while e != NONE {
                    let (other, other_level, next) = self.placed[e as usize];
                    if other_level >= level && overlaps(other, b) {
                        level = other_level + 1;
                    }
                    e = next;
                }
            }
        }
        let idx = self.placed.len() as u32;
        self.placed.push((b, level, NONE));
        self.insert(idx, self.cell_of(mx), self.cell_of(my));
        level
    }

    /// Номер клетки для координаты `v`.
    fn cell_of(&self, v: f32) -> i64 {
        // Округление вниз без вызова floorf (он не встраивается и заметно дорог здесь).
        let x = v / self.cell;
        let t = x as i64;
        if (t as f32) > x { t - 1 } else { t }
    }

    /// Клетка центра рамки `b`.
    fn center_cell(&self, b: Bounds) -> (i64, i64) {
        (
            self.cell_of((b[0] + b[2]) * 0.5),
            self.cell_of((b[1] + b[3]) * 0.5),
        )
    }

    fn insert(&mut self, idx: u32, x: i64, y: i64) {
        let mask = self.heads.len() - 1;
        let slot = &mut self.heads[bucket_of(x, y, mask)];
        let next = if slot.0 == self.generation {
            slot.1
        } else {
            NONE
        };
        self.placed[idx as usize].2 = next;
        *slot = (self.generation, idx);
    }

    /// Цепочки стали длинными: корзин в четыре раза больше, рамки раскладываются заново.
    fn grow(&mut self) {
        let size = self.heads.len() * 4;
        self.heads.clear();
        self.heads
            .resize(size, (self.generation.wrapping_sub(1), NONE));
        for idx in 0..self.placed.len() {
            let (x, y) = self.center_cell(self.placed[idx].0);
            self.insert(idx as u32, x, y);
        }
    }

    /// Сетка снова пустая. Размер сетки остаётся, как у самой большой группы.
    fn next_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            // Раз в 4 миллиарда сбросов: старые записи могли бы совпасть по поколению.
            self.heads.fill((u32::MAX, NONE));
        }
    }

    /// Забыть накопленное (память остаётся).
    fn clear(&mut self) {
        self.levels_of.clear();
        self.data.clear();
        self.per_level.clear();
        self.next_generation();
        self.placed.clear();
        self.max_half = [0.0; 2];
        self.levels = 0;
        self.floor = 0;
        self.shape = None;
    }

    /// Пишет накопленные экземпляры по слоям и по вызову на слой.
    fn flush(&mut self, out: &mut Vec<u8>, targets: &mut [Vec<Op>], written: &mut u32) {
        let n = self.levels_of.len();
        if n == 0 {
            return;
        }
        let ops = &mut targets[self.target];
        let mut first = *written;
        for &count in &self.per_level[..self.levels as usize] {
            ops.push(Op::Stencil {
                instance: first,
                count,
                index_count: self.index_count,
                even_odd: self.even_odd,
            });
            first += count;
        }
        if self.levels == 1 {
            out.extend_from_slice(&self.data);
        } else {
            // Слой за слоем, внутри слоя порядок сцены. Слоёв обычно единицы, поэтому
            // несколько проходов по списку дешевле общей сортировки.
            out.reserve(n * INSTANCE_SIZE);
            for level in 0..self.levels {
                for (data, _) in self
                    .data
                    .chunks_exact(INSTANCE_SIZE)
                    .zip(&self.levels_of)
                    .filter(|&(_, &l)| l == level)
                {
                    out.extend_from_slice(data);
                }
            }
        }
        *written += n as u32;
        self.levels_of.clear();
        self.data.clear();
        self.per_level.clear();
        if !self.placed.is_empty() {
            self.next_generation();
        }
        self.placed.clear();
        self.max_half = [0.0; 2];
        self.levels = 0;
        self.floor = 0;
        self.shape = None;
    }
}

/// Прямоугольники пересекаются (касание считается).
fn overlaps(a: Bounds, b: Bounds) -> bool {
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}

/// Пишет вид камеры в константы конвейера (`view_linear`, `view_offset` в шейдерах).
fn write_view(push: &mut [u8; PUSH_SIZE], view: [f32; 6]) {
    for (i, v) in view.into_iter().enumerate() {
        let at = VIEW_OFFSET + 4 * i;
        push[at..at + 4].copy_from_slice(&v.to_ne_bytes());
    }
}

/// Как [`pack`], и ещё блоки текста `texts` (по номерам [`reyvec_vector::TextId`]) и вид
/// камеры `view`: прямоугольники слоёв считаются уже на экране. Фигуры и блоки текста,
/// чья рамка на экране не задевает `visible`, не пишутся вовсе (отсечение).
fn pack_frame(
    ranges: &[MeshRange],
    texts: &[TextRange],
    items: &[SceneItem],
    fill: FillMode,
    view: reyvec_vector::Affine,
    visible: Bounds,
    run: &mut StencilRun,
) -> Packed {
    let mut out = Vec::new();
    let mut commands: Vec<[u32; 4]> = Vec::new();
    let mut targets: Vec<Vec<Op>> = vec![Vec::new()];
    let mut bounds: Vec<Bounds> = vec![EMPTY_BOUNDS];
    let mut layer_order = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut last: Option<(usize, ShapeId)> = None;
    // Подряд идущие фигуры трафаретом копятся в `run` и пишутся слоями (см. `StencilRun`);
    // его память живёт между кадрами.
    run.clear();
    let mut written = 0u32;
    let mut overflow = false;

    for item in items {
        let skipping = matches!(stack.last(), Some(Open::Skip { .. }));
        // Всё, кроме фигур, сначала дописывает накопленные фигуры трафаретом: порядок
        // рисования сохраняется.
        if !matches!(item, SceneItem::Shape(_)) {
            run.flush(&mut out, &mut targets, &mut written);
        }
        let inst = match *item {
            SceneItem::BeginLayer { opacity, blend } => {
                last = None;
                if skipping {
                    stack.push(Open::Skip { mask: false });
                    continue;
                }
                targets.push(Vec::new());
                bounds.push(EMPTY_BOUNDS);
                let opacity = if opacity.is_nan() {
                    0.0
                } else {
                    opacity.clamp(0.0, 1.0)
                };
                stack.push(Open::Layer {
                    idx: targets.len() - 1,
                    opacity,
                    blend,
                    mask: None,
                    hidden: false,
                });
                continue;
            }
            SceneItem::BeginMask { mode, inverted } => {
                last = None;
                // Маска только прямо внутри слоя, иначе она и её фигуры пропускаются.
                if !matches!(stack.last(), Some(Open::Layer { .. })) {
                    stack.push(Open::Skip { mask: true });
                    continue;
                }
                targets.push(Vec::new());
                bounds.push(EMPTY_BOUNDS);
                stack.push(Open::Mask {
                    idx: targets.len() - 1,
                    mode,
                    inverted,
                });
                continue;
            }
            SceneItem::EndLayer => {
                last = None;
                match stack.last() {
                    Some(Open::Layer { .. } | Open::Skip { mask: false }) => {}
                    _ => continue, // лишний или не тот конец
                }
                if let Some(Open::Layer {
                    idx,
                    opacity,
                    blend,
                    mask,
                    hidden,
                }) = stack.pop()
                {
                    close_layer(
                        (idx, opacity, blend, mask, hidden),
                        &stack,
                        &mut targets,
                        &mut bounds,
                        &mut layer_order,
                    );
                }
                continue;
            }
            SceneItem::EndMask => {
                last = None;
                match stack.last() {
                    Some(Open::Mask { .. } | Open::Skip { mask: true }) => {}
                    _ => continue,
                }
                if let Some(Open::Mask {
                    idx,
                    mode,
                    inverted,
                }) = stack.pop()
                {
                    close_mask(idx, mode, inverted, &mut stack, &targets, &mut layer_order);
                }
                continue;
            }
            SceneItem::Shape(_) | SceneItem::Text(_) if skipping => continue,
            SceneItem::Text(t) => {
                last = None;
                let target = stack.last().map_or(0, Open::target);
                let Some(range) = texts.get(t.text.0 as usize) else {
                    continue;
                };
                if range.glyph_count == 0 {
                    continue;
                }
                let on_screen = transform_bounds(&(view * t.transform), range.bounds);
                if !overlaps(on_screen, visible) {
                    continue;
                }
                if written as usize >= MAX_INSTANCES {
                    overflow = true;
                    continue;
                }
                let [a, b, c, d, e, f] = t.transform.as_coeffs().map(|v| v as f32);
                let [r, g, bl, al] = range.color.multiply(t.tint).to_array();
                out.extend_from_slice(&instance_bytes([
                    a.to_bits(),
                    b.to_bits(),
                    c.to_bits(),
                    d.to_bits(),
                    e.to_bits(),
                    f.to_bits(),
                    range.first_glyph,
                    range.glyph_count,
                    r.to_bits(),
                    g.to_bits(),
                    bl.to_bits(),
                    al.to_bits(),
                    0,
                    0,
                    0,
                    NO_PAINT,
                ]));
                if target != 0 {
                    bounds[target] = union(bounds[target], on_screen);
                }
                targets[target].push(Op::Text {
                    instance: written,
                    glyph_count: range.glyph_count,
                });
                written += 1;
                continue;
            }
            SceneItem::Shape(inst) => inst,
        };
        let target = stack.last().map_or(0, Open::target);
        let Some(range) = ranges.get(inst.shape.0 as usize) else {
            continue;
        };
        let stencil =
            range.curve_index_count > 0 && (range.index_count == 0 || fill.stencil(range.method));
        if range.index_count == 0 && !stencil {
            continue;
        }
        let on_screen = transform_bounds(&(view * inst.transform), range.bounds);
        if !overlaps(on_screen, visible) {
            continue;
        }
        if !(stencil && run.accepts(target, inst.shape)) {
            run.flush(&mut out, &mut targets, &mut written);
        }
        if written as usize + run.len() >= MAX_INSTANCES {
            overflow = true;
            continue;
        }
        if target != 0 {
            bounds[target] = union(bounds[target], on_screen);
        }
        // Фигуры трафаретом пишутся в накопитель (потом по слоям), остальные сразу.
        let out = if stencil {
            run.add(target, inst.shape, range, on_screen)
        } else {
            &mut out
        };
        let [a, b, c, d, e, f] = inst.transform.as_coeffs().map(|v| v as f32);
        let [r, g, bl, al] = range.color.multiply(inst.tint).to_array();
        out.extend_from_slice(&instance_bytes([
            a.to_bits(),
            b.to_bits(),
            c.to_bits(),
            d.to_bits(),
            e.to_bits(),
            f.to_bits(),
            range.first_index,
            range.curve_first_index,
            r.to_bits(),
            g.to_bits(),
            bl.to_bits(),
            al.to_bits(),
            range.morph_stride,
            range.morph_last,
            inst.morph.to_bits(),
            range.paint,
        ]));
        if stencil {
            last = None;
            continue;
        }
        let ops = &mut targets[target];
        {
            let same = last == Some((target, inst.shape));
            // `last` ставится только после команды треугольников этой цели и
            // сбрасывается на всём остальном, поэтому последняя команда и есть отрезок
            // этой фигуры.
            match (commands.last_mut(), ops.last()) {
                (Some(cmd), Some(Op::Triangles { .. })) if same => cmd[1] += 1,
                _ => {
                    commands.push([range.index_count, 1, 0, written]);
                    let new = commands.len() as u32 - 1;
                    match ops.last_mut() {
                        Some(Op::Triangles { first, count }) if *first + *count == new => {
                            *count += 1;
                        }
                        _ => ops.push(Op::Triangles {
                            first: new,
                            count: 1,
                        }),
                    }
                }
            }
            last = Some((target, inst.shape));
        }
        written += 1;
    }
    run.flush(&mut out, &mut targets, &mut written);
    // Незакрытые слои и маски закрываются в конце кадра.
    while let Some(open) = stack.pop() {
        match open {
            Open::Layer {
                idx,
                opacity,
                blend,
                mask,
                hidden,
            } => close_layer(
                (idx, opacity, blend, mask, hidden),
                &stack,
                &mut targets,
                &mut bounds,
                &mut layer_order,
            ),
            Open::Mask {
                idx,
                mode,
                inverted,
            } => close_mask(idx, mode, inverted, &mut stack, &targets, &mut layer_order),
            Open::Skip { .. } => {}
        }
    }
    Packed {
        instances: out,
        commands: commands
            .iter()
            .flatten()
            .flat_map(|w| w.to_ne_bytes())
            .collect(),
        targets,
        layer_order,
        bounds,
        overflow,
        drawn: written,
    }
}

/// Открытая скобка при раскладке кадра.
enum Open {
    Layer {
        idx: usize,
        opacity: f32,
        blend: BlendMode,
        mask: Option<LayerMask>,
        /// Маска пустая и не обратная: слой нигде не виден.
        hidden: bool,
    },
    Mask {
        idx: usize,
        mode: MaskMode,
        inverted: bool,
    },
    /// Пропускаемая скобка (маска вне слоя и всё внутри неё). `mask`: чем закрывается.
    Skip { mask: bool },
}

impl Open {
    /// Цель, куда рисуются фигуры внутри скобки.
    fn target(&self) -> usize {
        match *self {
            Open::Layer { idx, .. } | Open::Mask { idx, .. } => idx,
            Open::Skip { .. } => 0,
        }
    }
}

/// Закрывает слой: если в нём что-то есть и он не скрыт пустой маской, он попадает в
/// порядок рисования и накладывается в родителя. С маской (не обратной) слой виден
/// только в пересечении своего прямоугольника и прямоугольника маски; маска рисуется
/// в том же прямоугольнике, что и слой.
fn close_layer(
    (idx, opacity, blend, mask, hidden): (usize, f32, BlendMode, Option<LayerMask>, bool),
    stack: &[Open],
    targets: &mut [Vec<Op>],
    bounds: &mut [Bounds],
    layer_order: &mut Vec<u32>,
) {
    if targets[idx].is_empty() || hidden {
        return;
    }
    if let Some(m) = mask {
        let t = m.target as usize;
        if !m.inverted {
            bounds[idx] = intersect_bounds(bounds[idx], bounds[t]);
        }
        bounds[t] = bounds[idx];
    }
    layer_order.push(idx as u32);
    let parent = stack.last().map_or(0, Open::target);
    bounds[parent] = union(bounds[parent], bounds[idx]);
    targets[parent].push(Op::Composite {
        layer: idx as u32,
        opacity,
        blend,
        mask,
    });
}

/// Закрывает маску и отдаёт её слою, в котором она стоит.
fn close_mask(
    idx: usize,
    mode: MaskMode,
    inverted: bool,
    stack: &mut [Open],
    targets: &[Vec<Op>],
    layer_order: &mut Vec<u32>,
) {
    let Some(Open::Layer { mask, hidden, .. }) = stack.last_mut() else {
        return;
    };
    if targets[idx].is_empty() {
        // Пустая маска: обычная прячет слой целиком, обратная ничего не прячет.
        *hidden = !inverted;
        *mask = None;
        return;
    }
    *hidden = false;
    *mask = Some(LayerMask {
        target: idx as u32,
        mode,
        inverted,
    });
    layer_order.push(idx as u32);
}

/// Готовая картинка слоя для наложения.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LayerTexture {
    /// Номер в общем списке текстур.
    pub texture: u32,
    /// Где в картинке нарисованы фигуры слоя: накладывается только этот прямоугольник.
    pub rect: vk::Rect2D,
}

/// Общая часть двух прямоугольников пикселей или `None`, если её нет.
/// Прямоугольник кадра `r` в пикселях картинки, которая начинается в пикселе кадра `origin`.
fn shift(r: vk::Rect2D, origin: vk::Offset2D) -> vk::Rect2D {
    vk::Rect2D {
        offset: vk::Offset2D {
            x: r.offset.x - origin.x,
            y: r.offset.y - origin.y,
        },
        extent: r.extent,
    }
}

fn intersect(a: vk::Rect2D, b: vk::Rect2D) -> Option<vk::Rect2D> {
    let x0 = a.offset.x.max(b.offset.x);
    let y0 = a.offset.y.max(b.offset.y);
    let x1 = (a.offset.x + a.extent.width as i32).min(b.offset.x + b.extent.width as i32);
    let y1 = (a.offset.y + a.extent.height as i32).min(b.offset.y + b.extent.height as i32);
    (x1 > x0 && y1 > y0).then(|| vk::Rect2D {
        offset: vk::Offset2D { x: x0, y: y0 },
        extent: vk::Extent2D {
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
        },
    })
}

/// Пиксели кадра `extent`, которые накрывает прямоугольник сцены `b` (y вверх от −1 до 1,
/// x от −aspect до aspect), с запасом в 2 пикселя. Не меньше 1×1, внутри кадра.
fn pixel_rect(b: Bounds, aspect: f32, extent: vk::Extent2D) -> vk::Rect2D {
    const PAD: f32 = 2.0;
    let (w, h) = (extent.width as f32, extent.height as f32);
    let to_x = |x: f32| (x / aspect + 1.0) * 0.5 * w;
    let to_y = |y: f32| (1.0 - y) * 0.5 * h;
    let clamp = |v: f32, max: f32| if v.is_nan() { 0.0 } else { v.clamp(0.0, max) };
    let x0 = clamp((to_x(b[0]) - PAD).floor(), w - 1.0);
    let x1 = clamp((to_x(b[2]) + PAD).ceil(), w);
    let y0 = clamp((to_y(b[3]) - PAD).floor(), h - 1.0);
    let y1 = clamp((to_y(b[1]) + PAD).ceil(), h);
    vk::Rect2D {
        offset: vk::Offset2D {
            x: x0 as i32,
            y: y0 as i32,
        },
        extent: vk::Extent2D {
            width: ((x1 - x0) as u32).max(1),
            height: ((y1 - y0) as u32).max(1),
        },
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
        made.text = one("vector_text", Blend::Alpha, StencilUse::Ignore)?;
        for (i, blend) in [
            Blend::Premultiplied,
            Blend::Multiply,
            Blend::Screen,
            Blend::Add,
        ]
        .into_iter()
        .enumerate()
        {
            made.composite[i] = one("vector_layer", blend, StencilUse::Ignore)?;
        }
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

    #[test]
    fn arenas_grow_by_powers_of_two() {
        assert_eq!(arena_capacity(0), MIN_ARENA);
        assert_eq!(arena_capacity(MIN_ARENA + 1), MIN_ARENA * 2);
        assert_eq!(arena_capacity(1_000_000), 1 << 20);
    }

    /// Фигуры, разложенные двумя пачками, дают те же байты, что и одной: номера точек
    /// и градиентов во второй пачке продолжаются.
    #[test]
    fn packing_in_batches_matches_packing_at_once() {
        let mut lib = ShapeLibrary::new(0.01);
        let square = || {
            let mut p = reyvec_vector::BezPath::new();
            p.move_to((0.0, 0.0));
            p.line_to((1.0, 0.0));
            p.line_to((1.0, 1.0));
            p.close_path();
            p
        };
        let gradient = reyvec_vector::Gradient::linear((0.0, 0.0), (1.0, 0.0))
            .stop(0.0, Color::WHITE)
            .stop(1.0, Color::WHITE);
        lib.add(reyvec_vector::Shape::fill(square(), Color::WHITE))
            .unwrap();
        lib.add(reyvec_vector::Shape::fill(square(), gradient.clone()))
            .unwrap();
        lib.add(reyvec_vector::Shape::stroke(
            square(),
            reyvec_vector::Stroke::new(0.1),
            Color::WHITE,
        ))
        .unwrap();
        lib.add(reyvec_vector::Shape::fill(square(), gradient))
            .unwrap();
        let mut c = Counters::default();
        let all = pack_shapes(&lib, 0..4, &mut c);
        let mut c2 = Counters::default();
        let first = pack_shapes(&lib, 0..2, &mut c2);
        let second = pack_shapes(&lib, 2..4, &mut c2);
        for k in 0..6 {
            assert_eq!(
                all.bytes[k],
                [first.bytes[k].clone(), second.bytes[k].clone()].concat()
            );
        }
        assert_eq!(second.ranges[1].paint, 1);
        assert_eq!(
            second.ranges[0].first_index,
            first.ranges[1].first_index + first.ranges[1].index_count
        );
        assert_eq!(c.paints, 2);
        assert_eq!(stream_count(&lib, 0), 4);
        assert_eq!(stream_count(&lib, 3), 1);
    }
    use reyvec_vector::{Affine, Instance};

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
        bounds: [-1.0, -1.0, 1.0, 1.0],
    };

    fn items(instances: &[Instance]) -> Vec<SceneItem> {
        instances.iter().copied().map(SceneItem::from).collect()
    }

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
            targets,
            ..
        } = pack(&ranges, &items(&instances), FillMode::Triangles);
        assert_eq!(inst.len(), 5 * INSTANCE_SIZE);
        assert_eq!(targets, [[Op::Triangles { first: 0, count: 3 }]]);
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
            count: 1,
            index_count,
            even_odd: false,
        };
        let per_shape = pack(&ranges, &items(&instances), FillMode::PerShape);
        assert_eq!(
            per_shape.targets[0],
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
        let triangles = pack(&ranges, &items(&instances), FillMode::Triangles);
        assert_eq!(
            triangles.targets[0],
            [Op::Triangles { first: 0, count: 2 }, stencil(2, 12)]
        );
        let all = pack(&ranges, &items(&instances), FillMode::Stencil);
        assert_eq!(
            all.targets[0],
            [stencil(0, 6), stencil(1, 6), stencil(2, 12)]
        );
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
        let packed = pack(&ranges, &items(&instances), FillMode::Stencil);
        assert_eq!(
            packed.targets[0],
            [
                Op::Triangles { first: 0, count: 1 },
                Op::Stencil {
                    instance: 2,
                    count: 1,
                    index_count: 12,
                    even_odd: false
                },
                Op::Triangles { first: 1, count: 1 },
                Op::Stencil {
                    instance: 4,
                    count: 1,
                    index_count: 9,
                    even_odd: true
                },
                Op::Stencil {
                    instance: 5,
                    count: 1,
                    index_count: 9,
                    even_odd: true
                },
            ]
        );
        assert_eq!(words(&packed.commands), [3, 2, 0, 0, 3, 1, 0, 3]);
        // Номер первого треугольника веера лежит в экземпляре рядом с номером способа 1.
        assert_eq!(words(&packed.instances[24..32]), [3, 30]);
        // Тот же кадр треугольниками: все шесть экземпляров в командах.
        let triangles = pack(&ranges, &items(&instances), FillMode::Triangles);
        assert_eq!(triangles.targets[0], [Op::Triangles { first: 0, count: 4 }]);
    }

    #[test]
    fn layers_get_own_targets_and_composite_in_parent() {
        let ranges = [
            MeshRange {
                index_count: 3,
                ..PLAIN
            },
            MeshRange {
                first_index: 3,
                index_count: 3,
                ..PLAIN
            },
        ];
        let shape = |s: u32| SceneItem::from(Instance::new(ShapeId(s), Affine::IDENTITY));
        let items = [
            shape(0),
            SceneItem::layer(0.5),
            shape(0), // та же фигура, но в слое: отдельная команда
            SceneItem::blend(BlendMode::Multiply, 2.0),
            shape(1),
            SceneItem::EndLayer,
            shape(0), // после вложенного слоя: снова новая команда
            SceneItem::EndLayer,
            SceneItem::EndLayer, // лишний конец: пропускается
            SceneItem::layer(1.0),
            SceneItem::EndLayer, // пустой слой: не рисуется
            shape(0),
            SceneItem::blend(BlendMode::Add, 0.25),
            shape(1), // незакрытый слой закрывается в конце кадра
        ];
        let packed = pack(&ranges, &items, FillMode::PerShape);
        let tri = |first| Op::Triangles { first, count: 1 };
        assert_eq!(
            packed.targets[0],
            [
                tri(0),
                Op::Composite {
                    layer: 1,
                    opacity: 0.5,
                    blend: BlendMode::Normal,
                    mask: None
                },
                tri(4),
                Op::Composite {
                    layer: 4,
                    opacity: 0.25,
                    blend: BlendMode::Add,
                    mask: None
                },
            ]
        );
        assert_eq!(
            packed.targets[1],
            [
                tri(1),
                Op::Composite {
                    layer: 2,
                    opacity: 1.0,
                    blend: BlendMode::Multiply,
                    mask: None
                },
                tri(3),
            ]
        );
        assert_eq!(packed.targets[2], [tri(2)]);
        assert!(packed.targets[3].is_empty());
        assert_eq!(packed.targets[4], [tri(5)]);
        assert_eq!(packed.layer_order, [2, 1, 4], "вложенные раньше внешних");
        // Все экземпляры в нуле, прямоугольник фигуры ±1: у слоёв ±1, вложенный внутри
        // внешнего; у пустого слоя пусто.
        assert_eq!(packed.bounds[1], [-1.0, -1.0, 1.0, 1.0]);
        assert_eq!(packed.bounds[2], [-1.0, -1.0, 1.0, 1.0]);
        assert_eq!(packed.bounds[3], EMPTY_BOUNDS);
        assert_eq!(words(&packed.commands).len(), 6 * 4);
        assert!(!packed.overflow);
    }

    #[test]
    fn too_many_instances_are_dropped_with_flag() {
        let ranges = [MeshRange {
            index_count: 3,
            ..PLAIN
        }];
        let one = Instance::new(ShapeId(0), Affine::IDENTITY);
        let items = vec![SceneItem::from(one); MAX_INSTANCES + 2];
        let packed = pack(&ranges, &items, FillMode::PerShape);
        assert!(packed.overflow);
        assert_eq!(packed.instances.len(), MAX_INSTANCES * INSTANCE_SIZE);
        assert_eq!(words(&packed.commands)[1], MAX_INSTANCES as u32);
    }

    #[test]
    fn text_blocks_are_one_instance_each_and_split_shape_runs() {
        let ranges = [MeshRange {
            index_count: 3,
            ..PLAIN
        }];
        let gold = Color::linear(1.0, 0.8, 0.2, 1.0);
        let texts = [
            TextRange {
                first_glyph: 0,
                glyph_count: 5,
                color: gold,
                bounds: [0.0, -0.1, 0.5, 0.0],
            },
            TextRange {
                first_glyph: 5,
                glyph_count: 0,
                color: Color::WHITE,
                bounds: EMPTY_BOUNDS,
            },
        ];
        let shape = SceneItem::from(Instance::new(ShapeId(0), Affine::IDENTITY));
        let text = |id, x| {
            SceneItem::from(reyvec_vector::TextInstance::at(
                reyvec_vector::TextId(id),
                x,
                0.0,
            ))
        };
        let items = [
            shape,
            text(0, 0.0),
            shape,
            // Пустой блок и неизвестный номер пропускаются.
            text(1, 0.0),
            text(7, 0.0),
            SceneItem::layer(1.0),
            text(0, 1.0),
            SceneItem::EndLayer,
        ];
        let packed = pack_frame(
            &ranges,
            &texts,
            &items,
            FillMode::PerShape,
            reyvec_vector::Affine::IDENTITY,
            EVERYWHERE,
            &mut StencilRun::default(),
        );
        assert_eq!(
            packed.targets[0],
            vec![
                Op::Triangles { first: 0, count: 1 },
                Op::Text {
                    instance: 1,
                    glyph_count: 5
                },
                Op::Triangles { first: 1, count: 1 },
                Op::Composite {
                    layer: 1,
                    opacity: 1.0,
                    blend: BlendMode::Normal,
                    mask: None
                },
            ]
        );
        assert_eq!(
            packed.targets[1],
            vec![Op::Text {
                instance: 3,
                glyph_count: 5
            }]
        );
        assert_eq!(packed.bounds[1], [1.0, -0.1, 1.5, 0.0]);
        // Экземпляр блока: сдвиг, первая буква и число букв, цвет блока.
        let w = words(&packed.instances[INSTANCE_SIZE..2 * INSTANCE_SIZE]);
        assert_eq!(&w[6..8], &[0, 5]);
        assert_eq!(f32::from_bits(w[8]), 1.0);
        assert_eq!(f32::from_bits(w[9]), 0.8);
        assert_eq!(w[15], NO_PAINT);
        assert_eq!(packed.instances.len(), 4 * INSTANCE_SIZE);
    }

    #[test]
    fn layer_bounds_follow_instances_and_nested_layers() {
        let ranges = [MeshRange {
            index_count: 3,
            bounds: [0.0, 0.0, 0.1, 0.2],
            ..PLAIN
        }];
        let at = |t| SceneItem::from(Instance::new(ShapeId(0), t));
        let items = [
            SceneItem::layer(1.0),
            at(Affine::translate((1.0, 0.0))),
            SceneItem::layer(1.0),
            // Повёрнут на 90°: (0..0.1, 0..0.2) → (−0.2..0, 0..0.1), потом сдвиг.
            at(Affine::translate((-1.0, -0.5)) * Affine::rotate(std::f64::consts::FRAC_PI_2)),
            SceneItem::EndLayer,
            SceneItem::EndLayer,
        ];
        let packed = pack(&ranges, &items, FillMode::PerShape);
        let near = |a: Bounds, b: Bounds| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(
            near(packed.bounds[2], [-1.2, -0.5, -1.0, -0.4]),
            "{:?}",
            packed.bounds[2]
        );
        assert!(
            near(packed.bounds[1], [-1.2, -0.5, 1.1, 0.2]),
            "{:?}",
            packed.bounds[1]
        );
    }

    #[test]
    fn layer_bounds_are_on_screen_under_camera() {
        let ranges = [MeshRange {
            index_count: 3,
            bounds: [0.0, 0.0, 0.1, 0.2],
            ..PLAIN
        }];
        let items = [
            SceneItem::layer(1.0),
            SceneItem::from(Instance::new(ShapeId(0), Affine::translate((1.0, 0.0)))),
            SceneItem::EndLayer,
        ];
        // Камера с центром (1, 0) и приближением 4: фигура 1..1.1 × 0..0.2 на экране
        // 0..0.4 × 0..0.8 (слой рисуется в прямоугольнике там, где фигура видна).
        let view = reyvec_vector::Camera2D::new((1.0, 0.0), 4.0).view();
        let packed = pack_frame(
            &ranges,
            &[],
            &items,
            FillMode::PerShape,
            view,
            EVERYWHERE,
            &mut StencilRun::default(),
        );
        let near = |a: Bounds, b: Bounds| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(
            near(packed.bounds[1], [0.0, 0.0, 0.4, 0.8]),
            "{:?}",
            packed.bounds[1]
        );
        // Сами экземпляры не меняются: камеру применяет видеокарта.
        let plain = pack(&ranges, &items, FillMode::PerShape);
        assert_eq!(packed.instances, plain.instances);
    }

    #[test]
    fn stencil_levels_match_brute_force() {
        // Случайные рамки разного размера: слой по сетке должен совпасть со слоем,
        // посчитанным сравнением каждой рамки со всеми предыдущими.
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let range = MeshRange {
            curve_index_count: 3,
            method: FillMethod::Stencil,
            ..PLAIN
        };
        let mut run = StencilRun::default();
        for round in 0..3 {
            run.clear();
            let boxes: Vec<Bounds> = (0..2000)
                .map(|_| {
                    let (x, y) = (rnd() * 4.0 - 2.0, rnd() * 2.0 - 1.0);
                    let (w, h) = (0.02 + rnd() * 0.06, 0.02 + rnd() * 0.06);
                    [x, y, x + w, y + h]
                })
                .collect();
            for b in &boxes {
                run.add(0, ShapeId(0), &range, *b);
            }
            for (i, b) in boxes.iter().enumerate() {
                let expected = (0..i)
                    .filter(|&j| overlaps(boxes[j], *b))
                    .map(|j| run.levels_of[j] + 1)
                    .max()
                    .unwrap_or(0);
                assert_eq!(run.levels_of[i], expected, "round {round}, box {i}");
            }
        }
    }

    #[test]
    fn disjoint_stencil_shapes_share_one_draw() {
        let ranges = [MeshRange {
            index_count: 0,
            curve_index_count: 12,
            method: FillMethod::Stencil,
            bounds: [-0.1, -0.1, 0.1, 0.1],
            ..PLAIN
        }];
        let at = |x: f64| SceneItem::from(Instance::new(ShapeId(0), Affine::translate((x, 0.0))));
        // Первые три стоят порознь, четвёртая налезает на третью, пятая снова отдельно:
        // она идёт в первый слой, а четвёртая во второй, после третьей.
        let items = [at(0.0), at(0.5), at(1.0), at(1.15), at(-1.0)];
        let packed = pack(&ranges, &items, FillMode::PerShape);
        let stencil = |instance, count| Op::Stencil {
            instance,
            count,
            index_count: 12,
            even_odd: false,
        };
        assert_eq!(packed.targets[0], [stencil(0, 4), stencil(4, 1)]);
        let x = |i: usize| {
            f32::from_ne_bytes(
                packed.instances[i * INSTANCE_SIZE + 16..i * INSTANCE_SIZE + 20]
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!([x(0), x(1), x(2), x(3), x(4)], [0.0, 0.5, 1.0, -1.0, 1.15]);
        // Три одинаковые друг на друге: три слоя по одной, в порядке сцены.
        let packed = pack(&ranges, &[at(0.0), at(0.0), at(0.0)], FillMode::PerShape);
        assert_eq!(
            packed.targets[0],
            [stencil(0, 1), stencil(1, 1), stencil(2, 1)]
        );
        // Много фигур сеткой: одна команда на всех.
        let grid: Vec<_> = (0..400)
            .map(|i| {
                let (x, y) = ((i % 20) as f64 * 0.25, (i / 20) as f64 * 0.25);
                SceneItem::from(Instance::new(ShapeId(0), Affine::translate((x, y))))
            })
            .collect();
        let packed = pack(&ranges, &grid, FillMode::PerShape);
        assert_eq!(packed.targets[0], [stencil(0, 400)]);
    }

    /// Замер: `cargo test --release -p reyvec-gpu pack_10k -- --ignored --nocapture`.
    #[test]
    #[ignore = "замер, не проверка"]
    fn pack_10k_timing() {
        let ranges = [
            MeshRange {
                index_count: 0,
                curve_index_count: 300,
                method: FillMethod::Stencil,
                bounds: [-0.45, -0.45, 0.45, 0.45],
                ..PLAIN
            },
            MeshRange {
                index_count: 30,
                bounds: [-0.3, -0.3, 0.3, 0.3],
                ..PLAIN
            },
        ];
        for shape in [0u32, 1] {
            let items: Vec<_> = (0..10_000)
                .map(|i| {
                    let (c, r) = ((i % 125) as f64, (i / 125) as f64);
                    let t = Affine::translate((
                        -1.7 + c * 0.0273 + 0.006 * (i as f64).sin(),
                        -0.95 + r * 0.024,
                    )) * Affine::scale(0.0243);
                    SceneItem::from(Instance::new(ShapeId(shape), t))
                })
                .collect();
            let mut run = StencilRun::default();
            let started = std::time::Instant::now();
            let mut ops = 0;
            for _ in 0..100 {
                ops = pack_frame(
                    &ranges,
                    &[],
                    &items,
                    FillMode::PerShape,
                    Affine::IDENTITY,
                    visible_area(1.9),
                    &mut run,
                )
                .targets[0]
                    .len();
            }
            eprintln!(
                "shape {shape}: {:.3} ms per frame, {ops} ops",
                started.elapsed().as_secs_f64() * 10.0
            );
        }
    }

    #[test]
    fn shapes_off_screen_are_culled() {
        let ranges = [MeshRange {
            index_count: 3,
            bounds: [0.0, 0.0, 0.1, 0.1],
            ..PLAIN
        }];
        let at = |x: f64| SceneItem::from(Instance::new(ShapeId(0), Affine::translate((x, 0.0))));
        // Экран 16:9: x от −1.78 до 1.78. Вторая фигура далеко справа, третья только
        // краем заходит в кадр.
        let items = [at(0.0), at(5.0), at(1.75), at(0.2)];
        let visible = visible_area(16.0 / 9.0);
        let packed = pack_frame(
            &ranges,
            &[],
            &items,
            FillMode::PerShape,
            Affine::IDENTITY,
            visible,
            &mut StencilRun::default(),
        );
        assert_eq!(packed.drawn, 3);
        assert_eq!(packed.instances.len(), 3 * INSTANCE_SIZE);
        // Оставшиеся три подряд — одна команда на три экземпляра.
        assert_eq!(words(&packed.commands), [3, 3, 0, 0]);
        // С камерой, приближенной к дальней фигуре, видна только она.
        let view = reyvec_vector::Camera2D::new((5.05, 0.05), 8.0).view();
        let packed = pack_frame(
            &ranges,
            &[],
            &items,
            FillMode::PerShape,
            view,
            visible,
            &mut StencilRun::default(),
        );
        assert_eq!(packed.drawn, 1);
        assert_eq!(
            f32::from_ne_bytes(packed.instances[16..20].try_into().unwrap()),
            5.0
        );
    }

    #[test]
    fn pixel_rect_maps_scene_to_pixels_with_padding() {
        let extent = vk::Extent2D {
            width: 200,
            height: 100,
        };
        // aspect 2: сцена x −2..2, y −1..1. Квадрат 0..1 × 0..0.5 → пиксели x 100..150,
        // y 25..50, плюс 2 пикселя запаса.
        let r = pixel_rect([0.0, 0.0, 1.0, 0.5], 2.0, extent);
        assert_eq!((r.offset.x, r.offset.y), (98, 23));
        assert_eq!((r.extent.width, r.extent.height), (54, 29));
        // Вылезает за кадр: обрезается по краю.
        let r = pixel_rect([-5.0, -5.0, 5.0, 5.0], 2.0, extent);
        assert_eq!(
            (r.offset.x, r.offset.y, r.extent.width, r.extent.height),
            (0, 0, 200, 100)
        );
        // Пустой и за кадром: 1×1 внутри кадра.
        for b in [EMPTY_BOUNDS, [10.0, 10.0, 11.0, 11.0]] {
            let r = pixel_rect(b, 2.0, extent);
            assert_eq!((r.extent.width, r.extent.height), (1, 1));
            assert!(r.offset.x < 200 && r.offset.y < 100);
        }
        let full = vk::Rect2D::default().extent(extent);
        assert!(intersect(r, full).is_some());
        let a = pixel_rect([0.0, 0.0, 1.0, 0.5], 2.0, extent);
        assert_eq!(intersect(a, full).map(|r| r.extent.width), Some(54));
    }

    #[test]
    fn masks_belong_to_their_layer_and_clip_its_bounds() {
        let ranges = [MeshRange {
            index_count: 3,
            bounds: [0.0, 0.0, 1.0, 1.0],
            ..PLAIN
        }];
        let at = |x: f64| SceneItem::from(Instance::new(ShapeId(0), Affine::translate((x, 0.0))));
        let comp = |layer, mask| Op::Composite {
            layer,
            opacity: 1.0,
            blend: BlendMode::Normal,
            mask,
        };
        let items = [
            // 1: слой с маской 2 по яркости; маска сдвинута, слой виден в пересечении.
            SceneItem::layer(1.0),
            at(0.0),
            SceneItem::mask(MaskMode::Luminance, false),
            at(0.5),
            SceneItem::EndMask,
            SceneItem::EndLayer,
            // 3: обратная маска 4 не обрезает прямоугольник слоя.
            SceneItem::layer(1.0),
            SceneItem::mask(MaskMode::Alpha, true),
            at(3.5),
            SceneItem::EndMask,
            at(3.0),
            SceneItem::EndLayer,
            // 5: пустая обычная маска 6 прячет слой.
            SceneItem::layer(1.0),
            SceneItem::clip(),
            SceneItem::EndMask,
            at(5.0),
            SceneItem::EndLayer,
            // Маска вне слоя пропускается вместе с фигурами и вложенными слоями.
            SceneItem::clip(),
            at(9.0),
            SceneItem::layer(1.0),
            at(9.0),
            SceneItem::EndLayer,
            SceneItem::EndMask,
            SceneItem::EndMask, // лишний конец
            // 7: пустая обратная маска 8 ничего не прячет.
            SceneItem::layer(1.0),
            SceneItem::mask(MaskMode::Alpha, true),
            SceneItem::EndMask,
            at(7.0),
            SceneItem::EndLayer,
        ];
        let packed = pack(&ranges, &items, FillMode::PerShape);
        let mask = |target, mode, inverted| {
            Some(LayerMask {
                target,
                mode,
                inverted,
            })
        };
        let ops: Vec<_> = packed.targets[0]
            .iter()
            .filter(|op| matches!(op, Op::Composite { .. }))
            .copied()
            .collect();
        assert_eq!(
            ops,
            [
                comp(1, mask(2, MaskMode::Luminance, false)),
                comp(3, mask(4, MaskMode::Alpha, true)),
                comp(7, None),
            ]
        );
        assert_eq!(
            packed.targets.len(),
            9,
            "пропущенная маска не заводит целей"
        );
        assert_eq!(
            packed.layer_order,
            [2, 1, 4, 3, 7],
            "маска раньше своего слоя"
        );
        assert_eq!(
            packed.bounds[1],
            [0.5, 0.0, 1.0, 1.0],
            "пересечение со маской"
        );
        assert_eq!(
            packed.bounds[2], packed.bounds[1],
            "маска рисуется там же, где слой"
        );
        assert_eq!(packed.bounds[3], [3.0, 0.0, 4.0, 1.0]);
        assert_eq!(packed.bounds[4], packed.bounds[3]);
        assert_eq!(packed.instances.len(), 6 * INSTANCE_SIZE);
        assert_eq!(
            LayerMask {
                target: 0,
                mode: MaskMode::Luminance,
                inverted: true
            }
            .shader_mode(),
            6
        );
    }
}
