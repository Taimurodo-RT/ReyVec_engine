//! Общие части конвейеров: сборка графического конвейера из SPIR-V, барьеры, начало прохода.
//! Сам SPIR-V берётся из библиотеки шейдеров (`shaders.rs`).

use std::io::Cursor;

use ash::vk;

use crate::{GpuError, VkResultExt};

/// Цвет фона в линейном пространстве (тёмно-синий).
pub const CLEAR_COLOR: [f32; 4] = [0.010, 0.012, 0.024, 1.0];

/// Смешивание цвета с тем, что уже нарисовано.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Blend {
    /// Без смешивания: пиксель просто заменяется.
    Opaque,
    /// Обычная прозрачность по альфе.
    Alpha,
    /// Прозрачность для цвета, уже умноженного на альфу (так рисует egui).
    Premultiplied,
}

/// Собирает графический конвейер: вершинный и пиксельный шейдер, вывод в одну картинку `format`.
pub(crate) fn graphics_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    vert_spv: &[u8],
    frag_spv: &[u8],
    format: vk::Format,
    blend: Blend,
    samples: vk::SampleCountFlags,
) -> Result<vk::Pipeline, GpuError> {
    let vert = create_shader(device, vert_spv)?;
    let frag = match create_shader(device, frag_spv) {
        Ok(f) => f,
        Err(e) => {
            // SAFETY: модуль создан выше и больше нигде не используется.
            unsafe { device.destroy_shader_module(vert, None) };
            return Err(e);
        }
    };

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
    let multisample =
        vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples);
    let mut attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA);
    if blend == Blend::Alpha {
        attachment = attachment
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD);
    }
    if blend == Blend::Premultiplied {
        attachment = attachment
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD);
    }
    let blend_attachment = [attachment];
    let blend_state =
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
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
        .color_blend_state(&blend_state)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering);

    // SAFETY: все структуры живут до конца вызова, модули шейдеров валидны.
    let result =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };
    // SAFETY: после создания конвейера модули шейдеров не нужны.
    unsafe {
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
    }
    match result {
        Ok(p) => Ok(p[0]),
        Err((_, result)) => Err(GpuError::Vulkan {
            what: "создание конвейера",
            result,
        }),
    }
}

/// Начинает проход рисования в `view` с очисткой фоном и ставит область вывода на всю картинку.
/// Картинка уже должна быть в раскладке `COLOR_ATTACHMENT_OPTIMAL`.
///
/// # Safety
/// `cmd` в состоянии записи, `view` жив до конца выполнения команд.
pub(crate) unsafe fn begin_color_pass(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    view: vk::ImageView,
    extent: vk::Extent2D,
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
    // SAFETY: условия описаны в контракте функции.
    unsafe { begin_rendering(device, cmd, &attachments, extent) };
}

/// Начинает рисование в `attachments` и ставит область вывода на всю картинку.
///
/// # Safety
/// `cmd` в состоянии записи, картинки живы до конца выполнения команд.
unsafe fn begin_rendering(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    attachments: &[vk::RenderingAttachmentInfo],
    extent: vk::Extent2D,
) {
    let area = vk::Rect2D::default().extent(extent);
    let rendering = vk::RenderingInfo::default()
        .render_area(area)
        .layer_count(1)
        .color_attachments(attachments);
    let viewport = vk::Viewport::default()
        .width(extent.width as f32)
        .height(extent.height as f32)
        .max_depth(1.0);
    // SAFETY: условия описаны в контракте функции.
    unsafe {
        device.cmd_begin_rendering(cmd, &rendering);
        device.cmd_set_viewport(cmd, 0, &[viewport]);
        device.cmd_set_scissor(cmd, 0, &[area]);
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
