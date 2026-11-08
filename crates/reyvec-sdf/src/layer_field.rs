//! Поле слоя (7.18б, `docs/decisions/0020-level-field.md`): 2D-поле расстояний в
//! текстуре видеокарты, которое объёмная форма [`crate::Shape3::Field`] выдавливает в
//! толщину слоя. В отличие от поля 2D-фигуры ([`crate::profile2d::Profile`]), которое
//! едет видеокарте словами программы, это поле лежит в картинке и может быть большим
//! (поле целого уровня, 7.18в); в программе формы — только заголовок: номер картинки,
//! левый нижний угол сетки, шаг, число точек и прямоугольник фигуры.
//!
//! Числа хранятся как half float (16 бит) и усекаются к нулю, то есть к контуру: поле
//! остаётся оценкой снизу, по нему можно шагать лучом. Значения обрезаны полосой `±B`:
//! точка, где поле говорит «не ближе `B − клетка`», — только оценка, материал там не
//! известен ([`crate::shape3d::ESTIMATE_MATERIAL`]).
//!
//! Правило края: за сеткой расстояние считается от ближайшей точки её края —
//! `√(e² + max(v, 0)²)`, где `e` — до края, `v` — значение на краю; если задан
//! прямоугольник фигуры, ещё и не меньше расстояния до него (как у `Profile`). С
//! прямоугольником у формы есть шар-граница, как у выдавливания; поле без него (поле
//! целого уровня, 7.18в) бесконечно. Шейдер
//! `include/sdf3d.slang` (`sd2_layer`) читает те же слова той же формулой, что
//! [`LayerField::distance2`], только значения берёт из картинки с линейным смешиванием.

use glam::{Vec2, Vec3};

use crate::profile2d::{FIELD_HEADER_WORDS, Profile};
use crate::shape3d::{ESTIMATE_MATERIAL, FIELD_PRIM_WORDS, sd_extrude};

/// Ближе этого к форме (в её координатах) материал берётся из картинки номеров; дальше
/// он не нужен (материал смотрят только в точке попадания), и картинку не читают.
pub const MATERIAL_NEAR: f32 = 0.05;

/// Второй слой поля (7.18в, слой твёрдого уровня): своя толщина и своё место по Z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SecondLayer {
    /// Середина слоя по Z относительно середины первого.
    pub shift: f32,
    /// Половина толщины (больше нуля).
    pub half: f32,
    /// Материал всего слоя.
    pub material: u32,
}

/// Поле слоя: сетка значений и всё, что о ней нужно знать зеркалу на процессоре.
pub struct LayerField {
    /// Левый нижний угол клетки (0, 0) в локальных координатах формы.
    corner: Vec2,
    /// Шаг сетки.
    cell: f32,
    nx: u32,
    ny: u32,
    /// Полоса `B`: дальше неё поле не знает.
    band: f32,
    /// Прямоугольник фигуры (без запаса), если известен: за сеткой расстояние не меньше
    /// расстояния до него. У поля целого уровня (7.18в) его нет.
    rect: Option<(Vec2, Vec2)>,
    /// Значения по строкам снизу вверх, на точку два слоя (R16G16_SFLOAT, младший байт
    /// первым — как их читает видеокарта): основной и второй (у поля одной фигуры везде
    /// `+B`, у поля уровня — слой твёрдого).
    texels: Vec<u8>,
    /// Номера материалов первого слоя по точкам (R16_UNORM, младший байт первым); пусто —
    /// материал из слова примитива.
    materials: Vec<u8>,
    /// Второй слой, если он есть.
    second: Option<SecondLayer>,
    /// Номер картинки в общем списке текстур; ставит видеокарта после загрузки.
    texture: u32,
    /// Номер картинки материалов (0 — её нет).
    material_texture: Option<u32>,
}

impl LayerField {
    /// Поле на собственной сетке профиля: те же точки, что читает `Extrude`, только
    /// усечённые до half и обрезанные полосой `band`. Так выдавливание через поле
    /// отличается от выдавливания по словам только точностью half (опыт 7.18б).
    pub fn from_profile(profile: &Profile, band: f32) -> Self {
        let w = profile.words();
        let f = |i: usize| f32::from_bits(w[i]);
        let (nx, ny) = (w[3] & 0xffff, w[3] >> 16);
        let plus_band = f32_to_half_toward_zero(band);
        let halves = w[FIELD_HEADER_WORDS..]
            .iter()
            .map(|&bits| {
                let d = f32::from_bits(bits).clamp(-band, band);
                [f32_to_half_toward_zero(d), plus_band]
            })
            .collect::<Vec<_>>();
        Self::new(
            Vec2::new(f(0), f(1)),
            f(2),
            nx,
            ny,
            band,
            Some(profile.bounds()),
            halves,
        )
    }

    /// Поле из готовых значений (`nx · ny` чисел по строкам снизу вверх, уже в полосе).
    /// Сетка не меньше 2×2: иначе смешивать нечего.
    pub fn new(
        corner: Vec2,
        cell: f32,
        nx: u32,
        ny: u32,
        band: f32,
        rect: Option<(Vec2, Vec2)>,
        halves: Vec<[u16; 2]>,
    ) -> Self {
        assert_eq!(halves.len(), (nx * ny) as usize, "значений поля слоя");
        let texels = halves
            .iter()
            .flat_map(|[a, b]| {
                let [a0, a1] = a.to_le_bytes();
                let [b0, b1] = b.to_le_bytes();
                [a0, a1, b0, b1]
            })
            .collect();
        Self::from_texels(corner, cell, nx, ny, band, rect, texels, Vec::new())
    }

    /// Поле из готовых байтов картинки (`texels`, 4 байта на точку) и номеров материалов
    /// (`materials`, 2 байта на точку или пусто).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_texels(
        corner: Vec2,
        cell: f32,
        nx: u32,
        ny: u32,
        band: f32,
        rect: Option<(Vec2, Vec2)>,
        texels: Vec<u8>,
        materials: Vec<u8>,
    ) -> Self {
        assert!(nx >= 2 && ny >= 2, "сетка поля слоя {nx}×{ny}");
        let points = nx as usize * ny as usize;
        assert_eq!(texels.len(), points * 4, "значений поля слоя");
        assert!(
            materials.is_empty() || materials.len() == points * 2,
            "номеров материалов"
        );
        assert!(cell > 0.0 && band > 0.0);
        Self {
            corner,
            cell,
            nx,
            ny,
            band,
            rect,
            texels,
            materials,
            second: None,
            texture: 0,
            material_texture: None,
        }
    }

    /// То же поле со вторым слоем `second` (у поля уровня — слой твёрдого).
    pub fn with_second(mut self, second: SecondLayer) -> Self {
        assert!(second.half > 0.0, "толщина второго слоя");
        self.second = Some(second);
        self
    }

    /// Второй слой, если он есть.
    pub fn second(&self) -> Option<SecondLayer> {
        self.second
    }

    /// Байты картинки номеров материалов (R16_UNORM, строки снизу вверх), если они есть.
    pub fn material_bytes(&self) -> Option<&[u8]> {
        (!self.materials.is_empty()).then_some(&self.materials[..])
    }

    /// Ячейка картинки материалов (`None` — её нет или она ещё не загружена).
    pub fn material_texture(&self) -> Option<u32> {
        self.material_texture
    }

    /// Ставит ячейку, в которую загружены [`LayerField::material_bytes`]. Ячейка 0 тоже
    /// настоящая (её отдают заново после удаления других картинок), поэтому в заголовке
    /// хранится ячейка + 1, а 0 там значит «картинки нет».
    pub fn set_material_texture(&mut self, texture: u32) {
        self.material_texture = Some(texture);
    }

    /// Левый нижний угол клетки (0, 0).
    pub fn corner(&self) -> Vec2 {
        self.corner
    }

    /// Номер материала первого слоя в ближайшей к `p` точке сетки (`None` — номеров нет).
    pub fn material_at(&self, p: Vec2) -> Option<u32> {
        if self.materials.is_empty() {
            return None;
        }
        let g = ((p - self.corner) / self.cell).floor();
        let i = (g.x.max(0.0) as u32).min(self.nx - 1);
        let j = (g.y.max(0.0) as u32).min(self.ny - 1);
        let k = (j * self.nx + i) as usize * 2;
        Some(u32::from(u16::from_le_bytes([
            self.materials[k],
            self.materials[k + 1],
        ])))
    }

    /// Расстояние до формы поля в точке `p` её координат (первый слой толщиной ±`half`
    /// по Z с рёбрами, скруглёнными на `round`, второй — по [`SecondLayer`]) и материал
    /// ближайшего слоя: [`ESTIMATE_MATERIAL`], если поле там только оценка; у первого
    /// слоя вблизи формы — из номеров, иначе `material`. Зеркало ветки `PRIM_FIELD` в
    /// `include/sdf3d.slang`.
    pub fn sample(&self, p: Vec3, half: f32, round: f32, material: u32) -> (f32, u32) {
        let xy = Vec2::new(p.x, p.y);
        let (d2, estimate) = self.distance2(xy, 0);
        let mut d = sd_extrude(p, d2, half, round);
        let mut estimate = estimate;
        let mut first = true;
        if let Some(second) = self.second {
            let (d2, e2) = self.distance2(xy, 1);
            let q = Vec3::new(p.x, p.y, p.z - second.shift);
            let ds = sd_extrude(q, d2, second.half, round);
            if ds < d {
                d = ds;
                estimate = e2;
                first = false;
            }
        }
        let material = if estimate {
            ESTIMATE_MATERIAL
        } else if !first {
            self.second.map_or(material, |s| s.material)
        } else if d < MATERIAL_NEAR {
            self.material_at(xy).unwrap_or(material)
        } else {
            material
        };
        (d, material)
    }

    /// Номер картинки в общем списке текстур.
    pub fn texture(&self) -> u32 {
        self.texture
    }

    /// Ставит номер картинки, в которую загружены [`LayerField::bytes`].
    pub fn set_texture(&mut self, texture: u32) {
        self.texture = texture;
    }

    /// Точек по X и по Y.
    pub fn size(&self) -> (u32, u32) {
        (self.nx, self.ny)
    }

    /// Шаг сетки.
    pub fn cell(&self) -> f32 {
        self.cell
    }

    /// Полоса `B`.
    pub fn band(&self) -> f32 {
        self.band
    }

    /// Прямоугольник фигуры (левый нижний и правый верхний углы), если задан: тело формы
    /// не выходит из него.
    pub fn rect(&self) -> Option<(Vec2, Vec2)> {
        self.rect
    }

    /// То же поле без прямоугольника фигуры: за сеткой — только правило края, шара-границы
    /// у формы нет. Так устроено поле целого уровня (7.18в); здесь — чтобы проверить этот
    /// путь на поле одной фигуры.
    pub fn without_rect(mut self) -> Self {
        self.rect = None;
        self
    }

    /// Байты картинки R16G16_SFLOAT: строки снизу вверх, в точке два half (младший байт
    /// первым, как читает видеокарта).
    pub fn bytes(&self) -> &[u8] {
        &self.texels
    }

    /// Слова данных примитива `PRIM_FIELD` (раскладка — в `shape3d::FIELD_PRIM_WORDS`):
    /// номер картинки поля, ячейка картинки материалов + 1 (0 — нет, материал из слова
    /// примитива), угол, шаг, `nx | ny << 16`, прямоугольник фигуры (левый нижний и
    /// правый верхний углы; «нет прямоугольника» — `+∞, −∞`, то есть пустой), середина
    /// второго слоя по Z и его материал (у поля без второго слоя — нули).
    pub fn header_words(&self) -> [u32; FIELD_PRIM_WORDS] {
        let (lo, hi) = self
            .rect
            .unwrap_or((Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)));
        let (shift, material) = self.second.map_or((0.0, 0), |s| (s.shift, s.material));
        let materials = match self.material_texture {
            Some(slot) if !self.materials.is_empty() => slot + 1,
            _ => 0,
        };
        [
            self.texture,
            materials,
            self.corner.x.to_bits(),
            self.corner.y.to_bits(),
            self.cell.to_bits(),
            self.nx | (self.ny << 16),
            lo.x.to_bits(),
            lo.y.to_bits(),
            hi.x.to_bits(),
            hi.y.to_bits(),
            shift.to_bits(),
            material,
        ]
    }

    /// Значение слоя `layer` в точке сетки `(i, j)`.
    pub fn value(&self, i: u32, j: u32, layer: usize) -> f32 {
        let k = (j * self.nx + i) as usize * 4 + layer * 2;
        half_to_f32(u16::from_le_bytes([self.texels[k], self.texels[k + 1]]))
    }

    /// Расстояние до фигуры слоя `layer` в точке `p` (локальные координаты формы) и
    /// признак «только оценка»: поле там не ближе `B − клетка`, настоящая поверхность
    /// может быть дальше, материал не известен. Зеркало `sd2_layer` в
    /// `include/sdf3d.slang`.
    pub fn distance2(&self, p: Vec2, layer: usize) -> (f32, bool) {
        let g = (p - self.corner) / self.cell - 0.5;
        let top = Vec2::new(self.nx as f32 - 1.0, self.ny as f32 - 1.0);
        let gc = g.clamp(Vec2::ZERO, top);
        let e = (g - gc).length() * self.cell;
        let i0 = gc.floor().min(top - 1.0);
        let t = gc - i0;
        let (i, j) = (i0.x as u32, i0.y as u32);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let v = lerp(
            lerp(self.value(i, j, layer), self.value(i + 1, j, layer), t.x),
            lerp(
                self.value(i, j + 1, layer),
                self.value(i + 1, j + 1, layer),
                t.x,
            ),
            t.y,
        );
        let estimate = v >= self.band - self.cell;
        if e > 0.0 {
            let vp = v.max(0.0);
            let mut d = (e * e + vp * vp).sqrt();
            if let Some((lo, hi)) = self.rect {
                d = d.max((lo - p).max(p - hi).max(Vec2::ZERO).length());
            }
            (d, estimate)
        } else {
            (v, estimate)
        }
    }
}

impl std::fmt::Debug for LayerField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "LayerField({}×{} по {} м, картинка {})",
            self.nx, self.ny, self.cell, self.texture
        )
    }
}

/// f32 → half float (IEEE 754, 16 бит) с усечением мантиссы к нулю: число по модулю не
/// растёт. Слишком большое становится наибольшим конечным half, слишком малое — нулём
/// (знак сохраняется).
pub fn f32_to_half_toward_zero(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    if x.is_nan() {
        return sign | 0x7e00;
    }
    let exp = ((bits >> 23) & 0xff) as i32 - 127;
    let mant = bits & 0x7f_ffff;
    if exp > 15 {
        return sign | 0x7bff;
    }
    if exp >= -14 {
        return sign | (((exp + 15) as u16) << 10) | (mant >> 13) as u16;
    }
    if exp >= -24 {
        // Субнормальные half: значение = m · 2⁻²⁴, m — число без скрытой единицы.
        let m = (mant | 0x80_0000) >> (-1 - exp);
        return sign | m as u16;
    }
    sign
}

/// half float (IEEE 754, 16 бит) в f32.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let frac = f32::from(h & 0x3ff);
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 if frac == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
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

    #[test]
    fn half_round_trip_never_grows_and_stays_within_an_ulp() {
        let mut seed = 7u32;
        for k in 0..20_000 {
            let x = match k % 4 {
                0 => (xorshift(&mut seed) - 0.5) * 2.0,
                1 => (xorshift(&mut seed) - 0.5) * 1e-3,
                2 => (xorshift(&mut seed) - 0.5) * 1e-6,
                _ => (xorshift(&mut seed) - 0.5) * 100.0,
            };
            let y = half_to_f32(f32_to_half_toward_zero(x));
            assert!(y.abs() <= x.abs(), "{x} → {y}: модуль вырос");
            assert!(
                y == 0.0 || y.is_sign_negative() == x.is_sign_negative(),
                "{x} → {y}"
            );
            // Усечение теряет не больше одной единицы последнего разряда half (2⁻¹⁰
            // относительно) у нормальных чисел и не больше 2⁻²⁴ у субнормальных.
            assert!(
                (x - y).abs() <= x.abs() * 2f32.powi(-10) + 2f32.powi(-24),
                "{x} → {y}"
            );
        }
        // Точно представимые числа не меняются; переполнение — наибольшее конечное.
        for x in [0.0, 1.0, -1.0, 0.5, 0.25, 1024.0, -0.0078125] {
            assert_eq!(half_to_f32(f32_to_half_toward_zero(x)), x);
        }
        assert_eq!(f32_to_half_toward_zero(1e9), 0x7bff);
        assert_eq!(half_to_f32(0x7bff), 65504.0);
        assert_eq!(f32_to_half_toward_zero(-1e-30), 0x8000);
        assert_eq!(f32_to_half_toward_zero(0.0), 0);
        assert!(half_to_f32(f32_to_half_toward_zero(f32::NAN)).is_nan());
    }

    /// Внутри сетки поле слоя — то же, что поле профиля, с точностью усечения до half.
    #[test]
    fn inside_the_grid_the_layer_matches_the_profile() {
        let profile = Profile::new(STAR).unwrap();
        let field = LayerField::from_profile(&profile, 1.0);
        let (nx, ny) = field.size();
        assert_eq!(
            f32::from_bits(profile.words()[3] & 0xffff),
            f32::from_bits(nx)
        );
        assert_eq!(profile.words()[3] >> 16, ny);
        let w = field.header_words();
        assert_eq!(w[5], nx | (ny << 16));
        assert_eq!(f32::from_bits(w[4]), field.cell());
        let (lo, hi) = profile.bounds();
        assert_eq!(f32::from_bits(w[6]), lo.x);
        assert_eq!(f32::from_bits(w[9]), hi.y);
        let mut seed = 3u32;
        let mut checked = 0;
        for _ in 0..5000 {
            let q = Vec2::new(
                xorshift(&mut seed) * 3.0 - 1.5,
                xorshift(&mut seed) * 3.0 - 1.5,
            );
            let (got, estimate) = field.distance2(q, 0);
            let want = profile.distance(q);
            assert!(
                !estimate,
                "{q}: на собственной сетке поле всегда знает ответ"
            );
            // Снаружи сетки обе формулы совпадают с точностью half значения на краю.
            assert!(
                (got - want).abs() <= want.abs() * 2f32.powi(-10) + 1e-5,
                "{q}: поле слоя {got}, профиль {want}"
            );
            // Усечение — к контуру: по модулю поле слоя не больше профиля. У самого
            // контура смешивание усечённых соседей разного знака может дать чуть больший
            // модуль — на долю последнего разряда half значений соседей.
            assert!(
                got.abs() <= want.abs() + field.cell() * 2f32.powi(-9),
                "{q}: поле слоя {got} дальше профиля {want}"
            );
            checked += 1;
        }
        assert!(checked == 5000);
        // Второй слой — пустой: всюду +B.
        assert_eq!(field.distance2(Vec2::ZERO, 1).0, 1.0);
    }

    /// За сеткой: не меньше расстояния до прямоугольника фигуры (условие 0020), не
    /// меньше оценки от края и не больше настоящего расстояния плюс доля клетки (иначе
    /// у конца коридора стояла бы стена, которой нет).
    #[test]
    fn outside_the_grid_the_layer_is_a_tight_lower_bound() {
        let profile = Profile::new(STAR).unwrap();
        let contours = flatten_path(STAR);
        let field = LayerField::from_profile(&profile, 1.0);
        let cell = field.cell();
        let (lo, hi) = profile.bounds();
        let (nx, ny) = field.size();
        // Крайние точки сетки положительные (фигура не касается края).
        for i in 0..nx {
            assert!(field.value(i, 0, 0) > 0.0 && field.value(i, ny - 1, 0) > 0.0);
        }
        for j in 0..ny {
            assert!(field.value(0, j, 0) > 0.0 && field.value(nx - 1, j, 0) > 0.0);
        }
        let mut seed = 11u32;
        let mut outside = 0;
        for _ in 0..10_000 {
            let q = Vec2::new(
                xorshift(&mut seed) * 8.0 - 4.0,
                xorshift(&mut seed) * 8.0 - 4.0,
            );
            let to_rect = (lo - q).max(q - hi).max(Vec2::ZERO).length();
            if to_rect <= 0.0 {
                continue;
            }
            outside += 1;
            let (d, _) = field.distance2(q, 0);
            let want = exact_distance(&contours, q);
            assert!(
                d >= to_rect - 1e-5,
                "{q}: {d} меньше расстояния до прямоугольника {to_rect}"
            );
            assert!(d <= want + 0.7 * cell, "{q}: {d} больше настоящего {want}");
        }
        assert!(outside > 8000, "точек за прямоугольником: {outside}");
        // Напротив правого луча звезды (он лежит на прямоугольнике) оценка точная:
        // стены, которой нет, за ним не возникает.
        for k in 1..=30 {
            let s = k as f32 * 0.1;
            let q = Vec2::new(0.95 + s, 0.31);
            let (d, _) = field.distance2(q, 0);
            assert!((d - s).abs() <= 0.7 * cell, "{q}: {d} вместо {s}");
        }
    }

    /// Без прямоугольника фигуры (поле уровня, 7.18в) за сеткой остаётся ровно
    /// `√(e² + v²)`; заголовок говорит «прямоугольника нет» пустым прямоугольником.
    #[test]
    fn without_a_rectangle_only_the_edge_rule_applies() {
        let profile = Profile::new("M -1 -1 L 1 -1 L 1 1 L -1 1 Z").unwrap();
        let with = LayerField::from_profile(&profile, 1.0);
        let (nx, ny) = with.size();
        let halves: Vec<[u16; 2]> = (0..ny)
            .flat_map(|j| (0..nx).map(move |i| (i, j)))
            .map(|(i, j)| {
                [
                    f32_to_half_toward_zero(with.value(i, j, 0)),
                    f32_to_half_toward_zero(1.0),
                ]
            })
            .collect();
        let f = |i: usize| f32::from_bits(with.header_words()[i]);
        let without = LayerField::new(
            Vec2::new(f(2), f(3)),
            with.cell(),
            nx,
            ny,
            1.0,
            None,
            halves,
        );
        let w = without.header_words();
        assert!(
            f32::from_bits(w[6]) > f32::from_bits(w[8]),
            "пустой прямоугольник"
        );
        // Далеко по диагонали: у квадрата настоящее расстояние до угла, у поля без
        // прямоугольника — только оценка от края сетки.
        let q = Vec2::new(4.0, 4.0);
        let (a, _) = with.distance2(q, 0);
        let (b, _) = without.distance2(q, 0);
        assert!((a - (q - Vec2::ONE).length()).abs() < 1e-3, "{a}");
        assert!(b < a && b > 0.0, "{b} против {a}");
        // А внутри сетки обе одинаковы.
        for q in [Vec2::ZERO, Vec2::new(0.7, -0.4), Vec2::new(1.0, 0.0)] {
            assert_eq!(with.distance2(q, 0), without.distance2(q, 0));
        }
    }

    /// У узкой полосы поле за ней — только оценка: признак выставлен, где значение
    /// обрезано до `B`, и нигде рядом с фигурой.
    #[test]
    fn beyond_the_band_the_field_is_only_an_estimate() {
        let profile = Profile::new("M -1 -1 L 1 -1 L 1 1 L -1 1 Z").unwrap();
        let band = 0.05;
        let field = LayerField::from_profile(&profile, band);
        let cell = field.cell();
        // В 0,5 м от квадрата значение обрезано полосой — признак стоит, хотя
        // прямоугольник фигуры тут даёт точное расстояние (признак осторожный).
        let (far, estimate) = field.distance2(Vec2::new(1.5, 0.0), 0);
        assert!(estimate, "в 0,5 м от края поле не знает");
        assert!((far - 0.5).abs() < 1e-3, "{far}: по прямоугольнику фигуры");
        // Без прямоугольника остаётся оценка от края сетки.
        let (nx, ny) = field.size();
        let f = |i: usize| f32::from_bits(field.header_words()[i]);
        let halves = (0..ny)
            .flat_map(|j| (0..nx).map(move |i| (i, j)))
            .map(|(i, j)| {
                [
                    f32_to_half_toward_zero(field.value(i, j, 0)),
                    f32_to_half_toward_zero(band),
                ]
            })
            .collect();
        let bare = LayerField::new(Vec2::new(f(2), f(3)), cell, nx, ny, band, None, halves);
        let (far, estimate) = bare.distance2(Vec2::new(1.5, 0.0), 0);
        assert!(estimate && far < 0.5 && far > band, "{far}");
        let (near, estimate) = field.distance2(Vec2::new(1.0 + 0.4 * cell, 0.0), 0);
        assert!(!estimate && near < band - cell, "{near}");
        let (inside, estimate) = field.distance2(Vec2::ZERO, 0);
        assert!(!estimate && (inside + band).abs() < 1e-3, "{inside}");
        assert_eq!(
            field.bytes().len(),
            (field.size().0 * field.size().1 * 4) as usize
        );
    }
}
