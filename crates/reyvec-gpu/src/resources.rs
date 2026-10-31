//! Буферы и текстуры в памяти видеокарты, загрузка данных и отложенное удаление.
//!
//! Видеокарта отстаёт от процессора на 1–2 кадра. Поэтому ресурс, который больше
//! не нужен, нельзя уничтожить сразу: прошлый кадр ещё может его читать. Такой ресурс
//! кладётся в очередь удаления и уничтожается, когда все кадры, где он мог быть
//! использован, закончены.

use std::collections::VecDeque;

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};

use crate::bindless::Bindless;
use crate::context::GpuContext;
use crate::pipeline::{color_range, image_barrier};
use crate::{GpuError, VkResultExt};

/// Номер текстуры. Это же номер в общем списке текстур, по которому шейдер её читает.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(pub u32);

/// Номер буфера.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufferId(pub u32);

/// Где живёт буфер.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferMemory {
    /// Только видеокарта: самый быстрый вариант, данные загружаются через [`GpuResources::upload_buffer`].
    GpuOnly,
    /// Процессор пишет напрямую, видеокарта читает. Для данных, которые меняются каждый кадр.
    CpuToGpu,
    /// Видеокарта пишет, процессор читает (снимки кадра, результаты замеров).
    GpuToCpu,
}

pub(crate) struct Buffer {
    pub raw: vk::Buffer,
    pub allocation: Option<Allocation>,
    pub size: u64,
    pub address: vk::DeviceAddress,
}

pub(crate) struct Texture {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub allocation: Option<Allocation>,
}

enum Garbage {
    Buffer(Buffer, u32),
    Texture(Texture, u32),
}

/// Очередь отложенного удаления: элемент, положенный в кадре `n`, отдаётся,
/// когда закончен кадр `n` (видеокарта уже не может его читать).
pub(crate) struct DeletionQueue<T> {
    items: VecDeque<(u64, T)>,
}

impl<T> DeletionQueue<T> {
    pub fn new() -> Self {
        Self {
            items: VecDeque::new(),
        }
    }

    pub fn push(&mut self, frame: u64, item: T) {
        self.items.push_back((frame, item));
    }

    /// Забирает всё, что положено в кадрах не позже `completed`.
    pub fn drain_completed(&mut self, completed: u64) -> Vec<T> {
        let mut out = Vec::new();
        while self.items.front().is_some_and(|(f, _)| *f <= completed) {
            out.push(self.items.pop_front().expect("проверено выше").1);
        }
        out
    }

    pub fn drain_all(&mut self) -> Vec<T> {
        self.items.drain(..).map(|(_, t)| t).collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// Таблица с переиспользованием номеров. Номер возвращается в оборот только после
/// настоящего удаления ресурса, поэтому старый номер не укажет на новый ресурс раньше времени.
struct Slots<T> {
    items: Vec<Option<T>>,
    free: Vec<u32>,
}

impl<T> Slots<T> {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            free: Vec::new(),
        }
    }

    fn insert(&mut self, value: T) -> u32 {
        match self.free.pop() {
            Some(i) => {
                self.items[i as usize] = Some(value);
                i
            }
            None => {
                self.items.push(Some(value));
                (self.items.len() - 1) as u32
            }
        }
    }

    fn get(&self, i: u32) -> Option<&T> {
        self.items.get(i as usize).and_then(Option::as_ref)
    }

    fn take(&mut self, i: u32) -> Option<T> {
        self.items.get_mut(i as usize).and_then(Option::take)
    }

    fn release(&mut self, i: u32) {
        self.free.push(i);
    }

    fn drain(&mut self) -> Vec<T> {
        self.free.clear();
        self.items.drain(..).flatten().collect()
    }
}

/// Все буферы и текстуры движка плюс общий список текстур для шейдеров (bindless).
pub(crate) struct GpuResources {
    pub bindless: Bindless,
    buffers: Slots<Buffer>,
    textures: Slots<Texture>,
    deletion: DeletionQueue<Garbage>,
    /// Номер текущего кадра: всё, что удалено сейчас, ждёт окончания этого кадра.
    frame: u64,
    upload_pool: vk::CommandPool,
    upload_fence: vk::Fence,
}

impl GpuResources {
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuError> {
        let bindless = Bindless::new(ctx)?;
        let d = &ctx.device;
        // SAFETY: простые объекты устройства; уничтожаются в destroy().
        let (upload_pool, upload_fence) = unsafe {
            let pool = d
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .flags(vk::CommandPoolCreateFlags::TRANSIENT)
                        .queue_family_index(ctx.queue_family),
                    None,
                )
                .ctx("пул команд загрузки");
            let pool = match pool {
                Ok(p) => p,
                Err(e) => {
                    bindless.destroy(ctx);
                    return Err(e);
                }
            };
            match d.create_fence(&vk::FenceCreateInfo::default(), None) {
                Ok(f) => (pool, f),
                Err(e) => {
                    d.destroy_command_pool(pool, None);
                    bindless.destroy(ctx);
                    return Err(GpuError::Vulkan {
                        what: "забор загрузки",
                        result: e,
                    });
                }
            }
        };
        Ok(Self {
            bindless,
            buffers: Slots::new(),
            textures: Slots::new(),
            deletion: DeletionQueue::new(),
            frame: 0,
            upload_pool,
            upload_fence,
        })
    }

    /// Начало кадра `frame`. `completed`: последний кадр, который видеокарта точно закончила.
    pub fn begin_frame(&mut self, ctx: &GpuContext, frame: u64, completed: Option<u64>) {
        self.frame = frame;
        if let Some(done) = completed {
            for g in self.deletion.drain_completed(done) {
                self.free_garbage(ctx, g);
            }
        }
    }

    /// Сколько ресурсов ждёт удаления (для счётчиков и тестов).
    #[allow(dead_code)]
    pub fn pending_deletions(&self) -> usize {
        self.deletion.len()
    }

    pub fn create_buffer(
        &mut self,
        ctx: &GpuContext,
        size: u64,
        usage: vk::BufferUsageFlags,
        memory: BufferMemory,
        name: &str,
    ) -> Result<BufferId, GpuError> {
        let buffer = create_buffer(ctx, size, usage, memory, name)?;
        Ok(BufferId(self.buffers.insert(buffer)))
    }

    /// Адрес буфера в памяти видеокарты: шейдер читает буфер по этому адресу.
    pub fn buffer_address(&self, id: BufferId) -> vk::DeviceAddress {
        self.buffers.get(id.0).map_or(0, |b| b.address)
    }

    /// Пишет байты в буфер [`BufferMemory::CpuToGpu`] со смещением `offset`.
    pub fn write_buffer(&mut self, id: BufferId, offset: usize, bytes: &[u8]) {
        let Some(buffer) = self
            .buffers
            .items
            .get_mut(id.0 as usize)
            .and_then(Option::as_mut)
        else {
            return;
        };
        let Some(slice) = buffer
            .allocation
            .as_mut()
            .and_then(Allocation::mapped_slice_mut)
        else {
            tracing::error!("буфер {id:?} не доступен процессору для записи");
            return;
        };
        let end = (offset + bytes.len()).min(slice.len());
        if end > offset {
            slice[offset..end].copy_from_slice(&bytes[..end - offset]);
        }
    }

    /// Читает первые `len` байт буфера, доступного процессору ([`BufferMemory::GpuToCpu`]).
    /// Видеокарта должна закончить запись в него (кадр с записью закончен).
    pub fn read_buffer(&self, id: BufferId, len: usize) -> Option<Vec<u8>> {
        let buffer = self.buffers.get(id.0)?;
        // gpu-allocator всегда даёт такой памяти HOST_COHERENT: записи видеокарты видны
        // процессору после ожидания забора, без vkInvalidateMappedMemoryRanges.
        let slice = buffer.allocation.as_ref()?.mapped_slice()?;
        let bytes = &slice[..len.min(slice.len())];
        Some(bytes.to_vec())
    }

    /// Загружает данные в буфер [`BufferMemory::GpuOnly`] через временный буфер.
    #[allow(dead_code)]
    pub fn upload_buffer(
        &mut self,
        ctx: &GpuContext,
        id: BufferId,
        bytes: &[u8],
    ) -> Result<(), GpuError> {
        let Some((dst, size)) = self.buffers.get(id.0).map(|b| (b.raw, b.size)) else {
            return Ok(());
        };
        if bytes.len() as u64 > size {
            return Err(GpuError::Memory(format!(
                "в буфер на {size} байт не помещается {} байт",
                bytes.len()
            )));
        }
        let staging = self.staging(ctx, bytes)?;
        let size = bytes.len() as u64;
        let result = self.immediate(ctx, |d, cmd| {
            let region = [vk::BufferCopy::default().size(size)];
            // SAFETY: оба буфера живы до конца немедленной отправки.
            unsafe { d.cmd_copy_buffer(cmd, staging.raw, dst, &region) };
        });
        destroy_buffer(ctx, staging);
        result
    }

    /// Создаёт текстуру RGBA8 и загружает в неё пиксели. `srgb`: цвета в пространстве
    /// экрана (обычные картинки), иначе линейные данные (маски, нормали).
    /// Ждёт окончания загрузки, поэтому подходит для загрузки при старте, а не каждый кадр.
    pub fn create_texture_rgba8(
        &mut self,
        ctx: &GpuContext,
        width: u32,
        height: u32,
        pixels: &[u8],
        srgb: bool,
        name: &str,
    ) -> Result<TextureId, GpuError> {
        assert_eq!(
            pixels.len(),
            (width * height * 4) as usize,
            "размер пикселей"
        );
        let id = self.create_texture_empty(ctx, width, height, srgb, name)?;
        let image = self.texture_image(id).expect("текстура только что создана");
        let extent = vk::Extent2D { width, height };
        let staging = match self.staging(ctx, pixels) {
            Ok(s) => s,
            Err(e) => {
                self.destroy_texture(id);
                return Err(e);
            }
        };
        let result = self.immediate(ctx, |d, cmd| {
            // SAFETY: картинка и временный буфер живы до конца немедленной отправки.
            unsafe { record_texture_upload(d, cmd, image, staging.raw, [0, 0], extent, true) };
        });
        destroy_buffer(ctx, staging);
        if let Err(e) = result {
            self.destroy_texture(id);
            return Err(e);
        }
        Ok(id)
    }

    /// Создаёт текстуру RGBA8 без содержимого (раскладка UNDEFINED) и сразу кладёт её в
    /// общий список текстур. Содержимое загружается командами кадра
    /// ([`record_texture_upload`]).
    pub fn create_texture_empty(
        &mut self,
        ctx: &GpuContext,
        width: u32,
        height: u32,
        srgb: bool,
        name: &str,
    ) -> Result<TextureId, GpuError> {
        let format = if srgb {
            vk::Format::R8G8B8A8_SRGB
        } else {
            vk::Format::R8G8B8A8_UNORM
        };
        let texture = create_texture(
            ctx,
            vk::Extent2D { width, height },
            format,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
            name,
        )?;
        let view = texture.view;
        let slot = self.textures.insert(texture);
        // Ячейки в конце общего списка отданы временным картинкам графа кадра.
        if slot >= crate::graph::FIRST_TRANSIENT_SLOT {
            let t = self.textures.take(slot).expect("только что вставлена");
            self.textures.release(slot);
            destroy_texture(ctx, t);
            return Err(GpuError::Memory(format!(
                "текстур больше {}",
                crate::graph::FIRST_TRANSIENT_SLOT
            )));
        }
        self.bindless.write_texture(ctx, slot, view);
        Ok(TextureId(slot))
    }

    /// Картинка Vulkan текстуры `id` (для записи команд загрузки).
    pub fn texture_image(&self, id: TextureId) -> Option<vk::Image> {
        self.textures.get(id.0).map(|t| t.image)
    }

    /// Буфер Vulkan с номером `id` (для команд копирования).
    pub fn buffer_raw(&self, id: BufferId) -> Option<vk::Buffer> {
        self.buffers.get(id.0).map(|b| b.raw)
    }

    /// Убирает текстуру. Настоящее удаление случится, когда видеокарта закончит текущий кадр.
    pub fn destroy_texture(&mut self, id: TextureId) {
        if let Some(t) = self.textures.take(id.0) {
            self.deletion.push(self.frame, Garbage::Texture(t, id.0));
        }
    }

    /// Убирает буфер. Настоящее удаление случится, когда видеокарта закончит текущий кадр.
    #[allow(dead_code)]
    pub fn destroy_buffer(&mut self, id: BufferId) {
        if let Some(b) = self.buffers.take(id.0) {
            self.deletion.push(self.frame, Garbage::Buffer(b, id.0));
        }
    }

    fn free_garbage(&mut self, ctx: &GpuContext, g: Garbage) {
        match g {
            Garbage::Buffer(b, slot) => {
                destroy_buffer(ctx, b);
                self.buffers.release(slot);
            }
            Garbage::Texture(t, slot) => {
                destroy_texture(ctx, t);
                self.textures.release(slot);
            }
        }
    }

    fn staging(&self, ctx: &GpuContext, bytes: &[u8]) -> Result<Buffer, GpuError> {
        let mut staging = create_buffer(
            ctx,
            bytes.len() as u64,
            vk::BufferUsageFlags::TRANSFER_SRC,
            BufferMemory::CpuToGpu,
            "временный буфер загрузки",
        )?;
        match staging
            .allocation
            .as_mut()
            .and_then(Allocation::mapped_slice_mut)
        {
            Some(slice) => {
                slice[..bytes.len()].copy_from_slice(bytes);
                Ok(staging)
            }
            None => {
                destroy_buffer(ctx, staging);
                Err(GpuError::Memory(
                    "временный буфер не доступен процессору".into(),
                ))
            }
        }
    }

    /// Записывает команды и сразу ждёт их выполнения. Только для загрузки при старте.
    fn immediate(
        &self,
        ctx: &GpuContext,
        record: impl FnOnce(&ash::Device, vk::CommandBuffer),
    ) -> Result<(), GpuError> {
        let d = &ctx.device;
        // SAFETY: пул и забор принадлежат этому устройству; ждём выполнения до выхода,
        // поэтому командный буфер и всё, что он трогает, свободны после функции.
        unsafe {
            d.reset_command_pool(self.upload_pool, vk::CommandPoolResetFlags::empty())
                .ctx("сброс пула загрузки")?;
            let cmd = d
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.upload_pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .ctx("командный буфер загрузки")?[0];
            d.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .ctx("запись загрузки")?;
            record(d, cmd);
            d.end_command_buffer(cmd).ctx("конец записи загрузки")?;
            let cmds = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
            let submit = vk::SubmitInfo2::default().command_buffer_infos(&cmds);
            d.reset_fences(&[self.upload_fence])
                .ctx("сброс забора загрузки")?;
            d.queue_submit2(ctx.queue, &[submit], self.upload_fence)
                .ctx("отправка загрузки")?;
            d.wait_for_fences(&[self.upload_fence], true, u64::MAX)
                .ctx("ожидание загрузки")?;
            d.free_command_buffers(self.upload_pool, &[cmd]);
        }
        Ok(())
    }

    /// Уничтожает всё сразу. Вызывать, когда видеокарта простаивает.
    pub fn destroy(mut self, ctx: &GpuContext) {
        for g in self.deletion.drain_all() {
            self.free_garbage(ctx, g);
        }
        for b in self.buffers.drain() {
            destroy_buffer(ctx, b);
        }
        for t in self.textures.drain() {
            destroy_texture(ctx, t);
        }
        // SAFETY: видеокарта простаивает (условие функции).
        unsafe {
            ctx.device.destroy_fence(self.upload_fence, None);
            ctx.device.destroy_command_pool(self.upload_pool, None);
        }
        self.bindless.destroy(ctx);
    }
}

fn allocate(
    ctx: &GpuContext,
    name: &str,
    requirements: vk::MemoryRequirements,
    location: MemoryLocation,
    linear: bool,
) -> Result<Allocation, GpuError> {
    ctx.allocator
        .lock()
        .map_err(|_| GpuError::Memory("распределитель памяти сломан другим потоком".into()))?
        .allocate(&AllocationCreateDesc {
            name,
            requirements,
            location,
            linear,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| GpuError::Memory(format!("{name}: {e}")))
}

fn free(ctx: &GpuContext, allocation: Option<Allocation>) {
    if let Some(a) = allocation
        && let Ok(mut alloc) = ctx.allocator.lock()
        && let Err(e) = alloc.free(a)
    {
        tracing::error!("не удалось освободить память: {e}");
    }
}

pub(crate) fn create_buffer(
    ctx: &GpuContext,
    size: u64,
    usage: vk::BufferUsageFlags,
    memory: BufferMemory,
    name: &str,
) -> Result<Buffer, GpuError> {
    let d = &ctx.device;
    let usage = usage | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
    let info = vk::BufferCreateInfo::default()
        .size(size.max(4))
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: корректное описание буфера.
    let raw = unsafe { d.create_buffer(&info, None) }.ctx("создание буфера")?;
    // SAFETY: буфер только что создан на этом устройстве.
    let requirements = unsafe { d.get_buffer_memory_requirements(raw) };
    let location = match memory {
        BufferMemory::GpuOnly => MemoryLocation::GpuOnly,
        BufferMemory::CpuToGpu => MemoryLocation::CpuToGpu,
        BufferMemory::GpuToCpu => MemoryLocation::GpuToCpu,
    };
    let allocation = match allocate(ctx, name, requirements, location, true) {
        Ok(a) => a,
        Err(e) => {
            // SAFETY: буфер нигде не используется.
            unsafe { d.destroy_buffer(raw, None) };
            return Err(e);
        }
    };
    // SAFETY: память выделена под требования этого буфера.
    if let Err(e) = unsafe { d.bind_buffer_memory(raw, allocation.memory(), allocation.offset()) } {
        // SAFETY: буфер нигде не используется.
        unsafe { d.destroy_buffer(raw, None) };
        free(ctx, Some(allocation));
        return Err(GpuError::Vulkan {
            what: "привязка памяти буфера",
            result: e,
        });
    }
    // SAFETY: буфер создан с SHADER_DEVICE_ADDRESS, память привязана.
    let address =
        unsafe { d.get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(raw)) };
    Ok(Buffer {
        raw,
        allocation: Some(allocation),
        size,
        address,
    })
}

pub(crate) fn destroy_buffer(ctx: &GpuContext, b: Buffer) {
    // SAFETY: вызывающий гарантирует, что видеокарта буфер больше не читает.
    unsafe { ctx.device.destroy_buffer(b.raw, None) };
    free(ctx, b.allocation);
}

pub(crate) fn create_texture(
    ctx: &GpuContext,
    extent: vk::Extent2D,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
    name: &str,
) -> Result<Texture, GpuError> {
    let d = &ctx.device;
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: корректное описание картинки.
    let image = unsafe { d.create_image(&info, None) }.ctx("создание текстуры")?;
    // SAFETY: картинка только что создана на этом устройстве.
    let requirements = unsafe { d.get_image_memory_requirements(image) };
    let allocation = match allocate(ctx, name, requirements, MemoryLocation::GpuOnly, false) {
        Ok(a) => a,
        Err(e) => {
            // SAFETY: картинка нигде не используется.
            unsafe { d.destroy_image(image, None) };
            return Err(e);
        }
    };
    let mut texture = Texture {
        image,
        view: vk::ImageView::null(),
        allocation: Some(allocation),
    };
    let a = texture.allocation.as_ref().expect("только что выделена");
    // SAFETY: память выделена под требования этой картинки.
    if let Err(e) = unsafe { d.bind_image_memory(image, a.memory(), a.offset()) } {
        destroy_texture(ctx, texture);
        return Err(GpuError::Vulkan {
            what: "привязка памяти текстуры",
            result: e,
        });
    }
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(color_range());
    // SAFETY: картинка с привязанной памятью.
    match unsafe { d.create_image_view(&view_info, None) } {
        Ok(v) => texture.view = v,
        Err(e) => {
            destroy_texture(ctx, texture);
            return Err(GpuError::Vulkan {
                what: "вид текстуры",
                result: e,
            });
        }
    }
    Ok(texture)
}

/// Записывает загрузку пикселей из `src` (плотные строки RGBA8) в область текстуры
/// `image`, начиная с `offset`. После команд текстура готова к чтению шейдером.
/// `first`: текстура ещё ни разу не заполнялась (раскладка UNDEFINED), иначе она в
/// раскладке чтения и её могли читать прошлые команды.
///
/// # Safety
/// `cmd` в состоянии записи, `image` и `src` живы, пока видеокарта выполняет команды.
pub(crate) unsafe fn record_texture_upload(
    d: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    src: vk::Buffer,
    offset: [u32; 2],
    extent: vk::Extent2D,
    first: bool,
) {
    let (old_layout, src_stage) = if first {
        (vk::ImageLayout::UNDEFINED, vk::PipelineStageFlags2::NONE)
    } else {
        // Прошлые кадры могли читать текстуру в пиксельном шейдере: ждём их чтения.
        (
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags2::FRAGMENT_SHADER,
        )
    };
    let to_dst = [image_barrier(
        image,
        old_layout,
        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        src_stage,
        vk::AccessFlags2::NONE,
        vk::PipelineStageFlags2::COPY,
        vk::AccessFlags2::TRANSFER_WRITE,
    )];
    let region = [vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_offset(vk::Offset3D {
            x: offset[0] as i32,
            y: offset[1] as i32,
            z: 0,
        })
        .image_extent(vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        })];
    let to_read = [image_barrier(
        image,
        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::PipelineStageFlags2::COPY,
        vk::AccessFlags2::TRANSFER_WRITE,
        vk::PipelineStageFlags2::FRAGMENT_SHADER,
        vk::AccessFlags2::SHADER_SAMPLED_READ,
    )];
    // SAFETY: условие функции.
    unsafe {
        d.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(&to_dst),
        );
        d.cmd_copy_buffer_to_image(
            cmd,
            src,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &region,
        );
        d.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(&to_read),
        );
    }
}

pub(crate) fn destroy_texture(ctx: &GpuContext, t: Texture) {
    // SAFETY: вызывающий гарантирует, что видеокарта текстуру больше не читает;
    // null-вид допустим.
    unsafe {
        ctx.device.destroy_image_view(t.view, None);
        ctx.device.destroy_image(t.image, None);
    }
    free(ctx, t.allocation);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletion_waits_for_frame_completion() {
        let mut q = DeletionQueue::new();
        q.push(5, "a");
        q.push(6, "b");
        q.push(6, "c");
        assert!(q.drain_completed(4).is_empty());
        assert_eq!(q.drain_completed(5), vec!["a"]);
        assert_eq!(q.len(), 2);
        assert_eq!(q.drain_completed(10), vec!["b", "c"]);
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn slots_reuse_only_released_numbers() {
        let mut s = Slots::new();
        let a = s.insert('a');
        let b = s.insert('b');
        assert_eq!((a, b), (0, 1));
        assert_eq!(s.take(a), Some('a'));
        // Номер ещё не отпущен (ресурс ждёт удаления): новый получает следующий номер.
        assert_eq!(s.insert('c'), 2);
        s.release(a);
        assert_eq!(s.insert('d'), 0);
        assert_eq!(s.get(0), Some(&'d'));
    }
}
