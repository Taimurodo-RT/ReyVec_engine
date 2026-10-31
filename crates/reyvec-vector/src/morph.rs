//! Перетекание: фигура, которая плавно меняет форму от одного контура к другому.
//!
//! Процессор один раз, при добавлении фигуры, приводит все ключевые контуры к одному
//! устройству: одинаковое число кусков контура, одинаковое число кривых в каждом куске и
//! одинаковое число квадратичных кривых на каждую. Тогда треугольники веера и кривых
//! (как в `curves.rs`) у всех ключевых контуров получаются одни и те же, отличаются
//! только положения точек. Каждый кадр видеокарта сама смешивает положения точек двух
//! соседних ключевых контуров: процессор в кадре не делает ничего, кроме записи одного
//! числа в экземпляр фигуры ([`crate::Instance::morph`]).
//!
//! Смешанные точки дают ровно треугольники смешанного контура: веер и кривые линейно
//! зависят от точек контура. Поэтому промежуточная форма тоже рисуется точно, с кривыми
//! в шейдере, а самопересечения в середине пути не страшны: трафарет их считает.
//!
//! Как контуры сводятся друг к другу:
//!
//! 1. Прямые и квадратичные кривые становятся кубическими, незамкнутые куски
//!    замыкаются прямой.
//! 2. Если у какого-то контура кусков меньше, недостающие куски — точка в середине
//!    соответствующего куска соседа: она «вырастает» в него (так из круга получается
//!    кольцо).
//! 3. Кривые делятся на части по их длине, пока у всех контуров в куске не станет
//!    поровну кривых.
//! 4. Кусок, обходящий в другую сторону, разворачивается, а начало сдвигается по кругу
//!    так, чтобы точки шли ближе всего к точкам предыдущего ключевого контура: иначе
//!    фигура по пути закручивается.
//! 5. Каждая кривая делится на столько квадратичных, сколько нужно самому изогнутому из
//!    ключевых контуров для допуска `accuracy`.

use kurbo::{BezPath, CubicBez, ParamCurve, ParamCurveArea, PathEl, Point, Rect};

use crate::CurveMesh;

/// Треугольники перетекающей фигуры: один набор треугольников и положения точек для
/// каждого ключевого контура.
#[derive(Clone, Debug, PartialEq)]
pub struct MorphMesh {
    /// Треугольники и точки первого ключевого контура.
    pub mesh: CurveMesh,
    /// Положения тех же точек у остальных ключевых контуров по порядку.
    pub keyframes: Vec<Vec<[f32; 2]>>,
}

impl MorphMesh {
    /// Сколько всего ключевых контуров (вместе с первым).
    pub fn keyframe_count(&self) -> usize {
        1 + self.keyframes.len()
    }

    /// Положения точек ключевого контура `k` (0 = первый).
    pub fn positions(&self, k: usize) -> &[[f32; 2]] {
        if k == 0 {
            &self.mesh.vertices
        } else {
            &self.keyframes[k - 1]
        }
    }
}

/// Почему перетекание не построить.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MorphError {
    /// Ключевых контуров меньше двух.
    TooFewKeyframes,
    /// Ключевой контур с этим номером пустой.
    EmptyKeyframe(usize),
}

impl std::fmt::Display for MorphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooFewKeyframes => write!(f, "для перетекания нужно хотя бы два контура"),
            Self::EmptyKeyframe(k) => write!(f, "контур перетекания №{k} пустой"),
        }
    }
}

impl std::error::Error for MorphError {}

/// Кусок контура: замкнутая цепочка кубических кривых.
type Loop = Vec<CubicBez>;

/// Строит треугольники перетекания между контурами `keyframes` (не меньше двух).
pub fn morph_mesh(keyframes: &[BezPath], accuracy: f64) -> Result<MorphMesh, MorphError> {
    if keyframes.len() < 2 {
        return Err(MorphError::TooFewKeyframes);
    }
    let mut shapes: Vec<Vec<Loop>> = Vec::with_capacity(keyframes.len());
    for (k, path) in keyframes.iter().enumerate() {
        let loops = to_loops(path);
        if loops.is_empty() {
            return Err(MorphError::EmptyKeyframe(k));
        }
        shapes.push(loops);
    }

    // 2. Поровну кусков: недостающие становятся точкой в середине куска соседа.
    let loop_count = shapes.iter().map(Vec::len).max().unwrap_or(0);
    for j in 0..loop_count {
        let Some(center) = shapes
            .iter()
            .find_map(|s| s.get(j))
            .map(|l| bounds(l).center())
        else {
            continue;
        };
        for s in &mut shapes {
            if s.len() <= j {
                s.push(Vec::new());
            }
            if s[j].is_empty() {
                s[j] = vec![CubicBez::new(center, center, center, center)];
            }
        }
    }

    for j in 0..loop_count {
        // 3. Поровну кривых в куске.
        let target = shapes.iter().map(|s| s[j].len()).max().unwrap_or(1);
        for s in &mut shapes {
            split_to(&mut s[j], target);
        }
        // 4. Тот же обход и ближайшее начало, что у предыдущего ключевого контура.
        for k in 1..shapes.len() {
            let (before, after) = shapes.split_at_mut(k);
            let prev = &before[k - 1][j];
            let cur = &mut after[0][j];
            let (a, b) = (area(prev), area(cur));
            if a * b < 0.0 {
                reverse(cur);
            }
            let shift = best_rotation(prev, cur);
            cur.rotate_left(shift);
        }
    }

    // 5. Одинаковое число квадратичных кривых на каждую кубическую у всех контуров.
    let mut mesh = CurveMesh::default();
    let mut others: Vec<Vec<[f32; 2]>> = vec![Vec::new(); shapes.len() - 1];
    for j in 0..loop_count {
        let anchors: Vec<Point> = shapes.iter().map(|s| s[j][0].p0).collect();
        for i in 0..shapes[0][j].len() {
            let n = shapes
                .iter()
                .map(|s| s[j][i].to_quads(accuracy).count())
                .max()
                .unwrap_or(1)
                .max(1);
            for q in 0..n {
                let range = q as f64 / n as f64..(q + 1) as f64 / n as f64;
                for (k, s) in shapes.iter().enumerate() {
                    let c = s[j][i].subsegment(range.clone());
                    // Квадратичная кривая, ближайшая к куску кубической (как в kurbo).
                    let control =
                        ((c.p1.to_vec2() + c.p2.to_vec2()) * 3.0 - c.p0.to_vec2() - c.p3.to_vec2())
                            / 4.0;
                    let pts = [anchors[k], c.p0, c.p3, c.p0, control.to_point(), c.p3];
                    if k == 0 {
                        push_quad(&mut mesh, pts);
                    } else {
                        others[k - 1].extend(pts.map(|p| [p.x as f32, p.y as f32]));
                    }
                }
            }
        }
    }
    Ok(MorphMesh {
        mesh,
        keyframes: others,
    })
}

/// Треугольник веера и треугольник кривой. Вырожденные (нулевой площади) не
/// пропускаются: у другого ключевого контура те же треугольники могут быть настоящими.
fn push_quad(mesh: &mut CurveMesh, pts: [Point; 6]) {
    const UV: [[f32; 2]; 6] = [
        [0.0, 1.0],
        [0.0, 1.0],
        [0.0, 1.0],
        [0.0, 0.0],
        [0.5, 0.0],
        [1.0, 1.0],
    ];
    for (p, uv) in pts.into_iter().zip(UV) {
        mesh.indices.push(mesh.vertices.len() as u32);
        mesh.vertices.push([p.x as f32, p.y as f32]);
        mesh.uv.push(uv);
    }
}

/// Куски контура кубическими кривыми. Незамкнутые куски замыкаются прямой.
fn to_loops(path: &BezPath) -> Vec<Loop> {
    let mut loops: Vec<Loop> = Vec::new();
    let mut current: Loop = Vec::new();
    let mut start = Point::ZERO;
    let mut last = Point::ZERO;
    let finish = |current: &mut Loop, loops: &mut Vec<Loop>, start: Point, last: Point| {
        if current.is_empty() {
            return;
        }
        if last != start {
            current.push(line(last, start));
        }
        loops.push(std::mem::take(current));
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                finish(&mut current, &mut loops, start, last);
                start = p;
                last = p;
            }
            PathEl::LineTo(p) => {
                current.push(line(last, p));
                last = p;
            }
            PathEl::QuadTo(q, p) => {
                current.push(CubicBez::new(
                    last,
                    last.lerp(q, 2.0 / 3.0),
                    p.lerp(q, 2.0 / 3.0),
                    p,
                ));
                last = p;
            }
            PathEl::CurveTo(a, b, p) => {
                current.push(CubicBez::new(last, a, b, p));
                last = p;
            }
            PathEl::ClosePath => {
                finish(&mut current, &mut loops, start, last);
                last = start;
            }
        }
    }
    finish(&mut current, &mut loops, start, last);
    loops
}

fn line(a: Point, b: Point) -> CubicBez {
    CubicBez::new(a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b)
}

fn bounds(l: &Loop) -> Rect {
    l.iter().fold(Rect::from_points(l[0].p0, l[0].p0), |r, c| {
        [c.p1, c.p2, c.p3].into_iter().fold(r, |r, p| r.union_pt(p))
    })
}

/// Площадь со знаком: знак показывает направление обхода.
fn area(l: &Loop) -> f64 {
    l.iter().map(ParamCurveArea::signed_area).sum()
}

/// Делит кривые так, чтобы их стало `target`: каждой достаётся число кусков по её доле
/// длины (хотя бы один), куски равные по параметру. Так точки идут по контуру
/// равномернее, чем если делить пополам самую длинную.
fn split_to(l: &mut Loop, target: usize) {
    if l.len() >= target {
        return;
    }
    let lengths: Vec<f64> = l.iter().map(length).collect();
    let mut pieces = vec![1usize; l.len()];
    // Оставшиеся куски по одному туда, где кусок сейчас длиннее всего.
    for _ in l.len()..target {
        let i = (0..l.len())
            .max_by(|&a, &b| {
                (lengths[a] / pieces[a] as f64).total_cmp(&(lengths[b] / pieces[b] as f64))
            })
            .unwrap_or(0);
        pieces[i] += 1;
    }
    let mut out = Vec::with_capacity(target);
    for (c, n) in l.iter().zip(pieces) {
        for q in 0..n {
            out.push(c.subsegment(q as f64 / n as f64..(q + 1) as f64 / n as f64));
        }
    }
    *l = out;
}

/// Длина ломаной по точкам кривой: её хватает, чтобы выбрать, что делить.
fn length(c: &CubicBez) -> f64 {
    (c.p1 - c.p0).hypot() + (c.p2 - c.p1).hypot() + (c.p3 - c.p2).hypot()
}

fn reverse(l: &mut Loop) {
    l.reverse();
    for c in l.iter_mut() {
        *c = CubicBez::new(c.p3, c.p2, c.p1, c.p0);
    }
}

/// На сколько кривых сдвинуть начало `cur`, чтобы его точки были ближе всего к `prev`.
fn best_rotation(prev: &Loop, cur: &Loop) -> usize {
    let n = cur.len();
    (0..n)
        .min_by(|&a, &b| {
            let cost = |r: usize| -> f64 {
                (0..n)
                    .map(|i| (prev[i].p0 - cur[(i + r) % n].p0).hypot2())
                    .sum()
            };
            cost(a).total_cmp(&cost(b))
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Affine, Circle, Shape as _};

    fn star(points: usize, outer: f64, inner: f64) -> BezPath {
        let mut path = BezPath::new();
        for i in 0..points * 2 {
            let r = if i % 2 == 0 { outer } else { inner };
            let a = std::f64::consts::FRAC_PI_2 + i as f64 * std::f64::consts::PI / points as f64;
            let p = (r * a.cos(), r * a.sin());
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path.close_path();
        path
    }

    /// Контур из треугольников ключевого контура `k`, смешанного с `k + 1` в доле `f`:
    /// треугольники те же, точки смешаны. Возвращает сетку как обычный `CurveMesh`.
    fn blended(m: &MorphMesh, k: usize, f: f32) -> CurveMesh {
        let (a, b) = (m.positions(k), m.positions(k + 1));
        CurveMesh {
            vertices: a
                .iter()
                .zip(b)
                .map(|(p, q)| [p[0] + (q[0] - p[0]) * f, p[1] + (q[1] - p[1]) * f])
                .collect(),
            uv: m.mesh.uv.clone(),
            indices: m.mesh.indices.clone(),
        }
    }

    /// Число оборотов по треугольникам, как в тесте `curves.rs`.
    fn winding(mesh: &CurveMesh, p: Point) -> i32 {
        let mut total = 0;
        for t in mesh.indices.chunks_exact(3) {
            let v = |i: usize| {
                let [x, y] = mesh.vertices[t[i] as usize];
                Point::new(x.into(), y.into())
            };
            let (a, b, c) = (v(0), v(1), v(2));
            let det = (b - a).cross(c - a);
            if det.abs() < 1e-12 {
                continue;
            }
            let wb = (p - a).cross(c - a) / det;
            let wc = (b - a).cross(p - a) / det;
            let wa = 1.0 - wb - wc;
            if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                continue;
            }
            let uv = |i: usize| mesh.uv[t[i] as usize];
            let u = wa * f64::from(uv(0)[0]) + wb * f64::from(uv(1)[0]) + wc * f64::from(uv(2)[0]);
            let w = wa * f64::from(uv(0)[1]) + wb * f64::from(uv(1)[1]) + wc * f64::from(uv(2)[1]);
            if u * u - w > 0.0 {
                continue;
            }
            total += if det > 0.0 { 1 } else { -1 };
        }
        total
    }

    /// Внутри ли точка по правилу NonZero, по сетке и по настоящему контуру.
    fn same_inside(mesh: &CurveMesh, path: &BezPath, accuracy: f64) {
        let bounds = path.bounding_box().inflate(0.1, 0.1);
        let mut checked = 0;
        for ix in 0..=40 {
            for iy in 0..=40 {
                let p = Point::new(
                    bounds.x0 + bounds.width() * f64::from(ix) / 40.0 + 1e-4,
                    bounds.y0 + bounds.height() * f64::from(iy) / 40.0 + 3e-4,
                );
                let near_edge = [(1.0, 0.0), (0.0, 1.0)].iter().any(|&(dx, dy)| {
                    let d = kurbo::Vec2::new(dx, dy) * accuracy * 8.0;
                    (path.winding(p + d) != 0) != (path.winding(p - d) != 0)
                });
                if near_edge {
                    continue;
                }
                assert_eq!(winding(mesh, p) != 0, path.winding(p) != 0, "точка {p:?}");
                checked += 1;
            }
        }
        assert!(checked > 1000);
    }

    #[test]
    fn keyframes_are_drawn_exactly() {
        let circle = Circle::new((0.0, 0.0), 1.0).to_path(1e-6);
        let star = star(5, 1.0, 0.4);
        let mut ring = Circle::new((0.0, 0.0), 1.0).to_path(1e-6);
        ring.extend(
            Circle::new((0.0, 0.0), 0.5)
                .to_path(1e-6)
                .reverse_subpaths(),
        );
        let keys = [circle, star, ring];
        let m = morph_mesh(&keys, 1e-4).unwrap();
        assert_eq!(m.keyframe_count(), 3);
        for k in 0..3 {
            assert_eq!(m.positions(k).len(), m.mesh.vertices.len());
        }
        same_inside(&m.mesh, &keys[0], 1e-4);
        same_inside(&blended(&m, 0, 1.0), &keys[1], 1e-4);
        same_inside(&blended(&m, 1, 1.0), &keys[2], 1e-4);
    }

    #[test]
    fn halfway_is_the_blend_of_aligned_outlines() {
        // Два квадрата, второй повёрнут на 90° (начало контура в другом углу) и обходит
        // в другую сторону. После выравнивания середина пути — тот же квадрат, а не
        // перекрученная фигура.
        let square = |a: Affine| {
            let mut p = BezPath::new();
            p.move_to((-1.0, -1.0));
            p.line_to((1.0, -1.0));
            p.line_to((1.0, 1.0));
            p.line_to((-1.0, 1.0));
            p.close_path();
            p.apply_affine(a);
            p
        };
        let a = square(Affine::IDENTITY);
        let b = square(Affine::rotate(std::f64::consts::FRAC_PI_2)).reverse_subpaths();
        let m = morph_mesh(&[a.clone(), b], 1e-4).unwrap();
        same_inside(&blended(&m, 0, 0.5), &a, 1e-4);
    }

    #[test]
    fn errors() {
        let c = Circle::new((0.0, 0.0), 1.0).to_path(1e-6);
        assert_eq!(
            morph_mesh(std::slice::from_ref(&c), 1e-4),
            Err(MorphError::TooFewKeyframes)
        );
        assert_eq!(
            morph_mesh(&[c, BezPath::new()], 1e-4),
            Err(MorphError::EmptyKeyframe(1))
        );
    }
}
