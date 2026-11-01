//! Поля расстояний 2D (SDF): для точки `p` функция фигуры возвращает расстояние до её
//! края. Внутри фигуры число отрицательное, снаружи положительное, на краю ноль.
//!
//! Это зеркало шейдерной библиотеки `crates/reyvec-gpu/shaders/include/sdf2d.slang`:
//! те же формулы и те же имена, только на процессоре. Видеокарта по этим формулам
//! рисует свет, тени и мягкие фигуры (этап 4), а процессору они пригодятся для игры:
//! попала ли точка в фигуру, как далеко до стены (этап 6). Тесты здесь проверяют сами
//! формулы; что шейдер посчитал так же, проверяет эталонная картинка сцены «свет».
//!
//! Важно: после плавных операций (`op_smooth_*`) и обводки результат остаётся «полем
//! снизу»: расстояние не больше настоящего, поэтому по нему можно безопасно шагать
//! (реймарчинг теней не проскочит край), но точным оно бывает не везде.

/// Точка или вектор на плоскости. Y смотрит вверх (соглашение движка).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V2 {
    pub x: f32,
    pub y: f32,
}

impl V2 {
    pub const ZERO: Self = Self::new(0.0, 0.0);

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs())
    }

    fn max0(self) -> Self {
        Self::new(self.x.max(0.0), self.y.max(0.0))
    }
}

impl std::ops::Add for V2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for V2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl std::ops::Mul<f32> for V2 {
    type Output = Self;
    fn mul(self, k: f32) -> Self {
        Self::new(self.x * k, self.y * k)
    }
}

/// Круг радиуса `r` с центром в нуле.
pub fn sd_circle(p: V2, r: f32) -> f32 {
    p.length() - r
}

/// Прямоугольник с центром в нуле и половинами сторон `half`.
pub fn sd_box(p: V2, half: V2) -> f32 {
    let d = p.abs() - half;
    d.max0().length() + d.x.max(d.y).min(0.0)
}

/// Прямоугольник со скруглёнными углами радиуса `radius` (внешний размер как у
/// [`sd_box`] с теми же `half`).
pub fn sd_round_box(p: V2, half: V2, radius: f32) -> f32 {
    let r = radius.min(half.x).min(half.y).max(0.0);
    sd_box(p, half - V2::new(r, r)) - r
}

/// Отрезок от `a` до `b` (толщина ноль: расстояние всегда не меньше нуля). Толстая
/// линия — [`op_round`] от него.
pub fn sd_segment(p: V2, a: V2, b: V2) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let len2 = ba.dot(ba);
    let h = if len2 > 0.0 {
        (pa.dot(ba) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (pa - ba * h).length()
}

/// Квадратичная кривая Безье `a` → `c` с управляющей точкой `b` (как у векторов и SVG).
/// Точное расстояние (без знака): ищется ближайшая точка кривой решением кубического
/// уравнения (формула Иниго Килеса). Если кривая вырождается в прямую, считается отрезок.
pub fn sd_bezier(p: V2, a: V2, b: V2, c: V2) -> f32 {
    let aa = b - a;
    let bb = a - b * 2.0 + c;
    let cc = aa * 2.0;
    let d = a - p;
    let bb2 = bb.dot(bb);
    if bb2 < 1e-10 {
        return sd_segment(p, a, c);
    }
    let kk = 1.0 / bb2;
    let kx = kk * aa.dot(bb);
    let ky = kk * (2.0 * aa.dot(aa) + d.dot(bb)) / 3.0;
    let kz = kk * d.dot(aa);
    let pp = ky - kx * kx;
    let p3 = pp * pp * pp;
    let q = kx * (2.0 * kx * kx - 3.0 * ky) + kz;
    let h = q * q + 4.0 * p3;
    let at = |t: f32| {
        let v = d + (cc + bb * t) * t;
        v.dot(v)
    };
    let res = if h >= 0.0 {
        let h = h.sqrt();
        let x = V2::new((h - q) / 2.0, (-h - q) / 2.0);
        let cbrt = |v: f32| v.signum() * v.abs().powf(1.0 / 3.0);
        let t = (cbrt(x.x) + cbrt(x.y) - kx).clamp(0.0, 1.0);
        at(t)
    } else {
        let z = (-pp).sqrt();
        let v = (q / (pp * z * 2.0)).clamp(-1.0, 1.0).acos() / 3.0;
        let m = v.cos();
        let n = v.sin() * 1.732_050_8;
        let t0 = ((m + m) * z - kx).clamp(0.0, 1.0);
        let t1 = ((-n - m) * z - kx).clamp(0.0, 1.0);
        at(t0).min(at(t1))
    };
    res.sqrt()
}

/// Объединение: обе фигуры сразу.
pub fn op_union(a: f32, b: f32) -> f32 {
    a.min(b)
}

/// Вырезание: фигура `a` без фигуры `b`.
pub fn op_subtract(a: f32, b: f32) -> f32 {
    a.max(-b)
}

/// Пересечение: только общая часть.
pub fn op_intersect(a: f32, b: f32) -> f32 {
    a.max(b)
}

/// Плавное объединение: фигуры сливаются, как капли. `k` — ширина перехода (в тех же
/// единицах, что и расстояния); при `k <= 0` это обычное [`op_union`]. Дальше `k` друг
/// от друга фигуры не влияют одна на другую.
pub fn op_smooth_union(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

/// Плавное вырезание `b` из `a` с переходом ширины `k`.
pub fn op_smooth_subtract(a: f32, b: f32, k: f32) -> f32 {
    -op_smooth_union(-a, b, k)
}

/// Плавное пересечение с переходом ширины `k`.
pub fn op_smooth_intersect(a: f32, b: f32, k: f32) -> f32 {
    -op_smooth_union(-a, -b, k)
}

/// Скругление: фигура раздувается на `r` (у отрезка и кривой это толщина линии).
pub fn op_round(d: f32, r: f32) -> f32 {
    d - r
}

/// Кольцо по краю фигуры толщиной `2 * half_width` (обводка).
pub fn op_onion(d: f32, half_width: f32) -> f32 {
    d.abs() - half_width
}

/// Поворот точки на угол `-angle`: чтобы повернуть фигуру на `angle`, её функции дают
/// точку, повёрнутую в обратную сторону.
pub fn rotate(p: V2, angle: f32) -> V2 {
    let (s, c) = angle.sin_cos();
    V2::new(c * p.x + s * p.y, -s * p.x + c * p.y)
}

/// Доля пикселя, закрытая фигурой: 1 внутри, 0 снаружи, плавно на краю шириной в
/// пиксель (`pixel` — размер пикселя в единицах сцены). Так SDF-фигуры сглаживаются сами,
/// без MSAA, и остаются чёткими при любом приближении.
pub fn coverage(d: f32, pixel: f32) -> f32 {
    (0.5 - d / pixel.max(1e-12)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn circle_and_box() {
        assert!(close(sd_circle(V2::new(3.0, 4.0), 2.0), 3.0, 1e-6));
        assert!(close(sd_circle(V2::ZERO, 2.0), -2.0, 1e-6));
        let half = V2::new(2.0, 1.0);
        assert!(close(sd_box(V2::new(3.0, 0.0), half), 1.0, 1e-6));
        assert!(close(sd_box(V2::new(0.0, 0.0), half), -1.0, 1e-6));
        // Снаружи угла расстояние до самого угла.
        assert!(close(sd_box(V2::new(5.0, 5.0), half), 5.0, 1e-5));
        assert!(close(sd_round_box(V2::new(3.0, 0.0), half, 0.5), 1.0, 1e-6));
        // Скруглённый угол дальше острого.
        let corner = V2::new(2.5, 1.5);
        assert!(sd_round_box(corner, half, 0.5) > sd_box(corner, half));
        // Радиус больше половины стороны не ломает фигуру: выходит «таблетка».
        assert!(close(sd_round_box(V2::new(0.0, 2.0), half, 5.0), 1.0, 1e-6));
    }

    #[test]
    fn segment() {
        let (a, b) = (V2::new(-1.0, 0.0), V2::new(1.0, 0.0));
        assert!(close(sd_segment(V2::new(0.0, 2.0), a, b), 2.0, 1e-6));
        assert!(close(sd_segment(V2::new(4.0, 0.0), a, b), 3.0, 1e-6));
        // Отрезок нулевой длины — точка.
        assert!(close(
            sd_segment(V2::new(0.0, 3.0), a, a),
            10f32.sqrt(),
            1e-6
        ));
    }

    /// Расстояние до кривой перебором её точек: медленно, но заведомо верно.
    fn bezier_brute(p: V2, a: V2, b: V2, c: V2) -> f32 {
        (0..=20_000)
            .map(|i| {
                let t = i as f32 / 20_000.0;
                let u = 1.0 - t;
                let q = a * (u * u) + b * (2.0 * u * t) + c * (t * t);
                (q - p).length()
            })
            .fold(f32::MAX, f32::min)
    }

    #[test]
    fn bezier_matches_brute_force() {
        let curves = [
            (V2::new(-1.0, 0.0), V2::new(0.0, 1.5), V2::new(1.0, 0.0)),
            (V2::new(0.0, 0.0), V2::new(2.0, 0.0), V2::new(2.0, 2.0)),
            (V2::new(-0.5, -0.5), V2::new(0.7, 0.9), V2::new(0.2, -0.8)),
        ];
        let mut checked = 0;
        for (a, b, c) in curves {
            for iy in -6..=6 {
                for ix in -6..=6 {
                    let p = V2::new(ix as f32 * 0.37, iy as f32 * 0.29);
                    let exact = sd_bezier(p, a, b, c);
                    let brute = bezier_brute(p, a, b, c);
                    assert!(
                        close(exact, brute, 2e-3),
                        "кривая {a:?} {b:?} {c:?}, точка {p:?}: {exact} а перебором {brute}"
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 3 * 13 * 13);
    }

    #[test]
    fn straight_bezier_is_segment() {
        let (a, c) = (V2::new(-1.0, 0.0), V2::new(1.0, 0.0));
        let b = V2::new(0.0, 0.0);
        let p = V2::new(0.3, 0.8);
        assert!(close(sd_bezier(p, a, b, c), sd_segment(p, a, c), 1e-6));
    }

    #[test]
    fn boolean_operations() {
        assert_eq!(op_union(1.0, -2.0), -2.0);
        assert_eq!(op_intersect(1.0, -2.0), 1.0);
        // Точка внутри b вырезана из a.
        assert_eq!(op_subtract(-1.0, -0.5), 0.5);
        assert_eq!(op_round(1.0, 0.25), 0.75);
        assert_eq!(op_onion(-1.0, 0.25), 0.75);
    }

    #[test]
    fn smooth_operations() {
        let k = 0.5;
        // Далеко друг от друга (разница больше k): как обычные операции.
        assert_eq!(op_smooth_union(0.1, 2.0, k), 0.1);
        assert_eq!(op_smooth_intersect(0.1, 2.0, k), 2.0);
        assert_eq!(op_smooth_subtract(2.0, -0.1, k), 2.0);
        // Рядом: плавный переход, и он никогда не дальше обычного объединения
        // (поле «снизу», шагать по нему безопасно).
        for i in 0..100 {
            let a = -1.0 + i as f32 * 0.02;
            let b = 0.3;
            let s = op_smooth_union(a, b, k);
            assert!(s <= a.min(b));
            assert!(s >= a.min(b) - k * 0.25 - 1e-6);
            assert!(op_smooth_intersect(a, b, k) >= a.max(b));
        }
        // k = 0: ровно обычные операции.
        assert_eq!(op_smooth_union(0.2, 0.3, 0.0), 0.2);
        assert_eq!(op_smooth_subtract(0.2, 0.3, 0.0), op_subtract(0.2, 0.3));
    }

    #[test]
    fn rotation_and_coverage() {
        let q = rotate(V2::new(1.0, 0.0), std::f32::consts::FRAC_PI_2);
        assert!(close(q.x, 0.0, 1e-6) && close(q.y, -1.0, 1e-6));
        assert_eq!(coverage(-1.0, 0.01), 1.0);
        assert_eq!(coverage(1.0, 0.01), 0.0);
        assert!(close(coverage(0.0, 0.01), 0.5, 1e-6));
    }
}
