//! Запекание поля уровня (7.18в, `docs/decisions/0020-level-field.md`): неподвижные
//! пластины фигур и бруски твёрдого 2.5D-вида сводятся в одно [`LayerField`] — минимум
//! их 2D-расстояний на сетке, обрезанный полосой `B`. Первый слой — пластины (с номером
//! материала в каждой точке), второй — твёрдое. Считается в потоке загрузки, несколькими
//! потоками по плиткам сетки; на видеокарте это одна форма вместо сотни.
//!
//! Расстояние до каждой фигуры — то же, что у её формы в режиме SDF: прямоугольник
//! точно, контур — по его полю ([`Profile::distance`]), место — сдвиг, поворот, общий
//! размер и отражение (как у `Shape3::Move`). Поэтому поле отличается от форм только
//! клеткой сетки и усечением до half.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use glam::Vec2;

use crate::layer_field::{LayerField, SecondLayer, f32_to_half_toward_zero};
use crate::profile2d::Profile;

/// Посчитанная плитка: её номер, байты значений и байты номеров материалов.
type TileBytes = (usize, Vec<u8>, Vec<u8>);

/// Сторона плитки сетки, которую считает один поток за раз (точек).
const TILE: u32 = 64;
/// Запас сетки за полосой, точек: линейное смешивание у края читает соседей.
const MARGIN_CELLS: f32 = 2.0;

/// Форма фигуры на плоскости в её собственных координатах.
#[derive(Clone, Debug)]
pub enum Flat2 {
    /// Прямоугольник по осям: середина и половина размера.
    Rect { center: Vec2, half: Vec2 },
    /// Любой контур — по его полю.
    Profile(Profile),
}

impl Flat2 {
    fn distance(&self, u: Vec2) -> f32 {
        match self {
            Self::Rect { center, half } => {
                let d = (u - *center).abs() - *half;
                d.max(Vec2::ZERO).length() + d.max_element().min(0.0)
            }
            Self::Profile(profile) => profile.distance(u),
        }
    }

    fn bounds(&self) -> (Vec2, Vec2) {
        match self {
            Self::Rect { center, half } => (*center - *half, *center + *half),
            Self::Profile(profile) => profile.bounds(),
        }
    }
}

/// Фигура на месте: в мире точка `p` фигуры — `at + R(angle)·(±x, y)·scale` (отражение
/// по X до поворота, как у пластины 2.5D-вида).
#[derive(Clone, Debug)]
pub struct Placed2 {
    pub flat: Flat2,
    pub at: Vec2,
    /// Поворот, градусы против часовой стрелки.
    pub angle: f32,
    /// Общий размер (больше нуля).
    pub scale: f32,
    pub mirror: bool,
    /// Номер материала (у второго слоя не нужен).
    pub material: u16,
}

impl Placed2 {
    /// Расстояние до фигуры в точке мира `p`.
    pub fn distance(&self, p: Vec2) -> f32 {
        let (sin, cos) = (-self.angle.to_radians()).sin_cos();
        let d = p - self.at;
        let mut q = Vec2::new(cos * d.x - sin * d.y, sin * d.x + cos * d.y);
        if self.mirror {
            q.x = -q.x;
        }
        self.flat.distance(q / self.scale) * self.scale
    }

    /// Прямоугольник фигуры в мире.
    pub fn world_bounds(&self) -> (Vec2, Vec2) {
        let (lo, hi) = self.flat.bounds();
        let (sin, cos) = self.angle.to_radians().sin_cos();
        let mut min = Vec2::splat(f32::INFINITY);
        let mut max = Vec2::splat(f32::NEG_INFINITY);
        for c in [lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)] {
            let mut c = c * self.scale;
            if self.mirror {
                c.x = -c.x;
            }
            let w = self.at + Vec2::new(cos * c.x - sin * c.y, sin * c.x + cos * c.y);
            min = min.min(w);
            max = max.max(w);
        }
        (min, max)
    }
}

/// Что запечь: фигуры двух слоев, шаг сетки и полоса.
#[derive(Clone, Debug)]
pub struct LayerBake {
    /// Шаг сетки, метры.
    pub cell: f32,
    /// Полоса `B`: дальше неё поле не знает.
    pub band: f32,
    /// Фигуры первого слоя в порядке рисования: где их несколько, материал — у
    /// нарисованной позже (она сверху), вне всех — у ближайшей.
    pub first: Vec<Placed2>,
    /// Фигуры второго слоя (твёрдое).
    pub second: Vec<Placed2>,
    /// Где по Z второй слой; `None` — второго слоя нет (там везде `+B`).
    pub second_layer: Option<SecondLayer>,
    /// Сколько потоков считают (не меньше одного).
    pub threads: usize,
    /// Поднятый флаг — запекание больше не нужно: [`LayerBake::run`] бросает работу.
    pub cancel: Arc<AtomicBool>,
}

/// Сколько точек и плиток посчитано (для панели и проверок).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BakeStats {
    pub points: usize,
    /// В среднем фигур на плитку (сколько перебирает каждая точка, не больше).
    pub per_tile: f32,
}

impl LayerBake {
    /// Прямоугольник сетки: все фигуры, расширенные на полосу и запас.
    pub fn extent(&self) -> Option<(Vec2, Vec2)> {
        self.extent_at(self.cell)
    }

    /// То же при шаге `cell`: запас в [`MARGIN_CELLS`] клеток зависит от шага.
    fn extent_at(&self, cell: f32) -> Option<(Vec2, Vec2)> {
        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        for p in self.first.iter().chain(&self.second) {
            let (a, b) = p.world_bounds();
            if a.is_finite() && b.is_finite() {
                lo = lo.min(a);
                hi = hi.max(b);
            }
        }
        if !(lo.x <= hi.x && lo.y <= hi.y) {
            return None;
        }
        let pad = Vec2::splat(self.band + MARGIN_CELLS * cell);
        Some((lo - pad, hi + pad))
    }

    /// Точек сетки по X и по Y при шаге `cell`.
    pub fn points(&self, cell: f32) -> Option<(u32, u32)> {
        let (lo, hi) = self.extent_at(cell)?;
        let n = |len: f32| ((len / cell).ceil() as u64 + 1).max(2);
        let (nx, ny) = (n(hi.x - lo.x), n(hi.y - lo.y));
        (nx <= u64::from(u32::MAX) && ny <= u64::from(u32::MAX)).then_some((nx as u32, ny as u32))
    }

    /// Самый мелкий из шагов `cells`, при котором сетка не больше `max_side` точек по
    /// каждой стороне; ставит его в `cell`. `false` — не влезает ни один.
    pub fn fit(&mut self, cells: &[f32], max_side: u32) -> bool {
        for &cell in cells {
            if let Some((nx, ny)) = self.points(cell)
                && nx <= max_side
                && ny <= max_side
            {
                self.cell = cell;
                return true;
            }
        }
        false
    }

    /// Запекает поле. `None` — фигур нет или работу отменили.
    pub fn run(&self) -> Option<(LayerField, BakeStats)> {
        let cell = self.cell;
        let (corner, _) = self.extent()?;
        let (nx, ny) = self.points(cell)?;
        let area = self.run_area(corner, cell, [0, 0, nx, ny])?;
        let mut field = LayerField::from_texels(
            corner,
            cell,
            nx,
            ny,
            self.band,
            None,
            area.texels,
            area.materials,
        );
        if let Some(second) = self.second_layer {
            field = field.with_second(second);
        }
        Some((field, area.stats))
    }

    /// Запекает кусок сетки с углом `corner` и шагом `cell`: точки `i0..i0 + w` по X и
    /// `j0..j0 + h` по Y (`area` = `[i0, j0, w, h]`), строки снизу вверх. Точка (i, j)
    /// стоит там же, где при запекании всей сетки, поэтому кусок, запечённый по тем же
    /// фигурам, совпадает с ней байт в байт (так поле перепекается на месте, 7.18г).
    /// `None` — работу отменили.
    pub fn run_area(&self, corner: Vec2, cell: f32, area: [u32; 4]) -> Option<BakedArea> {
        let band = self.band;
        let [i0, j0, nx, ny] = area;
        if nx == 0 || ny == 0 {
            return Some(BakedArea::default());
        }
        let (tx, ty) = (nx.div_ceil(TILE), ny.div_ceil(TILE));
        // Фигуры по плиткам: плитка видит фигуру, если её прямоугольник, расширенный на
        // полосу, задевает плитку.
        let tiles = (tx * ty) as usize;
        let mut first_bins: Vec<Vec<u32>> = vec![Vec::new(); tiles];
        let mut second_bins: Vec<Vec<u32>> = vec![Vec::new(); tiles];
        let span = TILE as f32 * cell;
        let origin = corner + Vec2::new(i0 as f32, j0 as f32) * cell;
        let bin = |list: &[Placed2], bins: &mut [Vec<u32>]| -> Vec<(Vec2, Vec2)> {
            let mut boxes = Vec::with_capacity(list.len());
            for (k, p) in list.iter().enumerate() {
                let (a, b) = p.world_bounds();
                let (a, b) = (a - Vec2::splat(band), b + Vec2::splat(band));
                boxes.push((a, b));
                if !(a.is_finite() && b.is_finite()) {
                    continue;
                }
                // Плитки куска, которые задевает прямоугольник; мимо куска — никакие.
                let (lo, hi) = (origin, origin + Vec2::new(nx as f32, ny as f32) * cell);
                if b.x < lo.x - cell || a.x > hi.x + cell || b.y < lo.y - cell || a.y > hi.y + cell
                {
                    continue;
                }
                // Точка (i, j) стоит в corner + (i + 0,5)·cell.
                let t = |v: f32, o: f32, n: u32| {
                    ((v - o) / span).floor().clamp(0.0, (n - 1) as f32) as u32
                };
                for j in t(a.y, origin.y, ty)..=t(b.y, origin.y, ty) {
                    for i in t(a.x, origin.x, tx)..=t(b.x, origin.x, tx) {
                        bins[(j * tx + i) as usize].push(k as u32);
                    }
                }
            }
            boxes
        };
        let first_boxes = bin(&self.first, &mut first_bins);
        let second_boxes = bin(&self.second, &mut second_bins);
        let listed: usize = first_bins.iter().chain(&second_bins).map(Vec::len).sum();

        let points = nx as usize * ny as usize;
        let mut texels = vec![0u8; points * 4];
        let mut materials = vec![0u8; points * 2];
        let plus_band = f32_to_half_toward_zero(band).to_le_bytes();
        let next = AtomicUsize::new(0);
        let threads = self.threads.max(1).min(tiles.max(1));
        // Плитки раздаются по одной; каждый поток пишет только свои точки, поэтому
        // результаты складываются после — так не нужны общие изменяемые массивы.
        let work = || {
            let mut done = Vec::new();
            loop {
                if self.cancel.load(Ordering::Relaxed) {
                    return done;
                }
                let t = next.fetch_add(1, Ordering::Relaxed);
                if t >= tiles {
                    return done;
                }
                let (ti, tj) = (t as u32 % tx, t as u32 / tx);
                let (w, h) = (TILE.min(nx - ti * TILE), TILE.min(ny - tj * TILE));
                let mut tex = Vec::with_capacity((w * h * 4) as usize);
                let mut mat = Vec::with_capacity((w * h * 2) as usize);
                for j in tj * TILE..tj * TILE + h {
                    for i in ti * TILE..ti * TILE + w {
                        let p =
                            corner + Vec2::new((i0 + i) as f32 + 0.5, (j0 + j) as f32 + 0.5) * cell;
                        let (d1, m) = nearest(&self.first, &first_boxes, &first_bins[t], p, band);
                        tex.extend(f32_to_half_toward_zero(d1.clamp(-band, band)).to_le_bytes());
                        if self.second_layer.is_some() {
                            let d2 =
                                nearest(&self.second, &second_boxes, &second_bins[t], p, band).0;
                            tex.extend(
                                f32_to_half_toward_zero(d2.clamp(-band, band)).to_le_bytes(),
                            );
                        } else {
                            tex.extend(plus_band);
                        }
                        mat.extend(m.to_le_bytes());
                    }
                }
                done.push((t, tex, mat));
            }
        };
        let results: Vec<Vec<TileBytes>> = if threads == 1 {
            // Маленький кусок (перепекание в кадре) — без запуска потоков.
            vec![work()]
        } else {
            std::thread::scope(|scope| {
                let workers: Vec<_> = (0..threads).map(|_| scope.spawn(work)).collect();
                workers
                    .into_iter()
                    .map(|w| w.join().unwrap_or_default())
                    .collect()
            })
        };
        if self.cancel.load(Ordering::Relaxed) {
            return None;
        }
        let mut filled = 0;
        for (t, tex, mat) in results.into_iter().flatten() {
            let (ti, tj) = (t as u32 % tx, t as u32 / tx);
            let w = TILE.min(nx - ti * TILE) as usize;
            for (row, (tr, mr)) in tex
                .chunks_exact(w * 4)
                .zip(mat.chunks_exact(w * 2))
                .enumerate()
            {
                let start =
                    (tj as usize * TILE as usize + row) * nx as usize + (ti * TILE) as usize;
                texels[start * 4..start * 4 + w * 4].copy_from_slice(tr);
                materials[start * 2..start * 2 + w * 2].copy_from_slice(mr);
            }
            filled += 1;
        }
        if filled != tiles {
            // Поток упал: кусок неполный, лучше без него.
            return None;
        }
        Some(BakedArea {
            texels,
            materials,
            stats: BakeStats {
                points,
                per_tile: listed as f32 / tiles.max(1) as f32,
            },
        })
    }
}

/// Запечённый кусок сетки: байты значений (R16G16_SFLOAT) и номеров материалов
/// (R16_UNORM), строки снизу вверх, плотно.
#[derive(Clone, Debug, Default)]
pub struct BakedArea {
    pub texels: Vec<u8>,
    pub materials: Vec<u8>,
    pub stats: BakeStats,
}

/// Расстояние до ближайшей из фигур `bin` в точке `p` (не дальше полосы) и материал:
/// у последней по порядку, внутри которой точка, а если такой нет — у ближайшей.
fn nearest(
    list: &[Placed2],
    boxes: &[(Vec2, Vec2)],
    bin: &[u32],
    p: Vec2,
    band: f32,
) -> (f32, u16) {
    let mut best = band;
    let mut material = None;
    let mut inside = None;
    for &k in bin {
        let (a, b) = boxes[k as usize];
        if p.x < a.x || p.y < a.y || p.x > b.x || p.y > b.y {
            continue;
        }
        let shape = &list[k as usize];
        let d = shape.distance(p);
        if d < best {
            best = d;
            material = Some(shape.material);
        }
        if d <= 0.0 {
            inside = Some(shape.material);
        }
    }
    (best, inside.or(material).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile2d::tests_support::{exact_distance, flatten_path};

    const STAR: &str = "M 0 1 L 0.22 0.3 L 0.95 0.31 L 0.36 -0.12 L 0.59 -0.81 \
                        L 0 -0.38 L -0.59 -0.81 L -0.36 -0.12 L -0.95 0.31 L -0.22 0.3 Z";

    fn xorshift(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        (*seed % 100_000) as f32 / 100_000.0
    }

    fn rect(center: [f32; 2], half: [f32; 2], material: u16) -> Placed2 {
        Placed2 {
            flat: Flat2::Rect {
                center: Vec2::ZERO,
                half: Vec2::from(half),
            },
            at: Vec2::from(center),
            angle: 0.0,
            scale: 1.0,
            mirror: false,
            material,
        }
    }

    fn job(first: Vec<Placed2>, second: Vec<Placed2>, threads: usize) -> LayerBake {
        LayerBake {
            cell: 0.01,
            band: 1.0,
            first,
            second,
            second_layer: Some(SecondLayer {
                shift: -0.8,
                half: 0.8,
                material: 7,
            }),
            threads,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Место фигуры: сдвиг, поворот, размер и отражение — как у пластины 2.5D-вида.
    #[test]
    fn placed_shape_matches_its_transform() {
        let profile = Profile::new(STAR).unwrap();
        let contours = flatten_path(STAR);
        for (angle, scale, mirror) in [(0.0, 1.0, false), (30.0, 0.5, false), (-70.0, 2.0, true)] {
            let placed = Placed2 {
                flat: Flat2::Profile(profile.clone()),
                at: Vec2::new(3.0, -1.0),
                angle,
                scale,
                mirror,
                material: 0,
            };
            let (sin, cos) = angle.to_radians().sin_cos();
            let mut seed = 5u32;
            for _ in 0..500 {
                let u = Vec2::new(
                    xorshift(&mut seed) * 3.0 - 1.5,
                    xorshift(&mut seed) * 3.0 - 1.5,
                );
                let m = if mirror { Vec2::new(-u.x, u.y) } else { u } * scale;
                let p = placed.at + Vec2::new(cos * m.x - sin * m.y, sin * m.x + cos * m.y);
                // То же поле профиля, что у формы в режиме SDF, только на месте.
                let want = profile.distance(u) * scale;
                let got = placed.distance(p);
                assert!(
                    (got - want).abs() < 1e-4 * scale.max(1.0),
                    "{angle}° ×{scale} {mirror}: {u} → {got} вместо {want}"
                );
                let (lo, hi) = placed.world_bounds();
                if exact_distance(&contours, u) < 0.0 {
                    assert!(
                        p.x >= lo.x - 1e-4
                            && p.y >= lo.y - 1e-4
                            && p.x <= hi.x + 1e-4
                            && p.y <= hi.y + 1e-4
                    );
                }
            }
        }
    }

    /// Поле — минимум фигур с точностью клетки: в 10 000 случайных точках не дальше
    /// настоящего минимума больше чем на 0,71 клетки (смешивание между точками сетки) и
    /// не ближе (оценка снизу, по ней можно шагать лучом).
    #[test]
    fn baked_field_is_the_minimum_of_its_shapes() {
        let profile = Profile::new(STAR).unwrap();
        let star = |at: [f32; 2], angle: f32, scale: f32, mirror: bool, material: u16| Placed2 {
            flat: Flat2::Profile(profile.clone()),
            at: Vec2::from(at),
            angle,
            scale,
            mirror,
            material,
        };
        let first = vec![
            rect([0.0, 0.0], [3.0, 0.2], 1),
            star([1.0, 1.0], 20.0, 0.6, false, 2),
            star([-1.5, 0.9], -45.0, 0.4, true, 3),
            // Стопка: малый прямоугольник поверх большого — материал у малого.
            rect([2.5, 1.5], [0.5, 0.5], 4),
            rect([2.5, 1.5], [0.2, 0.2], 5),
        ];
        let second = vec![rect([0.0, -0.6], [4.0, 0.4], 0)];
        let bake = job(first.clone(), second.clone(), 3);
        let (mut field, stats) = bake.run().unwrap();
        assert_eq!(field.size(), bake.points(0.01).unwrap());
        assert_eq!(stats.points, (field.size().0 * field.size().1) as usize);
        let cell = field.cell();
        let (lo, hi) = bake.extent().unwrap();
        let mut seed = 9u32;
        for _ in 0..10_000 {
            let p = lo + (hi - lo) * Vec2::new(xorshift(&mut seed), xorshift(&mut seed));
            for (layer, list) in [(0, &first), (1, &second)] {
                let want = list
                    .iter()
                    .map(|s| s.distance(p))
                    .fold(f32::INFINITY, f32::min)
                    .clamp(-1.0, 1.0);
                let (got, estimate) = field.distance2(p, layer);
                assert!(
                    got <= want + 0.71 * cell + 2e-3 * want.abs().max(cell),
                    "{p} слой {layer}: {got} дальше {want}"
                );
                if !estimate {
                    assert!(
                        got >= want - 0.71 * cell - 2e-3 * want.abs().max(cell),
                        "{p} слой {layer}: {got} ближе {want}"
                    );
                }
                // Где ближе полосы ничего нет — ровно B и признак оценки.
                if want >= 1.0 + cell {
                    assert!(estimate && got == 1.0, "{p} слой {layer}: {got} {estimate}");
                }
            }
        }
        // Материалы: в стопке — верхняя, у основы — основа, рядом со звездой — звезда.
        assert_eq!(field.material_at(Vec2::new(2.5, 1.5)), Some(5));
        assert_eq!(field.material_at(Vec2::new(2.9, 1.9)), Some(4));
        assert_eq!(field.material_at(Vec2::new(-2.0, 0.0)), Some(1));
        assert_eq!(field.material_at(Vec2::new(1.0, 1.0)), Some(2));
        // Ячейка 0 — настоящая: в заголовке хранится ячейка + 1, а 0 значит «картинки нет».
        assert_eq!(field.header_words()[1], 0);
        field.set_material_texture(0);
        assert_eq!(field.header_words()[1], 1);
        // Слой твёрдого — ниже и толще: точка в нём получает его материал.
        let (d, m) = field.sample(glam::Vec3::new(0.0, -0.6, -0.8), 0.15, 0.0, 0);
        assert!(d < 0.0 && m == 7, "{d} {m}");
        let (d, m) = field.sample(glam::Vec3::new(2.5, 1.5, 0.15 - 0.01), 0.15, 0.0, 0);
        assert!(d < 0.0 && m == 5, "{d} {m}");
        // Вдали от всего — только оценка.
        let (_, m) = field.sample(glam::Vec3::new(lo.x + 0.01, hi.y - 0.01, 0.0), 0.15, 0.0, 0);
        assert_eq!(m, crate::shape3d::ESTIMATE_MATERIAL);
    }

    /// Сколько бы потоков ни считало, поле одно и то же; отменённое запекание ничего не
    /// даёт; без фигур поля нет.
    #[test]
    fn threads_do_not_change_the_field_and_cancel_stops_it() {
        let first = vec![
            rect([0.0, 0.0], [2.0, 0.3], 1),
            rect([1.0, 0.5], [0.3, 0.3], 2),
        ];
        let second = vec![rect([0.0, -1.0], [3.0, 0.5], 0)];
        let (one, _) = job(first.clone(), second.clone(), 1).run().unwrap();
        let (many, _) = job(first.clone(), second.clone(), 5).run().unwrap();
        assert_eq!(one.bytes(), many.bytes());
        assert_eq!(one.material_bytes(), many.material_bytes());
        let cancelled = job(first, second, 2);
        cancelled.cancel.store(true, Ordering::Relaxed);
        assert!(cancelled.run().is_none());
        assert!(job(Vec::new(), Vec::new(), 2).run().is_none());
        // Шаг сетки по размеру: 1 см не влезает в 100 точек — берётся 5 см.
        let mut big = job(vec![rect([0.0, 0.0], [1.0, 1.0], 0)], Vec::new(), 1);
        assert!(big.fit(&[0.01, 0.05], 100));
        assert_eq!(big.cell, 0.05);
        // Подобранный шаг даёт ту же сетку, что запечётся (запас зависит от шага).
        let (side, _) = big.run().unwrap().0.size();
        assert_eq!(big.points(0.05).unwrap().0, side);
        assert!(side <= 100);
        assert!(!big.fit(&[0.01], 100));
    }

    /// Перепекание на месте (7.18г): кусок вокруг убранной и добавленной фигуры,
    /// запечённый по новому списку и вписанный в старое поле, даёт то же поле, что
    /// запекание всей сетки по новому списку, байт в байт.
    #[test]
    fn patched_area_matches_a_full_bake() {
        let first = vec![
            rect([0.0, 0.0], [2.0, 0.5], 1),
            rect([1.0, 0.8], [0.3, 0.3], 2),
            rect([-1.2, 0.7], [0.2, 0.4], 3),
        ];
        let second = vec![rect([0.0, -1.0], [3.0, 0.3], 0)];
        let old = job(first.clone(), second.clone(), 3);
        let (mut field, _) = old.run().unwrap();
        let (corner, cell, (nx, ny)) = (field.corner(), field.cell(), field.size());
        // Убираем вторую фигуру, ставим новую рядом, сдвигаем брусок.
        let mut now = first.clone();
        now.remove(1);
        now.push(rect([0.4, 1.0], [0.25, 0.2], 4));
        let mut moved = second.clone();
        moved[0].at.x += 0.3;
        let new = job(now.clone(), moved.clone(), 2);
        let area_of = |p: &Placed2| {
            let (a, b) = p.world_bounds();
            let pad = Vec2::splat(new.band + 2.0 * cell);
            let lo = ((a - pad - corner) / cell).floor().max(Vec2::ZERO);
            let hi = ((b + pad - corner) / cell)
                .ceil()
                .min(Vec2::new(nx as f32, ny as f32));
            [
                lo.x as u32,
                lo.y as u32,
                (hi - lo).x as u32,
                (hi - lo).y as u32,
            ]
        };
        for changed in [&first[1], &now[2], &second[0], &moved[0]] {
            let area = area_of(changed);
            let baked = new.run_area(corner, cell, area).unwrap();
            assert_eq!(baked.stats.points, (area[2] * area[3]) as usize);
            field.patch(area, &baked.texels, &baked.materials);
            let (tex, mat) = field.area_bytes(area);
            assert_eq!((tex, mat), (baked.texels, baked.materials));
        }
        let whole = new.run_area(corner, cell, [0, 0, nx, ny]).unwrap();
        assert!(field.bytes() == whole.texels.as_slice(), "значения");
        assert!(
            field.material_bytes() == Some(whole.materials.as_slice()),
            "номера"
        );
        // Пустой кусок ничего не считает.
        assert_eq!(
            new.run_area(corner, cell, [5, 5, 0, 3])
                .unwrap()
                .stats
                .points,
            0
        );
    }
}
