//! Разбиение фигуры на треугольники (библиотека `lyon`).
//!
//! Кривые при этом заменяются ломаной, которая отходит от настоящей кривой не дальше
//! допуска (`tolerance`). Чем меньше допуск, тем больше треугольников и тем глаже кривые
//! при приближении.

use kurbo::PathEl;
use lyon_tessellation::geom::point;
use lyon_tessellation::path::Path;
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, StrokeOptions, StrokeTessellator,
    StrokeVertex, VertexBuffers,
};

use crate::shape::{FillRule, LineCap, LineJoin, Shape, Style};

/// Треугольники фигуры: точки в координатах фигуры и номера точек по три на треугольник.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Сколько треугольников.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Площадь всех треугольников (для проверок: у залитой фигуры она равна площади фигуры).
    pub fn area(&self) -> f64 {
        self.indices
            .chunks_exact(3)
            .map(|t| {
                let [a, b, c] = [0, 1, 2].map(|i| self.vertices[t[i] as usize]);
                let cross = (f64::from(b[0]) - f64::from(a[0]))
                    * (f64::from(c[1]) - f64::from(a[1]))
                    - (f64::from(b[1]) - f64::from(a[1])) * (f64::from(c[0]) - f64::from(a[0]));
                cross.abs() / 2.0
            })
            .sum()
    }
}

/// Фигуру не удалось разбить на треугольники (например, в контуре бесконечные числа).
#[derive(Debug, thiserror::Error)]
#[error("не удалось разбить фигуру на треугольники: {0}")]
pub struct TessellationError(String);

/// Разбивает фигуру на треугольники с допуском `tolerance` (в единицах фигуры).
pub fn tessellate(shape: &Shape, tolerance: f64) -> Result<Mesh, TessellationError> {
    let path = to_lyon(&shape.path)?;
    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let tolerance = tolerance as f32;
    match shape.style {
        Style::Fill(rule) => {
            let options = FillOptions::tolerance(tolerance).with_fill_rule(match rule {
                FillRule::NonZero => lyon_tessellation::FillRule::NonZero,
                FillRule::EvenOdd => lyon_tessellation::FillRule::EvenOdd,
            });
            FillTessellator::new()
                .tessellate_path(
                    &path,
                    &options,
                    &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| v.position().to_array()),
                )
                .map_err(|e| TessellationError(format!("{e:?}")))?;
        }
        Style::Stroke(stroke) => {
            let options = StrokeOptions::tolerance(tolerance)
                .with_line_width(stroke.width as f32)
                .with_miter_limit(
                    (stroke.miter_limit as f32).max(StrokeOptions::MINIMUM_MITER_LIMIT),
                )
                .with_line_join(match stroke.join {
                    LineJoin::Miter => lyon_tessellation::LineJoin::Miter,
                    LineJoin::Round => lyon_tessellation::LineJoin::Round,
                    LineJoin::Bevel => lyon_tessellation::LineJoin::Bevel,
                })
                .with_line_cap(match stroke.cap {
                    LineCap::Butt => lyon_tessellation::LineCap::Butt,
                    LineCap::Round => lyon_tessellation::LineCap::Round,
                    LineCap::Square => lyon_tessellation::LineCap::Square,
                });
            StrokeTessellator::new()
                .tessellate_path(
                    &path,
                    &options,
                    &mut BuffersBuilder::new(&mut buffers, |v: StrokeVertex| {
                        v.position().to_array()
                    }),
                )
                .map_err(|e| TessellationError(format!("{e:?}")))?;
        }
    }
    Ok(Mesh {
        vertices: buffers.vertices,
        indices: buffers.indices,
    })
}

/// Переводит контур `kurbo` в контур `lyon`.
fn to_lyon(path: &kurbo::BezPath) -> Result<Path, TessellationError> {
    let p = |pt: kurbo::Point| {
        if pt.x.is_finite() && pt.y.is_finite() {
            Ok(point(pt.x as f32, pt.y as f32))
        } else {
            Err(TessellationError(format!("точка контура {pt:?} не число")))
        }
    };
    let mut builder = Path::builder();
    // lyon требует начинать каждый кусок контура с begin и заканчивать end.
    let mut open = false;
    // Начало текущего куска: как в SVG и kurbo, линия сразу после ClosePath начинает
    // новый кусок из этой точки.
    let mut start = None;
    for el in path.elements() {
        let to = match *el {
            PathEl::MoveTo(to) => {
                if open {
                    builder.end(false);
                }
                let to = p(to)?;
                builder.begin(to);
                open = true;
                start = Some(to);
                continue;
            }
            PathEl::ClosePath => {
                if open {
                    builder.end(true);
                    open = false;
                }
                continue;
            }
            PathEl::LineTo(to) | PathEl::QuadTo(_, to) | PathEl::CurveTo(_, _, to) => to,
        };
        if !open {
            let Some(s) = start else {
                return Err(TessellationError(
                    "контур начинается не с MoveTo (нет начальной точки)".into(),
                ));
            };
            builder.begin(s);
            open = true;
        }
        match *el {
            PathEl::QuadTo(c, _) => {
                builder.quadratic_bezier_to(p(c)?, p(to)?);
            }
            PathEl::CurveTo(c1, c2, _) => {
                builder.cubic_bezier_to(p(c1)?, p(c2)?, p(to)?);
            }
            _ => {
                builder.line_to(p(to)?);
            }
        }
    }
    if open {
        builder.end(false);
    }
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, Shape, Stroke};
    use kurbo::{BezPath, Circle, Rect, Shape as _};

    fn square(size: f64) -> BezPath {
        Rect::new(0.0, 0.0, size, size).to_path(0.1)
    }

    /// Пятиконечная звезда одной линией (контур пересекает сам себя).
    fn pentagram() -> BezPath {
        let mut path = BezPath::new();
        for i in 0..5 {
            let a = std::f64::consts::FRAC_PI_2 + f64::from(i * 2) * std::f64::consts::TAU / 5.0;
            let pt = (a.cos(), a.sin());
            if i == 0 {
                path.move_to(pt);
            } else {
                path.line_to(pt);
            }
        }
        path.close_path();
        path
    }

    #[test]
    fn filled_square_has_its_area() {
        let mesh = tessellate(&Shape::fill(square(2.0), Color::WHITE), 0.01).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
        assert!((mesh.area() - 4.0).abs() < 1e-6);
    }

    #[test]
    fn circle_area_converges_with_tolerance() {
        let circle = Circle::new((0.0, 0.0), 1.0).to_path(1e-4);
        let coarse = tessellate(&Shape::fill(circle.clone(), Color::WHITE), 0.05).unwrap();
        let fine = tessellate(&Shape::fill(circle, Color::WHITE), 0.0005).unwrap();
        assert!(fine.triangle_count() > coarse.triangle_count());
        let pi = std::f64::consts::PI;
        assert!((fine.area() - pi).abs() < 0.01, "площадь {}", fine.area());
        assert!((coarse.area() - pi).abs() < 0.2);
    }

    #[test]
    fn fill_rules_differ_for_self_crossing_star() {
        let nonzero = tessellate(&Shape::fill(pentagram(), Color::WHITE), 0.001).unwrap();
        let evenodd = tessellate(
            &Shape::fill_with(pentagram(), FillRule::EvenOdd, Color::WHITE),
            0.001,
        )
        .unwrap();
        // У чётно-нечётного правила пятиугольник в центре пустой.
        assert!(evenodd.area() < nonzero.area() * 0.8);
        assert!(evenodd.area() > nonzero.area() * 0.3);
    }

    #[test]
    fn ring_from_two_circles_with_even_odd() {
        let mut path = Circle::new((0.0, 0.0), 1.0).to_path(1e-4);
        path.extend(Circle::new((0.0, 0.0), 0.5).to_path(1e-4));
        let ring = tessellate(
            &Shape::fill_with(path, FillRule::EvenOdd, Color::WHITE),
            0.0005,
        )
        .unwrap();
        let expected = std::f64::consts::PI * (1.0 - 0.25);
        assert!(
            (ring.area() - expected).abs() < 0.01,
            "площадь {}",
            ring.area()
        );
    }

    #[test]
    fn stroke_area_is_length_times_width() {
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((10.0, 0.0));
        let mesh = tessellate(
            &Shape::stroke(line.clone(), Stroke::new(0.5), Color::WHITE),
            0.01,
        )
        .unwrap();
        assert!((mesh.area() - 5.0).abs() < 1e-4);
        // Круглые концы добавляют по полукругу с каждой стороны.
        let round = tessellate(
            &Shape::stroke(
                line,
                Stroke::new(0.5).with_cap(LineCap::Round),
                Color::WHITE,
            ),
            0.001,
        )
        .unwrap();
        let caps = std::f64::consts::PI * 0.25 * 0.25;
        assert!(
            (round.area() - 5.0 - caps).abs() < 0.01,
            "площадь {}",
            round.area()
        );
    }

    #[test]
    fn several_subpaths_without_close() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((1.0, 0.0));
        path.move_to((0.0, 1.0));
        path.quad_to((0.5, 2.0), (1.0, 1.0));
        let mesh = tessellate(&Shape::stroke(path, Stroke::new(0.1), Color::WHITE), 0.01).unwrap();
        assert!(mesh.triangle_count() >= 4);
    }

    #[test]
    fn line_after_close_starts_from_subpath_start() {
        // Два треугольника одним контуром: второй начинается из начала первого.
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((1.0, 0.0));
        path.line_to((0.0, 1.0));
        path.close_path();
        path.line_to((-1.0, 0.0));
        path.line_to((0.0, -1.0));
        path.close_path();
        let mesh = tessellate(&Shape::fill(path, Color::WHITE), 0.01).unwrap();
        assert!((mesh.area() - 1.0).abs() < 1e-6, "площадь {}", mesh.area());
    }

    #[test]
    fn broken_paths_are_errors_not_panics() {
        let mut nan = BezPath::new();
        nan.move_to((0.0, 0.0));
        nan.line_to((f64::NAN, 1.0));
        assert!(tessellate(&Shape::fill(nan, Color::WHITE), 0.01).is_err());

        let empty = tessellate(&Shape::fill(BezPath::new(), Color::WHITE), 0.01).unwrap();
        assert_eq!(empty.triangle_count(), 0);
    }
}
