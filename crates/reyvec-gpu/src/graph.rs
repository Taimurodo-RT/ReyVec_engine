//! Граф кадра (render graph): кадр собирается из проходов, а барьеры между ними
//! движок расставляет сам.
//!
//! Каждый проход объявляет, какие картинки он читает и в какие пишет. По этим
//! объявлениям граф:
//! - ставит барьеры синхронизации и смены раскладки между проходами (руками их писать
//!   не нужно, и ошибиться в них нельзя);
//! - выдаёт временные картинки (например, промежуточный кадр сцены) из пула: они
//!   создаются при первой надобности, переиспользуются следующими кадрами и удаляются,
//!   когда кадр перестал их просить (например, после изменения размера окна).
//!
//! Проходы выполняются в порядке добавления. Сейчас всё идёт в одну очередь видеокарты.

use ash::vk;

use crate::GpuError;
use crate::bindless::{Bindless, MAX_TEXTURES};
use crate::context::GpuContext;
use crate::pipeline::image_barrier;
use crate::resources::{Texture, create_texture, destroy_texture};
use crate::timing::GpuTimer;

/// Сколько ячеек общего списка текстур отдано временным картинкам графа (в конце списка).
pub(crate) const TRANSIENT_TEXTURES: u32 = 256;
/// Первая ячейка общего списка, отданная временным картинкам.
pub(crate) const FIRST_TRANSIENT_SLOT: u32 = MAX_TEXTURES - TRANSIENT_TEXTURES;

/// Картинка внутри графа одного кадра.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ImageHandle(usize);

/// Как проход использует картинку.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    /// Рисует в неё (цель рисования).
    ColorAttachment,
    /// Читает её в пиксельном шейдере (через общий список текстур).
    Sampled,
    /// Копирует из неё.
    TransferSrc,
    /// Копирует в неё (понадобится загрузке данных через граф).
    #[allow(dead_code)]
    TransferDst,
    /// Отдаёт на экран (только как конечное состояние картинки окна).
    Present,
}

/// Раскладка картинки и последнее, что с ней делала видеокарта.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImageState {
    pub layout: vk::ImageLayout,
    pub stage: vk::PipelineStageFlags2,
    pub access: vk::AccessFlags2,
}

impl ImageState {
    /// Картинка без содержимого, с которой ещё ничего не делали.
    pub const UNDEFINED: Self = Self {
        layout: vk::ImageLayout::UNDEFINED,
        stage: vk::PipelineStageFlags2::NONE,
        access: vk::AccessFlags2::NONE,
    };
}

const WRITE_ACCESS: vk::AccessFlags2 = vk::AccessFlags2::from_raw(
    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE.as_raw()
        | vk::AccessFlags2::TRANSFER_WRITE.as_raw()
        | vk::AccessFlags2::SHADER_STORAGE_WRITE.as_raw()
        | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE.as_raw(),
);

impl Access {
    pub fn state(self) -> ImageState {
        let (layout, stage, access) = match self {
            Access::ColorAttachment => (
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE | vk::AccessFlags2::COLOR_ATTACHMENT_READ,
            ),
            Access::Sampled => (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::AccessFlags2::SHADER_SAMPLED_READ,
            ),
            Access::TransferSrc => (
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::PipelineStageFlags2::COPY,
                vk::AccessFlags2::TRANSFER_READ,
            ),
            Access::TransferDst => (
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::PipelineStageFlags2::COPY,
                vk::AccessFlags2::TRANSFER_WRITE,
            ),
            Access::Present => (
                vk::ImageLayout::PRESENT_SRC_KHR,
                vk::PipelineStageFlags2::NONE,
                vk::AccessFlags2::NONE,
            ),
        };
        ImageState {
            layout,
            stage,
            access,
        }
    }

    fn usage(self) -> vk::ImageUsageFlags {
        match self {
            Access::ColorAttachment => vk::ImageUsageFlags::COLOR_ATTACHMENT,
            Access::Sampled => vk::ImageUsageFlags::SAMPLED,
            Access::TransferSrc => vk::ImageUsageFlags::TRANSFER_SRC,
            Access::TransferDst => vk::ImageUsageFlags::TRANSFER_DST,
            Access::Present => vk::ImageUsageFlags::empty(),
        }
    }
}

/// Барьер, который граф поставит перед проходом.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlannedBarrier {
    pub image: ImageHandle,
    pub old: ImageState,
    pub new: ImageState,
}

/// Переход картинки из состояния `cur` к использованию `next`. Возвращает барьер, если он
/// нужен, и обновляет `cur`. Барьер не нужен, только если оба раза картинку читают в одной
/// раскладке: тогда следующая запись должна дождаться обоих чтений, и они копятся в `cur`.
fn transition(
    image: ImageHandle,
    cur: &mut ImageState,
    next: ImageState,
) -> Option<PlannedBarrier> {
    let cur_writes = cur.access.intersects(WRITE_ACCESS);
    let next_writes = next.access.intersects(WRITE_ACCESS);
    if cur.layout == next.layout
        && !cur_writes
        && !next_writes
        && cur.layout != vk::ImageLayout::UNDEFINED
    {
        cur.stage |= next.stage;
        cur.access |= next.access;
        return None;
    }
    let old = *cur;
    *cur = next;
    Some(PlannedBarrier {
        image,
        // Делать видимыми нужно только записи; для чтений хватает ожидания по стадии.
        old: ImageState {
            access: old.access & WRITE_ACCESS,
            ..old
        },
        new: next,
    })
}

/// План барьеров: `initial` — состояния картинок до кадра, `passes` — что читает и пишет
/// каждый проход, `finals` — в каком виде картинка нужна после кадра (если нужна).
/// Возвращает барьеры перед каждым проходом, барьеры в конце и состояния после кадра.
pub(crate) fn plan(
    initial: &[ImageState],
    passes: &[Vec<(ImageHandle, Access)>],
    finals: &[Option<Access>],
) -> (
    Vec<Vec<PlannedBarrier>>,
    Vec<PlannedBarrier>,
    Vec<ImageState>,
) {
    let mut state = initial.to_vec();
    let mut before = Vec::with_capacity(passes.len());
    for uses in passes {
        let mut barriers = Vec::new();
        for &(h, access) in uses {
            if let Some(b) = transition(h, &mut state[h.0], access.state()) {
                barriers.push(b);
            }
        }
        before.push(barriers);
    }
    let mut after = Vec::new();
    for (i, f) in finals.iter().enumerate() {
        if let Some(access) = f
            && let Some(b) = transition(ImageHandle(i), &mut state[i], access.state())
        {
            after.push(b);
        }
    }
    (before, after, state)
}

enum Source {
    Imported {
        image: vk::Image,
        view: vk::ImageView,
    },
    Transient,
}

struct GraphImage {
    name: &'static str,
    source: Source,
    extent: vk::Extent2D,
    format: vk::Format,
    initial: ImageState,
    final_access: Option<Access>,
}

type Record<'a> = Box<dyn FnOnce(&PassContext) + 'a>;

struct Pass<'a> {
    name: &'static str,
    uses: Vec<(ImageHandle, Access)>,
    record: Record<'a>,
}

/// Картинка, готовая к использованию в проходе.
#[derive(Clone, Copy)]
struct Resolved {
    image: vk::Image,
    view: vk::ImageView,
    extent: vk::Extent2D,
    /// Ячейка в общем списке текстур, если картинку можно читать в шейдере.
    slot: Option<u32>,
}

/// То, что получает проход при записи команд.
pub(crate) struct PassContext<'r> {
    pub device: &'r ash::Device,
    pub cmd: vk::CommandBuffer,
    images: &'r [Resolved],
}

impl PassContext<'_> {
    pub fn view(&self, h: ImageHandle) -> vk::ImageView {
        self.images[h.0].view
    }

    pub fn image(&self, h: ImageHandle) -> vk::Image {
        self.images[h.0].image
    }

    pub fn extent(&self, h: ImageHandle) -> vk::Extent2D {
        self.images[h.0].extent
    }

    /// Номер картинки в общем списке текстур: по нему шейдер её читает.
    pub fn texture(&self, h: ImageHandle) -> u32 {
        self.images[h.0]
            .slot
            .expect("картинку читают в шейдере, но она не объявлена как Access::Sampled")
    }
}

/// Граф одного кадра. Собирается заново каждый кадр: это дёшево, тяжёлые объекты живут в пуле.
pub(crate) struct RenderGraph<'a> {
    images: Vec<GraphImage>,
    passes: Vec<Pass<'a>>,
}

/// Что получилось в кадре: для лога и счётчиков.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphReport {
    /// Проходы в порядке выполнения.
    pub passes: Vec<&'static str>,
    /// Сколько барьеров поставил граф.
    pub barriers: usize,
    /// Сколько временных картинок взято из пула.
    pub transient_images: usize,
}

impl GraphReport {
    /// Строка вида «проходы: сцена → вывод; временных картинок: 1, барьеров: 4».
    pub fn describe(&self) -> String {
        format!(
            "проходы: {}; временных картинок: {}, барьеров: {}",
            self.passes.join(" → "),
            self.transient_images,
            self.barriers
        )
    }
}

impl<'a> RenderGraph<'a> {
    pub fn new() -> Self {
        Self {
            images: Vec::new(),
            passes: Vec::new(),
        }
    }

    /// Подключает картинку, созданную вне графа (например, изображение окна).
    /// `initial`: её состояние до кадра, `final_access`: в каком виде она нужна после.
    #[allow(clippy::too_many_arguments)]
    pub fn import(
        &mut self,
        name: &'static str,
        image: vk::Image,
        view: vk::ImageView,
        extent: vk::Extent2D,
        format: vk::Format,
        initial: ImageState,
        final_access: Option<Access>,
    ) -> ImageHandle {
        self.images.push(GraphImage {
            name,
            source: Source::Imported { image, view },
            extent,
            format,
            initial,
            final_access,
        });
        ImageHandle(self.images.len() - 1)
    }

    /// Временная картинка: живёт только внутри кадра, содержимое между кадрами не хранится.
    pub fn create(
        &mut self,
        name: &'static str,
        extent: vk::Extent2D,
        format: vk::Format,
    ) -> ImageHandle {
        self.images.push(GraphImage {
            name,
            source: Source::Transient,
            extent,
            format,
            initial: ImageState::UNDEFINED,
            final_access: None,
        });
        ImageHandle(self.images.len() - 1)
    }

    /// Добавляет проход. `uses`: какие картинки он трогает и как.
    pub fn add_pass(
        &mut self,
        name: &'static str,
        uses: &[(ImageHandle, Access)],
        record: impl FnOnce(&PassContext) + 'a,
    ) {
        for (i, (h, a)) in uses.iter().enumerate() {
            debug_assert!(
                !uses[..i].iter().any(|(h2, _)| h2 == h),
                "проход «{name}» дважды объявил картинку «{}» ({a:?})",
                self.images[h.0].name
            );
        }
        self.passes.push(Pass {
            name,
            uses: uses.to_vec(),
            record: Box::new(record),
        });
    }

    /// Записывает все проходы с барьерами в `cmd`. `frame`: сквозной номер кадра.
    ///
    /// # Safety
    /// `cmd` в состоянии записи; импортированные картинки живы до конца выполнения кадра
    /// и действительно находятся в объявленном начальном состоянии.
    pub unsafe fn execute(
        self,
        ctx: &GpuContext,
        cmd: vk::CommandBuffer,
        pool: &mut TransientPool,
        bindless: &Bindless,
        frame: u64,
        mut timer: Option<&mut GpuTimer>,
    ) -> Result<GraphReport, GpuError> {
        // Как каждую временную картинку будут использовать: от этого зависит, как её создать.
        let mut usage = vec![vk::ImageUsageFlags::empty(); self.images.len()];
        for pass in &self.passes {
            for &(h, a) in &pass.uses {
                usage[h.0] |= a.usage();
            }
        }

        let mut resolved = Vec::with_capacity(self.images.len());
        let mut initial = Vec::with_capacity(self.images.len());
        let mut pooled = Vec::new();
        for (i, img) in self.images.iter().enumerate() {
            match img.source {
                Source::Imported { image, view } => {
                    resolved.push(Resolved {
                        image,
                        view,
                        extent: img.extent,
                        slot: None,
                    });
                    initial.push(img.initial);
                }
                Source::Transient => {
                    let key = TransientKey {
                        extent: img.extent,
                        format: img.format,
                        usage: usage[i],
                    };
                    let entry = pool.acquire(ctx, bindless, key, frame, img.name)?;
                    let e = &pool.entries[entry];
                    resolved.push(Resolved {
                        image: e.texture.image,
                        view: e.texture.view,
                        extent: img.extent,
                        slot: e.slot,
                    });
                    // Содержимое прошлого кадра не нужно (UNDEFINED), но дождаться
                    // прошлого использования этой картинки видеокартой нужно.
                    initial.push(ImageState {
                        layout: vk::ImageLayout::UNDEFINED,
                        ..e.state
                    });
                    pooled.push((i, entry));
                }
            }
        }

        let uses: Vec<_> = self.passes.iter().map(|p| p.uses.clone()).collect();
        let finals: Vec<_> = self.images.iter().map(|i| i.final_access).collect();
        let (before, after, end_states) = plan(&initial, &uses, &finals);

        let d = &ctx.device;
        let report = GraphReport {
            passes: self.passes.iter().map(|p| p.name).collect(),
            barriers: before.iter().map(Vec::len).sum::<usize>() + after.len(),
            transient_images: pooled.len(),
        };
        let emit = |barriers: &[PlannedBarrier]| {
            if barriers.is_empty() {
                return;
            }
            let raw: Vec<_> = barriers
                .iter()
                .map(|b| {
                    image_barrier(
                        resolved[b.image.0].image,
                        b.old.layout,
                        b.new.layout,
                        b.old.stage,
                        b.old.access,
                        b.new.stage,
                        b.new.access,
                    )
                })
                .collect();
            // SAFETY: cmd в состоянии записи (условие функции), картинки живы.
            unsafe {
                d.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().image_memory_barriers(&raw),
                );
            }
        };
        let pass_ctx = PassContext {
            device: d,
            cmd,
            images: &resolved,
        };
        if let Some(t) = timer.as_deref_mut() {
            // SAFETY: cmd в состоянии записи (условие функции).
            unsafe { t.record_start(d, cmd) };
        }
        for (pass, barriers) in self.passes.into_iter().zip(&before) {
            let name = pass.name;
            if let Some(t) = timer.as_deref_mut() {
                t.pass_begin(name);
            }
            emit(barriers);
            (pass.record)(&pass_ctx);
            if let Some(t) = timer.as_deref_mut() {
                // SAFETY: cmd в состоянии записи, метка начала кадра поставлена выше.
                unsafe { t.pass_end(d, cmd, name) };
            }
        }
        emit(&after);
        if let Some(t) = timer {
            t.record_end();
        }

        for (i, entry) in pooled {
            pool.entries[entry].state = end_states[i];
        }
        Ok(report)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TransientKey {
    extent: vk::Extent2D,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
}

struct PoolEntry {
    key: TransientKey,
    texture: Texture,
    slot: Option<u32>,
    state: ImageState,
    last_used: u64,
}

/// Пул временных картинок графа. Живёт всё время работы рендерера.
pub(crate) struct TransientPool {
    entries: Vec<PoolEntry>,
    free_slots: Vec<u32>,
}

impl TransientPool {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            // В обратном порядке, чтобы pop() отдавал ячейки по возрастанию.
            free_slots: (FIRST_TRANSIENT_SLOT..MAX_TEXTURES).rev().collect(),
        }
    }

    /// Начало кадра `frame`. Удаляет картинки, которые не понадобились прошлому кадру и
    /// которые видеокарта уже не трогает (их последний кадр не позже `completed`).
    pub fn begin_frame(&mut self, ctx: &GpuContext, frame: u64, completed: Option<u64>) {
        let Some(done) = completed else { return };
        let mut i = 0;
        while i < self.entries.len() {
            let e = &self.entries[i];
            if e.last_used <= done && e.last_used + 1 < frame {
                let e = self.entries.swap_remove(i);
                tracing::debug!(
                    "граф: удалена временная картинка {}×{}, в пуле осталось {}",
                    e.key.extent.width,
                    e.key.extent.height,
                    self.entries.len()
                );
                if let Some(slot) = e.slot {
                    self.free_slots.push(slot);
                }
                destroy_texture(ctx, e.texture);
            } else {
                i += 1;
            }
        }
    }

    /// Сколько картинок сейчас в пуле.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn acquire(
        &mut self,
        ctx: &GpuContext,
        bindless: &Bindless,
        key: TransientKey,
        frame: u64,
        name: &str,
    ) -> Result<usize, GpuError> {
        // Свободная картинка того же вида: не отданная уже в этом кадре.
        if let Some(i) = self
            .entries
            .iter()
            .position(|e| e.key == key && e.last_used != frame)
        {
            self.entries[i].last_used = frame;
            return Ok(i);
        }
        let texture = create_texture(ctx, key.extent, key.format, key.usage, name)?;
        let slot = if key.usage.contains(vk::ImageUsageFlags::SAMPLED) {
            match self.free_slots.pop() {
                Some(s) => {
                    bindless.write_texture(ctx, s, texture.view);
                    Some(s)
                }
                None => {
                    destroy_texture(ctx, texture);
                    return Err(GpuError::Memory(format!(
                        "временных картинок больше {TRANSIENT_TEXTURES}"
                    )));
                }
            }
        } else {
            None
        };
        tracing::debug!(
            "граф: новая временная картинка «{name}» {}×{} {:?}",
            key.extent.width,
            key.extent.height,
            key.format
        );
        self.entries.push(PoolEntry {
            key,
            texture,
            slot,
            state: ImageState::UNDEFINED,
            last_used: frame,
        });
        Ok(self.entries.len() - 1)
    }

    /// Уничтожает всё. Вызывать, когда видеокарта простаивает.
    pub fn destroy(&mut self, ctx: &GpuContext) {
        for e in self.entries.drain(..) {
            destroy_texture(ctx, e.texture);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: ImageHandle = ImageHandle(0);
    const B: ImageHandle = ImageHandle(1);

    /// Сцена рисуется во временную картинку A, затем копируется на экран B.
    #[test]
    fn scene_then_present() {
        let initial = [ImageState::UNDEFINED, ImageState::UNDEFINED];
        let passes = vec![
            vec![(A, Access::ColorAttachment)],
            vec![(A, Access::Sampled), (B, Access::ColorAttachment)],
        ];
        let (before, after, end) = plan(&initial, &passes, &[None, Some(Access::Present)]);

        assert_eq!(before[0].len(), 1);
        assert_eq!(
            before[0][0].new.layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );

        // Перед выводом: A из цели рисования в чтение, B в цель рисования.
        assert_eq!(before[1].len(), 2);
        let a = before[1][0];
        assert_eq!(a.image, A);
        assert_eq!(a.old.layout, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        assert_eq!(a.old.access, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
        assert_eq!(a.new.layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        assert_eq!(a.new.stage, vk::PipelineStageFlags2::FRAGMENT_SHADER);

        assert_eq!(after.len(), 1);
        assert_eq!(after[0].image, B);
        assert_eq!(after[0].new.layout, vk::ImageLayout::PRESENT_SRC_KHR);
        assert_eq!(end[0].layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
    }

    /// Два чтения подряд без барьера, а запись после них ждёт оба.
    #[test]
    fn reads_share_one_barrier() {
        let initial = [ImageState::UNDEFINED];
        let passes = vec![
            vec![(A, Access::ColorAttachment)],
            vec![(A, Access::Sampled)],
            vec![(A, Access::Sampled)],
            vec![(A, Access::TransferDst)],
        ];
        let (before, _, _) = plan(&initial, &passes, &[None]);
        assert_eq!(
            before.iter().map(Vec::len).collect::<Vec<_>>(),
            [1, 1, 0, 1]
        );
        let w = before[3][0];
        assert_eq!(w.old.stage, vk::PipelineStageFlags2::FRAGMENT_SHADER);
        // Чтения не надо «делать видимыми»: в барьере только ожидание по стадии.
        assert_eq!(w.old.access, vk::AccessFlags2::NONE);
    }

    /// Две записи подряд в одну раскладку: барьер всё равно нужен (запись после записи).
    #[test]
    fn write_after_write_has_barrier() {
        let initial = [ImageState::UNDEFINED];
        let passes = vec![
            vec![(A, Access::ColorAttachment)],
            vec![(A, Access::ColorAttachment)],
        ];
        let (before, _, _) = plan(&initial, &passes, &[None]);
        assert_eq!(before[1].len(), 1);
        assert_eq!(
            before[1][0].old.layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );
    }

    /// Временная картинка из прошлого кадра: содержимое не нужно, но ожидание прошлого
    /// чтения остаётся, иначе новый кадр затрёт картинку, пока старый её читает.
    #[test]
    fn reused_transient_waits_for_previous_frame() {
        let initial = [ImageState {
            layout: vk::ImageLayout::UNDEFINED,
            ..Access::Sampled.state()
        }];
        let (before, _, _) = plan(&initial, &[vec![(A, Access::ColorAttachment)]], &[None]);
        let b = before[0][0];
        assert_eq!(b.old.layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(b.old.stage, vk::PipelineStageFlags2::FRAGMENT_SHADER);
    }

    #[test]
    fn report_reads_well() {
        let r = GraphReport {
            passes: vec!["сцена", "вывод"],
            barriers: 3,
            transient_images: 1,
        };
        assert_eq!(
            r.describe(),
            "проходы: сцена → вывод; временных картинок: 1, барьеров: 3"
        );
    }
}
