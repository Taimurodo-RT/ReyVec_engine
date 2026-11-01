//! Объекты целиком из SDF (этап 4, задача 4.5): слизь, капли, магия.
//!
//! Объект — несколько простых частей (кругов и «капсул» — отрезков с толщиной),
//! слитых плавно: где части близко, между ними вырастает перемычка, как у капель воды
//! или слизи. Форму в каждой точке считает видеокарта (`shaders/sdf_object.slang`) только
//! внутри прямоугольника объекта, а процессор каждый кадр пишет короткий список частей.
//! Объекты освещаются лампами, отбрасывают мягкие тени (рисуются в маску теней), а
//! светящиеся сами светят и дают свечение (4.4).

#[cfg(test)]
use reyvec_sdf::{V2, op_smooth_union, sd_segment};

use crate::lights::Light;

/// Сколько объектов и частей видеокарта берёт за кадр.
pub(crate) const MAX_OBJECTS: usize = 16;
pub(crate) const MAX_BLOBS: usize = 128;
/// Размеры записей в буферах (как `struct Object` и `struct Blob` в `sdf_object.slang`).
pub(crate) const OBJECT_SIZE: usize = 48;
pub(crate) const BLOB_SIZE: usize = 32;

/// Объект светится сам: не затеняется и ярче белого (его подхватывает свечение).
pub(crate) const OBJECT_EMISSIVE: u32 = 1;
/// Объект отбрасывает тень (попадает в маску теней).
pub(crate) const OBJECT_CASTS_SHADOW: u32 = 2;

/// Часть объекта: капсула от `a` до `b` толщиной `r` (у круга `a == b`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Blob {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub r: f32,
}

impl Blob {
    pub fn circle(at: [f32; 2], r: f32) -> Self {
        Self { a: at, b: at, r }
    }

    pub fn capsule(a: [f32; 2], b: [f32; 2], r: f32) -> Self {
        Self { a, b, r }
    }

    pub fn to_bytes(self) -> [u8; BLOB_SIZE] {
        let words = [
            self.a[0], self.a[1], self.b[0], self.b[1], self.r, 0.0, 0.0, 0.0,
        ];
        let mut out = [0u8; BLOB_SIZE];
        for (i, w) in words.iter().enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&w.to_ne_bytes());
        }
        out
    }

    /// Расстояние со знаком от точки `p` (внутри отрицательное). Зеркало шейдера (для
    /// проверок на процессоре).
    #[cfg(test)]
    pub fn distance(&self, p: [f32; 2]) -> f32 {
        let v = |q: [f32; 2]| V2::new(q[0], q[1]);
        sd_segment(v(p), v(self.a), v(self.b)) - self.r
    }
}

/// Объект из частей `blobs`, слитых с плавностью `k` (размер перемычки, единицы сцены).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SdfObject {
    pub blobs: Vec<Blob>,
    pub k: f32,
    /// Цвет (линейный); у светящихся может быть ярче 1.
    pub color: [f32; 4],
    pub flags: u32,
}

impl SdfObject {
    /// Прямоугольник, в котором объект может быть виден: части плюс перемычки и запас
    /// на мягкий край.
    pub fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let mut lo = [f32::MAX; 2];
        let mut hi = [f32::MIN; 2];
        for b in &self.blobs {
            for p in [b.a, b.b] {
                for i in 0..2 {
                    lo[i] = lo[i].min(p[i] - b.r);
                    hi[i] = hi[i].max(p[i] + b.r);
                }
            }
        }
        let pad = self.k * 0.5 + 0.02;
        ([lo[0] - pad, lo[1] - pad], [hi[0] + pad, hi[1] + pad])
    }

    /// Расстояние со знаком от точки `p` с плавным слиянием частей. Зеркало шейдера (для
    /// проверок на процессоре).
    #[cfg(test)]
    pub fn distance(&self, p: [f32; 2]) -> f32 {
        let mut d = 1e9f32;
        for b in &self.blobs {
            d = op_smooth_union(d, b.distance(p), self.k);
        }
        d
    }
}

/// Записи объектов и частей для буферов видеокарты (не больше [`MAX_OBJECTS`] и
/// [`MAX_BLOBS`]; лишнее отбрасывается).
pub(crate) fn pack(objects: &[SdfObject]) -> (Vec<u8>, Vec<u8>, u32) {
    let mut object_bytes = Vec::new();
    let mut blob_bytes = Vec::new();
    let mut first = 0usize;
    let mut count = 0u32;
    for o in objects.iter().take(MAX_OBJECTS) {
        let n = o.blobs.len().min(MAX_BLOBS - first);
        if n == 0 {
            break;
        }
        for b in &o.blobs[..n] {
            blob_bytes.extend_from_slice(&b.to_bytes());
        }
        let (lo, hi) = o.bounds();
        let words: [u32; OBJECT_SIZE / 4] = [
            first as u32,
            n as u32,
            o.k.to_bits(),
            o.flags,
            o.color[0].to_bits(),
            o.color[1].to_bits(),
            o.color[2].to_bits(),
            o.color[3].to_bits(),
            lo[0].to_bits(),
            lo[1].to_bits(),
            hi[0].to_bits(),
            hi[1].to_bits(),
        ];
        for w in words {
            object_bytes.extend_from_slice(&w.to_ne_bytes());
        }
        first += n;
        count += 1;
    }
    (object_bytes, blob_bytes, count)
}

/// Центр магического шара в момент `t`.
fn orb_center(t: f32) -> [f32; 2] {
    [0.0, 0.3 + 0.05 * (t * 1.5).sin()]
}

/// Объекты сцены «слизь и магия» в момент `t` (секунды).
/// - Слизь слева: тело из нескольких покачивающихся кругов, снизу растёт капля, отрывается
///   и падает в лужицу.
/// - Дождь справа: капли падают, вытягиваясь от скорости, и сливаются с лужей.
/// - Магический шар в середине: светится сам, вокруг кружат искры, сливаясь с ним, когда
///   подлетают близко.
pub(crate) fn demo_objects(t: f32) -> Vec<SdfObject> {
    let mut objects = Vec::new();

    // Слизь.
    let s = |f: f32, ph: f32| (t * f + ph).sin();
    let mut slime = vec![
        Blob::circle([-0.8 + 0.03 * s(2.0, 0.0), -0.42], 0.17),
        Blob::circle([-0.63, -0.47 + 0.03 * s(2.3, 1.0)], 0.12),
        Blob::circle([-0.97, -0.48 + 0.02 * s(1.7, 2.0)], 0.11),
        Blob::circle(
            [-0.78 + 0.04 * s(1.3, 0.0), -0.27 + 0.02 * s(3.0, 0.0)],
            0.08,
        ),
    ];
    // Капля: 2,4 секунды растёт снизу тела, потом падает до лужи.
    let period = 2.4;
    let phase = t.rem_euclid(period + 0.6);
    let drip_x = -0.74;
    let top = -0.56;
    if phase < period {
        let grow = phase / period;
        let end = top - 0.03 - 0.1 * grow;
        slime.push(Blob::capsule(
            [drip_x, top],
            [drip_x, end],
            0.025 + 0.01 * grow,
        ));
        slime.push(Blob::circle([drip_x, end], 0.02 + 0.025 * grow));
    } else {
        let fall = phase - period;
        let y = top - 0.13 - 0.5 * 3.0 * fall * fall;
        slime.push(Blob::capsule(
            [drip_x, y],
            [drip_x, y + 0.02 + 0.08 * fall],
            0.04,
        ));
    }
    // Лужица слизи у пола.
    slime.push(Blob::capsule([-0.86, -0.97], [-0.66, -0.97], 0.025));
    slime.push(Blob::circle([-0.74, -0.96], 0.04));
    objects.push(SdfObject {
        blobs: slime,
        k: 0.12,
        color: [0.22, 0.8, 0.3, 1.0],
        flags: OBJECT_CASTS_SHADOW,
    });

    // Дождь в лужу справа.
    let mut rain = vec![
        Blob::capsule([0.78, -0.6], [1.55, -0.6], 0.035),
        Blob::circle([1.0, -0.6], 0.05),
        Blob::circle([1.3, -0.61], 0.045),
    ];
    let cycle = 1.6;
    let g = 2.0 * 1.45 / (cycle * cycle);
    for i in 0..5 {
        let x = 0.85 + 0.16 * i as f32;
        let ph = (t + i as f32 * 0.37).rem_euclid(cycle);
        let y = 0.85 - 0.5 * g * ph * ph;
        let speed = g * ph;
        let tail = 0.01 + 0.04 * speed;
        rain.push(Blob::capsule([x, y], [x, y + tail], 0.028));
    }
    objects.push(SdfObject {
        blobs: rain,
        k: 0.08,
        color: [0.3, 0.55, 1.0, 1.0],
        flags: OBJECT_CASTS_SHADOW,
    });

    // Магический шар и искры.
    let c = orb_center(t);
    let mut orb = vec![Blob::circle(c, 0.085 + 0.01 * (t * 4.0).sin())];
    for i in 0..5 {
        let a = t * 1.6 + i as f32 * std::f32::consts::TAU / 5.0;
        let r = 0.2 + 0.07 * (t * 1.1 + i as f32 * 1.7).sin();
        orb.push(Blob::circle(
            [c[0] + r * a.cos(), c[1] + r * a.sin() * 0.7],
            0.028,
        ));
    }
    objects.push(SdfObject {
        blobs: orb,
        k: 0.1,
        color: [0.75, 0.35, 1.0, 1.0],
        flags: OBJECT_EMISSIVE,
    });
    objects
}

/// Источники сцены «слизь и магия»: тёплая лампа сверху слева, свет магического шара и
/// слабый лунный свет.
pub(crate) fn demo_lights(t: f32) -> Vec<Light> {
    let pulse = 1.0 + 0.15 * (t * 4.0).sin();
    vec![
        Light::point([-1.35, 0.65], [1.6, 1.15, 0.7], 2.2, 0.06),
        Light::point(
            orb_center(t),
            [0.9 * pulse, 0.45 * pulse, 1.5 * pulse],
            1.1,
            0.04,
        ),
        Light::directional([-0.6, 0.8], [0.12, 0.15, 0.25], 1.0, 0.12),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_shader() {
        let b = Blob::capsule([1.0, 2.0], [3.0, 4.0], 5.0).to_bytes();
        let f = |i: usize| f32::from_ne_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
        assert_eq!([f(0), f(1), f(2), f(3), f(4)], [1.0, 2.0, 3.0, 4.0, 5.0]);
        let objects = demo_objects(0.7);
        let (o, blobs, count) = pack(&objects);
        assert_eq!(count as usize, objects.len());
        assert_eq!(o.len(), objects.len() * OBJECT_SIZE);
        let total: usize = objects.iter().map(|o| o.blobs.len()).sum();
        assert_eq!(blobs.len(), total * BLOB_SIZE);
        // Вторая запись начинается после частей первой.
        let first = u32::from_ne_bytes(o[OBJECT_SIZE..OBJECT_SIZE + 4].try_into().unwrap());
        assert_eq!(first as usize, objects[0].blobs.len());
    }

    #[test]
    fn close_parts_merge_and_far_parts_do_not() {
        let k = 0.1;
        // Два круга радиусом 0,1 с зазором 0,04: посередине без слияния снаружи,
        // с плавным слиянием внутри (перемычка).
        let two = |gap: f32| SdfObject {
            blobs: vec![
                Blob::circle([-0.1 - gap / 2.0, 0.0], 0.1),
                Blob::circle([0.1 + gap / 2.0, 0.0], 0.1),
            ],
            k,
            color: [1.0; 4],
            flags: 0,
        };
        assert!(two(0.04).distance([0.0, 0.0]) < 0.0, "близкие сливаются");
        assert!(two(0.3).distance([0.0, 0.0]) > 0.1, "далёкие нет");
        // Вдали от перемычки форма не меняется.
        let d = two(0.3).distance([-0.35, 0.0]);
        assert!((d - (0.35 - 0.25 - 0.1)).abs() < 1e-5);
    }

    #[test]
    fn bounds_contain_the_shape() {
        for t in [0.0f32, 1.3, 2.5, 2.8, 5.0] {
            for o in demo_objects(t) {
                let (lo, hi) = o.bounds();
                // На краях прямоугольника и за ним объекта нет.
                for i in 0..=20 {
                    let s = i as f32 / 20.0;
                    for p in [
                        [lo[0] + (hi[0] - lo[0]) * s, lo[1]],
                        [lo[0] + (hi[0] - lo[0]) * s, hi[1]],
                        [lo[0], lo[1] + (hi[1] - lo[1]) * s],
                        [hi[0], lo[1] + (hi[1] - lo[1]) * s],
                    ] {
                        assert!(o.distance(p) > 0.0, "t {t}: край {p:?} внутри объекта");
                    }
                }
            }
        }
    }

    #[test]
    fn counts_fit_the_buffers() {
        for t in [0.0f32, 1.0, 2.6] {
            let objects = demo_objects(t);
            assert!(objects.len() <= MAX_OBJECTS);
            assert!(objects.iter().map(|o| o.blobs.len()).sum::<usize>() <= MAX_BLOBS);
            assert!(demo_lights(t).len() <= crate::lights::MAX_LIGHTS);
        }
    }
}
