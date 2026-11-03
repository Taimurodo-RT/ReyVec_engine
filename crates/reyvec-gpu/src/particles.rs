//! Частицы на видеокарте (6.6): пыль, искры, брызги.
//!
//! Частицы лежат в кольцевом буфере на видеокарте ([`CAPACITY`] мест). Процессор только
//! пишет новые частицы (не больше [`MAX_EMIT`] за кадр) и список твёрдого уровня в
//! буфер загрузки кадра; дальше всё делает видеокарта: вычислительный шейдер
//! (`shaders/particles.slang`, `cs_main`) двигает живые частицы, а рисование берёт их
//! прямо из того же буфера. Каждый кадр видеокарта трогает только живой отрезок кольца:
//! от самой старой ещё живой частицы до последней новой, поэтому мало частиц — мало
//! работы.
//!
//! Твёрдое для отскока — те же прямоугольники и склоны, на которых стоит герой
//! (`Collider` логики), только видимые на экране, не больше [`MAX_SOLIDS`].
//!
//! [`step_particle`] — зеркало шейдера на процессоре: тесты проверяют на нём отскок,
//! склоны и платформы, а кадры в CI — что видеокарта делает то же.

use std::collections::VecDeque;

use ash::vk;

use crate::GpuError;
use crate::resources::{BufferId, BufferMemory, GpuResources};

/// Мест в кольце частиц.
pub const CAPACITY: u32 = 1 << 17;
/// Сколько новых частиц можно добавить за один кадр.
pub const MAX_EMIT: usize = 16_384;
/// Сколько твёрдых прямоугольников видит расчёт за кадр.
pub const MAX_SOLIDS: usize = 128;
/// Размер частицы в буфере (как `struct Particle` в шейдере).
pub(crate) const PARTICLE_SIZE: usize = 64;
const SOLID_SIZE: usize = 32;
/// Где в буфере загрузки начинается твёрдое.
const SOLIDS_OFFSET: usize = MAX_EMIT * PARTICLE_SIZE;
const UPLOAD_SIZE: usize = SOLIDS_OFFSET + MAX_SOLIDS * SOLID_SIZE;

/// Тяжесть для частиц, единиц сцены в секунду за секунду (около тяжести героя).
pub const GRAVITY: f32 = 8.0;

/// Отскакивает от твёрдого уровня.
pub const PARTICLE_COLLIDE: u32 = 1;
/// К концу жизни уменьшается.
pub const PARTICLE_SHRINK: u32 = 2;
/// Светится сама: не освещается уровнем, рисуется поверх освещённого кадра.
pub const PARTICLE_GLOW: u32 = 4;
/// Размер постоянных шейдера частиц (как `struct Push` в шейдере).
pub(crate) const PUSH_SIZE: usize = 76;

/// Частица: где, куда летит, сколько живёт и как выглядит.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub pos: [f32; 2],
    pub vel: [f32; 2],
    pub life: f32,
    pub max_life: f32,
    /// Поперечник кружка, единицы сцены.
    pub size: f32,
    /// Доля тяжести сцены.
    pub gravity: f32,
    /// Линейный цвет (может быть ярче 1) и непрозрачность.
    pub color: [f32; 4],
    /// Торможение о воздух: доля скорости за секунду.
    pub drag: f32,
    /// Отскок: доля скорости поперёк поверхности.
    pub bounce: f32,
    pub flags: u32,
}

impl Particle {
    fn to_bytes(self) -> [u8; PARTICLE_SIZE] {
        let mut b = [0u8; PARTICLE_SIZE];
        let floats = [
            self.pos[0],
            self.pos[1],
            self.vel[0],
            self.vel[1],
            self.life,
            self.max_life,
            self.size,
            self.gravity,
            self.color[0],
            self.color[1],
            self.color[2],
            self.color[3],
            self.drag,
            self.bounce,
        ];
        for (i, v) in floats.into_iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
        }
        b[56..60].copy_from_slice(&self.flags.to_ne_bytes());
        b
    }
}

/// Твёрдое для отскока: прямоугольник, у склона ещё подъём слева направо.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleSolid {
    pub lo: [f32; 2],
    pub hi: [f32; 2],
    pub slope: f32,
    pub oneway: bool,
}

impl ParticleSolid {
    fn to_bytes(self) -> [u8; SOLID_SIZE] {
        let mut b = [0u8; SOLID_SIZE];
        for (i, v) in [self.lo[0], self.lo[1], self.hi[0], self.hi[1], self.slope]
            .into_iter()
            .enumerate()
        {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
        }
        b[20..24].copy_from_slice(&u32::from(self.oneway).to_ne_bytes());
        b
    }

    #[cfg(test)]
    fn surface(&self, x: f32) -> f32 {
        let k = ((x - self.lo[0]) / (self.hi[0] - self.lo[0]).max(1e-6)).clamp(0.0, 1.0);
        let k = if self.slope < 0.0 { 1.0 - k } else { k };
        self.hi[1] - self.slope.abs() * (1.0 - k)
    }
}

#[cfg(test)]
const EPS: f32 = 1e-4;

/// Шаг одной частицы на процессоре: то же, что `step_particle` в шейдере.
#[cfg(test)]
pub fn step_particle(mut p: Particle, solids: &[ParticleSolid], gravity: f32, dt: f32) -> Particle {
    if p.life <= 0.0 {
        return p;
    }
    p.life -= dt;
    p.vel[1] -= gravity * p.gravity * dt;
    let keep = (1.0 - p.drag * dt).max(0.0);
    p.vel = [p.vel[0] * keep, p.vel[1] * keep];
    let from = p.pos;
    let mut to = [p.pos[0] + p.vel[0] * dt, p.pos[1] + p.vel[1] * dt];
    if p.flags & PARTICLE_COLLIDE != 0 {
        for s in solids {
            if to[0] <= s.lo[0] || to[0] >= s.hi[0] || to[1] <= s.lo[1] || to[1] >= s.hi[1] {
                continue;
            }
            if s.slope != 0.0 {
                let top = s.surface(to[0]);
                if to[1] >= top || from[1] < s.surface(from[0]) - EPS {
                    continue;
                }
                let nx = if s.slope > 0.0 {
                    -s.slope.abs()
                } else {
                    s.slope.abs()
                };
                let ny = s.hi[0] - s.lo[0];
                let len = nx.hypot(ny);
                let n = [nx / len, ny / len];
                to[1] = top + EPS;
                let vn = p.vel[0] * n[0] + p.vel[1] * n[1];
                if vn < 0.0 {
                    p.vel[0] -= (1.0 + p.bounce) * vn * n[0];
                    p.vel[1] -= (1.0 + p.bounce) * vn * n[1];
                }
                p.vel = [p.vel[0] * 0.8, p.vel[1] * 0.8];
                continue;
            }
            if s.oneway {
                if from[1] < s.hi[1] - EPS || p.vel[1] > 0.0 {
                    continue;
                }
                to[1] = s.hi[1] + EPS;
                p.vel[1] = -p.vel[1] * p.bounce;
                p.vel[0] *= 0.8;
                continue;
            }
            if from[1] >= s.hi[1] - EPS {
                to[1] = s.hi[1] + EPS;
                p.vel[1] = -p.vel[1] * p.bounce;
                p.vel[0] *= 0.8;
            } else if from[1] <= s.lo[1] + EPS {
                to[1] = s.lo[1] - EPS;
                p.vel[1] = -p.vel[1] * p.bounce;
            } else if from[0] <= s.lo[0] + EPS {
                to[0] = s.lo[0] - EPS;
                p.vel[0] = -p.vel[0] * p.bounce;
            } else if from[0] >= s.hi[0] - EPS {
                to[0] = s.hi[0] + EPS;
                p.vel[0] = -p.vel[0] * p.bounce;
            } else {
                p.life = 0.0;
            }
        }
    }
    p.pos = to;
    p
}

/// Какую часть кольца трогать в этом кадре.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RingSpan {
    /// Первое место живого отрезка.
    pub start: u32,
    /// Мест от `start` (живые и новые).
    pub count: u32,
    /// Куда встаёт первая новая частица.
    pub head: u32,
    pub emit_count: u32,
}

/// Учёт кольца на процессоре: где голова и с какого места ещё могут быть живые
/// частицы. Сами частицы процессор не видит, поэтому считает по самой долгой жизни
/// каждой порции: порция умерла, когда прошло её время. Порции идут по порядку; если
/// поздняя умирает раньше ранней, отрезок просто чуть длиннее нужного (мёртвые
/// частицы видеокарта пропускает).
#[derive(Debug, Default)]
pub(crate) struct Ring {
    /// Сколько частиц добавлено всего.
    total: u64,
    /// Порции: с какого номера, до какого (не включая) и когда умрёт последняя.
    batches: VecDeque<(u64, u64, f64)>,
}

impl Ring {
    /// Кадр в момент `now`: добавить `emit` новых частиц, самая долгая живёт `life`.
    pub fn frame(&mut self, now: f64, emit: u32, life: f32) -> RingSpan {
        let cap = u64::from(CAPACITY);
        while self
            .batches
            .front()
            .is_some_and(|&(_, _, dies)| dies <= now)
        {
            self.batches.pop_front();
        }
        let head_total = self.total;
        if emit > 0 {
            self.total += u64::from(emit);
            self.batches
                .push_back((head_total, self.total, now + f64::from(life)));
        }
        // Кольцо перезаписывает самые старые, если новых больше, чем мест.
        let oldest = self
            .batches
            .front()
            .map_or(self.total, |&(start, _, _)| start)
            .max(self.total.saturating_sub(cap));
        RingSpan {
            start: (oldest % cap) as u32,
            count: (self.total - oldest) as u32,
            head: (head_total % cap) as u32,
            emit_count: emit,
        }
    }
}

/// Что нужно видеокарте для частиц в этом кадре.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ParticleFrame {
    pub span: RingSpan,
    pub ring: vk::DeviceAddress,
    pub emitted: vk::DeviceAddress,
    pub solids: vk::DeviceAddress,
    pub solid_count: u32,
    pub dt: f32,
    pub gravity: f32,
}

/// Частицы на видеокарте: кольцо и буферы загрузки по кадрам.
pub(crate) struct ParticleSystem {
    ring_buffer: BufferId,
    uploads: Vec<BufferId>,
    ring: Ring,
    /// Сколько частиц не влезло в кадр (больше [`MAX_EMIT`]) за всё время.
    pub dropped: u64,
}

impl ParticleSystem {
    pub fn new(
        ctx: &crate::context::GpuContext,
        res: &mut GpuResources,
        frames_in_flight: usize,
    ) -> Result<Self, GpuError> {
        let ring_buffer = res.create_buffer(
            ctx,
            u64::from(CAPACITY) * PARTICLE_SIZE as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            BufferMemory::GpuOnly,
            "частицы",
        )?;
        let mut uploads = Vec::new();
        for i in 0..frames_in_flight {
            uploads.push(res.create_buffer(
                ctx,
                UPLOAD_SIZE as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                BufferMemory::CpuToGpu,
                &format!("новые частицы кадра {i}"),
            )?);
        }
        Ok(Self {
            ring_buffer,
            uploads,
            ring: Ring::default(),
            dropped: 0,
        })
    }

    /// Готовит шаги расчёта этого кадра: по шагу на каждый элемент `steps` (момент,
    /// длина шага и новые частицы этого шага). Пишет новые частицы и твёрдое в буфер загрузки кадра `slot`, который
    /// видеокарта в этот момент читать не должна.
    pub fn prepare(
        &mut self,
        res: &mut GpuResources,
        slot: usize,
        steps: Vec<(f64, f32, Vec<Particle>)>,
        solids: &[ParticleSolid],
        gravity: f32,
    ) -> Vec<ParticleFrame> {
        let upload = self.uploads[slot % self.uploads.len()];
        let base = res.buffer_address(upload);
        let solid_count = solids.len().min(MAX_SOLIDS);
        if solid_count > 0 {
            let bytes: Vec<u8> = solids[..solid_count]
                .iter()
                .flat_map(|s| s.to_bytes())
                .collect();
            res.write_buffer(upload, SOLIDS_OFFSET, &bytes);
        }
        let ring = res.buffer_address(self.ring_buffer);
        let mut offset = 0usize;
        let mut frames = Vec::with_capacity(steps.len());
        for (now, dt, mut emitted) in steps {
            let room = MAX_EMIT - offset;
            if emitted.len() > room {
                self.dropped += (emitted.len() - room) as u64;
                emitted.truncate(room);
            }
            let life = emitted.iter().map(|p| p.life).fold(0.0, f32::max);
            let span = self.ring.frame(now, emitted.len() as u32, life);
            if !emitted.is_empty() {
                let bytes: Vec<u8> = emitted.iter().flat_map(|p| p.to_bytes()).collect();
                res.write_buffer(upload, offset * PARTICLE_SIZE, &bytes);
            }
            frames.push(ParticleFrame {
                span,
                ring,
                emitted: base + (offset * PARTICLE_SIZE) as u64,
                solids: base + SOLIDS_OFFSET as u64,
                solid_count: solid_count as u32,
                dt,
                gravity,
            });
            offset += emitted.len();
        }
        frames
    }
}

/// Постоянные шейдера частиц (как `struct Push` в `particles.slang`).
pub(crate) fn push(
    f: &ParticleFrame,
    center: [f32; 2],
    aspect: f32,
    zoom: f32,
    glow_pass: bool,
) -> [u8; PUSH_SIZE] {
    let mut b = [0u8; PUSH_SIZE];
    b[72..76].copy_from_slice(&u32::from(glow_pass).to_ne_bytes());
    b[0..8].copy_from_slice(&f.ring.to_ne_bytes());
    b[8..16].copy_from_slice(&f.emitted.to_ne_bytes());
    b[16..24].copy_from_slice(&f.solids.to_ne_bytes());
    let words = [
        CAPACITY,
        f.span.start,
        f.span.count,
        f.span.head,
        f.span.emit_count,
        f.solid_count,
    ];
    for (i, w) in words.into_iter().enumerate() {
        b[24 + i * 4..28 + i * 4].copy_from_slice(&w.to_ne_bytes());
    }
    let floats = [f.dt, f.gravity, center[0], center[1], aspect, zoom];
    for (i, v) in floats.into_iter().enumerate() {
        b[48 + i * 4..52 + i * 4].copy_from_slice(&v.to_ne_bytes());
    }
    b
}

/// Расчёт частиц: барьер после рисования прошлого кадра, шейдер `cs_main`, барьер
/// перед рисованием.
///
/// # Safety
/// `cmd` в состоянии записи вне прохода рисования; буферы кадра живы, пока он выполняется.
pub(crate) unsafe fn record_update(
    d: &ash::Device,
    cmd: vk::CommandBuffer,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    push: &[u8; PUSH_SIZE],
    count: u32,
) {
    // До: прошлое рисование (или прошлый шаг расчёта) с кольцом закончило.
    let before = [vk::MemoryBarrier2::default()
        .src_stage_mask(
            vk::PipelineStageFlags2::VERTEX_SHADER | vk::PipelineStageFlags2::COMPUTE_SHADER,
        )
        .src_access_mask(
            vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
        )
        .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .dst_access_mask(
            vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
        )];
    let after = [vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::VERTEX_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_READ)];
    // SAFETY: условие функции.
    unsafe {
        d.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&before));
        if count > 0 {
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
            d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::ALL, 0, push);
            d.cmd_dispatch(cmd, count.div_ceil(64), 1, 1);
        }
        d.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&after));
    }
}

/// Сколько частиц сейчас в работе и сколько не влезло.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParticleStats {
    /// Мест кольца, которые видеокарта трогает в этом кадре (живые и только что умершие).
    pub live: u32,
    pub dropped: u64,
    /// Включён фонтан для замера (100 000 частиц).
    pub fountain: bool,
}

/// Простой повторяемый случайный ряд (splitmix64): одно событие — одни и те же частицы.
pub(crate) struct Rand(u64);

impl Rand {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    /// Число от 0 до 1.
    pub fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Число от `a` до `b`.
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

/// Пыль: серо-бурые клочки, медленно оседают, ложатся на пол.
fn dust(at: [f32; 2], vel: [f32; 2], r: &mut Rand) -> Particle {
    let life = r.range(0.35, 0.7);
    let shade = r.range(0.8, 1.1);
    Particle {
        pos: at,
        vel,
        life,
        max_life: life,
        size: r.range(0.03, 0.06),
        gravity: 0.25,
        // Светлее пола: в тёмной комнате бурая пыль на буром полу не видна.
        color: [0.62 * shade, 0.56 * shade, 0.48 * shade, 0.5],
        drag: 3.0,
        bounce: 0.1,
        flags: PARTICLE_COLLIDE | PARTICLE_SHRINK,
    }
}

/// Частицы события игры. `seed` делает их повторяемыми (номер шага и события).
pub(crate) fn burst(event: &reyvec_scene::GameEvent, seed: u64) -> Vec<Particle> {
    use reyvec_scene::GameEvent as E;
    let mut r = Rand::new(seed);
    let mut out = Vec::new();
    match *event {
        E::Landed { at, speed } => {
            let n = (4.0 + speed * 4.0).min(24.0) as usize;
            let push = (0.3 + speed * 0.25).min(1.4);
            for i in 0..n {
                let side = if i % 2 == 0 { 1.0 } else { -1.0 };
                let v = [side * r.range(0.3, 1.0) * push, r.range(0.05, 0.35)];
                let p = [at[0] + side * r.range(0.0, 0.05), at[1] + 0.01];
                out.push(dust(p, v, &mut r));
            }
        }
        E::Jumped { at } => {
            for i in 0..5 {
                let side = if i % 2 == 0 { 1.0 } else { -1.0 };
                let v = [side * r.range(0.2, 0.6), r.range(0.0, 0.2)];
                out.push(dust([at[0], at[1] + 0.01], v, &mut r));
            }
        }
        E::Run { at, dir } => {
            for _ in 0..3 {
                let v = [-dir * r.range(0.2, 0.6), r.range(0.1, 0.4)];
                out.push(dust([at[0] - dir * 0.03, at[1] + 0.01], v, &mut r));
            }
        }
        E::Dashed { at, dir } => {
            for _ in 0..10 {
                let v = [-dir * r.range(0.4, 1.2), r.range(-0.15, 0.15)];
                let p = [at[0], at[1] + r.range(-0.1, 0.1)];
                let mut d = dust(p, v, &mut r);
                d.gravity = 0.05;
                d.color[3] = 0.4;
                out.push(d);
            }
        }
        // Взмах слышно, но пыли он не поднимает; число урона рисует интерфейс.
        E::Swing { .. } | E::Damage { .. } => {}
        E::Hurt { at, dir, .. } => {
            // Красные брызги от героя в сторону отброса.
            for _ in 0..14 {
                let v = [dir * r.range(0.3, 1.4), r.range(0.2, 1.4)];
                let life = r.range(0.25, 0.5);
                out.push(Particle {
                    pos: at,
                    vel: v,
                    life,
                    max_life: life,
                    size: r.range(0.016, 0.028),
                    gravity: 0.9,
                    color: [3.0, 0.25, 0.2, 1.0],
                    drag: 1.5,
                    bounce: 0.2,
                    flags: PARTICLE_SHRINK | PARTICLE_GLOW,
                });
            }
        }
        E::Died { at } => {
            // Облако пыли кольцом.
            for i in 0..24 {
                let angle = std::f32::consts::TAU * i as f32 / 24.0;
                let speed = r.range(0.4, 1.0);
                let v = [speed * angle.cos(), speed * angle.sin()];
                let mut d = dust(at, v, &mut r);
                d.gravity = 0.1;
                out.push(d);
            }
        }
        E::Pickup { at } => {
            // Золотые блёстки кольцом вверх, медленно гаснут.
            for i in 0..16 {
                let angle = std::f32::consts::TAU * i as f32 / 16.0 + r.range(-0.1, 0.1);
                let speed = r.range(0.4, 0.9);
                let v = [speed * angle.cos(), speed * angle.sin() + 0.3];
                let life = r.range(0.4, 0.7);
                out.push(Particle {
                    pos: at,
                    vel: v,
                    life,
                    max_life: life,
                    size: r.range(0.014, 0.024),
                    gravity: 0.2,
                    color: [3.2, 2.4, 0.6, 1.0],
                    drag: 2.5,
                    bounce: 0.3,
                    flags: PARTICLE_SHRINK | PARTICLE_GLOW,
                });
            }
        }
        E::Hit { at, dir } => {
            // Искры: ярче белого (их подхватывает свечение), летят веером от удара.
            for i in 0..28 {
                let angle = r.range(-1.0, 1.1);
                let speed = r.range(1.2, 3.4);
                let v = [dir * speed * angle.cos(), speed * angle.sin() + 0.4];
                let life = r.range(0.2, 0.5);
                let hot = i % 5 == 0;
                out.push(Particle {
                    pos: at,
                    vel: v,
                    life,
                    max_life: life,
                    size: r.range(0.018, 0.032),
                    gravity: 0.7,
                    color: if hot {
                        [5.0, 3.6, 1.2, 1.0]
                    } else {
                        // Оранжевые: при сведении к экрану не выгорают в белое.
                        [3.0, 0.9, 0.15, 1.0]
                    },
                    drag: 2.0,
                    bounce: 0.45,
                    flags: PARTICLE_COLLIDE | PARTICLE_SHRINK | PARTICLE_GLOW,
                });
            }
        }
    }
    out
}

/// Фонтан для замера: столько частиц за шаг `dt`, чтобы живых было около 100 000.
pub(crate) fn fountain(at: [f32; 2], dt: f32, seed: u64) -> Vec<Particle> {
    const LIFE: f32 = 2.0;
    const ALIVE: f32 = 100_000.0;
    let n = ((ALIVE / LIFE) * dt).round() as usize;
    let mut r = Rand::new(seed);
    (0..n.min(MAX_EMIT))
        .map(|_| {
            let a = r.range(-0.6, 0.6);
            let speed = r.range(2.0, 4.0);
            let hue = r.next();
            Particle {
                pos: at,
                vel: [speed * a.sin(), speed * a.cos()],
                life: LIFE,
                max_life: LIFE,
                size: 0.012,
                gravity: 1.0,
                color: [0.4 + 1.2 * hue, 0.5 + 0.6 * (1.0 - hue), 1.4 - hue, 0.9],
                drag: 0.2,
                bounce: 0.5,
                flags: PARTICLE_COLLIDE,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dust(pos: [f32; 2], vel: [f32; 2]) -> Particle {
        Particle {
            pos,
            vel,
            life: 2.0,
            max_life: 2.0,
            size: 0.02,
            gravity: 1.0,
            color: [1.0; 4],
            drag: 0.0,
            bounce: 0.5,
            flags: PARTICLE_COLLIDE,
        }
    }

    fn floor() -> ParticleSolid {
        ParticleSolid {
            lo: [-5.0, -1.0],
            hi: [5.0, 0.0],
            slope: 0.0,
            oneway: false,
        }
    }

    fn run(mut p: Particle, solids: &[ParticleSolid], steps: usize) -> Particle {
        for _ in 0..steps {
            p = step_particle(p, solids, 9.8, 1.0 / 60.0);
        }
        p
    }

    #[test]
    fn particle_falls_bounces_lower_and_rests_on_the_floor() {
        let solids = [floor()];
        let mut p = dust([0.0, 0.5], [0.0, 0.0]);
        let mut bounces = 0;
        let mut tops = Vec::new();
        let mut rising = false;
        for _ in 0..240 {
            let before = p.vel[1];
            p = step_particle(p, &solids, 9.8, 1.0 / 60.0);
            assert!(p.pos[1] >= 0.0, "в полу: {p:?}");
            if before < 0.0 && p.vel[1] > 0.0 {
                bounces += 1;
                rising = true;
            }
            if rising && p.vel[1] <= 0.0 {
                tops.push(p.pos[1]);
                rising = false;
            }
        }
        assert!(bounces >= 2, "отскоков {bounces}");
        assert!(
            tops.windows(2).all(|w| w[1] < w[0]),
            "каждый отскок ниже: {tops:?}"
        );
        assert!(
            tops[0] < 0.5 * 0.5,
            "первый отскок ниже четверти высоты: {tops:?}"
        );
        assert!(p.pos[1] < 0.01, "легла на пол: {p:?}");
    }

    #[test]
    fn particle_hits_wall_from_the_side() {
        let wall = ParticleSolid {
            lo: [1.0, -1.0],
            hi: [1.2, 2.0],
            slope: 0.0,
            oneway: false,
        };
        let mut p = dust([0.5, 1.0], [3.0, 0.0]);
        p.gravity = 0.0;
        let p = run(p, &[wall], 30);
        assert!(p.pos[0] < 1.0, "не прошла стену: {p:?}");
        assert!(p.vel[0] < 0.0, "отскочила назад: {p:?}");
    }

    #[test]
    fn platform_holds_from_above_only() {
        let plank = ParticleSolid {
            lo: [-1.0, 0.0],
            hi: [1.0, 0.05],
            slope: 0.0,
            oneway: true,
        };
        // Снизу пролетает сквозь.
        let mut up = dust([0.0, -0.3], [0.0, 3.0]);
        up.gravity = 0.0;
        let up = run(up, &[plank], 20);
        assert!(up.pos[1] > 0.5, "пролетела снизу: {up:?}");
        // Сверху ложится.
        let down = run(dust([0.0, 0.4], [0.0, 0.0]), &[plank], 120);
        assert!(
            (down.pos[1] - 0.05).abs() < 0.01,
            "лежит на доске: {down:?}"
        );
    }

    #[test]
    fn particle_slides_down_a_slope_and_stays_above_it() {
        // Склон поднимается слева направо от 0 до 1 на ширине 1.
        let slope = ParticleSolid {
            lo: [0.0, -1.0],
            hi: [1.0, 1.0],
            slope: 1.0,
            oneway: false,
        };
        let s = slope;
        let mut p = dust([0.7, 1.2], [0.0, 0.0]);
        p.bounce = 0.0;
        for _ in 0..120 {
            p = step_particle(p, &[s, floor()], 9.8, 1.0 / 60.0);
            if p.pos[0] > 0.0 && p.pos[0] < 1.0 {
                assert!(p.pos[1] >= s.surface(p.pos[0]) - 1e-3, "под склоном: {p:?}");
            }
        }
        assert!(p.pos[0] < 0.7, "скатилась влево: {p:?}");
    }

    #[test]
    fn dead_particles_stay_dead_and_life_runs_out() {
        let p = run(dust([0.0, 3.0], [0.0, 0.0]), &[], 130);
        assert!(p.life <= 0.0);
        let q = run(p, &[], 10);
        assert_eq!(p, q);
    }

    #[test]
    fn ring_span_covers_alive_batches_only() {
        let mut ring = Ring::default();
        let a = ring.frame(0.0, 100, 1.0);
        assert_eq!((a.start, a.count, a.head, a.emit_count), (0, 100, 0, 100));
        let b = ring.frame(0.5, 50, 0.2);
        assert_eq!((b.start, b.count, b.head), (0, 150, 100));
        // Первая порция умерла в 1,0, вторая в 0,7.
        let c = ring.frame(1.0, 0, 0.0);
        assert_eq!(c.count, 0, "всё умерло: {c:?}");
        let d = ring.frame(1.1, 10, 1.0);
        assert_eq!((d.start, d.count, d.head), (150, 10, 150));
    }

    #[test]
    fn ring_wraps_and_never_spans_more_than_capacity() {
        let mut ring = Ring::default();
        let mut last = RingSpan::default();
        for i in 0..20 {
            last = ring.frame(f64::from(i) * 0.016, MAX_EMIT as u32, 10.0);
            assert!(last.count <= CAPACITY);
        }
        assert_eq!(last.count, CAPACITY);
        let total = 20 * MAX_EMIT as u64;
        assert_eq!(
            u64::from(last.head),
            (total - MAX_EMIT as u64) % u64::from(CAPACITY)
        );
        assert_eq!(
            u64::from(last.start),
            (total - u64::from(CAPACITY)) % u64::from(CAPACITY)
        );
    }

    #[test]
    fn events_give_repeatable_bursts() {
        use reyvec_scene::GameEvent;
        let hit = GameEvent::Hit {
            at: [1.0, 0.0],
            dir: -1.0,
        };
        let a = burst(&hit, 7);
        assert_eq!(a, burst(&hit, 7), "одно событие — одни и те же частицы");
        assert_ne!(a, burst(&hit, 8));
        // Искры летят в сторону удара и ярче белого.
        let left = a.iter().filter(|p| p.vel[0] < 0.0).count();
        assert!(left > a.len() * 3 / 4, "{left} из {}", a.len());
        assert!(a.iter().all(|p| p.color[0] > 1.0));
        let soft = burst(
            &GameEvent::Landed {
                at: [0.0, 0.0],
                speed: 0.5,
            },
            1,
        );
        let hard = burst(
            &GameEvent::Landed {
                at: [0.0, 0.0],
                speed: 4.0,
            },
            1,
        );
        assert!(hard.len() > soft.len(), "сильнее упал — больше пыли");
    }

    #[test]
    fn fountain_keeps_about_a_hundred_thousand_alive() {
        let per_second: usize = (0..60)
            .map(|i| fountain([0.0, 0.0], 1.0 / 60.0, i).len())
            .sum();
        assert!((49_000..=51_000).contains(&per_second), "{per_second}");
    }

    #[test]
    fn bytes_match_the_shader_layout() {
        let p = dust([1.0, 2.0], [3.0, 4.0]);
        let b = p.to_bytes();
        let f = |o: usize| f32::from_ne_bytes(b[o..o + 4].try_into().unwrap());
        assert_eq!((f(0), f(4), f(8), f(12)), (1.0, 2.0, 3.0, 4.0));
        assert_eq!((f(16), f(24), f(28)), (2.0, 0.02, 1.0));
        assert_eq!(f(32), 1.0);
        assert_eq!(f(52), 0.5);
        assert_eq!(
            u32::from_ne_bytes(b[56..60].try_into().unwrap()),
            PARTICLE_COLLIDE
        );
    }
}
