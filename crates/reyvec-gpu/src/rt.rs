//! RT-ядра как сито для теней сцены «3D» (опыт Р1, «Р1» в docs/3d-beyond-triangles.md).
//!
//! Видеокарте даются не треугольники, а коробки предметов: три дерева коробок (BLAS),
//! в каждом коробки всех предметов, раздутые на ширину полутени своего отрезка луча
//! ([`RAY_INFLATE`]). Одно верхнее дерево (TLAS) держит их тремя экземплярами с масками
//! 1, 2 и 4. Луч тени в шейдере (`include/shadow3d.slang`, `soft_shadow_rt`) спрашивает
//! дерево, через какие коробки он пройдёт, и шагает только по формам этих предметов.
//! Формулы те же, поэтому картинка та же; меняется только то, кого спрашивать.
//!
//! Коробки лежат в словах сцены кадра (раздел RT, `crate::space::set_ray_boxes`), дерево
//! строится из них прямо по адресу буфера сцены каждый кадр (быстрая постройка: на
//! 4096 коробок это доли миллисекунды). У каждого кадра в полёте свои деревья.

use ash::{khr, vk};

use crate::GpuError;
use crate::context::GpuContext;
use crate::resources::{Buffer, BufferMemory, create_buffer, destroy_buffer};

/// Отрезков луча тени, у каждого своё дерево коробок.
pub(crate) const RAY_LEVELS: usize = 3;

/// На сколько метров раздуты коробки дерева отрезка `i`. Полутень даёт только форма
/// ближе `t / k` к точке луча на расстоянии `t` (k — жёсткость тени, `soft_shadow`), поэтому
/// отрезок `i` кончается на `k · RAY_INFLATE[i]`, последний — на дальности луча.
/// Последнее число — 30 м / k = 10 (лампа): дальше луч тени не идёт.
pub(crate) const RAY_INFLATE: [f32; RAY_LEVELS] = [0.1, 0.5, 3.0];

/// Слов на коробку в разделе RT: наименьший и наибольший углы (как VkAabbPositionsKHR).
pub(crate) const BOX_WORDS: usize = 6;

struct Accel {
    buffer: Buffer,
    handle: vk::AccelerationStructureKHR,
    address: vk::DeviceAddress,
}

struct Slot {
    blas: Vec<Accel>,
    tlas: Accel,
    /// Три экземпляра верхнего дерева (не меняются: деревья коробок всегда те же объекты).
    instances: Buffer,
    /// Память для постройки: три нижних дерева подряд, потом верхнее.
    scratch: Buffer,
}

/// Деревья коробок для всех кадров в полёте.
pub(crate) struct RayBoxes {
    loader: khr::acceleration_structure::Device,
    slots: Vec<Slot>,
    /// Где в памяти постройки начинается каждое нижнее дерево и верхнее.
    scratch_blas: u64,
    max_boxes: u32,
}

fn align(x: u64, a: u64) -> u64 {
    x.div_ceil(a) * a
}

fn aabb_geometry<'a>(data: vk::DeviceAddress) -> vk::AccelerationStructureGeometryKHR<'a> {
    vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::AABBS)
        .geometry(vk::AccelerationStructureGeometryDataKHR {
            aabbs: vk::AccelerationStructureGeometryAabbsDataKHR::default()
                .data(vk::DeviceOrHostAddressConstKHR {
                    device_address: data,
                })
                .stride((BOX_WORDS * 4) as u64),
        })
        // Без «непрозрачности» шейдер видит каждую коробку кандидатом и сам решает, что с
        // ней делать (шагает по форме), а обход дерева не прекращается на первой.
        .flags(vk::GeometryFlagsKHR::empty())
}

fn instance_geometry<'a>(data: vk::DeviceAddress) -> vk::AccelerationStructureGeometryKHR<'a> {
    vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::INSTANCES)
        .geometry(vk::AccelerationStructureGeometryDataKHR {
            instances: vk::AccelerationStructureGeometryInstancesDataKHR::default()
                .array_of_pointers(false)
                .data(vk::DeviceOrHostAddressConstKHR {
                    device_address: data,
                }),
        })
}

impl RayBoxes {
    /// Создаёт деревья для `frames` кадров в полёте на `max_boxes` коробок в каждом.
    /// `None` — видеокарта не умеет ray query.
    pub fn new(ctx: &GpuContext, frames: usize, max_boxes: u32) -> Result<Option<Self>, GpuError> {
        if !ctx.ray_query {
            return Ok(None);
        }
        let loader = khr::acceleration_structure::Device::new(ctx.instance(), &ctx.device);
        let size_of = |geometry: &vk::AccelerationStructureGeometryKHR, ty, count: u32| {
            let geometries = [*geometry];
            let info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
                .ty(ty)
                .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_BUILD)
                .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
                .geometries(&geometries);
            let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
            // SAFETY: описание постройки корректно, расширение включено (ctx.ray_query).
            unsafe {
                loader.get_acceleration_structure_build_sizes(
                    vk::AccelerationStructureBuildTypeKHR::DEVICE,
                    &info,
                    &[count],
                    &mut sizes,
                );
            }
            sizes
        };
        let blas_sizes = size_of(
            &aabb_geometry(0),
            vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
            max_boxes,
        );
        let tlas_sizes = size_of(
            &instance_geometry(0),
            vk::AccelerationStructureTypeKHR::TOP_LEVEL,
            RAY_LEVELS as u32,
        );
        let a = ctx.accel_scratch_align.max(256);
        let scratch_blas = align(blas_sizes.build_scratch_size, a);
        let scratch_size = scratch_blas * RAY_LEVELS as u64 + tlas_sizes.build_scratch_size;

        let mut this = Self {
            loader,
            slots: Vec::new(),
            scratch_blas,
            max_boxes,
        };
        for f in 0..frames {
            match this.make_slot(
                ctx,
                f,
                blas_sizes.acceleration_structure_size,
                tlas_sizes.acceleration_structure_size,
                scratch_size,
            ) {
                Ok(slot) => this.slots.push(slot),
                Err(e) => {
                    this.destroy(ctx);
                    return Err(e);
                }
            }
        }
        tracing::info!(
            "RT-ядра для теней готовы: деревья коробок на {max_boxes} предметов, {} КБ на кадр",
            (blas_sizes.acceleration_structure_size * RAY_LEVELS as u64
                + tlas_sizes.acceleration_structure_size
                + scratch_size)
                / 1024
        );
        Ok(Some(this))
    }

    fn make_accel(
        &self,
        ctx: &GpuContext,
        size: u64,
        ty: vk::AccelerationStructureTypeKHR,
        name: &str,
    ) -> Result<Accel, GpuError> {
        let buffer = create_buffer(
            ctx,
            size,
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR,
            BufferMemory::GpuOnly,
            name,
        )?;
        let info = vk::AccelerationStructureCreateInfoKHR::default()
            .buffer(buffer.raw)
            .size(size)
            .ty(ty);
        // SAFETY: буфер создан под дерево этого размера; расширение включено.
        let handle = match unsafe { self.loader.create_acceleration_structure(&info, None) } {
            Ok(h) => h,
            Err(e) => {
                destroy_buffer(ctx, buffer);
                return Err(GpuError::Vulkan {
                    what: "создание дерева коробок",
                    result: e,
                });
            }
        };
        // SAFETY: дерево только что создано на этом устройстве.
        let address = unsafe {
            self.loader.get_acceleration_structure_device_address(
                &vk::AccelerationStructureDeviceAddressInfoKHR::default()
                    .acceleration_structure(handle),
            )
        };
        Ok(Accel {
            buffer,
            handle,
            address,
        })
    }

    fn destroy_accel(&self, ctx: &GpuContext, a: Accel) {
        // SAFETY: вызывающий гарантирует, что видеокарта дерево больше не читает.
        unsafe { self.loader.destroy_acceleration_structure(a.handle, None) };
        destroy_buffer(ctx, a.buffer);
    }

    fn make_slot(
        &self,
        ctx: &GpuContext,
        frame: usize,
        blas_size: u64,
        tlas_size: u64,
        scratch_size: u64,
    ) -> Result<Slot, GpuError> {
        let mut blas = Vec::new();
        let cleanup = |blas: Vec<Accel>| {
            for a in blas {
                self.destroy_accel(ctx, a);
            }
        };
        for i in 0..RAY_LEVELS {
            match self.make_accel(
                ctx,
                blas_size,
                vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
                &format!("коробки RT {i} кадра {frame}"),
            ) {
                Ok(a) => blas.push(a),
                Err(e) => {
                    cleanup(blas);
                    return Err(e);
                }
            }
        }
        let tlas = match self.make_accel(
            ctx,
            tlas_size,
            vk::AccelerationStructureTypeKHR::TOP_LEVEL,
            &format!("дерево RT кадра {frame}"),
        ) {
            Ok(a) => a,
            Err(e) => {
                cleanup(blas);
                return Err(e);
            }
        };
        let instance_bytes = std::mem::size_of::<vk::AccelerationStructureInstanceKHR>();
        let mut instances = match create_buffer(
            ctx,
            (instance_bytes * RAY_LEVELS) as u64,
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
            BufferMemory::CpuToGpu,
            &format!("экземпляры RT кадра {frame}"),
        ) {
            Ok(b) => b,
            Err(e) => {
                cleanup(blas);
                self.destroy_accel(ctx, tlas);
                return Err(e);
            }
        };
        // Экземпляр `i` — дерево отрезка `i` без сдвига, маска 1 << i: шейдер спрашивает
        // отрезок своей маской и видит только его коробки.
        let identity = vk::TransformMatrixKHR {
            matrix: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        };
        let records: Vec<vk::AccelerationStructureInstanceKHR> = blas
            .iter()
            .enumerate()
            .map(|(i, b)| vk::AccelerationStructureInstanceKHR {
                transform: identity,
                instance_custom_index_and_mask: vk::Packed24_8::new(i as u32, 1 << i),
                instance_shader_binding_table_record_offset_and_flags: vk::Packed24_8::new(
                    0,
                    vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE.as_raw() as u8,
                ),
                acceleration_structure_reference: vk::AccelerationStructureReferenceKHR {
                    device_handle: b.address,
                },
            })
            .collect();
        if let Some(slice) = instances
            .allocation
            .as_mut()
            .and_then(gpu_allocator::vulkan::Allocation::mapped_slice_mut)
        {
            // SAFETY: записи — простые структуры Vulkan без указателей; байты копируются
            // в отображённую память буфера нужного размера.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    records.as_ptr().cast::<u8>(),
                    instance_bytes * records.len(),
                )
            };
            slice[..bytes.len()].copy_from_slice(bytes);
        }
        let scratch = match create_buffer(
            ctx,
            scratch_size,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            BufferMemory::GpuOnly,
            &format!("постройка RT кадра {frame}"),
        ) {
            Ok(b) => b,
            Err(e) => {
                cleanup(blas);
                self.destroy_accel(ctx, tlas);
                destroy_buffer(ctx, instances);
                return Err(e);
            }
        };
        Ok(Slot {
            blas,
            tlas,
            instances,
            scratch,
        })
    }

    /// Адрес верхнего дерева кадра в полёте `slot`: его пишут в слова сцены.
    pub fn tlas_address(&self, slot: usize) -> vk::DeviceAddress {
        self.slots.get(slot).map_or(0, |s| s.tlas.address)
    }

    /// Больше коробок дерево не держит.
    pub fn max_boxes(&self) -> u32 {
        self.max_boxes
    }

    /// Записывает постройку деревьев кадра `slot` из `count` коробок на уровень, лежащих
    /// подряд по адресу `boxes` (уровень `i` — с `boxes + i · count · 24` байт), и барьер
    /// перед шейдерами, которые их читают.
    ///
    /// # Safety
    /// `cmd` записывается и не внутри прохода рисования; буфер с коробками жив, пока кадр
    /// рисуется, создан с `ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR`, процессор
    /// записал его до отправки кадра; прошлый кадр этой ячейки закончен.
    pub unsafe fn record_build(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        slot: usize,
        boxes: vk::DeviceAddress,
        count: u32,
    ) {
        let Some(s) = self.slots.get(slot) else {
            return;
        };
        let count = count.min(self.max_boxes);
        let geometries: Vec<[vk::AccelerationStructureGeometryKHR; 1]> = (0..RAY_LEVELS)
            .map(|i| {
                [aabb_geometry(
                    boxes + (i * count as usize * BOX_WORDS * 4) as u64,
                )]
            })
            .collect();
        let infos: Vec<vk::AccelerationStructureBuildGeometryInfoKHR> = s
            .blas
            .iter()
            .enumerate()
            .map(|(i, b)| {
                vk::AccelerationStructureBuildGeometryInfoKHR::default()
                    .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
                    .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_BUILD)
                    .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
                    .dst_acceleration_structure(b.handle)
                    .geometries(&geometries[i])
                    .scratch_data(vk::DeviceOrHostAddressKHR {
                        device_address: s.scratch.address + self.scratch_blas * i as u64,
                    })
            })
            .collect();
        let range = [vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(count)];
        let ranges: Vec<&[vk::AccelerationStructureBuildRangeInfoKHR]> =
            (0..RAY_LEVELS).map(|_| &range[..]).collect();

        let tlas_geometry = [instance_geometry(s.instances.address)];
        let tlas_info = [vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(vk::AccelerationStructureTypeKHR::TOP_LEVEL)
            .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_BUILD)
            .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
            .dst_acceleration_structure(s.tlas.handle)
            .geometries(&tlas_geometry)
            .scratch_data(vk::DeviceOrHostAddressKHR {
                device_address: s.scratch.address + self.scratch_blas * RAY_LEVELS as u64,
            })];
        let tlas_range = [vk::AccelerationStructureBuildRangeInfoKHR::default()
            .primitive_count(RAY_LEVELS as u32)];

        let build = vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR;
        let between = [vk::MemoryBarrier2::default()
            .src_stage_mask(build)
            .src_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR)
            .dst_stage_mask(build)
            .dst_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR)];
        let after = [vk::MemoryBarrier2::default()
            .src_stage_mask(build)
            .src_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR)
            .dst_stage_mask(
                vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COMPUTE_SHADER,
            )
            .dst_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR)];
        // Прошлый кадр этой ячейки закончен (условие функции), поэтому память постройки и
        // сами деревья можно переписывать без барьера перед постройкой.
        // SAFETY: условия функции; все описания живут до конца вызовов.
        unsafe {
            self.loader
                .cmd_build_acceleration_structures(cmd, &infos, &ranges);
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().memory_barriers(&between),
            );
            self.loader
                .cmd_build_acceleration_structures(cmd, &tlas_info, &[&tlas_range]);
            device
                .cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&after));
        }
    }

    pub fn destroy(&mut self, ctx: &GpuContext) {
        for s in std::mem::take(&mut self.slots) {
            for a in s.blas {
                self.destroy_accel(ctx, a);
            }
            self.destroy_accel(ctx, s.tlas);
            destroy_buffer(ctx, s.instances);
            destroy_buffer(ctx, s.scratch);
        }
    }
}

/// Коробки предметов для раздела RT: по [`RAY_LEVELS`] раздутий подряд, у каждого
/// `boxes.len()` коробок (`None` — предмет теней не даёт: коробка выключена, у Vulkan это
/// не-число в наименьшем углу).
pub(crate) fn box_words(boxes: &[Option<(glam::Vec3, glam::Vec3)>]) -> Vec<u32> {
    let mut out = Vec::with_capacity(boxes.len() * BOX_WORDS * RAY_LEVELS);
    for inflate in RAY_INFLATE {
        for b in boxes {
            match b {
                Some((lo, hi)) => {
                    let (lo, hi) = (*lo - inflate, *hi + inflate);
                    out.extend([lo.x, lo.y, lo.z, hi.x, hi.y, hi.z].map(f32::to_bits));
                }
                None => out.extend([f32::NAN, 0.0, 0.0, 0.0, 0.0, 0.0].map(f32::to_bits)),
            }
        }
    }
    out
}
