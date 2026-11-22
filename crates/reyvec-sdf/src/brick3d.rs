//! «Кирпич поля» (этап 8.15): расстояние до полигональной модели, запечённое один раз в
//! маленькую объёмную сетку. По нему модель бросает тень и затеняет углы.
//!
//! Раньше тень модели бросали заменители из плоскостей граней (7.6ж, 8.14): у бака
//! около 18 плоскостей, у станка набегало под 70, и луч тени перебирал их все на каждом
//! шаге. На Radeon сцена с двумя станками тратила на тени 15 мс. Кирпич — один примитив
//! [`crate::Shape3::Brick`]: луч читает два числа из картинки, сколько бы граней ни было.
//! Заодно тень есть и у невыпуклых кусков.
//!
//! Как печётся:
//! - сетка — коробка модели с запасом [`PAD_CELLS`] клеток по краям, по длинной стороне
//!   не больше [`MAX_SIDE`] точек (картинка видеокарты — слои Z друг под другом, высота
//!   `ny · nz` не больше 4096);
//! - расстояние без знака считается только у точек в полосе [`BAND_CELLS`] клеток от
//!   каждого треугольника, дальше хранится сама полоса — оценка снизу, луч просто шагает
//!   короче;
//! - внутри или снаружи — по чётности пересечений столбика вдоль Z с каждым замкнутым
//!   куском модели (куски — [`crate::model3d::pieces`]): точка внутри, если она внутри
//!   хотя бы одного куска. Так куски могут пересекаться (бак в корпусе), а обход
//!   треугольников не важен. У незамкнутого куска нет «внутри»: тени он не даёт;
//! - числа хранятся half float с усечением к нулю (по модулю не растут).
//!
//! Заменитель должен быть чуть внутри модели, иначе луч тени с её поверхности начнётся в
//! заменителе и модель затенит сама себя: расстояние кирпича — число сетки плюс
//! [`Brick::inset`] (одна клетка).
//!
//! За сеткой расстояние — оценка снизу от края сетки (как у 2D-поля фигуры): точки края
//! дальше полосы от модели, а модель внутри коробки, которая ещё на полосу глубже.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use glam::Vec3;

use crate::layer_field::{f32_to_half_toward_zero, half_to_f32};

/// Больше точек по стороне сетки не бывает (и по стороне картинки: `ny · nz ≤ 4096`).
pub const MAX_SIDE: u32 = 64;
/// Полоса точного расстояния, в клетках: дальше хранится сама полоса.
pub const BAND_CELLS: f32 = 4.0;
/// Запас сетки за коробкой модели, в клетках (больше полосы: точки края всегда дальше
/// полосы от модели).
pub const PAD_CELLS: u32 = 5;
/// На сколько клеток заменитель внутри модели.
pub const INSET_CELLS: f32 = 1.0;
/// Слов данных у примитива «кирпич поля» (`BRICK_PRIM_WORDS` в `include/sdf3d.slang`,
/// менять вместе): номер картинки, угол сетки (x, y, z — середина первой клетки минус
/// полклетки), шаг, `nx | ny << 16`, `nz`, 1 запасное. Параметры примитива: отступ внутрь
/// (`inset`), полоса.
pub const BRICK_PRIM_WORDS: usize = 8;
/// Номер картинки «ещё не загружена».
pub(crate) const NO_TEXTURE: u32 = u32::MAX;

/// Запечённая сетка расстояний модели (в координатах модели).
pub struct Brick {
    /// Угол сетки: точка (i, j, k) — в `corner + (i + 0.5, j + 0.5, k + 0.5) · cell`.
    pub(crate) corner: Vec3,
    pub(crate) cell: f32,
    pub(crate) size: [u32; 3],
    /// half float, X быстрее всего, потом Y, потом Z — так же лежит картинка: строка
    /// `j + k · ny`.
    pub(crate) values: Vec<u16>,
    /// На сколько клеток поверхность внутри сетки: у заменителя модели
    /// [`INSET_CELLS`], у глины (М.9, [`crate::clay`]) — 0.
    pub(crate) inset_cells: f32,
    /// Номер картинки в общем списке видеокарты (ставит загрузка, [`Brick::set_texture`]).
    pub(crate) texture: AtomicU32,
}

impl std::fmt::Debug for Brick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [nx, ny, nz] = self.size;
        write!(f, "Brick({nx}×{ny}×{nz} по {} м)", self.cell)
    }
}

/// Кирпич, общий для формы, её программы и загрузки на видеокарту. Две ссылки равны, если
/// это один и тот же кирпич (по адресу).
#[derive(Clone)]
pub struct BrickRef(pub Arc<Brick>);

impl PartialEq for BrickRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for BrickRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl Brick {
    /// Печёт кирпич по треугольникам модели. Ответ — кирпич (если хотя бы один кусок
    /// замкнут) и число незамкнутых кусков (они тени не дают).
    pub fn bake(triangles: &[[Vec3; 3]]) -> (Option<Brick>, usize) {
        let pieces = crate::model3d::pieces(triangles);
        let (closed, open): (Vec<_>, Vec<_>) = pieces.into_iter().partition(|p| is_closed(p));
        if closed.is_empty() {
            return (None, open.len());
        }
        let (lo, hi) = closed.iter().flatten().flatten().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), v| (lo.min(*v), hi.max(*v)),
        );
        let longest = (hi - lo).max_element();
        if !(longest > 0.0 && longest.is_finite()) {
            return (None, open.len());
        }
        let cell = longest / (MAX_SIDE - 2 * PAD_CELLS) as f32;
        let corner = lo - Vec3::splat(PAD_CELLS as f32 * cell);
        let size = ((hi - lo) / cell)
            .to_array()
            .map(|e| ((e.ceil() as u32) + 2 * PAD_CELLS).clamp(2, MAX_SIDE));
        let band = BAND_CELLS * cell;
        let [nx, ny, nz] = size.map(|s| s as usize);
        let at = |i: usize, j: usize, k: usize| i + nx * (j + ny * k);
        let point = |i: usize, j: usize, k: usize| {
            corner + (Vec3::new(i as f32, j as f32, k as f32) + 0.5) * cell
        };
        // Расстояние без знака в полосе каждого треугольника.
        let mut dist = vec![band; nx * ny * nz];
        let range = |lo: f32, hi: f32, c: f32, n: usize| {
            let a = ((lo - band - c) / cell - 0.5).floor().max(0.0) as usize;
            let b = (((hi + band - c) / cell - 0.5).ceil().max(0.0) as usize).min(n - 1);
            a..=b
        };
        for piece in &closed {
            for t in piece {
                let tlo = t[0].min(t[1]).min(t[2]);
                let thi = t[0].max(t[1]).max(t[2]);
                for k in range(tlo.z, thi.z, corner.z, nz) {
                    for j in range(tlo.y, thi.y, corner.y, ny) {
                        for i in range(tlo.x, thi.x, corner.x, nx) {
                            let d = point_triangle_distance(point(i, j, k), t);
                            let slot = &mut dist[at(i, j, k)];
                            *slot = slot.min(d);
                        }
                    }
                }
            }
        }
        // Знак: чётность пересечений столбика (луч вдоль +Z из −∞) с каждым куском.
        let mut inside = vec![false; nx * ny * nz];
        // Сдвиг столбика на долю клетки — чтобы он не шёл ровно по ребру или вершине.
        let jitter = Vec3::new(0.731_7e-3, 0.318_3e-3, 0.0) * cell;
        for piece in &closed {
            let mut hits: HashMap<(usize, usize), Vec<f32>> = HashMap::new();
            for t in piece {
                let tlo = t[0].min(t[1]).min(t[2]);
                let thi = t[0].max(t[1]).max(t[2]);
                let cols = |lo: f32, hi: f32, c: f32, n: usize| {
                    let a = ((lo - c) / cell - 0.5).floor().max(0.0) as usize;
                    let b = (((hi - c) / cell - 0.5).ceil().max(0.0) as usize).min(n - 1);
                    a..=b
                };
                for j in cols(tlo.y, thi.y, corner.y, ny) {
                    for i in cols(tlo.x, thi.x, corner.x, nx) {
                        let p = point(i, j, 0) + jitter;
                        if let Some(z) = column_hit(p.x, p.y, t) {
                            hits.entry((i, j)).or_default().push(z);
                        }
                    }
                }
            }
            for ((i, j), mut zs) in hits {
                zs.sort_by(f32::total_cmp);
                let mut below = 0;
                for k in 0..nz {
                    let z = point(i, j, k).z;
                    while below < zs.len() && zs[below] < z {
                        below += 1;
                    }
                    if below % 2 == 1 {
                        inside[at(i, j, k)] = true;
                    }
                }
            }
        }
        let values = dist
            .iter()
            .zip(&inside)
            .map(|(&d, &inn)| f32_to_half_toward_zero(if inn { -d } else { d }))
            .collect();
        (
            Some(Brick {
                corner,
                cell,
                size,
                values,
                inset_cells: INSET_CELLS,
                texture: AtomicU32::new(NO_TEXTURE),
            }),
            open.len(),
        )
    }

    /// Точек по сторонам сетки.
    pub fn size(&self) -> [u32; 3] {
        self.size
    }

    /// Шаг сетки, м.
    pub fn cell(&self) -> f32 {
        self.cell
    }

    /// На сколько заменитель внутри модели, м.
    pub fn inset(&self) -> f32 {
        self.inset_cells * self.cell
    }

    /// Полоса точного расстояния, м.
    pub fn band(&self) -> f32 {
        BAND_CELLS * self.cell
    }

    /// Картинка для видеокарты: R16_SFLOAT шириной `nx` и высотой `ny · nz` (слои Z
    /// друг под другом).
    pub fn bytes(&self) -> Vec<u8> {
        self.values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// Ширина и высота картинки.
    pub fn image_size(&self) -> (u32, u32) {
        (self.size[0], self.size[1] * self.size[2])
    }

    /// Номер картинки в общем списке видеокарты (после загрузки).
    pub fn set_texture(&self, slot: u32) {
        self.texture.store(slot, Ordering::Relaxed);
    }

    /// Номер картинки, если она загружена.
    pub fn texture(&self) -> Option<u32> {
        Some(self.texture.load(Ordering::Relaxed)).filter(|&s| s != NO_TEXTURE)
    }

    /// Слова данных примитива (см. [`BRICK_PRIM_WORDS`]).
    pub fn header_words(&self) -> [u32; BRICK_PRIM_WORDS] {
        let [nx, ny, nz] = self.size;
        [
            self.texture.load(Ordering::Relaxed),
            self.corner.x.to_bits(),
            self.corner.y.to_bits(),
            self.corner.z.to_bits(),
            self.cell.to_bits(),
            nx | (ny << 16),
            nz,
            0,
        ]
    }

    /// Шар, в котором вся сетка (середина и радиус): снаружи него расстояние до кирпича
    /// не меньше расстояния до шара.
    pub fn bounds(&self) -> (Vec3, f32) {
        let half = Vec3::from(self.size.map(|s| s as f32)) * self.cell * 0.5;
        (self.corner + half, half.length())
    }

    /// Коробка сетки (наименьший и наибольший углы, координаты модели): модель внутри.
    pub fn bounds_box(&self) -> (Vec3, Vec3) {
        let size = Vec3::from(self.size.map(|s| s as f32)) * self.cell;
        (self.corner, self.corner + size)
    }

    /// Число сетки в точке `p` (координаты модели), без отступа внутрь: то же, что
    /// считает шейдер (`sd3_brick` в `include/sdf3d.slang`), только смешивание точное.
    pub fn sample(&self, p: Vec3) -> f32 {
        let [nx, ny, nz] = self.size;
        let top = Vec3::new(nx as f32 - 1.0, ny as f32 - 1.0, nz as f32 - 1.0);
        let g = (p - self.corner) / self.cell - 0.5;
        let gc = g.clamp(Vec3::ZERO, top);
        let e = (g - gc).length() * self.cell;
        let i0 = gc.floor().min(top - 1.0);
        let t = gc - i0;
        let (i, j, k) = (i0.x as usize, i0.y as usize, i0.z as usize);
        let (nx, ny) = (nx as usize, ny as usize);
        let v = |i: usize, j: usize, k: usize| half_to_f32(self.values[i + nx * (j + ny * k)]);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let layer = |k: usize| {
            lerp(
                lerp(v(i, j, k), v(i + 1, j, k), t.x),
                lerp(v(i, j + 1, k), v(i + 1, j + 1, k), t.x),
                t.y,
            )
        };
        let d = lerp(layer(k), layer(k + 1), t.z);
        if e > 0.0 {
            let dp = d.max(0.0);
            return (e * e + dp * dp).sqrt();
        }
        d
    }
}

/// Замкнут ли кусок: каждое ребро (по месту вершин) — у чётного числа треугольников.
fn is_closed(triangles: &[[Vec3; 3]]) -> bool {
    let key = |v: Vec3| (v * 1.0e4).round().as_ivec3().to_array();
    let mut edges: HashMap<([i32; 3], [i32; 3]), u32> = HashMap::new();
    for t in triangles {
        for e in 0..3 {
            let (a, b) = (key(t[e]), key(t[(e + 1) % 3]));
            if a == b {
                continue;
            }
            *edges
                .entry(if a < b { (a, b) } else { (b, a) })
                .or_default() += 1;
        }
    }
    !edges.is_empty() && edges.values().all(|&n| n % 2 == 0)
}

/// Где прямая вдоль Z через (x, y) пересекает треугольник (высота z), если пересекает.
fn column_hit(x: f32, y: f32, t: &[Vec3; 3]) -> Option<f32> {
    let (a, b, c) = (t[0], t[1], t[2]);
    let area = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
    if area.abs() < 1e-12 {
        return None;
    }
    let w0 = ((b.x - x) * (c.y - y) - (c.x - x) * (b.y - y)) / area;
    let w1 = ((c.x - x) * (a.y - y) - (a.x - x) * (c.y - y)) / area;
    let w2 = 1.0 - w0 - w1;
    (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0).then_some(w0 * a.z + w1 * b.z + w2 * c.z)
}

/// Расстояние от точки до треугольника (ближайшая точка по областям Вороного, как у
/// Эриксона, «Real-Time Collision Detection», 5.1.5).
fn point_triangle_distance(p: Vec3, t: &[Vec3; 3]) -> f32 {
    let (a, b, c) = (t[0], t[1], t[2]);
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length();
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (p - (a + ab * v)).length();
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (p - (a + ac * w)).length();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (p - (b + (c - b) * w)).length();
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    (p - (a + ab * v + ac * w)).length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh3d::Mesh3;

    fn block(at: Vec3, size: f32) -> Vec<[Vec3; 3]> {
        Mesh3::Block { size: [size; 3] }
            .triangles()
            .into_iter()
            .map(|t| t.map(|v| v + at))
            .collect()
    }

    #[test]
    fn box_brick_matches_box_distance() {
        let (brick, open) = Brick::bake(&block(Vec3::ZERO, 1.0));
        let brick = brick.unwrap();
        assert_eq!(open, 0);
        assert!(brick.size().iter().all(|&s| s <= MAX_SIDE));
        let cell = brick.cell();
        // У плоской грани смешивание точное: ошибка — только half.
        for (p, want) in [
            (Vec3::new(0.0, 0.0, 0.5 + 2.0 * cell), 2.0 * cell),
            (Vec3::new(0.1, 0.5 + cell, -0.2), cell),
            (Vec3::new(0.3, 0.5 - 2.0 * cell, 0.0), -2.0 * cell),
        ] {
            let got = brick.sample(p);
            assert!((got - want).abs() < 0.05 * cell, "{p}: {got} вместо {want}");
        }
        // Середина внутри (дальше полосы — сама полоса со знаком минус), далеко —
        // оценка снизу, но не больше настоящего расстояния.
        assert!(brick.sample(Vec3::ZERO) <= -brick.band() * 0.99);
        let far = Vec3::new(3.0, 0.0, 0.0);
        let d = brick.sample(far);
        assert!(d > 2.0 && d <= 2.5, "{d}");
    }

    #[test]
    fn overlapping_pieces_are_inside_and_open_piece_is_skipped() {
        // Две пересекающиеся коробки (бак в корпусе) и одинокий треугольник.
        let mut tris = block(Vec3::ZERO, 1.0);
        tris.extend(block(Vec3::new(0.4, 0.0, 0.0), 0.6));
        tris.push([
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.5, 0.0, 0.0),
            Vec3::new(2.0, 0.5, 0.0),
        ]);
        let (brick, open) = Brick::bake(&tris);
        let brick = brick.unwrap();
        assert_eq!(open, 1);
        // Внутри обеих коробок, только во второй, снаружи обеих.
        assert!(brick.sample(Vec3::new(0.4, 0.0, 0.0)) < 0.0);
        assert!(brick.sample(Vec3::new(0.65, 0.0, 0.0)) < 0.0);
        assert!(brick.sample(Vec3::new(0.0, 0.0, 0.8)) > 0.0);
    }

    #[test]
    fn open_model_has_no_brick() {
        let tri = [[Vec3::ZERO, Vec3::X, Vec3::Y]];
        assert!(matches!(Brick::bake(&tri), (None, 1)));
    }

    #[test]
    fn brick_is_lower_bound_outside() {
        // Кирпич шара из треугольников: снаружи число не больше расстояния до самих
        // треугольников (с запасом на смешивание по клетке).
        let tris = Mesh3::Icosphere {
            radius: 0.5,
            detail: 2,
        }
        .triangles();
        let (brick, _) = Brick::bake(&tris);
        let brick = brick.unwrap();
        let exact = |p: Vec3| {
            tris.iter()
                .map(|t| point_triangle_distance(p, t))
                .fold(f32::MAX, f32::min)
        };
        for k in 0..200 {
            let a = k as f32 * 0.7;
            let r = 0.5 + 0.02 + (k % 20) as f32 * 0.08;
            let p = Vec3::new(a.cos() * r, ((k * 7) % 13) as f32 * 0.05 - 0.3, a.sin() * r);
            let got = brick.sample(p);
            assert!(
                got <= exact(p) + 0.5 * brick.cell(),
                "{p}: {got} > {}",
                exact(p)
            );
        }
    }
}
