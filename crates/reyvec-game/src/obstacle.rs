//! Препятствия для частиц (9.5).
//!
//! Частицы (пыль, искры, брызги) считает видеокарта, и ей нужны простые формы: повёрнутый
//! прямоугольник со скруглением, капсула (круг — капсула нулевой длины, тонкая линия —
//! капсула нулевой толщины) и выпуклый многоугольник. [`obstacles`] раскладывает на них
//! поверхность мира столкновений ([`Surface`]): уровень, фигуры сцены, тела физики,
//! мягкие тела. Поэтому частицы отскакивают от того же, обо что ударяется герой:
//! от повёрнутого ящика, звезды, холма, желе.
//!
//! У препятствия есть скорость: ящик, который едет, отбрасывает частицы, а частица на
//! лифте едет с ним.

use rapier2d::parry::math::{Pose, Vector};
use rapier2d::parry::shape::Shape;

use crate::Aabb;
use crate::space::Surface;

/// Форма препятствия в координатах сцены.
#[derive(Clone, Debug, PartialEq)]
pub enum ObstacleShape {
    /// Прямоугольник: середина, направление его оси x (единичное), половины сторон и
    /// скругление углов (прямоугольник целиком — половины плюс скругление).
    Box {
        center: [f32; 2],
        axis: [f32; 2],
        half: [f32; 2],
        round: f32,
    },
    /// Капсула от `a` до `b` радиусом `r`: круг — `a == b`, тонкая линия — `r == 0`.
    Capsule { a: [f32; 2], b: [f32; 2], r: f32 },
    /// Выпуклый многоугольник, вершины против часовой стрелки.
    Poly(Vec<[f32; 2]>),
}

/// Препятствие для частиц.
#[derive(Clone, Debug, PartialEq)]
pub struct Obstacle {
    pub shape: ObstacleShape,
    /// Скорость, единиц в секунду, и вращение, радиан в секунду против часовой стрелки
    /// вокруг `pivot`.
    pub vel: [f32; 2],
    pub spin: f32,
    pub pivot: [f32; 2],
    /// Держит только сверху (доска).
    pub oneway: bool,
    /// Описанный прямоугольник.
    pub aabb: Aabb,
}

/// Раскладывает поверхность на препятствия для частиц. `dt` — длина шага логики, за
/// который поверхность сдвинулась на [`Surface::moved`] (0 — стоит).
pub fn obstacles(s: &Surface, dt: f64, out: &mut Vec<Obstacle>) {
    let (vel, spin) = if dt > 1e-9 {
        (
            [(s.moved[0] / dt) as f32, (s.moved[1] / dt) as f32],
            (s.turned / dt) as f32,
        )
    } else {
        ([0.0, 0.0], 0.0)
    };
    let base = Obstacle {
        shape: ObstacleShape::Capsule {
            a: [0.0, 0.0],
            b: [0.0, 0.0],
            r: 0.0,
        },
        vel,
        spin,
        pivot: [s.center[0] as f32, s.center[1] as f32],
        oneway: s.oneway,
        aabb: s.aabb,
    };
    shape_obstacles(s.shape.as_ref(), &s.pose, &base, out);
}

fn shape_obstacles(shape: &dyn Shape, pose: &Pose, base: &Obstacle, out: &mut Vec<Obstacle>) {
    let pt = |q: Vector| {
        let w = pose.transform_point(q);
        [w.x, w.y]
    };
    let push = |shape: ObstacleShape, out: &mut Vec<Obstacle>| {
        let mut o = base.clone();
        o.aabb = shape_aabb(&shape);
        o.shape = shape;
        out.push(o);
    };
    let axis = {
        let x = pose.rotation * Vector::X;
        [x.x, x.y]
    };
    let center = [pose.translation.x, pose.translation.y];
    if let Some(c) = shape.as_cuboid() {
        push(
            ObstacleShape::Box {
                center,
                axis,
                half: [c.half_extents.x, c.half_extents.y],
                round: 0.0,
            },
            out,
        );
    } else if let Some(c) = shape.as_round_cuboid() {
        let h = c.inner_shape.half_extents;
        push(
            ObstacleShape::Box {
                center,
                axis,
                half: [h.x, h.y],
                round: c.border_radius,
            },
            out,
        );
    } else if let Some(b) = shape.as_ball() {
        push(
            ObstacleShape::Capsule {
                a: center,
                b: center,
                r: b.radius,
            },
            out,
        );
    } else if let Some(c) = shape.as_capsule() {
        push(
            ObstacleShape::Capsule {
                a: pt(c.segment.a),
                b: pt(c.segment.b),
                r: c.radius,
            },
            out,
        );
    } else if let Some(sg) = shape.as_segment() {
        push(
            ObstacleShape::Capsule {
                a: pt(sg.a),
                b: pt(sg.b),
                r: 0.0,
            },
            out,
        );
    } else if let Some(p) = shape.as_convex_polygon() {
        push(
            ObstacleShape::Poly(p.points().iter().map(|q| pt(*q)).collect()),
            out,
        );
    } else if let Some(p) = shape.as_round_convex_polygon() {
        push(
            ObstacleShape::Poly(p.inner_shape.points().iter().map(|q| pt(*q)).collect()),
            out,
        );
    } else if let Some(l) = shape.as_polyline() {
        for sg in l.segments() {
            push(
                ObstacleShape::Capsule {
                    a: pt(sg.a),
                    b: pt(sg.b),
                    r: 0.0,
                },
                out,
            );
        }
    } else if let Some(c) = shape.as_compound() {
        for (child_pose, child) in c.shapes() {
            shape_obstacles(child.as_ref(), &(*pose * *child_pose), base, out);
        }
    }
}

/// Описанный прямоугольник формы препятствия.
pub fn shape_aabb(s: &ObstacleShape) -> Aabb {
    match s {
        ObstacleShape::Box {
            center,
            axis,
            half,
            round,
        } => {
            let ex = (axis[0] * half[0]).abs() + (axis[1] * half[1]).abs() + round;
            let ey = (axis[1] * half[0]).abs() + (axis[0] * half[1]).abs() + round;
            Aabb {
                min: [f64::from(center[0] - ex), f64::from(center[1] - ey)],
                max: [f64::from(center[0] + ex), f64::from(center[1] + ey)],
            }
        }
        ObstacleShape::Capsule { a, b, r } => Aabb {
            min: [f64::from(a[0].min(b[0]) - r), f64::from(a[1].min(b[1]) - r)],
            max: [f64::from(a[0].max(b[0]) + r), f64::from(a[1].max(b[1]) + r)],
        },
        ObstacleShape::Poly(points) => Aabb::of_points(
            points.iter().map(|q| [f64::from(q[0]), f64::from(q[1])]),
        )
        .unwrap_or(Aabb {
            min: [0.0, 0.0],
            max: [0.0, 0.0],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Collider, Form};

    #[test]
    fn surfaces_become_simple_shapes() {
        let mut out = Vec::new();
        let floor = Surface::of_collider(&Collider::solid(Aabb {
            min: [-1.0, -1.0],
            max: [1.0, 0.0],
        }));
        obstacles(&floor, 1.0 / 60.0, &mut out);
        assert!(matches!(
            out[0].shape,
            ObstacleShape::Box {
                half: [1.0, 0.5],
                ..
            }
        ));
        // Повёрнутый ящик, едущий вправо.
        let mut s = Surface::placed(
            2,
            Form::Box {
                half: [0.5, 0.1],
                corner: 0.0,
            }
            .build(1.0, 1.0)
            .unwrap(),
            [0.0, 1.0],
            std::f64::consts::FRAC_PI_2,
        );
        s.moved = [0.01, 0.0];
        obstacles(&s, 0.01, &mut out);
        let ObstacleShape::Box { axis, .. } = out[1].shape else {
            panic!("{:?}", out[1]);
        };
        assert!(
            axis[0].abs() < 1e-5 && (axis[1] - 1.0).abs() < 1e-5,
            "{axis:?}"
        );
        assert!((out[1].vel[0] - 1.0).abs() < 1e-5);
        assert!((out[1].aabb.max[1] - 1.5).abs() < 1e-5, "{:?}", out[1].aabb);
        // Линии — капсулы, контур звезды — отрезки нулевой толщины.
        let stroke = Form::Stroke {
            lines: vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]],
            r: 0.1,
        }
        .build(1.0, 1.0)
        .unwrap();
        out.clear();
        obstacles(&Surface::placed(3, stroke, [0.0, 0.0], 0.0), 0.0, &mut out);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0].shape, ObstacleShape::Capsule { r, .. } if (r - 0.1).abs() < 1e-6));
    }
}
