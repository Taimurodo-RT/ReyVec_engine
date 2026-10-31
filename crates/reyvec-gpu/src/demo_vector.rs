//! Сцена песочницы этапа 3: векторные фигуры.
//!
//! Все фигуры разбиваются на треугольники один раз, при создании сцены. Каждый кадр
//! меняются только их положения, повороты, масштабы и оттенки ([`Instance`]), так что
//! процессор не пересчитывает ни одной кривой.

use std::f64::consts::{FRAC_PI_2, TAU};

use reyvec_vector::{
    Affine, BezPath, Circle, Color, FillRule, Instance, LineCap, LineJoin, Shape, ShapeId,
    ShapeLibrary, Stroke,
};

/// Допуск разбиения кривых в единицах сцены (высота кадра = 2 единицы). В окне высотой
/// 1080 пикселей это около четверти пикселя.
const TOLERANCE: f64 = 0.0005;
/// Лепестков у цветка в центре: одна фигура, показанная 12 раз.
const PETALS: usize = 12;

pub(crate) struct VectorDemo {
    pub library: ShapeLibrary,
    star_nonzero: ShapeId,
    star_evenodd: ShapeId,
    heart: ShapeId,
    ring: ShapeId,
    petal: ShapeId,
    core: ShapeId,
    joins: [ShapeId; 3],
    wave: ShapeId,
}

impl VectorDemo {
    pub fn new() -> Self {
        let mut library = ShapeLibrary::new(TOLERANCE);
        let mut add = |shape| {
            library
                .add(shape)
                .expect("фигуры сцены песочницы корректны (проверено тестом)")
        };
        let gold = Color::from_srgb8(242, 193, 78);
        let star_nonzero = add(Shape::fill(pentagram(0.32), gold));
        let star_evenodd = add(Shape::fill_with(
            pentagram(0.32),
            FillRule::EvenOdd,
            Color::from_srgb8(78, 205, 196),
        ));
        let heart = add(Shape::fill(heart(0.3), Color::from_srgb8(232, 74, 95)));
        let mut ring_path = circle(0.28);
        ring_path.extend(circle(0.16));
        let ring = add(Shape::fill_with(
            ring_path,
            FillRule::EvenOdd,
            Color::from_srgb8(123, 108, 246),
        ));
        let petal = add(Shape::fill(petal(), Color::WHITE));
        let core = add(Shape::fill(circle(0.07), Color::from_srgb8(255, 233, 168)));
        let blue = Color::from_srgb8(154, 209, 245);
        let joins = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel].map(|join| {
            add(Shape::stroke(
                zigzag(),
                Stroke::new(0.06).with_join(join),
                blue,
            ))
        });
        let wave = add(Shape::stroke(
            wave(),
            Stroke::new(0.035).with_cap(LineCap::Round),
            Color::from_srgb8(245, 163, 199),
        ));
        Self {
            library,
            star_nonzero,
            star_evenodd,
            heart,
            ring,
            petal,
            core,
            joins,
            wave,
        }
    }

    /// Фигуры на сцене в момент `time` (секунды), в порядке рисования.
    pub fn instances(&self, time: f32) -> Vec<Instance> {
        let t = f64::from(time);
        let mut out = Vec::with_capacity(10 + PETALS);
        // Слева: одна и та же звезда одной линией с разными правилами заливки.
        out.push(Instance::new(
            self.star_nonzero,
            Affine::translate((-1.2, 0.42)) * Affine::rotate(t * 0.4),
        ));
        out.push(Instance::new(
            self.star_evenodd,
            Affine::translate((-1.2, -0.42)) * Affine::rotate(-t * 0.4),
        ));
        // Справа: сердце бьётся (масштаб), кольцо сплющивается (разный масштаб по осям).
        let beat = 1.0 + 0.08 * (t * 3.0).sin().max(0.0);
        out.push(Instance::new(
            self.heart,
            Affine::translate((1.2, 0.42)) * Affine::scale(beat),
        ));
        let squash = 0.2 * (t * 1.5).sin();
        out.push(Instance::new(
            self.ring,
            Affine::translate((1.2, -0.42)) * Affine::scale_non_uniform(1.0 + squash, 1.0 - squash),
        ));
        // В центре: цветок из 12 копий одного лепестка с разными оттенками.
        for i in 0..PETALS {
            let k = i as f64 / PETALS as f64;
            let [r, g, b] = soft_hue(k as f32);
            out.push(
                Instance::new(
                    self.petal,
                    Affine::translate((0.0, 0.05)) * Affine::rotate(k * TAU + t * 0.5),
                )
                .with_tint(Color::linear(r, g, b, 0.9)),
            );
        }
        out.push(Instance::new(self.core, Affine::translate((0.0, 0.05))));
        // Сверху: углы обводки (острый, скруглённый, срезанный).
        for (i, &shape) in self.joins.iter().enumerate() {
            out.push(Instance::new(
                shape,
                Affine::translate((-0.55 + 0.55 * i as f64, 0.78)),
            ));
        }
        // Снизу: волна со скруглёнными концами покачивается.
        out.push(Instance::new(
            self.wave,
            Affine::translate((0.08 * (t * 1.2).sin(), -0.78)),
        ));
        out
    }
}

/// Круг радиуса `r` с центром в нуле (четыре кубические кривые).
fn circle(r: f64) -> BezPath {
    use reyvec_vector::kurbo::Shape as _;
    Circle::new((0.0, 0.0), r).to_path(1e-6)
}

/// Пятиконечная звезда одной линией (контур пересекает сам себя) радиуса `r`.
fn pentagram(r: f64) -> BezPath {
    let mut path = BezPath::new();
    for i in 0..5 {
        let a = FRAC_PI_2 + f64::from(i * 2) * TAU / 5.0;
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

/// Сердце из четырёх кубических кривых Безье, высотой около `2 * size`.
fn heart(size: f64) -> BezPath {
    let mut path = BezPath::new();
    path.move_to((0.0, -1.0));
    path.curve_to((-0.2, -0.75), (-1.0, -0.35), (-1.0, 0.25));
    path.curve_to((-1.0, 0.75), (-0.45, 1.0), (0.0, 0.55));
    path.curve_to((0.45, 1.0), (1.0, 0.75), (1.0, 0.25));
    path.curve_to((1.0, -0.35), (0.2, -0.75), (0.0, -1.0));
    path.close_path();
    path.apply_affine(Affine::scale(size));
    path
}

/// Лепесток из двух квадратичных кривых: от центра вверх и обратно.
fn petal() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((0.0, 0.06));
    path.quad_to((0.11, 0.2), (0.0, 0.36));
    path.quad_to((-0.11, 0.2), (0.0, 0.06));
    path.close_path();
    path
}

/// Ломаная из трёх отрезков с острыми углами: на ней видна разница соединений обводки.
fn zigzag() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((-0.15, -0.06));
    path.line_to((-0.05, 0.07));
    path.line_to((0.05, -0.06));
    path.line_to((0.15, 0.07));
    path
}

/// Волна из кубических кривых шириной 1.4.
fn wave() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((-0.7, 0.0));
    path.curve_to((-0.55, 0.12), (-0.4, 0.12), (-0.35, 0.0));
    path.curve_to((-0.3, -0.12), (-0.05, -0.12), (0.0, 0.0));
    path.curve_to((0.05, 0.12), (0.3, 0.12), (0.35, 0.0));
    path.curve_to((0.4, -0.12), (0.55, -0.12), (0.7, 0.0));
    path
}

/// Мягкий цвет радуги: оттенок `h` от 0 до 1, разбавленный белым.
fn soft_hue(h: f32) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        1.0 - (k.min(4.0 - k).clamp(0.0, 1.0))
    };
    [f(5.0), f(3.0), f(1.0)].map(|c| 0.35 + 0.65 * c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_builds_and_reuses_petal_mesh() {
        let demo = VectorDemo::new();
        assert!(demo.library.iter().all(|(_, _, m)| m.triangle_count() > 0));
        let instances = demo.instances(1.0);
        let petals = instances.iter().filter(|i| i.shape == demo.petal).count();
        assert_eq!(petals, PETALS);
        // Кадры отличаются только экземплярами: фигуры не пересчитываются.
        let rev = demo.library.revision();
        let _ = demo.instances(2.0);
        assert_eq!(demo.library.revision(), rev);
    }
}
