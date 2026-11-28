//! Мир для частиц на видеокарте (9.5): препятствия настоящей формы и сетка поиска.
//!
//! Раньше частицы отскакивали только от прямоугольников уровня. Теперь им видны те же
//! препятствия, что и герою: повёрнутые ящики, круги, капсулы, многоугольники, контуры
//! фигур, тела физики и мягкие тела ([`Obstacle`]), и у
//! каждого — скорость (едущий ящик отбрасывает частицы, лифт везёт).
//!
//! Процессор раз в кадр кладёт препятствия у экрана в один буфер слов ([`ParticleWorld`])
//! и раскладывает их по клеткам сетки: в клетке — номера препятствий, которые ближе
//! [`MARGIN`] к ней. Частица смотрит только в клетку, где она была, и в клетку, куда
//! летит. Шейдер `particles.slang` читает те же слова; [`collide`] — его зеркало на
//! процессоре: тесты проверяют на нём отскоки.
//!
//! Слова буфера (все по 4 байта):
//! - заголовок ([`HEADER_WORDS`]): угол сетки x, y, сторона клетки, клеток по x, по y,
//!   где начинаются препятствия, вершины, клетки, номера, сколько препятствий;
//! - препятствие ([`OBSTACLE_WORDS`]): вид, флаги, первая вершина, вершин, 7 чисел формы,
//!   скорость x, y, вращение, середина вращения x, y;
//! - вершины многоугольников: по два числа;
//! - клетки: где в номерах начинается список клетки и сколько в нём;
//! - номера препятствий.

// Зеркало шейдера ([`collide`]) нужно только проверкам.
#![cfg_attr(not(test), allow(dead_code))]

use reyvec_scene::game::{Obstacle, ObstacleShape};

/// Сколько препятствий видит кадр.
pub const MAX_OBSTACLES: usize = 4096;
/// Сколько всего вершин у многоугольников кадра.
pub const MAX_VERTS: usize = 16_384;
/// Сколько всего номеров в клетках.
pub const MAX_REFS: usize = 65_536;
/// Клеток сетки по стороне, не больше.
pub const GRID_MAX: u32 = 64;
/// На сколько частица может улететь за шаг и ещё видеть препятствия своей клетки.
pub const MARGIN: f32 = 0.25;

pub const HEADER_WORDS: usize = 12;
pub const OBSTACLE_WORDS: usize = 16;
/// Всего слов в буфере мира.
pub const WORLD_WORDS: usize = HEADER_WORDS
    + MAX_OBSTACLES * OBSTACLE_WORDS
    + MAX_VERTS * 2
    + (GRID_MAX * GRID_MAX) as usize * 2
    + MAX_REFS;

/// Виды препятствий (как в шейдере).
pub const KIND_BOX: u32 = 0;
pub const KIND_CAPSULE: u32 = 1;
pub const KIND_POLY: u32 = 2;
/// Флаг: держит только сверху.
pub const FLAG_ONEWAY: u32 = 1;

/// Отступ частицы от поверхности после касания.
pub const EPS: f32 = 1e-4;

/// Буфер мира для частиц.
#[derive(Clone, Debug, Default)]
pub struct ParticleWorld {
    pub words: Vec<u32>,
    /// Сколько препятствий у экрана не влезло (больше [`MAX_OBSTACLES`], вершин больше
    /// [`MAX_VERTS`]) и сколько номеров клеток (больше [`MAX_REFS`]).
    pub dropped: usize,
}

impl ParticleWorld {
    /// Мир для частиц в прямоугольнике от `lo` до `hi`: препятствия, которые с ним
    /// пересекаются, по клеткам. Пусто — частицам не обо что удариться.
    pub fn build(obstacles: &[Obstacle], lo: [f32; 2], hi: [f32; 2]) -> Self {
        let size = [(hi[0] - lo[0]).max(1e-3), (hi[1] - lo[1]).max(1e-3)];
        let cell = (size[0].max(size[1]) / GRID_MAX as f32).max(0.05);
        let dims = [
            ((size[0] / cell).ceil() as u32).clamp(1, GRID_MAX),
            ((size[1] / cell).ceil() as u32).clamp(1, GRID_MAX),
        ];
        let mut prims: Vec<u32> = Vec::new();
        let mut verts: Vec<u32> = Vec::new();
        // Клетки, которые задевает каждое взятое препятствие.
        let mut ranges: Vec<[u32; 4]> = Vec::new();
        let mut dropped = 0;
        for o in obstacles {
            let a = o.aabb;
            if a.max[0] < f64::from(lo[0])
                || a.min[0] > f64::from(hi[0])
                || a.max[1] < f64::from(lo[1])
                || a.min[1] > f64::from(hi[1])
            {
                continue;
            }
            if ranges.len() >= MAX_OBSTACLES {
                dropped += 1;
                continue;
            }
            let mut w = [0u32; OBSTACLE_WORDS];
            let f = |v: f32| v.to_bits();
            w[1] = if o.oneway { FLAG_ONEWAY } else { 0 };
            match &o.shape {
                ObstacleShape::Box {
                    center,
                    axis,
                    half,
                    round,
                } => {
                    w[0] = KIND_BOX;
                    for (i, v) in [
                        center[0], center[1], axis[0], axis[1], half[0], half[1], *round,
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        w[4 + i] = f(v);
                    }
                }
                ObstacleShape::Capsule { a, b, r } => {
                    w[0] = KIND_CAPSULE;
                    for (i, v) in [a[0], a[1], b[0], b[1], *r].into_iter().enumerate() {
                        w[4 + i] = f(v);
                    }
                }
                ObstacleShape::Poly(points) => {
                    if points.len() < 3 || verts.len() / 2 + points.len() > MAX_VERTS {
                        dropped += 1;
                        continue;
                    }
                    w[0] = KIND_POLY;
                    w[2] = (verts.len() / 2) as u32;
                    w[3] = points.len() as u32;
                    // Против часовой стрелки: по знаку площади.
                    let area: f32 = (0..points.len())
                        .map(|i| {
                            let (p, q) = (points[i], points[(i + 1) % points.len()]);
                            p[0] * q[1] - q[0] * p[1]
                        })
                        .sum();
                    let ordered: Vec<[f32; 2]> = if area >= 0.0 {
                        points.clone()
                    } else {
                        points.iter().rev().copied().collect()
                    };
                    for p in ordered {
                        verts.push(f(p[0]));
                        verts.push(f(p[1]));
                    }
                }
            }
            for (i, v) in [o.vel[0], o.vel[1], o.spin, o.pivot[0], o.pivot[1]]
                .into_iter()
                .enumerate()
            {
                w[11 + i] = f(v);
            }
            prims.extend_from_slice(&w);
            let cell_of = |v: f64, axis: usize| {
                (((v as f32 - lo[axis]) / cell).floor() as i64).clamp(0, i64::from(dims[axis]) - 1)
                    as u32
            };
            let m = f64::from(MARGIN);
            ranges.push([
                cell_of(a.min[0] - m, 0),
                cell_of(a.min[1] - m, 1),
                cell_of(a.max[0] + m, 0),
                cell_of(a.max[1] + m, 1),
            ]);
        }
        // Номера по клеткам: сначала сколько в каждой, потом сами списки.
        let cells_n = (dims[0] * dims[1]) as usize;
        let mut counts = vec![0u32; cells_n];
        for r in &ranges {
            for y in r[1]..=r[3] {
                for x in r[0]..=r[2] {
                    counts[(y * dims[0] + x) as usize] += 1;
                }
            }
        }
        let mut cells = vec![0u32; cells_n * 2];
        let mut at = 0u32;
        for i in 0..cells_n {
            let c = counts[i];
            let take = c.min(MAX_REFS as u32 - at);
            dropped += (c - take) as usize;
            cells[i * 2] = at;
            cells[i * 2 + 1] = 0;
            at += take;
            counts[i] = take;
        }
        let mut refs = vec![0u32; at as usize];
        for (k, r) in ranges.iter().enumerate() {
            for y in r[1]..=r[3] {
                for x in r[0]..=r[2] {
                    let i = (y * dims[0] + x) as usize;
                    if cells[i * 2 + 1] < counts[i] {
                        refs[(cells[i * 2] + cells[i * 2 + 1]) as usize] = k as u32;
                        cells[i * 2 + 1] += 1;
                    }
                }
            }
        }
        let obstacles_at = HEADER_WORDS;
        let verts_at = obstacles_at + prims.len();
        let cells_at = verts_at + verts.len();
        let refs_at = cells_at + cells.len();
        let mut words = vec![0u32; HEADER_WORDS];
        for (i, v) in [lo[0], lo[1], cell].into_iter().enumerate() {
            words[i] = v.to_bits();
        }
        words[3] = dims[0];
        words[4] = dims[1];
        words[5] = obstacles_at as u32;
        words[6] = verts_at as u32;
        words[7] = cells_at as u32;
        words[8] = refs_at as u32;
        words[9] = ranges.len() as u32;
        words.extend(prims);
        words.extend(verts);
        words.extend(cells);
        words.extend(refs);
        Self { words, dropped }
    }

    /// Сколько препятствий в мире.
    pub fn len(&self) -> usize {
        self.words.get(9).map_or(0, |n| *n as usize)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// Ниже — зеркало шейдера: те же шаги в том же порядке.

type V = [f32; 2];

fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1]]
}
fn mul(a: V, k: f32) -> V {
    [a[0] * k, a[1] * k]
}
fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: V, b: V) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}
fn length(a: V) -> f32 {
    dot(a, a).sqrt()
}
fn normalize(a: V) -> V {
    let l = length(a);
    if l > 1e-12 {
        mul(a, 1.0 / l)
    } else {
        [0.0, 1.0]
    }
}
fn sign(x: f32) -> f32 {
    if x < 0.0 { -1.0 } else { 1.0 }
}

fn fl(w: &[u32], i: usize) -> f32 {
    f32::from_bits(w[i])
}
fn v2(w: &[u32], i: usize) -> V {
    [fl(w, i), fl(w, i + 1)]
}

/// Клетка сетки, где точка `p` (номер клетки или `None` — вне сетки).
fn cell_index(w: &[u32], p: V) -> Option<u32> {
    let (o, cell) = (v2(w, 0), fl(w, 2));
    let (nx, ny) = (w[3], w[4]);
    let x = ((p[0] - o[0]) / cell).floor();
    let y = ((p[1] - o[1]) / cell).floor();
    if x < 0.0 || y < 0.0 || x >= nx as f32 || y >= ny as f32 {
        return None;
    }
    Some(y as u32 * nx + x as u32)
}

/// Когда отрезок пути от `from` на `d` входит в круг (`None` — не входит или уже
/// внутри).
fn enter_circle(c: V, r: f32, from: V, d: V) -> Option<(f32, V)> {
    let f = sub(from, c);
    let a = dot(d, d);
    let cc = dot(f, f) - r * r;
    if cc <= 0.0 || a < 1e-20 {
        return None;
    }
    let b = dot(f, d);
    let disc = b * b - a * cc;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - disc.sqrt()) / a;
    (0.0..=1.0)
        .contains(&t)
        .then(|| (t, normalize(add(f, mul(d, t)))))
}

/// Когда путь входит в прямоугольник с серединой `c`, осью `ax` и половинами `h`. Ещё
/// возвращает, по какой оси вошёл (0 — x, 1 — y).
fn enter_box(c: V, ax: V, h: V, from: V, d: V) -> Option<(f32, V, usize)> {
    let ay = [-ax[1], ax[0]];
    let rel = sub(from, c);
    let l0 = [dot(rel, ax), dot(rel, ay)];
    let dl = [dot(d, ax), dot(d, ay)];
    if l0[0].abs() < h[0] && l0[1].abs() < h[1] {
        return None;
    }
    let mut t_in = f32::NEG_INFINITY;
    let mut t_out = f32::INFINITY;
    let mut axis = 0;
    let mut side = 1.0;
    for i in 0..2 {
        if dl[i].abs() < 1e-12 {
            if l0[i].abs() >= h[i] {
                return None;
            }
            continue;
        }
        let t1 = (-h[i] - l0[i]) / dl[i];
        let t2 = (h[i] - l0[i]) / dl[i];
        let (near, far) = (t1.min(t2), t1.max(t2));
        if near > t_in {
            t_in = near;
            axis = i;
            side = -sign(dl[i]);
        }
        t_out = t_out.min(far);
    }
    if t_in > t_out || !(0.0..=1.0).contains(&t_in) {
        return None;
    }
    let n = if axis == 0 {
        mul(ax, side)
    } else {
        mul(ay, side)
    };
    Some((t_in, n, axis))
}

/// Когда путь входит в препятствие номер `k` и нормаль там.
fn enter(w: &[u32], k: u32, from: V, to: V) -> Option<(f32, V)> {
    let o = w[5] as usize + k as usize * OBSTACLE_WORDS;
    let d = sub(to, from);
    match w[o] {
        KIND_BOX => {
            let round = fl(w, o + 10);
            let h = add(v2(w, o + 8), [round, round]);
            enter_box(v2(w, o + 4), v2(w, o + 6), h, from, d).map(|(t, n, _)| (t, n))
        }
        KIND_CAPSULE => {
            let (a, b, r) = (v2(w, o + 4), v2(w, o + 6), fl(w, o + 8));
            let e = sub(b, a);
            let len = length(e);
            if r <= 0.0 {
                // Тонкая линия: где путь её пересекает.
                let den = cross(d, e);
                if den.abs() < 1e-12 {
                    return None;
                }
                let af = sub(a, from);
                let t = cross(af, e) / den;
                let u = cross(af, d) / den;
                if !(0.0..=1.0).contains(&t) || !(0.0..=1.0).contains(&u) {
                    return None;
                }
                let mut n = normalize([-e[1], e[0]]);
                if dot(n, sub(from, a)) < 0.0 || (dot(n, sub(from, a)) == 0.0 && dot(n, d) > 0.0) {
                    n = mul(n, -1.0);
                }
                return Some((t, n));
            }
            if inside(w, k, from).is_some() {
                return None;
            }
            let mut best = enter_circle(a, r, from, d);
            if len > 1e-9 {
                if let Some(h) = enter_circle(b, r, from, d)
                    && best.is_none_or(|(t, _)| h.0 < t)
                {
                    best = Some(h);
                }
                let ax = mul(e, 1.0 / len);
                let mid = mul(add(a, b), 0.5);
                if let Some((t, n, axis)) = enter_box(mid, ax, [len * 0.5, r], from, d)
                    && axis == 1
                    && best.is_none_or(|(bt, _)| t < bt)
                {
                    best = Some((t, n));
                }
            }
            best
        }
        _ => {
            let (first, count) = (w[o + 2] as usize, w[o + 3] as usize);
            let vs = w[6] as usize + first * 2;
            let mut t_in = f32::NEG_INFINITY;
            let mut t_out = f32::INFINITY;
            let mut n_in = [0.0, 1.0];
            let mut outside = false;
            for i in 0..count {
                let p0 = v2(w, vs + i * 2);
                let p1 = v2(w, vs + ((i + 1) % count) * 2);
                let e = sub(p1, p0);
                let n = normalize([e[1], -e[0]]);
                let num = dot(n, sub(from, p0));
                let den = dot(n, d);
                if num > 0.0 {
                    outside = true;
                }
                if den.abs() < 1e-12 {
                    if num > 0.0 {
                        return None;
                    }
                    continue;
                }
                let t = -num / den;
                if den < 0.0 {
                    if t > t_in {
                        t_in = t;
                        n_in = n;
                    }
                } else {
                    t_out = t_out.min(t);
                }
            }
            if !outside || t_in > t_out || !(0.0..=1.0).contains(&t_in) {
                return None;
            }
            Some((t_in, n_in))
        }
    }
}

/// Внутри ли точка `p` препятствия номер `k`: насколько глубоко (меньше нуля) и куда
/// выталкивать.
fn inside(w: &[u32], k: u32, p: V) -> Option<(f32, V)> {
    let o = w[5] as usize + k as usize * OBSTACLE_WORDS;
    match w[o] {
        KIND_BOX => {
            let (c, ax, h, round) = (v2(w, o + 4), v2(w, o + 6), v2(w, o + 8), fl(w, o + 10));
            let ay = [-ax[1], ax[0]];
            let rel = sub(p, c);
            let l = [dot(rel, ax), dot(rel, ay)];
            let q = [l[0].abs() - h[0], l[1].abs() - h[1]];
            let out = [q[0].max(0.0), q[1].max(0.0)];
            let d = length(out) + q[0].max(q[1]).min(0.0) - round;
            if d >= 0.0 {
                return None;
            }
            let nl = if q[0] > 0.0 || q[1] > 0.0 {
                let n = normalize(out);
                [n[0] * sign(l[0]), n[1] * sign(l[1])]
            } else if q[0] > q[1] {
                [sign(l[0]), 0.0]
            } else {
                [0.0, sign(l[1])]
            };
            Some((d, add(mul(ax, nl[0]), mul(ay, nl[1]))))
        }
        KIND_CAPSULE => {
            let (a, b, r) = (v2(w, o + 4), v2(w, o + 6), fl(w, o + 8));
            if r <= 0.0 {
                return None;
            }
            let e = sub(b, a);
            let ee = dot(e, e);
            let t = if ee > 1e-18 {
                (dot(sub(p, a), e) / ee).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let off = sub(p, add(a, mul(e, t)));
            let dist = length(off);
            if dist >= r {
                return None;
            }
            let n = if dist > 1e-9 {
                mul(off, 1.0 / dist)
            } else {
                normalize([-e[1], e[0]])
            };
            Some((dist - r, n))
        }
        _ => {
            let (first, count) = (w[o + 2] as usize, w[o + 3] as usize);
            let vs = w[6] as usize + first * 2;
            let mut best = f32::NEG_INFINITY;
            let mut n_best = [0.0, 1.0];
            for i in 0..count {
                let p0 = v2(w, vs + i * 2);
                let p1 = v2(w, vs + ((i + 1) % count) * 2);
                let e = sub(p1, p0);
                let n = normalize([e[1], -e[0]]);
                let dist = dot(n, sub(p, p0));
                if dist > best {
                    best = dist;
                    n_best = n;
                }
            }
            (best < 0.0).then_some((best, n_best))
        }
    }
}

/// Скорость точки `p` препятствия номер `k`.
fn velocity_at(w: &[u32], k: u32, p: V) -> V {
    let o = w[5] as usize + k as usize * OBSTACLE_WORDS;
    let (vel, spin, pivot) = (v2(w, o + 11), fl(w, o + 13), v2(w, o + 14));
    let r = sub(p, pivot);
    add(vel, mul([-r[1], r[0]], spin))
}

/// Отскок от поверхности с нормалью `n`, которая движется со скоростью `pv`: поперёк —
/// с долей `bounce`, вдоль — с трением (остаётся 0,8).
fn respond(vel: V, n: V, pv: V, bounce: f32) -> V {
    let rel = sub(vel, pv);
    let vn = dot(rel, n);
    let vt = sub(rel, mul(n, vn));
    let vn = if vn < 0.0 { -vn * bounce } else { vn };
    add(add(mul(vt, 0.8), mul(n, vn)), pv)
}

/// Препятствия клетки `c`: где начинается их список в словах и сколько их.
fn cell_list(w: &[u32], c: u32) -> (usize, usize) {
    let at = w[7] as usize + c as usize * 2;
    (w[8] as usize + w[at] as usize, w[at + 1] as usize)
}

/// Частица летит из `from` в `to` со скоростью `vel`: где она окажется и с какой
/// скоростью, если по пути что-то есть. Сначала — первое препятствие на пути, потом —
/// не оказалась ли внутри движущегося (ящик въехал в лежащую пыль).
pub fn collide(w: &[u32], from: V, to: V, vel: V, bounce: f32) -> (V, V) {
    if w.len() < HEADER_WORDS || w[9] == 0 {
        return (to, vel);
    }
    let cf = cell_index(w, from);
    let ct = cell_index(w, to);
    let mut best_t = 2.0;
    let mut best_n = [0.0, 1.0];
    let mut best_k = 0;
    for (pass, c) in [cf, ct].into_iter().enumerate() {
        let Some(c) = c else { continue };
        if pass == 1 && Some(c) == cf {
            continue;
        }
        let (at, n) = cell_list(w, c);
        for j in 0..n {
            let k = w[at + j];
            let Some((t, nrm)) = enter(w, k, from, to) else {
                continue;
            };
            let o = w[5] as usize + k as usize * OBSTACLE_WORDS;
            if w[o + 1] & FLAG_ONEWAY != 0 && nrm[1] <= 0.5 {
                continue;
            }
            if t < best_t {
                best_t = t;
                best_n = nrm;
                best_k = k;
            }
        }
    }
    let mut to = to;
    let mut vel = vel;
    let mut ct = ct;
    if best_t <= 1.0 {
        // Встаёт в точку касания и проезжает остаток пути вдоль поверхности (иначе пыль
        // на шаре или склоне еле ползла бы).
        let d = sub(to, from);
        let hit = add(add(from, mul(d, best_t)), mul(best_n, EPS));
        let rest = mul(d, 1.0 - best_t);
        let slide = sub(rest, mul(best_n, dot(rest, best_n)));
        to = add(hit, slide);
        vel = respond(vel, best_n, velocity_at(w, best_k, hit), bounce);
        ct = cell_index(w, to);
    }
    if let Some(c) = ct {
        let (at, n) = cell_list(w, c);
        for j in 0..n {
            let k = w[at + j];
            let o = w[5] as usize + k as usize * OBSTACLE_WORDS;
            if w[o + 1] & FLAG_ONEWAY != 0 {
                continue;
            }
            if let Some((d, nrm)) = inside(w, k, to) {
                to = add(to, mul(nrm, -d + EPS));
                vel = respond(vel, nrm, velocity_at(w, k, to), bounce);
            }
        }
    }
    (to, vel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_scene::game::Aabb;

    fn obstacle(shape: ObstacleShape) -> Obstacle {
        Obstacle {
            aabb: reyvec_scene::game::obstacle::shape_aabb(&shape),
            shape,
            vel: [0.0, 0.0],
            spin: 0.0,
            pivot: [0.0, 0.0],
            oneway: false,
        }
    }

    fn world(list: &[Obstacle]) -> ParticleWorld {
        ParticleWorld::build(list, [-2.0, -2.0], [2.0, 2.0])
    }

    #[test]
    fn grid_lists_only_near_obstacles() {
        let far = obstacle(ObstacleShape::Capsule {
            a: [10.0, 0.0],
            b: [10.0, 0.0],
            r: 0.1,
        });
        let near = obstacle(ObstacleShape::Capsule {
            a: [1.0, 1.0],
            b: [1.0, 1.0],
            r: 0.1,
        });
        let w = world(&[far, near]);
        assert_eq!(w.len(), 1, "дальнее не взято");
        let c = cell_index(&w.words, [1.0, 1.0]).unwrap();
        assert_eq!(cell_list(&w.words, c).1, 1);
        let c = cell_index(&w.words, [-1.5, -1.5]).unwrap();
        assert_eq!(cell_list(&w.words, c).1, 0);
        let _ = Aabb::around([0.0, 0.0], [1.0, 1.0]);
    }

    #[test]
    fn hits_a_turned_box_a_ball_a_thin_line_and_a_polygon() {
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let list = [
            // Ромб: прямоугольник, повёрнутый на 45°.
            obstacle(ObstacleShape::Box {
                center: [0.0, 0.0],
                axis: [s, s],
                half: [0.2, 0.2],
                round: 0.0,
            }),
            obstacle(ObstacleShape::Capsule {
                a: [1.0, 0.0],
                b: [1.0, 0.0],
                r: 0.1,
            }),
            obstacle(ObstacleShape::Capsule {
                a: [-1.2, -0.5],
                b: [-0.8, -0.5],
                r: 0.0,
            }),
            obstacle(ObstacleShape::Poly(vec![
                [0.5, -1.0],
                [0.9, -1.0],
                [0.7, -0.6],
            ])),
        ];
        let w = world(&list);
        // Падает на верх ромба: верх — угол на 0,2·√2.
        let top = 0.2 * std::f32::consts::SQRT_2;
        // Чуть правее угла: правая верхняя грань идёт вниз под 45°.
        let x = 0.05;
        let (at, vel) = collide(&w.words, [x, top + 0.02], [x, top - 0.08], [0.0, -6.0], 0.5);
        let face = top - at[0];
        assert!((at[1] - face).abs() < 2e-3, "на грани: {at:?}");
        assert!(vel[0] > 0.0, "отскочила вправо: {vel:?}");
        // Падает на круг.
        let (at, _) = collide(&w.words, [1.0, 0.13], [1.0, 0.05], [0.0, -3.0], 0.5);
        assert!((at[1] - 0.1).abs() < 1e-3, "{at:?}");
        // Тонкая линия не пропускает даже быструю.
        let (at, vel) = collide(&w.words, [-1.0, -0.3], [-1.0, -0.7], [0.0, -24.0], 0.3);
        assert!(at[1] > -0.5, "{at:?}");
        assert!(vel[1] > 0.0);
        // Многоугольник: на крутую сторону сверху — уходит вбок, по грани.
        // Правая сторона треугольника над x = 0,85 — на высоте -0,9; по ней съезжает.
        let (at, vel) = collide(&w.words, [0.85, -0.85], [0.85, -0.95], [0.0, -3.0], 0.5);
        let edge = -1.0 + (0.9 - at[0]) / 0.2 * 0.4;
        assert!((at[1] - edge).abs() < 2e-3, "на грани: {at:?}");
        assert!(vel[0] > 0.0, "{vel:?}");
    }

    #[test]
    fn moving_box_pushes_and_carries() {
        let mut lift = obstacle(ObstacleShape::Box {
            center: [0.0, 0.0],
            axis: [1.0, 0.0],
            half: [0.5, 0.1],
            round: 0.0,
        });
        lift.vel = [0.0, 1.0];
        let w = world(&[lift]);
        // Лежащая пыль оказалась внутри поднявшегося лифта: её выталкивает вверх с его
        // скоростью.
        let (at, vel) = collide(&w.words, [0.2, 0.09], [0.2, 0.09], [0.0, 0.0], 0.0);
        assert!((at[1] - 0.1).abs() < 1e-3, "{at:?}");
        assert!((vel[1] - 1.0).abs() < 1e-3, "{vel:?}");
    }

    #[test]
    fn platform_holds_only_from_above() {
        let mut plank = obstacle(ObstacleShape::Box {
            center: [0.0, 0.0],
            axis: [1.0, 0.0],
            half: [0.5, 0.02],
            round: 0.0,
        });
        plank.oneway = true;
        let w = world(&[plank]);
        let (at, _) = collide(&w.words, [0.0, -0.1], [0.0, 0.1], [0.0, 6.0], 0.5);
        assert!((at[1] - 0.1).abs() < 1e-6, "снизу проходит: {at:?}");
        let (at, _) = collide(&w.words, [0.0, 0.05], [0.0, -0.05], [0.0, -6.0], 0.5);
        assert!((at[1] - 0.02).abs() < 1e-3, "сверху держит: {at:?}");
    }
}
