//! Разбиение фигуры на треугольники (библиотека `lyon`).
//!
//! Кривые при этом заменяются ломаной, которая отходит от настоящей кривой не дальше
//! допуска (`tolerance`). Чем меньше допуск, тем больше треугольников и тем глаже кривые
//! при приближении.
//!
//! Обводки дополнительно получают данные для сглаживания краёв в шейдере ([`EdgeData`]):
//! треугольники делаются чуть шире настоящей линии (на запас `aa_margin`), а шейдер по
//! расстоянию до настоящего края решает, насколько закрашен пиксель. Так у тонких линий
//! получается 256 оттенков на краю, а не 5, как у одного MSAA 4×.

use kurbo::{PathEl, Point, Vec2};
use lyon_tessellation::geom::point;
use lyon_tessellation::path::Path;
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, Side, StrokeOptions,
    StrokeTessellator, StrokeVertex, VertexBuffers,
};

use crate::shape::{FillRule, LineCap, LineJoin, Shape, Style};

/// Треугольники фигуры: точки в координатах фигуры и номера точек по три на треугольник.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<[f32; 2]>,
    /// Данные для сглаживания краёв, по одной записи на точку (длина как у `vertices`).
    pub edges: Vec<EdgeData>,
    pub indices: Vec<u32>,
}

/// Что шейдер знает о точке, чтобы сгладить край обводки.
///
/// Внутри треугольника видеокарта плавно смешивает эти числа между его точками, и шейдер
/// по ним находит расстояние до края линии. Боковые края: `half_width - |across|`. Круглые
/// стыки и концы: ещё и `half_width - длина(offset)` (расстояние до точки контура), берётся
/// большее из двух (у круглых концов только второе). Расстояние в пикселях и даёт долю
/// закрашенного пикселя.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeData {
    /// Поперёк линии со знаком стороны. На краю треугольников `±(half_width + запас)`.
    pub across: f32,
    /// Смещение точки от точки контура, из которой она построена.
    pub offset: [f32; 2],
    /// Половина толщины настоящей линии. 0 у заливок: их края сглаживает только MSAA.
    pub half_width: f32,
    /// 0: прямые края, 1: круглый стык, 2: круглый конец линии.
    pub round: f32,
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

/// Разбивает фигуру на треугольники с допуском `tolerance` (в единицах фигуры), без запаса
/// под сглаживание в шейдере: треугольники ровно по фигуре.
pub fn tessellate(shape: &Shape, tolerance: f64) -> Result<Mesh, TessellationError> {
    tessellate_smooth(shape, tolerance, 0.0)
}

/// Как [`tessellate`], но обводки получаются шире на `aa_margin` с каждой стороны (в единицах
/// фигуры), чтобы шейдеру было где нарисовать мягкий край. Запас нужен примерно в один
/// пиксель при том размере, в котором фигуру покажут.
pub fn tessellate_smooth(
    shape: &Shape,
    tolerance: f64,
    aa_margin: f64,
) -> Result<Mesh, TessellationError> {
    let path = to_lyon(&shape.path)?;
    let tolerance = tolerance as f32;
    match shape.style {
        Style::Fill(rule) => {
            let options = FillOptions::tolerance(tolerance).with_fill_rule(match rule {
                FillRule::NonZero => lyon_tessellation::FillRule::NonZero,
                FillRule::EvenOdd => lyon_tessellation::FillRule::EvenOdd,
            });
            let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
            FillTessellator::new()
                .tessellate_path(
                    &path,
                    &options,
                    &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| v.position().to_array()),
                )
                .map_err(|e| TessellationError(format!("{e:?}")))?;
            Ok(Mesh {
                edges: vec![EdgeData::default(); buffers.vertices.len()],
                vertices: buffers.vertices,
                indices: buffers.indices,
            })
        }
        Style::Stroke(stroke) => {
            let half_width = (stroke.width / 2.0).max(0.0) as f32;
            let margin = aa_margin.max(0.0) as f32;
            // Квадратный конец = продлённая на полтолщины линия с плоским концом. Так у
            // конца остаются точки на самом контуре, и бока сглаживаются до конца.
            let (path, cap) = if stroke.cap == LineCap::Square {
                (
                    to_lyon(&extend_open_ends(&shape.path, f64::from(half_width)))?,
                    LineCap::Butt,
                )
            } else {
                (path, stroke.cap)
            };
            let options = StrokeOptions::tolerance(tolerance)
                .with_line_width(2.0 * (half_width + margin))
                .with_miter_limit(
                    (stroke.miter_limit as f32).max(StrokeOptions::MINIMUM_MITER_LIMIT),
                )
                .with_line_join(match stroke.join {
                    LineJoin::Miter => lyon_tessellation::LineJoin::Miter,
                    LineJoin::Round => lyon_tessellation::LineJoin::Round,
                    LineJoin::Bevel => lyon_tessellation::LineJoin::Bevel,
                })
                .with_line_cap(match cap {
                    LineCap::Butt | LineCap::Square => lyon_tessellation::LineCap::Butt,
                    LineCap::Round => lyon_tessellation::LineCap::Round,
                });
            let mut buffers: VertexBuffers<RawStrokeVertex, u32> = VertexBuffers::new();
            StrokeTessellator::new()
                .tessellate_path(
                    &path,
                    &options,
                    &mut BuffersBuilder::new(&mut buffers, |v: StrokeVertex| RawStrokeVertex {
                        position: v.position().to_array(),
                        on_path: v.position_on_path().to_array(),
                        positive: v.side() == Side::Positive,
                    }),
                )
                .map_err(|e| TessellationError(format!("{e:?}")))?;
            let round_ends = if cap == LineCap::Round {
                open_ends(&shape.path)
            } else {
                Vec::new()
            };
            let round_joins = stroke.join == LineJoin::Round;
            let full = half_width + margin;
            let edges = buffers
                .vertices
                .iter()
                .map(|v| {
                    let offset = [v.position[0] - v.on_path[0], v.position[1] - v.on_path[1]];
                    // Обычно точка стоит на краю треугольников, ровно `full` поперёк. Точки
                    // ближе к контуру (центр круглого стыка) так и считаются, а дальние
                    // (вершина острого угла, внутренний угол стыка) тоже стоят на краю.
                    let across = offset[0].hypot(offset[1]).min(full);
                    let on_path = Point::new(v.on_path[0].into(), v.on_path[1].into());
                    let at_round_end = round_ends
                        .iter()
                        .any(|p| p.distance_squared(on_path) <= END_MATCH_SQ);
                    let round = if at_round_end {
                        2.0
                    } else if round_joins {
                        1.0
                    } else {
                        0.0
                    };
                    EdgeData {
                        across: if v.positive { across } else { -across },
                        offset,
                        half_width,
                        round,
                    }
                })
                .collect();
            Ok(Mesh {
                vertices: buffers.vertices.iter().map(|v| v.position).collect(),
                edges,
                indices: buffers.indices,
            })
        }
    }
}

/// Насколько близко (в квадрате) точка на контуре должна быть к концу линии, чтобы считаться им.
const END_MATCH_SQ: f64 = 1e-10;

/// Точка обводки, как её отдаёт lyon: где стоит, от какой точки контура построена, с какой стороны.
#[derive(Clone, Copy)]
struct RawStrokeVertex {
    position: [f32; 2],
    on_path: [f32; 2],
    positive: bool,
}

/// Концы незамкнутых кусков контура.
fn open_ends(path: &kurbo::BezPath) -> Vec<Point> {
    ends_with_directions(path)
        .into_iter()
        .map(|(p, _)| p)
        .collect()
}

/// Тот же контур, но каждый незамкнутый кусок продлён на `by` с обоих концов по прямой.
fn extend_open_ends(path: &kurbo::BezPath, by: f64) -> kurbo::BezPath {
    let ends = ends_with_directions(path);
    let find = |p: Point| {
        ends.iter()
            .find(|(e, _)| e.distance_squared(p) <= END_MATCH_SQ)
            .map(|&(_, d)| d)
    };
    let mut out = kurbo::BezPath::new();
    let els = path.elements();
    let mut start: Option<Point> = None;
    let mut current: Option<Point> = None;
    for (i, el) in els.iter().enumerate() {
        let next_starts_new = match els.get(i + 1) {
            None | Some(PathEl::MoveTo(_)) => true,
            Some(_) => false,
        };
        match *el {
            PathEl::MoveTo(p) => {
                start = Some(p);
                current = Some(p);
                // Кусок из одной точки или замкнутый не продлеваем.
                match (find(p), els.get(i + 1)) {
                    (
                        Some(d),
                        Some(PathEl::LineTo(_) | PathEl::QuadTo(..) | PathEl::CurveTo(..)),
                    ) => {
                        out.move_to(p + d * by);
                        out.line_to(p);
                    }
                    _ => out.move_to(p),
                }
            }
            PathEl::ClosePath => {
                out.close_path();
                current = start;
            }
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
                if current == start && i > 0 && matches!(els[i - 1], PathEl::ClosePath) {
                    // Линия после ClosePath: новый кусок из начала прошлого.
                    if let Some(s) = start {
                        match find(s) {
                            Some(d) => {
                                out.move_to(s + d * by);
                                out.line_to(s);
                            }
                            None => out.move_to(s),
                        }
                    }
                }
                out.push(*el);
                current = Some(p);
                if next_starts_new && let Some(d) = find(p) {
                    out.line_to(p + d * by);
                }
            }
        }
    }
    out
}

/// Концы незамкнутых кусков контура и направление «наружу» из каждого конца (единичное).
fn ends_with_directions(path: &kurbo::BezPath) -> Vec<(Point, Vec2)> {
    // Одна точка с направлением: (точка, направление внутрь куска от начала, направление
    // наружу от конца). Направления берутся по первой точке, отличной от конца.
    fn dir(from: Point, towards: &[Point]) -> Option<Vec2> {
        towards
            .iter()
            .map(|&p| p - from)
            .find(|v| v.hypot2() > 1e-24)
            .map(|v| v / v.hypot())
    }
    let mut out = Vec::new();
    let mut start: Option<Point> = None;
    let mut current: Option<Point> = None;
    let mut first_dir: Option<Vec2> = None;
    let mut last_dir: Option<Vec2> = None;
    let mut open = false;
    let mut finish =
        |start: Option<Point>, current: Option<Point>, first: Option<Vec2>, last: Option<Vec2>| {
            if let (Some(s), Some(c), Some(f), Some(l)) = (start, current, first, last) {
                out.push((s, -f));
                out.push((c, l));
            }
        };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if open {
                    finish(start, current, first_dir, last_dir);
                }
                start = Some(p);
                current = Some(p);
                first_dir = None;
                last_dir = None;
                open = true;
            }
            PathEl::ClosePath => {
                open = false;
                current = start;
                first_dir = None;
                last_dir = None;
            }
            PathEl::LineTo(_) | PathEl::QuadTo(..) | PathEl::CurveTo(..) => {
                let Some(from) = current else { continue };
                if !open {
                    // Линия после ClosePath начинает новый кусок из начала прошлого.
                    open = true;
                    first_dir = None;
                    last_dir = None;
                }
                let pts: Vec<Point> = match *el {
                    PathEl::LineTo(p) => vec![p],
                    PathEl::QuadTo(c, p) => vec![c, p],
                    PathEl::CurveTo(c1, c2, p) => vec![c1, c2, p],
                    _ => unreachable!(),
                };
                let to = *pts.last().expect("в куске есть точка");
                if first_dir.is_none() {
                    first_dir = dir(from, &pts);
                }
                let mut back: Vec<Point> = pts[..pts.len() - 1].iter().rev().copied().collect();
                back.push(from);
                if let Some(d) = dir(to, &back) {
                    last_dir = Some(-d);
                }
                current = Some(to);
            }
        }
    }
    if open {
        finish(start, current, first_dir, last_dir);
    }
    out
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
    use kurbo::{Point, Vec2};

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

    /// Расстояние до края линии, как его считает шейдер: смешиваем данные точек
    /// треугольника, в котором лежит `p`. `None`, если `p` не попала ни в один треугольник.
    fn shader_distance(mesh: &Mesh, p: Point) -> Option<f64> {
        for t in mesh.indices.chunks_exact(3) {
            let v = |i: usize| {
                let [x, y] = mesh.vertices[t[i] as usize];
                Point::new(x.into(), y.into())
            };
            let (a, b, c) = (v(0), v(1), v(2));
            let det = (b - a).cross(c - a);
            if det.abs() < 1e-18 {
                continue;
            }
            let wb = (p - a).cross(c - a) / det;
            let wc = (b - a).cross(p - a) / det;
            let wa = 1.0 - wb - wc;
            if wa < -1e-9 || wb < -1e-9 || wc < -1e-9 {
                continue;
            }
            let e = |i: usize| mesh.edges[t[i] as usize];
            let mix = |f: fn(EdgeData) -> f32| {
                wa * f64::from(f(e(0))) + wb * f64::from(f(e(1))) + wc * f64::from(f(e(2)))
            };
            let across = mix(|e| e.across);
            let offset = Vec2::new(mix(|e| e.offset[0]), mix(|e| e.offset[1]));
            let round = mix(|e| e.round);
            let hw = f64::from(e(0).half_width);
            let side = hw - across.abs();
            let radial = hw - offset.hypot();
            return Some(if round > 1.5 {
                radial
            } else if round > 0.5 {
                side.max(radial)
            } else {
                side
            });
        }
        None
    }

    #[test]
    fn fills_have_no_shader_edges_and_strokes_grow_by_margin() {
        let fill = tessellate_smooth(&Shape::fill(square(1.0), Color::WHITE), 0.01, 0.1).unwrap();
        assert_eq!(fill.edges.len(), fill.vertices.len());
        assert!(fill.edges.iter().all(|e| e.half_width == 0.0));
        assert!((fill.area() - 1.0).abs() < 1e-6);

        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((10.0, 0.0));
        let mesh = tessellate_smooth(
            &Shape::stroke(line, Stroke::new(0.5), Color::WHITE),
            0.01,
            0.1,
        )
        .unwrap();
        assert_eq!(mesh.edges.len(), mesh.vertices.len());
        // Шире на запас с каждой стороны: 10 × (0.5 + 2 × 0.1).
        assert!((mesh.area() - 7.0).abs() < 1e-4, "площадь {}", mesh.area());
    }

    #[test]
    fn shader_distance_is_exact_across_a_straight_line() {
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((10.0, 0.0));
        let mesh = tessellate_smooth(
            &Shape::stroke(line, Stroke::new(0.5), Color::WHITE),
            0.01,
            0.1,
        )
        .unwrap();
        for i in -34..=34 {
            let y = f64::from(i) * 0.01;
            let d = shader_distance(&mesh, Point::new(5.0, y)).expect("точка внутри треугольников");
            assert!((d - (0.25 - y.abs())).abs() < 1e-5, "y {y}: {d}");
        }
    }

    #[test]
    fn round_and_square_caps_are_soft_too() {
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((2.0, 0.0));
        let round = tessellate_smooth(
            &Shape::stroke(
                line.clone(),
                Stroke::new(0.5).with_cap(LineCap::Round),
                Color::WHITE,
            ),
            0.0005,
            0.05,
        )
        .unwrap();
        let square = tessellate_smooth(
            &Shape::stroke(
                line,
                Stroke::new(0.5).with_cap(LineCap::Square),
                Color::WHITE,
            ),
            0.0005,
            0.05,
        )
        .unwrap();
        for end in [Point::new(0.0, 0.0), Point::new(2.0, 0.0)] {
            for k in 0..24 {
                let angle = f64::from(k) * std::f64::consts::TAU / 24.0;
                for r in [0.1, 0.2, 0.25, 0.28] {
                    let p = end + Vec2::new(angle.cos(), angle.sin()) * r;
                    let beyond = (p.x - 1.0).abs() - 1.0;
                    if beyond <= 0.0 {
                        continue;
                    }
                    let d = shader_distance(&round, p).expect("точка в круглом конце");
                    assert!((d - (0.25 - r)).abs() < 2e-3, "круглый {p:?}: {d}");
                    // У квадратного конца сглажены бока, а сам торец остаётся MSAA.
                    if beyond < 0.2 {
                        let want = 0.25 - p.y.abs();
                        let d = shader_distance(&square, p).expect("точка в квадратном конце");
                        assert!(
                            (d - want).abs() < 1e-4,
                            "квадратный {p:?}: {d} вместо {want}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn sharp_joins_keep_their_shape() {
        // Зигзаг с острыми углами, как в песочнице: внутри линии шейдер должен считать
        // точку закрашенной, снаружи нет. Точность: ползапаса, а у среза острого угла
        // (Bevel) целый запас: там край среза немного сдвинут внутрь.
        let mut zigzag = BezPath::new();
        zigzag.move_to((0.0, 0.0));
        zigzag.line_to((0.3, 0.6));
        zigzag.line_to((0.6, 0.0));
        zigzag.line_to((0.9, 0.5));
        let (width, margin) = (0.06, 0.004);
        for join in [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel] {
            let mesh = tessellate_smooth(
                &Shape::stroke(
                    zigzag.clone(),
                    Stroke::new(width).with_join(join),
                    Color::WHITE,
                ),
                0.0005,
                margin,
            )
            .unwrap();
            let kurbo_join = match join {
                LineJoin::Miter => kurbo::Join::Miter,
                LineJoin::Round => kurbo::Join::Round,
                LineJoin::Bevel => kurbo::Join::Bevel,
            };
            let outline = kurbo::stroke(
                zigzag.elements().iter().copied(),
                &kurbo::Stroke::new(width)
                    .with_join(kurbo_join)
                    .with_caps(kurbo::Cap::Butt),
                &kurbo::StrokeOpts::default(),
                1e-5,
            );
            let slack = if join == LineJoin::Bevel {
                margin
            } else {
                margin * 0.5
            };
            let mut checked = 0;
            for ix in -20..=200 {
                for iy in -20..=140 {
                    let p = Point::new(f64::from(ix) * 0.005, f64::from(iy) * 0.005);
                    let Some(d) = shader_distance(&mesh, p) else {
                        continue;
                    };
                    let inside = outline.contains(p);
                    if inside {
                        assert!(d > -slack, "{join:?} {p:?} внутри, а шейдер {d}");
                    } else {
                        assert!(d < slack, "{join:?} {p:?} снаружи, а шейдер {d}");
                    }
                    checked += 1;
                }
            }
            assert!(checked > 1000);
        }
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
