//! Форма столкновения фигуры (9.3): из чего собрать настоящую форму `parry2d` для мира
//! столкновений героя и для Rapier.
//!
//! Сцена описывает фигуру точками в её собственных координатах (контур векторной
//! фигуры уже разбит на отрезки, SDF-объект — круги и капсулы). Форма собирается один
//! раз на размер ([`Form::build`]): поворот и сдвиг фигуры — это место формы
//! ([`crate::Surface::pose`]), а не новая форма, поэтому вращающаяся фигура форму не
//! пересобирает.

use rapier2d::parry::math::{Pose, Vector};
use rapier2d::parry::shape::SharedShape;

/// Из чего состоит форма столкновения, в собственных координатах фигуры.
#[derive(Clone, Debug, PartialEq)]
pub enum Form {
    /// Прямоугольник с половинами сторон `half` и скруглением углов `corner`.
    Box { half: [f64; 2], corner: f64 },
    /// Круг.
    Ball { r: f64 },
    /// Залитые контуры (замкнутые ломаные). Один выпуклый контур — выпуклый
    /// многоугольник, иначе — сами контуры как ломаная (дыры остаются дырами).
    Fill { loops: Vec<Vec<[f64; 2]>> },
    /// Линии толщиной `2 r` с круглыми концами и углами.
    Stroke { lines: Vec<Vec<[f64; 2]>>, r: f64 },
    /// Капсула от `a` до `b` толщиной `2 r` (часть SDF-объекта; круг — `a == b`).
    Capsule { a: [f64; 2], b: [f64; 2], r: f64 },
    /// Несколько форм вместе (SDF-объект из кругов и капсул).
    Parts(Vec<Form>),
}

impl Form {
    /// Настоящая форма при растяжении фигуры на `sx` по её оси x и `sy` по оси y
    /// (отрицательное — отражение). `None` — пустая или вырожденная.
    pub fn build(&self, sx: f64, sy: f64) -> Option<SharedShape> {
        let (ax, ay) = (sx.abs(), sy.abs());
        let even = (ax - ay).abs() <= 1e-9 * ax.max(ay);
        let thick = (ax * ay).sqrt();
        let pt = |q: [f64; 2]| Vector::new((q[0] * sx) as f32, (q[1] * sy) as f32);
        match self {
            Self::Box { half, corner } => {
                let (hx, hy) = (half[0] * ax, half[1] * ay);
                if hx <= 0.0 || hy <= 0.0 {
                    return None;
                }
                let r = (corner * ax.min(ay)).min(hx.min(hy) - 1e-6).max(0.0);
                Some(if r > 1e-6 {
                    SharedShape::round_cuboid((hx - r) as f32, (hy - r) as f32, r as f32)
                } else {
                    SharedShape::cuboid(hx as f32, hy as f32)
                })
            }
            Self::Ball { r } => {
                if *r <= 0.0 {
                    return None;
                }
                if even {
                    return Some(SharedShape::ball((r * ax) as f32));
                }
                // Растянутый круг — эллипс многоугольником.
                let pts: Vec<Vector> = (0..32)
                    .map(|i| {
                        let a = std::f64::consts::TAU * f64::from(i) / 32.0;
                        pt([r * a.cos(), r * a.sin()])
                    })
                    .collect();
                SharedShape::convex_hull(&pts)
            }
            Self::Fill { loops } => {
                let loops: Vec<Vec<Vector>> = loops
                    .iter()
                    .map(|l| dedup(l.iter().map(|q| pt(*q)).collect()))
                    .filter(|l| l.len() >= 3)
                    .collect();
                match loops.as_slice() {
                    [] => None,
                    [one] if convex(one) => SharedShape::convex_hull(one),
                    _ => {
                        let mut points = Vec::new();
                        let mut edges = Vec::new();
                        for l in &loops {
                            let base = points.len() as u32;
                            let n = l.len() as u32;
                            points.extend_from_slice(l);
                            edges.extend((0..n).map(|i| [base + i, base + (i + 1) % n]));
                        }
                        Some(SharedShape::polyline(points, Some(edges)))
                    }
                }
            }
            Self::Stroke { lines, r } => {
                let r = (r * thick) as f32;
                if r <= 0.0 {
                    return None;
                }
                let mut parts = Vec::new();
                for l in lines {
                    let l = dedup(l.iter().map(|q| pt(*q)).collect());
                    if l.len() == 1 {
                        parts.push((Pose::from_translation(l[0]), SharedShape::ball(r)));
                    }
                    for w in l.windows(2) {
                        parts.push((Pose::IDENTITY, SharedShape::capsule(w[0], w[1], r)));
                    }
                }
                compound(parts)
            }
            Self::Capsule { a, b, r } => {
                let r = (r * thick) as f32;
                if r <= 0.0 {
                    return None;
                }
                // Круг (`a == b`) — капсула нулевой длины: так он может стоять не в
                // середине фигуры без составной формы.
                Some(SharedShape::capsule(pt(*a), pt(*b), r))
            }
            Self::Parts(parts) => {
                // Составная форма не может лежать внутри другой: части частей — рядом.
                let mut all = Vec::new();
                for s in parts.iter().filter_map(|f| f.build(sx, sy)) {
                    match s.as_compound() {
                        Some(c) => all.extend(c.shapes().iter().cloned()),
                        None => all.push((Pose::IDENTITY, s)),
                    }
                }
                compound(all)
            }
        }
    }
}

/// Убирает повторы подряд (и замыкающую точку, равную первой).
fn dedup(mut l: Vec<Vector>) -> Vec<Vector> {
    l.dedup_by(|a, b| (*a - *b).length() < 1e-7);
    if l.len() > 1 && (l[0] - l[l.len() - 1]).length() < 1e-7 {
        l.pop();
    }
    l
}

fn convex(l: &[Vector]) -> bool {
    let n = l.len();
    let mut sign = 0.0f32;
    for i in 0..n {
        let (a, b, c) = (l[i], l[(i + 1) % n], l[(i + 2) % n]);
        let cross = (b - a).perp_dot(c - b);
        if cross.abs() < 1e-9 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

fn compound(mut parts: Vec<(Pose, SharedShape)>) -> Option<SharedShape> {
    match parts.len() {
        0 => None,
        1 if parts[0].0 == Pose::IDENTITY => Some(parts.pop()?.1),
        _ => Some(SharedShape::compound(parts)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convex_loop_is_a_polygon_and_star_is_an_outline() {
        let square = Form::Fill {
            loops: vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]],
        };
        let s = square.build(1.0, 1.0).unwrap();
        assert!(s.as_convex_polygon().is_some());
        let star: Vec<[f64; 2]> = (0..10)
            .map(|i| {
                let a = std::f64::consts::TAU * f64::from(i) / 10.0;
                let r = if i % 2 == 0 { 1.0 } else { 0.4 };
                [r * a.cos(), r * a.sin()]
            })
            .collect();
        let s = Form::Fill { loops: vec![star] }.build(2.0, 2.0).unwrap();
        let p = s.as_polyline().expect("звезда — контур");
        assert_eq!(p.num_segments(), 10);
        // Растяжение входит в форму.
        let a = s.compute_local_aabb();
        assert!((a.maxs.x - 2.0).abs() < 1e-5, "{a:?}");
    }

    #[test]
    fn stretched_ball_and_round_box() {
        let e = Form::Ball { r: 1.0 }.build(2.0, 1.0).unwrap();
        let a = e.compute_local_aabb();
        assert!(
            (a.maxs.x - 2.0).abs() < 1e-3 && (a.maxs.y - 1.0).abs() < 1e-3,
            "{a:?}"
        );
        let b = Form::Box {
            half: [1.0, 0.5],
            corner: 0.2,
        }
        .build(1.0, 1.0)
        .unwrap();
        assert!(b.as_round_cuboid().is_some());
        let a = b.compute_local_aabb();
        assert!((a.maxs.x - 1.0).abs() < 1e-5 && (a.maxs.y - 0.5).abs() < 1e-5);
    }

    #[test]
    fn stroke_is_capsules() {
        let s = Form::Stroke {
            lines: vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]],
            r: 0.1,
        }
        .build(1.0, 1.0)
        .unwrap();
        assert_eq!(s.as_compound().unwrap().shapes().len(), 2);
    }
}
