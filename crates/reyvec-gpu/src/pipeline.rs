//! Конвейер тестового треугольника этапа 1 и запись команд рисования.

use std::io::Cursor;

use ash::vk;

use crate::{GpuError, VkResultExt};

const VERT_SPV: &[u8] = include_bytes!("../shaders/triangle.vert.spv");
const FRAG_SPV: &[u8] = include_bytes!("../shaders/triangle.frag.spv");

/// Цвет фона в линейном пространстве (тёмно-синий).
pub const CLEAR_COLOR: [f32; 4] = [0.010, 0.012, 0.024, 1.0];

/// Данные, которые каждый кадр передаются в шейдер (push constants).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct TrianglePush {
    pub time: f32,
    pub aspect: f32,
}

impl TrianglePush {
    fn as_bytes(&self) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[..4].copy_from_slice(&self.time.to_ne_bytes());
        out[4..].copy_from_slice(&self.aspect.to_ne_bytes());
        out
    }
}

pub(crate) struct TrianglePipeline {
    pub layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
    pub format: vk::Format,
}

impl TrianglePipeline {
    pub fn new(device: &ash::Device, format: vk::Format) -> Result<Self, GpuError> {
        let vert = create_shader(device, VERT_SPV)?;
        let frag = match create_shader(device, FRAG_SPV) {
            Ok(f) => f,
            Err(e) => {
                // SAFETY: модуль создан выше и больше нигде не используется.
                unsafe { device.destroy_shader_module(vert, None) };
                return Err(e);
            }
        };
        let result = Self::build(device, format, vert, frag);
        // SAFETY: после создания конвейера модули шейдеров не нужны.
        unsafe {
            device.destroy_shader_module(vert, None);
            device.destroy_shader_module(frag, None);
        }
        result
    }

    fn build(
        device: &ash::Device,
        format: vk::Format,
        vert: vk::ShaderModule,
        frag: vk::ShaderModule,
    ) -> Result<Self, GpuError> {
        let push_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(std::mem::size_of::<TrianglePush>() as u32)];
        let layout_info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_range);
        // SAFETY: корректное описание без внешних указателей.
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .ctx("раскладка конвейера")?;

        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag)
                .name(c"main"),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_attachment = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = [format];
        let mut rendering =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);

        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);

        // SAFETY: все структуры живут до конца вызова, модули шейдеров валидны.
        let pipelines =
            unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };
        match pipelines {
            Ok(p) => Ok(Self {
                layout,
                pipeline: p[0],
                format,
            }),
            Err((_, result)) => {
                // SAFETY: раскладка создана выше и больше нигде не используется.
                unsafe { device.destroy_pipeline_layout(layout, None) };
                Err(GpuError::Vulkan {
                    what: "создание конвейера",
                    result,
                })
            }
        }
    }

    /// Записывает рисование треугольника в `view`. Изображение уже должно быть
    /// в раскладке `COLOR_ATTACHMENT_OPTIMAL`.
    ///
    /// # Safety
    /// `cmd` в состоянии записи, `view` и конвейер живы до конца выполнения команд.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        view: vk::ImageView,
        extent: vk::Extent2D,
        push: TrianglePush,
    ) {
        let attachments = [vk::RenderingAttachmentInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: CLEAR_COLOR,
                },
            })];
        let area = vk::Rect2D::default().extent(extent);
        let rendering = vk::RenderingInfo::default()
            .render_area(area)
            .layer_count(1)
            .color_attachments(&attachments);
        let viewport = vk::Viewport::default()
            .width(extent.width as f32)
            .height(extent.height as f32)
            .max_depth(1.0);

        // SAFETY: условия описаны в контракте функции.
        unsafe {
            device.cmd_begin_rendering(cmd, &rendering);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_set_viewport(cmd, 0, &[viewport]);
            device.cmd_set_scissor(cmd, 0, &[area]);
            device.cmd_push_constants(
                cmd,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                &push.as_bytes(),
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_rendering(cmd);
        }
    }

    /// # Safety
    /// Видеокарта больше не использует конвейер.
    pub unsafe fn destroy(&self, device: &ash::Device) {
        // SAFETY: условия описаны в контракте функции.
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
        }
    }
}

fn create_shader(device: &ash::Device, spv: &[u8]) -> Result<vk::ShaderModule, GpuError> {
    let code = ash::util::read_spv(&mut Cursor::new(spv)).map_err(|_| GpuError::Vulkan {
        what: "чтение SPIR-V",
        result: vk::Result::ERROR_INITIALIZATION_FAILED,
    })?;
    let info = vk::ShaderModuleCreateInfo::default().code(&code);
    // SAFETY: code — корректный SPIR-V, собранный glslangValidator.
    unsafe { device.create_shader_module(&info, None) }.ctx("модуль шейдера")
}

/// Барьер смены раскладки изображения цвета (synchronization2).
#[allow(clippy::too_many_arguments)]
pub(crate) fn image_barrier(
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_stage: vk::PipelineStageFlags2,
    src_access: vk::AccessFlags2,
    dst_stage: vk::PipelineStageFlags2,
    dst_access: vk::AccessFlags2,
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .image(image)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .subresource_range(color_range())
}

pub(crate) fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

#[cfg(test)]
mod tests {
    #[test]
    fn spirv_is_embedded() {
        for spv in [super::VERT_SPV, super::FRAG_SPV] {
            assert!(spv.len() > 20 && spv.len() % 4 == 0);
            assert_eq!(
                &spv[..4],
                &0x0723_0203u32.to_le_bytes(),
                "магическое число SPIR-V"
            );
        }
    }
}
