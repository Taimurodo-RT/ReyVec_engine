//! Замер времени проходов на видеокарте (timestamp-запросы Vulkan) и передача замеров в Tracy.
//!
//! Видеокарта сама записывает метки времени в начале кадра и после каждого прохода графа.
//! Результаты читаются, когда кадр закончен (после ожидания его забора), то есть с опозданием
//! на `FRAMES_IN_FLIGHT` кадров. Время прохода считается от конца прошлого прохода до конца
//! этого, поэтому сюда входят и барьеры перед проходом, а сумма проходов равна времени кадра.

use std::collections::HashMap;

use ash::vk;

use crate::context::GpuContext;
use crate::{GpuError, VkResultExt};

/// Больше проходов в кадре не замеряется (остальные войдут в последний замеренный).
pub(crate) const MAX_TIMED_PASSES: usize = 32;
/// Имя «функции» у зон видеокарты в Tracy.
const TRACY_FUNCTION: &str = "граф кадра";
const QUERIES_PER_FRAME: u32 = MAX_TIMED_PASSES as u32 + 1;

/// Время одного прохода на видеокарте.
#[derive(Clone, Debug, PartialEq)]
pub struct PassTiming {
    /// Имя прохода в графе кадра.
    pub name: &'static str,
    /// Миллисекунды.
    pub ms: f64,
}

/// Сколько видеокарта потратила на кадр и на каждый его проход.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuFrameTimings {
    /// Весь кадр от первого до последнего прохода, миллисекунды.
    pub total_ms: f64,
    /// Проходы в порядке выполнения.
    pub passes: Vec<PassTiming>,
}

impl GpuFrameTimings {
    /// Строка вида «сцена 0.12 мс, вывод 0.05 мс (всего 0.17 мс)».
    pub fn describe(&self) -> String {
        let parts: Vec<String> = self
            .passes
            .iter()
            .map(|p| format!("{} {:.3} мс", p.name, p.ms))
            .collect();
        format!("{} (всего {:.3} мс)", parts.join(", "), self.total_ms)
    }
}

/// Копит замеры за несколько кадров и выдаёт среднее по каждому проходу.
#[derive(Clone, Debug, Default)]
pub struct GpuTimingAverage {
    frames: u32,
    total_ms: f64,
    /// Имя прохода → сумма мс. Порядок проходов отдельно.
    sums: HashMap<&'static str, f64>,
    order: Vec<&'static str>,
}

impl GpuTimingAverage {
    /// Добавляет замер одного кадра.
    pub fn add(&mut self, t: &GpuFrameTimings) {
        self.frames += 1;
        self.total_ms += t.total_ms;
        for p in &t.passes {
            *self.sums.entry(p.name).or_insert_with(|| {
                self.order.push(p.name);
                0.0
            }) += p.ms;
        }
    }

    /// Сколько кадров накоплено.
    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// Среднее за накопленные кадры. `None`, если замеров не было. Проход, которого не было
    /// в части кадров (например, панель спрятали), считается в них за 0 мс: так сумма
    /// проходов равна среднему времени кадра.
    pub fn average(&self) -> Option<GpuFrameTimings> {
        if self.frames == 0 {
            return None;
        }
        Some(GpuFrameTimings {
            total_ms: self.total_ms / self.frames as f64,
            passes: self
                .order
                .iter()
                .map(|&name| PassTiming {
                    name,
                    ms: self.sums[name] / self.frames as f64,
                })
                .collect(),
        })
    }

    /// Начать копить заново.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Что записано в ячейку кадра и ждёт результатов.
#[derive(Default)]
struct Slot {
    /// Имена замеренных проходов; меток записано на одну больше (метка начала кадра).
    passes: Vec<&'static str>,
    started: bool,
    /// Зоны этого кадра отправляются в Tracy (он был подключён, когда кадр записывался).
    tracy: bool,
    tracy_frame: Option<tracy_client::GpuSpan>,
    /// Зоны Tracy по проходам (`None`, если Tracy не дал зону).
    tracy_passes: Vec<Option<tracy_client::GpuSpan>>,
}

/// Таймер проходов: одна группа запросов на каждый кадр в работе.
pub(crate) struct GpuTimer {
    pool: vk::QueryPool,
    slots: Vec<Slot>,
    current: usize,
    /// Наносекунд на один тик счётчика видеокарты.
    period_ns: f64,
    /// Сколько младших бит метки времени настоящие (остальные мусор).
    mask: u64,
    tracy: Option<tracy_client::GpuContext>,
    last: Option<GpuFrameTimings>,
}

impl GpuTimer {
    /// Создаёт таймер на `frames` кадров в работе. `None`, если очередь видеокарты не умеет
    /// ставить метки времени (тогда замеров просто нет, движок работает как раньше).
    pub fn new(ctx: &GpuContext, frames: usize) -> Result<Option<Self>, GpuError> {
        let instance = ctx.instance();
        // SAFETY: physical получен из этого экземпляра.
        let props = unsafe { instance.get_physical_device_properties(ctx.physical) };
        // SAFETY: то же.
        let families =
            unsafe { instance.get_physical_device_queue_family_properties(ctx.physical) };
        let bits = families
            .get(ctx.queue_family as usize)
            .map_or(0, |f| f.timestamp_valid_bits);
        let period_ns = props.limits.timestamp_period as f64;
        if bits == 0 || period_ns <= 0.0 {
            tracing::info!("Видеокарта не умеет замерять время проходов: замеров не будет");
            return Ok(None);
        }
        let mask = if bits >= 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        let info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(QUERIES_PER_FRAME * frames as u32);
        // SAFETY: простой объект устройства, уничтожается в destroy().
        let pool = unsafe { ctx.device.create_query_pool(&info, None) }.ctx("пул замеров")?;
        let mut timer = Self {
            pool,
            slots: (0..frames).map(|_| Slot::default()).collect(),
            current: 0,
            period_ns,
            mask,
            tracy: None,
            last: None,
        };
        if let Some(client) = tracy_client::Client::running() {
            match timer.calibrate(ctx) {
                Ok(gpu_now) => match client.new_gpu_context(
                    Some("видеокарта"),
                    tracy_client::GpuContextType::Vulkan,
                    gpu_now,
                    period_ns as f32,
                ) {
                    Ok(t) => timer.tracy = Some(t),
                    Err(e) => tracing::warn!("Tracy: не создан контекст видеокарты: {e}"),
                },
                Err(e) => {
                    timer.destroy(ctx);
                    return Err(e);
                }
            }
        }
        tracing::info!(
            "Замер проходов на видеокарте включён (тик {:.2} нс, {bits} бит){}",
            period_ns,
            if timer.tracy.is_some() {
                ", замеры уходят в Tracy"
            } else {
                ""
            }
        );
        Ok(Some(timer))
    }

    /// Текущее показание часов видеокарты: записываем метку отдельной отправкой и ждём её.
    /// Нужно Tracy, чтобы совместить время видеокарты со временем процессора.
    fn calibrate(&self, ctx: &GpuContext) -> Result<i64, GpuError> {
        let d = &ctx.device;
        // SAFETY: временные пул, буфер и забор живут только внутри функции; ждём выполнения
        // до их уничтожения. Запрос 0 пула сбрасывается в той же отправке до записи.
        unsafe {
            let pool = d
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(ctx.queue_family),
                    None,
                )
                .ctx("пул команд калибровки")?;
            let fence = match d.create_fence(&vk::FenceCreateInfo::default(), None) {
                Ok(f) => f,
                Err(e) => {
                    d.destroy_command_pool(pool, None);
                    return Err(e).ctx("забор калибровки");
                }
            };
            let result = (|| {
                let cmd = d
                    .allocate_command_buffers(
                        &vk::CommandBufferAllocateInfo::default()
                            .command_pool(pool)
                            .level(vk::CommandBufferLevel::PRIMARY)
                            .command_buffer_count(1),
                    )
                    .ctx("буфер калибровки")?[0];
                d.begin_command_buffer(
                    cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .ctx("запись калибровки")?;
                d.cmd_reset_query_pool(cmd, self.pool, 0, 1);
                d.cmd_write_timestamp2(cmd, vk::PipelineStageFlags2::ALL_COMMANDS, self.pool, 0);
                d.end_command_buffer(cmd).ctx("конец калибровки")?;
                let cmds = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
                d.queue_submit2(
                    ctx.queue,
                    &[vk::SubmitInfo2::default().command_buffer_infos(&cmds)],
                    fence,
                )
                .ctx("отправка калибровки")?;
                d.wait_for_fences(&[fence], true, u64::MAX)
                    .ctx("ожидание калибровки")?;
                let mut value = [0u64; 1];
                d.get_query_pool_results(
                    self.pool,
                    0,
                    &mut value,
                    vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
                )
                .ctx("метка калибровки")?;
                Ok((value[0] & self.mask) as i64)
            })();
            d.destroy_fence(fence, None);
            d.destroy_command_pool(pool, None);
            result
        }
    }

    /// Начало кадра в ячейке `slot`: её прошлый кадр закончен (его забор дождались),
    /// поэтому читаем его результаты. Вызывать после ожидания забора ячейки.
    pub fn begin_frame(&mut self, ctx: &GpuContext, slot: usize) -> Result<(), GpuError> {
        self.current = slot;
        let s = std::mem::take(&mut self.slots[slot]);
        if !s.started {
            return Ok(());
        }
        let mut ticks = vec![0u64; s.passes.len() + 1];
        let first = slot as u32 * QUERIES_PER_FRAME;
        // SAFETY: все эти запросы записаны в отправленном кадре, и кадр закончен.
        unsafe {
            ctx.device.get_query_pool_results(
                self.pool,
                first,
                &mut ticks,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )
        }
        .ctx("чтение замеров")?;
        for t in &mut ticks {
            *t &= self.mask;
        }
        let ms = |a: u64, b: u64| (b.wrapping_sub(a) & self.mask) as f64 * self.period_ns / 1e6;
        let passes = s
            .passes
            .iter()
            .enumerate()
            .map(|(i, &name)| PassTiming {
                name,
                ms: ms(ticks[i], ticks[i + 1]),
            })
            .collect();
        let total_ms = ms(ticks[0], ticks[ticks.len() - 1]);
        self.last = Some(GpuFrameTimings { total_ms, passes });

        // Tracy ждёт метки в порядке возрастания: начало кадра, потом проходы, потом конец кадра.
        if let Some(frame) = &s.tracy_frame {
            frame.upload_timestamp_start(ticks[0] as i64);
        }
        for (i, span) in s.tracy_passes.iter().enumerate() {
            let Some(span) = span else { continue };
            span.upload_timestamp_start(ticks[i] as i64);
            span.upload_timestamp_end(ticks[i + 1] as i64);
        }
        if let Some(frame) = &s.tracy_frame {
            frame.upload_timestamp_end(ticks[ticks.len() - 1] as i64);
        }
        Ok(())
    }

    /// Ставит метку начала кадра. Вызывать в начале записи команд кадра, вне прохода.
    ///
    /// # Safety
    /// `cmd` в состоянии записи и будет отправлен в этом кадре.
    pub unsafe fn record_start(&mut self, device: &ash::Device, cmd: vk::CommandBuffer) {
        let first = self.current as u32 * QUERIES_PER_FRAME;
        // SAFETY: условие функции; запросы этой ячейки видеокарта уже не трогает (кадр закончен).
        unsafe {
            device.cmd_reset_query_pool(cmd, self.pool, first, QUERIES_PER_FRAME);
            device.cmd_write_timestamp2(
                cmd,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                self.pool,
                first,
            );
        }
        let s = &mut self.slots[self.current];
        s.started = true;
        s.passes.clear();
        s.tracy_passes.clear();
        // Пока Tracy не подключён, он выбрасывает начала зон. Если отправить ему метки кадра,
        // начатого до подключения, он получит время зон, о которых не знает, и упадёт.
        // Поэтому зоны заводим только для кадров, начатых при подключённом Tracy.
        s.tracy = self.tracy.is_some() && tracy_client::Client::is_connected();
        if s.tracy
            && let Some(t) = &self.tracy
        {
            s.tracy_frame = t.span_alloc("кадр", TRACY_FUNCTION, file!(), line!()).ok();
        }
    }

    /// Проход `name` начинается (до его барьеров). Только для Tracy: метку ставит [`Self::pass_end`].
    pub fn pass_begin(&mut self, name: &'static str) {
        let s = &mut self.slots[self.current];
        if s.passes.len() >= MAX_TIMED_PASSES {
            return;
        }
        if s.tracy
            && let Some(t) = &self.tracy
        {
            s.tracy_passes
                .push(t.span_alloc(name, TRACY_FUNCTION, file!(), line!()).ok());
        }
    }

    /// Проход `name` записан: ставим метку его конца.
    ///
    /// # Safety
    /// Как у [`Self::record_start`], и метка начала кадра уже поставлена.
    pub unsafe fn pass_end(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        name: &'static str,
    ) {
        let s = &mut self.slots[self.current];
        if !s.started || s.passes.len() >= MAX_TIMED_PASSES {
            return;
        }
        s.passes.push(name);
        let query = self.current as u32 * QUERIES_PER_FRAME + s.passes.len() as u32;
        // SAFETY: условие функции; запрос сброшен в record_start.
        unsafe {
            device.cmd_write_timestamp2(
                cmd,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                self.pool,
                query,
            );
        }
        if let Some(Some(span)) = s.tracy_passes.last_mut() {
            span.end_zone();
        }
    }

    /// Все проходы кадра записаны.
    pub fn record_end(&mut self) {
        if let Some(span) = self.slots[self.current].tracy_frame.as_mut() {
            span.end_zone();
        }
    }

    /// Последний прочитанный замер (кадр, закончившийся последним).
    pub fn last(&self) -> Option<&GpuFrameTimings> {
        self.last.as_ref()
    }

    /// Уничтожает пул запросов. Вызывать, когда видеокарта простаивает.
    pub fn destroy(&mut self, ctx: &GpuContext) {
        // Незаконченные зоны Tracy закрываем без меток: кадров с ними уже не будет.
        self.slots.clear();
        // SAFETY: видеокарта простаивает (условие функции).
        unsafe { ctx.device.destroy_query_pool(self.pool, None) };
        self.pool = vk::QueryPool::null();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(scene: f64, present: f64) -> GpuFrameTimings {
        GpuFrameTimings {
            total_ms: scene + present,
            passes: vec![
                PassTiming {
                    name: "сцена",
                    ms: scene,
                },
                PassTiming {
                    name: "вывод",
                    ms: present,
                },
            ],
        }
    }

    #[test]
    fn average_keeps_pass_order() {
        let mut avg = GpuTimingAverage::default();
        assert!(avg.average().is_none());
        avg.add(&frame(1.0, 0.5));
        avg.add(&frame(3.0, 0.5));
        let a = avg.average().unwrap();
        assert_eq!(avg.frames(), 2);
        assert!((a.total_ms - 2.5).abs() < 1e-9);
        assert_eq!(a.passes[0].name, "сцена");
        assert!((a.passes[0].ms - 2.0).abs() < 1e-9);
        assert_eq!(a.passes[1].name, "вывод");
        // Проход только в одном из трёх кадров: вклад делится на все кадры.
        avg.add(&GpuFrameTimings {
            total_ms: 1.0,
            passes: vec![PassTiming {
                name: "сцена",
                ms: 1.0,
            }],
        });
        let a = avg.average().unwrap();
        let sum: f64 = a.passes.iter().map(|p| p.ms).sum();
        assert!((sum - a.total_ms).abs() < 1e-9);
        avg.clear();
        assert_eq!(avg.frames(), 0);
    }

    #[test]
    fn describe_lists_passes() {
        let s = frame(0.25, 0.125).describe();
        assert_eq!(s, "сцена 0.250 мс, вывод 0.125 мс (всего 0.375 мс)");
    }
}
