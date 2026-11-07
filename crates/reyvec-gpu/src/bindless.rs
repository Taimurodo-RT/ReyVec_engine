//! Bindless: один общий список текстур для всех шейдеров.
//!
//! Шейдер получает номер текстуры (например, из данных спрайта) и читает
//! `textures[номер]`. Не нужно перепривязывать текстуры перед каждым рисованием,
//! поэтому тысячи разных спрайтов рисуются одной командой.
//!
//! Раскладка (set 0), общая для всех конвейеров движка:
//! - binding 0: `texture2D textures[MAX_TEXTURES]`;
//! - binding 1: `sampler samplers[2]`: 0 = сглаживающий, 1 = без сглаживания (для пикселей);
//! - binding 2: `RWTexture2D images[TRANSIENT_TEXTURES]` — запись в картинки из шейдеров
//!   (временные картинки графа: ячейка `i` здесь = ячейка `FIRST_TRANSIENT_SLOT + i` в
//!   binding 0). Есть, только если видеокарта умеет (`GpuContext::storage_images`);
//! - binding 3: `Texture3D<float> field_volumes[FIELD_VOLUMES]` — слои запечённого поля
//!   расстояний сцены «3D» (7.10е);
//! - binding 4: `SamplerState field_sampler` — сглаживающий с повтором по краям: слои поля
//!   лежат кольцом (точка `w` в ячейке `w mod size`), повтор и есть это кольцо.
//!
//! Буферы шейдеры читают по адресу (buffer device address), его передают в push constants.

use ash::vk;

use crate::context::GpuContext;
use crate::graph::TRANSIENT_TEXTURES;
use crate::{GpuError, VkResultExt};

/// Сколько текстур помещается в общий список.
pub const MAX_TEXTURES: u32 = 4096;

/// Ячеек для слоёв запечённого поля (binding 3), как `field3d::FIELD_LEVELS`.
pub const FIELD_VOLUMES: u32 = 4;

/// Размер push constants, общий для всех конвейеров (гарантированный минимум Vulkan).
pub const PUSH_CONSTANTS_SIZE: u32 = 128;

/// Номер сэмплера в `samplers[]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum SamplerKind {
    /// Сглаживание между пикселями текстуры.
    Linear = 0,
    /// Чёткие пиксели (пиксель-арт).
    Nearest = 1,
}

pub(crate) struct Bindless {
    pub set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    samplers: [vk::Sampler; 2],
    /// Сэмплер слоёв поля (binding 4).
    field_sampler: vk::Sampler,
    /// Есть ли binding 2 (запись в картинки).
    pub storage_images: bool,
}

impl Bindless {
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuError> {
        let d = &ctx.device;
        let mut this = Self {
            set_layout: vk::DescriptorSetLayout::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            pool: vk::DescriptorPool::null(),
            set: vk::DescriptorSet::null(),
            samplers: [vk::Sampler::null(); 2],
            field_sampler: vk::Sampler::null(),
            storage_images: ctx.storage_images,
        };
        // При ошибке на любом шаге убираем то, что успели создать (null-дескрипторы допустимы).
        match this.init(d) {
            Ok(()) => Ok(this),
            Err(e) => {
                this.destroy(ctx);
                Err(e)
            }
        }
    }

    fn init(&mut self, d: &ash::Device) -> Result<(), GpuError> {
        let make_sampler = |filter: vk::Filter| {
            vk::SamplerCreateInfo::default()
                .mag_filter(filter)
                .min_filter(filter)
                .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .max_lod(vk::LOD_CLAMP_NONE)
        };
        // SAFETY: все объекты принадлежат этому устройству; уничтожаются в destroy().
        unsafe {
            self.samplers[SamplerKind::Linear as usize] = d
                .create_sampler(&make_sampler(vk::Filter::LINEAR), None)
                .ctx("сэмплер")?;
            self.samplers[SamplerKind::Nearest as usize] = d
                .create_sampler(&make_sampler(vk::Filter::NEAREST), None)
                .ctx("сэмплер")?;

            let repeat = vk::SamplerAddressMode::REPEAT;
            self.field_sampler = d
                .create_sampler(
                    &make_sampler(vk::Filter::LINEAR)
                        .address_mode_u(repeat)
                        .address_mode_v(repeat)
                        .address_mode_w(repeat),
                    None,
                )
                .ctx("сэмплер поля")?;
            let field_samplers = [self.field_sampler];

            let updatable = vk::DescriptorBindingFlags::PARTIALLY_BOUND
                | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
                | vk::DescriptorBindingFlags::UPDATE_UNUSED_WHILE_PENDING;
            let mut bindings = vec![
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(MAX_TEXTURES)
                    .stage_flags(vk::ShaderStageFlags::ALL),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(self.samplers.len() as u32)
                    .stage_flags(vk::ShaderStageFlags::ALL)
                    .immutable_samplers(&self.samplers),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(3)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(FIELD_VOLUMES)
                    .stage_flags(vk::ShaderStageFlags::ALL),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(4)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::ALL)
                    .immutable_samplers(&field_samplers),
            ];
            let mut flags = vec![
                updatable,
                vk::DescriptorBindingFlags::empty(),
                updatable,
                vk::DescriptorBindingFlags::empty(),
            ];
            let mut sizes = vec![
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: MAX_TEXTURES + FIELD_VOLUMES,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLER,
                    descriptor_count: self.samplers.len() as u32 + 1,
                },
            ];
            if self.storage_images {
                bindings.push(
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(2)
                        .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                        .descriptor_count(TRANSIENT_TEXTURES)
                        .stage_flags(vk::ShaderStageFlags::ALL),
                );
                flags.push(updatable);
                sizes.push(vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::STORAGE_IMAGE,
                    descriptor_count: TRANSIENT_TEXTURES,
                });
            }
            let mut flags_info =
                vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
            self.set_layout = d
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default()
                        .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
                        .bindings(&bindings)
                        .push_next(&mut flags_info),
                    None,
                )
                .ctx("раскладка bindless")?;
            self.pool = d
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .ctx("пул дескрипторов")?;
            let layouts = [self.set_layout];
            self.set = d
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(self.pool)
                        .set_layouts(&layouts),
                )
                .ctx("набор дескрипторов")?[0];

            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::ALL)
                .offset(0)
                .size(PUSH_CONSTANTS_SIZE)];
            self.pipeline_layout = d
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&layouts)
                        .push_constant_ranges(&push),
                    None,
                )
                .ctx("общая раскладка конвейеров")?;
        }
        Ok(())
    }

    /// Кладёт текстуру в ячейку `slot` общего списка.
    pub fn write_texture(&self, ctx: &GpuContext, slot: u32, view: vk::ImageView) {
        let image = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(0)
            .dst_array_element(slot)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&image);
        // SAFETY: набор создан с UPDATE_AFTER_BIND, ячейка в пределах MAX_TEXTURES
        // (проверяет вызывающий), вид текстуры жив.
        unsafe { ctx.device.update_descriptor_sets(&[write], &[]) };
    }

    /// Кладёт картинку для записи в ячейку `index` списка binding 2 (раскладка GENERAL).
    pub fn write_storage_image(&self, ctx: &GpuContext, index: u32, view: vk::ImageView) {
        if !self.storage_images {
            return;
        }
        let image = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(2)
            .dst_array_element(index)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(&image);
        // SAFETY: binding 2 создан с UPDATE_AFTER_BIND, ячейка в пределах
        // TRANSIENT_TEXTURES (проверяет вызывающий), вид картинки жив.
        unsafe { ctx.device.update_descriptor_sets(&[write], &[]) };
    }

    /// Кладёт слой запечённого поля `level` в binding 3 (раскладка GENERAL: в слой пишет
    /// копирование, пока прошлые кадры могли его читать).
    pub fn write_field_volume(&self, ctx: &GpuContext, level: u32, view: vk::ImageView) {
        let image = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(3)
            .dst_array_element(level)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&image);
        // SAFETY: binding 3 создан с UPDATE_AFTER_BIND, ячейка в пределах FIELD_VOLUMES
        // (проверяет вызывающий), вид картинки жив.
        unsafe { ctx.device.update_descriptor_sets(&[write], &[]) };
    }

    /// Привязывает общий список к командному буферу для графики.
    ///
    /// # Safety
    /// `cmd` в состоянии записи.
    pub unsafe fn bind(&self, d: &ash::Device, cmd: vk::CommandBuffer) {
        // SAFETY: условие функции.
        unsafe {
            d.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.set],
                &[],
            );
        }
    }

    /// То же для вычислительных шейдеров.
    ///
    /// # Safety
    /// Как у [`Bindless::bind`].
    pub unsafe fn bind_compute(&self, d: &ash::Device, cmd: vk::CommandBuffer) {
        // SAFETY: условие функции.
        unsafe {
            d.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_layout,
                0,
                &[self.set],
                &[],
            );
        }
    }

    pub fn destroy(&self, ctx: &GpuContext) {
        let d = &ctx.device;
        // SAFETY: вызывается, когда видеокарта не использует эти объекты; null допустим.
        unsafe {
            d.destroy_pipeline_layout(self.pipeline_layout, None);
            d.destroy_descriptor_pool(self.pool, None);
            d.destroy_descriptor_set_layout(self.set_layout, None);
            for s in self.samplers {
                d.destroy_sampler(s, None);
            }
            d.destroy_sampler(self.field_sampler, None);
        }
    }
}
