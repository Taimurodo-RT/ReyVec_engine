//! Мягкие тела Rapier 0.36: верёвка, желе, сетка. Частицы на пружинках, которые
//! сталкиваются с телами и уровнем; верёвку можно привязать к телу, сетку — порвать.

use rapier2d::prelude::*;

use super::{BodyId, Physics, p, v};

/// Форма мягкого тела.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SoftKind {
    /// Верёвка из `count` частиц от `from` до `to`.
    Rope {
        from: [f64; 2],
        to: [f64; 2],
        count: usize,
    },
    /// Круглое желе: `count` частиц по кругу радиусом `radius`, держит площадь.
    Jelly {
        center: [f64; 2],
        radius: f64,
        count: usize,
    },
    /// Сетка (полотно, мост из ткани) `size` с `cols` × `rows` частицами.
    Net {
        center: [f64; 2],
        size: [f64; 2],
        cols: usize,
        rows: usize,
    },
}

/// Какие частицы закреплены в воздухе.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pin {
    #[default]
    None,
    /// Первая частица верёвки.
    Start,
    /// Обе крайние частицы верёвки.
    Ends,
    /// Левый и правый край сетки.
    Sides,
    /// Верхний край сетки.
    Top,
}

/// Мягкое тело из файла сцены.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftDesc {
    pub kind: SoftKind,
    /// Жёсткость: частота дрожания пружинок, раз в секунду (больше — твёрже).
    pub stiffness: f64,
    /// Гашение дрожания: 0 — дрожит долго, 1 — почти сразу успокаивается.
    pub damping: f64,
    /// Масса всего тела.
    pub mass: f64,
    pub pin: Pin,
    /// Рвётся, если растянуть больше чем на эту долю длины (0,5 — на половину).
    pub tear: Option<f64>,
    /// Привязать первую и последнюю частицу к телам (груз на верёвке).
    pub attach_start: Option<BodyId>,
    pub attach_end: Option<BodyId>,
    /// Вид для рисования: его выбирает тот, кто рисует.
    pub style: u32,
}

impl SoftDesc {
    pub fn new(kind: SoftKind) -> Self {
        Self {
            kind,
            stiffness: 30.0,
            damping: 0.5,
            mass: 0.05,
            pin: Pin::None,
            tear: None,
            attach_start: None,
            attach_end: None,
            style: 0,
        }
    }
}

/// Номер мягкого тела.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SoftId(SoftBodyHandle);

/// Мягкое тело для рисования: частицы, отрезки между ними и толщина. Порванное тело —
/// несколько таких кусков с одним видом.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoftView {
    pub style: u32,
    pub points: Vec<[f64; 2]>,
    pub segments: Vec<[u32; 2]>,
    pub radius: f64,
    /// Желе: тело замкнутое, рисуется залитым.
    pub closed: bool,
}

/// Номера частиц сетки: столбец `i`, строка `j`.
fn net_index(rows: usize, i: usize, j: usize) -> u32 {
    (i * rows + j) as u32
}

impl Physics {
    /// Новое мягкое тело.
    pub fn add_soft(&mut self, d: &SoftDesc) -> SoftId {
        let soft = SpringCoefficients::new(d.stiffness.max(0.5) as f32, d.damping.max(0.0) as f32);
        let bend = SpringCoefficients::new(
            (d.stiffness * 0.3).max(0.5) as f32,
            d.damping.max(0.0) as f32,
        );
        let material = SoftBodyMaterial {
            edge_softness: soft,
            bend_softness: bend,
            volume_softness: soft,
            shape_matching_softness: bend,
            tear_strain: d.tear.map(|t| t as f32),
            ..SoftBodyMaterial::default()
        };
        let (builder, count) = match d.kind {
            SoftKind::Rope { from, to, count } => {
                let count = count.clamp(2, 400);
                let mut b = SoftBodyBuilder::rope(v(from), v(to), count);
                b = match d.pin {
                    Pin::Start => b.pinned_particles([0]),
                    Pin::Ends => b.pinned_particles([0, count as u32 - 1]),
                    _ => b,
                };
                (b, count)
            }
            SoftKind::Jelly {
                center,
                radius,
                count,
            } => {
                let count = count.clamp(3, 200);
                (
                    SoftBodyBuilder::disk(v(center), radius as f32, count),
                    count,
                )
            }
            SoftKind::Net {
                center,
                size,
                cols,
                rows,
            } => {
                let (cols, rows) = (cols.clamp(2, 64), rows.clamp(2, 64));
                let (b, n) = net(center, size, cols, rows);
                let pins: Vec<u32> = match d.pin {
                    Pin::Sides => (0..rows)
                        .flat_map(|j| [net_index(rows, 0, j), net_index(rows, cols - 1, j)])
                        .collect(),
                    Pin::Top => (0..cols).map(|i| net_index(rows, i, rows - 1)).collect(),
                    _ => Vec::new(),
                };
                (b.pinned_particles(pins), n)
            }
        };
        let builder = builder
            .material(material)
            .particle_mass((d.mass.max(1e-4) / count as f64) as f32)
            .linear_damping(0.2)
            .user_data(u128::from(d.style));
        let h = self
            .soft_bodies
            .insert(builder, &mut self.bodies, &mut self.colliders);
        if let Some(s) = self.soft_bodies.get_mut(h) {
            let last = s.num_particles().saturating_sub(1);
            if let Some(body) = d.attach_start {
                s.attach_particle(0, body.0, &self.bodies);
            }
            if let Some(body) = d.attach_end {
                s.attach_particle(last, body.0, &self.bodies);
            }
        }
        self.soft_styles.insert(h, d.style);
        SoftId(h)
    }

    /// Вид мягкого тела: свой или того тела, от которого оно оторвалось.
    fn soft_style(&self, mut h: SoftBodyHandle) -> u32 {
        for _ in 0..64 {
            if let Some(style) = self.soft_styles.get(&h) {
                return *style;
            }
            match self.soft_bodies.get(h).and_then(|s| s.origin()) {
                Some(parent) => h = parent,
                None => break,
            }
        }
        self.soft_bodies.get(h).map_or(0, |s| s.user_data as u32)
    }

    /// Все мягкие тела (и куски порванных) для рисования.
    pub fn soft_views(&self) -> Vec<SoftView> {
        self.soft_bodies
            .iter()
            .map(|(h, s)| {
                // Видимые стороны: у верёвки — её отрезки, у желе — край, у сетки —
                // стороны клеток (без диагоналей).
                let closed = s.volume_preservation_enabled();
                let segments = s.boundary().to_vec();
                SoftView {
                    style: self.soft_style(h),
                    points: s.particle_positions().map(p).collect(),
                    segments,
                    radius: f64::from(s.particle_radius()),
                    closed,
                }
            })
            .collect()
    }

    /// Середина мягкого тела (для проверок).
    pub fn soft_center(&self, id: SoftId) -> Option<[f64; 2]> {
        self.soft_bodies.get(id.0).map(|s| p(s.center_of_mass()))
    }

    /// Сколько кусков у мягкого тела (1 — целое; больше — порвано).
    pub fn soft_pieces(&self, id: SoftId) -> usize {
        let mut all = vec![id.0];
        let mut i = 0;
        while i < all.len() {
            if let Some(s) = self.soft_bodies.get(all[i]) {
                all.extend(s.pieces().iter().copied());
            }
            i += 1;
        }
        all.len()
    }
}

/// Сетка: частицы столбцами, пружинки по сторонам и диагоналям клеток, пружинки
/// изгиба через одну. Сталкивается всеми сторонами клеток.
fn net(center: [f64; 2], size: [f64; 2], cols: usize, rows: usize) -> (SoftBodyBuilder, usize) {
    let step = [
        size[0] / (cols - 1) as f64,
        size[1] / (rows - 1).max(1) as f64,
    ];
    let origin = [center[0] - size[0] * 0.5, center[1] - size[1] * 0.5];
    let idx = |i: usize, j: usize| net_index(rows, i, j);
    let mut points = Vec::with_capacity(cols * rows);
    for i in 0..cols {
        for j in 0..rows {
            points.push(v([
                origin[0] + step[0] * i as f64,
                origin[1] + step[1] * j as f64,
            ]));
        }
    }
    let mut edges = Vec::new();
    let mut surface = Vec::new();
    let mut bend = Vec::new();
    for i in 0..cols {
        for j in 0..rows {
            if i + 1 < cols {
                edges.push([idx(i, j), idx(i + 1, j)]);
                surface.push([idx(i, j), idx(i + 1, j)]);
            }
            if j + 1 < rows {
                edges.push([idx(i, j), idx(i, j + 1)]);
                surface.push([idx(i, j), idx(i, j + 1)]);
            }
            if i + 1 < cols && j + 1 < rows {
                edges.push([idx(i, j), idx(i + 1, j + 1)]);
                edges.push([idx(i + 1, j), idx(i, j + 1)]);
            }
            if i + 2 < cols {
                bend.push([idx(i, j), idx(i + 2, j)]);
            }
            if j + 2 < rows {
                bend.push([idx(i, j), idx(i, j + 2)]);
            }
        }
    }
    let radius = step[0].min(step[1]) * 0.5;
    let b = SoftBodyBuilder::new(points)
        .edges(edges)
        .bend_edges(bend)
        .surface(surface)
        .particle_radius(radius as f32);
    (b, cols * rows)
}
