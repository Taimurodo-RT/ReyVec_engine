//! Глина (М.9, `docs/models-3d.md`): деталь модели, которую мнут кистями. Это тот же
//! «кирпич поля» ([`crate::brick3d`], сетка расстояний до 64 точек по стороне), только
//! поверхность лежит ровно по числам сетки (без отступа внутрь) и сетка хранится в своём
//! файле `*.clay` рядом с моделью: `Clay(file: "кузница/заготовка.clay")` в дереве формы.
//!
//! Глину можно получить из любой формы ([`Brick::from_shape`]: числа сетки — расстояние
//! до формы в каждой точке) и мять кистями ([`Brick::brush`]):
//! - «добавить» — плавное слияние с шаром, вдавленным в поверхность на долю радиуса;
//! - «вдавить» — плавный вырез таким же шаром снаружи;
//! - «сгладить» — числа тянутся к среднему соседей, сильнее у середины кисти.
//!
//! Слияние и вырез — те же действия, что у форм (`Union`/`Subtract` с `smooth`), поэтому
//! числа остаются расстоянием (не завышают его), и луч по глине шагает так же надёжно.
//! Дальше полосы [`crate::brick3d::BAND_CELLS`] клеток от поверхности хранится сама
//! полоса: кисть, которая выдавила поверхность дальше, упрётся в край сетки.

use std::sync::atomic::AtomicU32;

use glam::Vec3;

use crate::brick3d::{BAND_CELLS, Brick, MAX_SIDE, NO_TEXTURE, PAD_CELLS};
use crate::layer_field::{f32_to_half_toward_zero, half_to_f32};
use crate::shape3d::Shape3;

/// Окончание файла глины.
pub const EXTENSION: &str = ".clay";

/// Начало файла: вид и версия.
const MAGIC: &[u8; 8] = b"RVCLAY01";

/// Байтов заголовка: метка, угол (3 числа), клетка, размеры (3 числа).
const HEADER: usize = 8 + 4 * 4 + 3 * 4;

/// Кисть глины.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    /// Наростить: шар сливается с поверхностью.
    Add,
    /// Вдавить: шар вырезается из поверхности.
    Push,
    /// Сгладить: бугры и ямки выравниваются.
    Smooth,
}

/// Плавный минимум (как у `Union` с `smooth`): `k` — ширина перехода.
fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

impl Brick {
    /// Сетка расстояний до формы `shape` (готовой: без `Use` и `Clay`), не больше `side`
    /// точек по длинной стороне. Ошибка — форма бесконечна или пуста.
    pub fn from_shape(shape: &Shape3, side: u32) -> Result<Brick, String> {
        let side = side.clamp(2 * PAD_CELLS + 4, MAX_SIDE);
        let (lo, hi) = shape
            .bounds_box()
            .ok_or("форма бесконечна: в глину не превратить")?;
        let longest = (hi - lo).max_element();
        if !(longest > 0.0 && longest.is_finite()) {
            return Err("форма пуста: в глину не превратить".into());
        }
        let cell = longest / (side - 2 * PAD_CELLS) as f32;
        let corner = lo - Vec3::splat(PAD_CELLS as f32 * cell);
        let size = ((hi - lo) / cell)
            .to_array()
            .map(|e| ((e.ceil() as u32) + 2 * PAD_CELLS).clamp(2, MAX_SIDE));
        let band = BAND_CELLS * cell;
        let [nx, ny, nz] = size.map(|s| s as usize);
        let mut values = Vec::with_capacity(nx * ny * nz);
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let p = corner + (Vec3::new(i as f32, j as f32, k as f32) + 0.5) * cell;
                    let d = shape.distance(p).clamp(-band, band);
                    values.push(f32_to_half_toward_zero(d));
                }
            }
        }
        Ok(Brick {
            corner,
            cell,
            size,
            values,
            inset_cells: 0.0,
            texture: AtomicU32::new(NO_TEXTURE),
        })
    }

    /// Копия сетки (ещё не загруженная на видеокарту): кисть правит копию, а старая
    /// остаётся у кадра, пока новая не загрузится.
    pub fn duplicate(&self) -> Brick {
        Brick {
            corner: self.corner,
            cell: self.cell,
            size: self.size,
            values: self.values.clone(),
            inset_cells: self.inset_cells,
            texture: AtomicU32::new(NO_TEXTURE),
        }
    }

    /// Файл глины: заголовок и числа сетки.
    pub fn to_clay_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.values.len() * 2);
        out.extend_from_slice(MAGIC);
        for v in [self.corner.x, self.corner.y, self.corner.z, self.cell] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for s in self.size {
            out.extend_from_slice(&s.to_le_bytes());
        }
        for v in &self.values {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Глина из файла.
    pub fn from_clay_bytes(bytes: &[u8]) -> Result<Brick, String> {
        if bytes.len() < HEADER || &bytes[..8] != MAGIC {
            return Err("это не файл глины (нет метки RVCLAY01)".into());
        }
        let word = |at: usize| {
            let mut b = [0u8; 4];
            b.copy_from_slice(&bytes[at..at + 4]);
            b
        };
        let f = |at: usize| f32::from_le_bytes(word(at));
        let u = |at: usize| u32::from_le_bytes(word(at));
        let corner = Vec3::new(f(8), f(12), f(16));
        let cell = f(20);
        let size = [u(24), u(28), u(32)];
        if !(cell > 0.0 && cell.is_finite() && corner.is_finite()) {
            return Err("в файле глины плохой шаг сетки".into());
        }
        if size.iter().any(|&s| !(2..=MAX_SIDE).contains(&s)) {
            return Err(format!(
                "сетка глины {}×{}×{}: по стороне от 2 до {MAX_SIDE} точек",
                size[0], size[1], size[2]
            ));
        }
        let n = size.iter().map(|&s| s as usize).product::<usize>();
        if bytes.len() != HEADER + n * 2 {
            return Err(format!(
                "файл глины обрезан: чисел {} вместо {n}",
                (bytes.len().saturating_sub(HEADER)) / 2
            ));
        }
        let values = bytes[HEADER..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(Brick {
            corner,
            cell,
            size,
            values,
            inset_cells: 0.0,
            texture: AtomicU32::new(NO_TEXTURE),
        })
    }

    /// Направление наружу из точки `p` (по разнице чисел вокруг неё).
    pub fn normal(&self, p: Vec3) -> Vec3 {
        let e = self.cell * 0.5;
        let d = |o: Vec3| self.sample(p + o) - self.sample(p - o);
        Vec3::new(d(Vec3::X * e), d(Vec3::Y * e), d(Vec3::Z * e))
            .try_normalize()
            .unwrap_or(Vec3::Y)
    }

    /// Мазок кисти `kind` в точке поверхности `at` (координаты глины) радиусом `radius`;
    /// `strength` от 0 до 1 — насколько глубоко (у «сгладить» — насколько сильно).
    /// Ответ — поменялось ли хоть одно число.
    pub fn brush(&mut self, kind: Brush, at: Vec3, radius: f32, strength: f32) -> bool {
        let radius = radius.max(self.cell);
        let s = strength.clamp(0.02, 1.0);
        let band = BAND_CELLS * self.cell;
        let n = self.normal(at);
        let k = radius * 0.5;
        // Шар кисти: для «добавить» он утоплен в поверхность и торчит на s·r, для
        // «вдавить» — снаружи и входит на s·r.
        let center = match kind {
            Brush::Add => at - n * radius * (1.0 - s),
            Brush::Push => at + n * radius * (1.0 - s),
            Brush::Smooth => at,
        };
        let reach = radius + k + band;
        let [nx, ny, nz] = self.size.map(|v| v as usize);
        let grid = |c: f32, lo: f32, n: usize| {
            let a = ((c - reach - lo) / self.cell - 0.5).floor().max(0.0) as usize;
            let b = (((c + reach - lo) / self.cell - 0.5).ceil().max(0.0) as usize).min(n - 1);
            a..=b
        };
        let old = (kind == Brush::Smooth).then(|| self.values.clone());
        let at_index = |i: usize, j: usize, k: usize| i + nx * (j + ny * k);
        let mut changed = false;
        for kk in grid(center.z, self.corner.z, nz) {
            for j in grid(center.y, self.corner.y, ny) {
                for i in grid(center.x, self.corner.x, nx) {
                    let p =
                        self.corner + (Vec3::new(i as f32, j as f32, kk as f32) + 0.5) * self.cell;
                    let idx = at_index(i, j, kk);
                    let d = half_to_f32(self.values[idx]);
                    let sphere = (p - center).length() - radius;
                    let new = match kind {
                        Brush::Add => smin(d, sphere, k),
                        // Плавное max(d, −шар).
                        Brush::Push => -smin(-d, sphere, k),
                        Brush::Smooth => {
                            let Some(old) = &old else { continue };
                            let t = (p - at).length() / radius;
                            if t >= 1.0 {
                                continue;
                            }
                            let w = (1.0 - t * t).powi(2) * s;
                            let v =
                                |i: usize, j: usize, k: usize| half_to_f32(old[at_index(i, j, k)]);
                            let mut sum = 0.0;
                            let mut count = 0.0;
                            for (di, dj, dk) in [
                                (-1, 0, 0),
                                (1, 0, 0),
                                (0, -1, 0),
                                (0, 1, 0),
                                (0, 0, -1),
                                (0, 0, 1),
                            ] {
                                let (a, b, c) =
                                    (i as isize + di, j as isize + dj, kk as isize + dk);
                                if a >= 0
                                    && b >= 0
                                    && c >= 0
                                    && (a as usize) < nx
                                    && (b as usize) < ny
                                    && (c as usize) < nz
                                {
                                    sum += v(a as usize, b as usize, c as usize);
                                    count += 1.0;
                                }
                            }
                            d + (sum / count - d) * w
                        }
                    };
                    // Края сетки остаются «снаружи»: глина не выходит за коробку.
                    let edge =
                        i == 0 || j == 0 || kk == 0 || i == nx - 1 || j == ny - 1 || kk == nz - 1;
                    let new = if edge { new.max(d.min(band)) } else { new };
                    let h = f32_to_half_toward_zero(new.clamp(-band, band));
                    if h != self.values[idx] {
                        self.values[idx] = h;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            let lo = [
                grid(center.x, self.corner.x, nx),
                grid(center.y, self.corner.y, ny),
                grid(center.z, self.corner.z, nz),
            ];
            self.keep_distance(lo.map(|r| (*r.start(), *r.end())));
        }
        changed
    }

    /// Числа соседних точек расстояния не отличаются больше чем на клетку. Сглаживание
    /// это нарушает — тогда луч мог бы шагнуть за поверхность. Здесь числа в коробке
    /// `range` (с полосой вокруг) прижимаются к поверхности, пока правило не выполнится:
    /// знак не меняется, расстояние только уменьшается.
    fn keep_distance(&mut self, range: [(usize, usize); 3]) {
        let [nx, ny, nz] = self.size.map(|v| v as usize);
        let pad = BAND_CELLS as usize + 1;
        let span = |(a, b): (usize, usize), n: usize| (a.saturating_sub(pad), (b + pad).min(n - 1));
        let (x, y, z) = (span(range[0], nx), span(range[1], ny), span(range[2], nz));
        let cell = self.cell;
        let idx = |i: usize, j: usize, k: usize| i + nx * (j + ny * k);
        let mut v: Vec<f32> = self.values.iter().map(|&h| half_to_f32(h)).collect();
        for pass in 0..4 {
            let mut moved = false;
            let order: Box<dyn Iterator<Item = usize>> = if pass % 2 == 0 {
                Box::new(z.0..=z.1)
            } else {
                Box::new((z.0..=z.1).rev())
            };
            for k in order {
                for j in y.0..=y.1 {
                    for i in x.0..=x.1 {
                        let here = idx(i, j, k);
                        let d = v[here];
                        let mut bound = d.abs();
                        for (di, dj, dk) in [
                            (-1, 0, 0),
                            (1, 0, 0),
                            (0, -1, 0),
                            (0, 1, 0),
                            (0, 0, -1),
                            (0, 0, 1),
                        ] {
                            let (a, b, c) = (i as isize + di, j as isize + dj, k as isize + dk);
                            if a < 0
                                || b < 0
                                || c < 0
                                || a as usize >= nx
                                || b as usize >= ny
                                || c as usize >= nz
                            {
                                continue;
                            }
                            let n = v[idx(a as usize, b as usize, c as usize)];
                            // Сосед по другую сторону поверхности: она между ними.
                            let reach = if (n < 0.0) == (d < 0.0) {
                                n.abs() + cell
                            } else {
                                cell
                            };
                            bound = bound.min(reach);
                        }
                        if bound < d.abs() {
                            v[here] = bound.copysign(d);
                            moved = true;
                        }
                    }
                }
            }
            if !moved {
                break;
            }
        }
        for (h, d) in self.values.iter_mut().zip(&v) {
            *h = f32_to_half_toward_zero(*d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ball() -> Shape3 {
        Shape3::Sphere { radius: 0.1 }
    }

    #[test]
    fn shape_turns_into_clay_and_back_from_file() {
        let b = Brick::from_shape(&ball(), 48).unwrap();
        assert_eq!(b.inset(), 0.0);
        for p in [
            Vec3::ZERO,
            Vec3::new(0.1, 0.0, 0.0),
            Vec3::new(0.0, 0.12, 0.0),
        ] {
            let band = crate::brick3d::BAND_CELLS * b.cell();
            let want = ball().distance(p).clamp(-band, band);
            assert!(
                (b.sample(p) - want).abs() < b.cell() * 0.3,
                "{p}: {} {want}",
                b.sample(p)
            );
        }
        let bytes = b.to_clay_bytes();
        let back = Brick::from_clay_bytes(&bytes).unwrap();
        assert_eq!(back.size(), b.size());
        assert_eq!(back.to_clay_bytes(), bytes);
        assert!(Brick::from_clay_bytes(&bytes[..bytes.len() - 2]).is_err());
        assert!(Brick::from_clay_bytes(b"not clay at all, sorry, not at all").is_err());
        let plane = Shape3::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 0.0,
        };
        assert!(Brick::from_shape(&plane, 32).is_err());
    }

    #[test]
    fn brushes_add_push_and_smooth() {
        let mut b = Brick::from_shape(&ball(), 48).unwrap();
        let top = Vec3::new(0.0, 0.1, 0.0);
        // Добавить: над верхом шара появляется глина.
        let above = Vec3::new(0.0, 0.115, 0.0);
        assert!(b.sample(above) > 0.0);
        assert!(b.brush(Brush::Add, top, 0.04, 0.5));
        assert!(b.sample(above) < 0.0, "{}", b.sample(above));
        // Сбоку, далеко от кисти, ничего не поменялось.
        let side = Vec3::new(0.1, 0.0, 0.0);
        assert!(b.sample(side).abs() < b.cell() * 0.3);
        // Вдавить сбоку: точка чуть под поверхностью становится снаружи.
        let under = Vec3::new(0.09, 0.0, 0.0);
        assert!(b.sample(under) < 0.0);
        b.brush(Brush::Push, side, 0.04, 0.5);
        assert!(b.sample(under) > 0.0, "{}", b.sample(under));
        // Сгладить: бугор сверху становится ниже.
        let peak = |b: &Brick| {
            (0..200)
                .map(|i| Vec3::new(0.0, 0.1 + i as f32 * 0.0005, 0.0))
                .take_while(|&p| b.sample(p) < 0.0)
                .count()
        };
        let before = peak(&b);
        for _ in 0..10 {
            b.brush(Brush::Smooth, Vec3::new(0.0, 0.12, 0.0), 0.05, 1.0);
        }
        assert!(peak(&b) < before, "{} {before}", peak(&b));
        // Шаг луча по глине не завышен: расстояние не больше, чем до точки на поверхности.
        let p = Vec3::new(0.0, 0.3, 0.0);
        let mut t = 0.0;
        let mut q = p;
        for _ in 0..200 {
            q = p - Vec3::Y * t;
            let d = b.sample(q);
            if d < 1e-4 {
                break;
            }
            t += d;
        }
        assert!(
            b.sample(q) > -b.cell(),
            "луч проскочил внутрь: {}",
            b.sample(q)
        );
    }

    /// Заготовка кузницы для `scenes/3d/forge.ron`: брусок со следами молота и
    /// оттянутым концом. Пересоздать: `cargo test -p reyvec-sdf make_forge_billet --
    /// --ignored`.
    #[test]
    #[ignore]
    fn make_forge_billet() {
        let bar = Shape3::Box {
            size: [0.22, 0.035, 0.035],
            round: 0.006,
        };
        let mut b = Brick::from_shape(&bar, 64).unwrap();
        // Следы молота по верху.
        for (x, z) in [
            (-0.07, 0.004),
            (-0.035, -0.003),
            (0.0, 0.005),
            (0.035, -0.002),
        ] {
            b.brush(Brush::Push, Vec3::new(x, 0.0175, z), 0.016, 0.22);
        }
        // Конец оттянут: сжат сверху и с боков, а по оси вытянут наплывом.
        for x in [0.085, 0.1] {
            b.brush(Brush::Push, Vec3::new(x, 0.0175, 0.0), 0.014, 0.35);
            b.brush(Brush::Push, Vec3::new(x, 0.0, 0.0175), 0.012, 0.3);
            b.brush(Brush::Push, Vec3::new(x, 0.0, -0.0175), 0.012, 0.3);
        }
        b.brush(Brush::Add, Vec3::new(-0.11, 0.0, 0.0), 0.014, 0.25);
        for x in [-0.06, -0.02, 0.02, 0.06, 0.1] {
            b.brush(Brush::Smooth, Vec3::new(x, 0.0175, 0.0), 0.02, 0.5);
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scenes/3d/models/кузница/заготовка.clay");
        std::fs::write(&dir, b.to_clay_bytes()).unwrap();
    }
}
