//! Способ 2: заливка через трафарет, кривые считаются в шейдере (Loop–Blinn + трафарет).
//!
//! Процессор ничего не разбивает на треугольники по-настоящему: он только перечисляет
//! точки контура. Из них получаются треугольники двух видов:
//!
//! - **веер**: из первой точки каждого куска контура к каждому отрезку (по хорде кривой);
//! - **кривые**: на каждую квадратичную кривую треугольник (начало, контрольная точка,
//!   конец) с координатами `(u, v)` = (0, 0), (½, 0), (1, 1). Внутри кривой `u² − v < 0`,
//!   снаружи больше нуля: шейдер отбрасывает пиксели снаружи.
//!
//! Видеокарта рисует все эти треугольники в трафарет: треугольник по часовой стрелке
//! прибавляет единицу, против часовой вычитает. Веер и кривые вместе дают в каждом пикселе
//! число оборотов контура вокруг него. Потом проход заливки красит пиксели, где число
//! оборотов не ноль (NonZero) или нечётное (EvenOdd). Так работает любое правило заливки,
//! самопересечения и дыры без всякого разбора на процессоре.
//!
//! Кубические кривые заменяются несколькими квадратичными с допуском `accuracy`.

use kurbo::{BezPath, CubicBez, PathEl, Point};

/// Треугольники фигуры для заливки через трафарет.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurveMesh {
    /// Точки в координатах фигуры.
    pub vertices: Vec<[f32; 2]>,
    /// Координаты кривой у каждой точки. У веера `(0, 1)`: `u² − v = −1`, всегда внутри.
    pub uv: Vec<[f32; 2]>,
    /// Номера точек по три на треугольник.
    pub indices: Vec<u32>,
}

impl CurveMesh {
    /// Сколько треугольников.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    fn push(&mut self, p: Point, uv: [f32; 2]) -> u32 {
        self.vertices.push([p.x as f32, p.y as f32]);
        self.uv.push(uv);
        self.vertices.len() as u32 - 1
    }

    fn fan(&mut self, anchor: Point, a: Point, b: Point) {
        if a == b || a == anchor || b == anchor {
            return;
        }
        for p in [anchor, a, b] {
            let i = self.push(p, [0.0, 1.0]);
            self.indices.push(i);
        }
    }

    fn curve(&mut self, a: Point, c: Point, b: Point) {
        if (c - a).cross(b - a) == 0.0 {
            return; // кривая выродилась в отрезок: хватит веера
        }
        for (p, uv) in [(a, [0.0, 0.0]), (c, [0.5, 0.0]), (b, [1.0, 1.0])] {
            let i = self.push(p, uv);
            self.indices.push(i);
        }
    }
}

/// Строит треугольники веера и кривых для заливки контура `path`. Незамкнутые куски
/// замыкаются прямой, как при обычной заливке.
pub fn curve_mesh(path: &BezPath, accuracy: f64) -> CurveMesh {
    let mut mesh = CurveMesh::default();
    let mut start: Option<Point> = None;
    let mut current: Option<Point> = None;
    let close = |mesh: &mut CurveMesh, start: Option<Point>, current: Option<Point>| {
        if let (Some(s), Some(c)) = (start, current) {
            mesh.fan(s, c, s);
        }
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                close(&mut mesh, start, current);
                start = Some(p);
                current = Some(p);
            }
            PathEl::ClosePath => {
                close(&mut mesh, start, current);
                current = start;
            }
            PathEl::LineTo(p) => {
                let (Some(s), Some(c)) = (start, current) else {
                    continue;
                };
                mesh.fan(s, c, p);
                current = Some(p);
            }
            PathEl::QuadTo(q, p) => {
                let (Some(s), Some(c)) = (start, current) else {
                    continue;
                };
                mesh.fan(s, c, p);
                mesh.curve(c, q, p);
                current = Some(p);
            }
            PathEl::CurveTo(c1, c2, p) => {
                let (Some(s), Some(c)) = (start, current) else {
                    continue;
                };
                for (_, _, quad) in CubicBez::new(c, c1, c2, p).to_quads(accuracy) {
                    mesh.fan(s, quad.p0, quad.p2);
                    mesh.curve(quad.p0, quad.p1, quad.p2);
                }
                current = Some(p);
            }
        }
    }
    close(&mut mesh, start, current);
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Shape as _, Vec2};

    /// Число оборотов в точке `p`, как его посчитает видеокарта: сумма знаков треугольников,
    /// которые накрывают точку (у треугольников кривых только внутри кривой).
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

    /// Сравнивает число оборотов с kurbo на сетке точек (точки у самого края пропускаем).
    fn check(path: &BezPath, accuracy: f64) {
        let mesh = curve_mesh(path, accuracy);
        let bounds = path.bounding_box().inflate(0.1, 0.1);
        let mut checked = 0;
        for ix in 0..=60 {
            for iy in 0..=60 {
                let p = Point::new(
                    bounds.x0 + bounds.width() * f64::from(ix) / 60.0 + 1e-4,
                    bounds.y0 + bounds.height() * f64::from(iy) / 60.0 + 3e-4,
                );
                // Рядом с краем приближение кубической кривой может ошибиться на допуск.
                let near_edge = [Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)].iter().any(|&d| {
                    path.winding(p + d * accuracy * 4.0) != path.winding(p - d * accuracy * 4.0)
                });
                if near_edge {
                    continue;
                }
                assert_eq!(winding(&mesh, p), path.winding(p), "точка {p:?}");
                checked += 1;
            }
        }
        assert!(checked > 2000);
    }

    #[test]
    fn circle_and_ring() {
        check(&Circle::new((0.0, 0.0), 1.0).to_path(1e-4), 1e-4);
        let mut ring = Circle::new((0.0, 0.0), 1.0).to_path(1e-4);
        ring.extend(Circle::new((0.0, 0.0), 0.5).to_path(1e-4));
        check(&ring, 1e-4);
    }

    #[test]
    fn self_crossing_star_and_open_path() {
        let mut star = BezPath::new();
        for i in 0..5 {
            let a = std::f64::consts::FRAC_PI_2 + f64::from(i * 2) * std::f64::consts::TAU / 5.0;
            let pt = (a.cos(), a.sin());
            if i == 0 {
                star.move_to(pt);
            } else {
                star.line_to(pt);
            }
        }
        let mut closed = star.clone();
        closed.close_path();
        check(&closed, 1e-4);
        // Без ClosePath заливка замыкает сама: треугольники те же.
        assert_eq!(curve_mesh(&star, 1e-4), curve_mesh(&closed, 1e-4));
    }

    #[test]
    fn heart_with_cubics_and_quads() {
        let mut heart = BezPath::new();
        heart.move_to((0.0, -0.8));
        heart.curve_to((-0.9, -0.2), (-0.9, 0.7), (-0.45, 0.75));
        heart.quad_to((-0.1, 0.8), (0.0, 0.45));
        heart.quad_to((0.1, 0.8), (0.45, 0.75));
        heart.curve_to((0.9, 0.7), (0.9, -0.2), (0.0, -0.8));
        heart.close_path();
        check(&heart, 1e-4);
        assert!(curve_mesh(&heart, 1e-4).triangle_count() < 200);
    }
}
