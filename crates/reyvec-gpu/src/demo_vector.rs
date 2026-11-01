//! Сцены песочницы этапа 3: векторные фигуры, перетекание и слои.
//!
//! Все фигуры готовятся один раз, при создании сцены. Каждый кадр меняются только их
//! положения, повороты, масштабы, оттенки и форма перетекания ([`Instance`]), так что
//! процессор не пересчитывает ни одной кривой.
//!
//! Сердце, кольцо и волна закрашены градиентами (радиальный с бликом, линейный и
//! повторяющийся вдоль обводки), остальные фигуры одним цветом.
//!
//! Способы заливки выбраны для каждой фигуры: всё треугольниками, кроме сердца. Его
//! показывает лупа (клавиша Z), а при сильном приближении треугольники видны ломаной,
//! поэтому сердце рисуется трафаретом. Перетекающие фигуры всегда рисуются трафаретом.

use std::f64::consts::{FRAC_PI_2, TAU};

use reyvec_vector::{
    Affine, BezPath, BlendMode, Circle, Color, FillMethod, FillRule, Gradient, Instance, LineCap,
    LineJoin, Rect, SceneItem, Shape, ShapeId, ShapeLibrary, Spread, Stroke,
};

/// Допуск разбиения кривых в единицах сцены (высота кадра = 2 единицы). В окне высотой
/// 1080 пикселей это около четверти пикселя.
const TOLERANCE: f64 = 0.0005;
/// Лепестков у цветка в центре: одна фигура, показанная 12 раз.
const PETALS: usize = 12;
/// Сетка маленьких перетекающих фигур в сцене «перетекание»: столбцов и строк.
const MORPH_GRID: (usize, usize) = (17, 9);
/// Фон режимов смешивания в сцене «слои»: центр x, y, ширина, высота.
const BACKDROP: (f64, f64, f64, f64) = (0.95, 0.0, 1.6, 1.7);

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
    /// Круг → звезда → сердце → кольцо.
    morph: ShapeId,
    /// Сцена «слои»: белый круг радиуса 1 и белый квадрат 1×1 (размер и цвет задают
    /// экземпляры), фон с градиентом и оранжевая капсула.
    disk: ShapeId,
    square: ShapeId,
    backdrop: ShapeId,
    pill: ShapeId,
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
        let mut ring_path = circle(0.28);
        ring_path.extend(circle(0.16));
        // Кольцо: линейный градиент сверху вниз.
        let ring = add(Shape::fill_with(
            ring_path,
            FillRule::EvenOdd,
            Gradient::linear((0.0, 0.28), (0.0, -0.28))
                .stop(0.0, Color::from_srgb8(160, 140, 255))
                .stop(1.0, Color::from_srgb8(60, 40, 170)),
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
        // Волна: обводка с повторяющимся градиентом вдоль.
        let wave = add(Shape::stroke(
            wave(),
            Stroke::new(0.035).with_cap(LineCap::Round),
            Gradient::linear((-0.7, 0.0), (-0.35, 0.0))
                .with_spread(Spread::Reflect)
                .stop(0.0, Color::from_srgb8(245, 163, 199))
                .stop(1.0, Color::from_srgb8(120, 200, 255)),
        ));
        // Сердце трафаретом: его показывает лупа.
        // Сердце: радиальный градиент с бликом слева сверху.
        let heart_shape = Shape::fill(
            heart(0.3),
            Gradient::radial((0.0, 0.05), 0.42)
                .with_focus((-0.12, 0.15))
                .stop(0.0, Color::from_srgb8(255, 170, 180))
                .stop(0.45, Color::from_srgb8(232, 74, 95))
                .stop(1.0, Color::from_srgb8(120, 20, 45)),
        );
        let morph_keyframes = [circle(0.4), star(5, 0.45, 0.2), heart(0.4), {
            let mut ring = circle(0.4);
            ring.extend(circle(0.22).reverse_subpaths());
            ring
        }];
        let disk = add(Shape::fill(circle(1.0), Color::WHITE));
        let square = add(Shape::fill(rect(1.0, 1.0), Color::WHITE));
        // Фон режимов смешивания: светлый сверху, бирюзовый в середине, тёмный снизу.
        let backdrop = add(Shape::fill(
            rect(BACKDROP.2, BACKDROP.3),
            Gradient::linear((0.0, BACKDROP.3 / 2.0), (0.0, -BACKDROP.3 / 2.0))
                .stop(0.0, Color::from_srgb8(245, 245, 240))
                .stop(0.5, Color::from_srgb8(50, 140, 170))
                .stop(1.0, Color::from_srgb8(15, 15, 25)),
        ));
        let pill = add(Shape::fill(
            {
                use reyvec_vector::kurbo::{RoundedRect, Shape as _};
                RoundedRect::new(-0.13, -0.6, 0.13, 0.6, 0.13).to_path(1e-6)
            },
            Color::from_srgb8(255, 140, 40),
        ));
        let heart = library
            .add_with(heart_shape, FillMethod::Stencil)
            .expect("сердце корректно (проверено тестом)");
        let morph = library
            .add_morph(&morph_keyframes, FillRule::NonZero, Color::WHITE)
            .expect("контуры перетекания корректны (проверено тестом)");
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
            morph,
            disk,
            square,
            backdrop,
            pill,
        }
    }

    /// Сцена «слои» в момент `time`.
    ///
    /// Слева два снеговика на полосатом фоне. У левого каждая часть полупрозрачная
    /// отдельно: перекрытия светлее, сквозь тело видны края шаров и шляпы. Правый
    /// целиком в слое с прозрачностью 0.5: он ровно полупрозрачный, как в редакторах.
    /// Его голова со шляпой лежат во вложенном слое (проверка вложенности: если она
    /// сломана, головы не будет).
    ///
    /// Справа фон с градиентом и четыре капсулы, каждая в своём слое с режимом
    /// смешивания: обычное, умножение, экран, сложение (слева направо).
    pub fn layer_items(&self, time: f32) -> Vec<SceneItem> {
        let t = f64::from(time);
        let mut out = Vec::with_capacity(64);
        let shape = |id, transform| SceneItem::from(Instance::new(id, transform));
        let tinted =
            |id, transform, c: Color| SceneItem::from(Instance::new(id, transform).with_tint(c));
        // Полосатый фон под снеговиками.
        let (x0, x1) = (-1.7, -0.2);
        let stripes = 10;
        let w = (x1 - x0) / f64::from(stripes);
        for i in 0..stripes {
            let c = if i % 2 == 0 {
                Color::from_srgb8(200, 60, 80)
            } else {
                Color::from_srgb8(40, 50, 90)
            };
            let x = x0 + w * (f64::from(i) + 0.5);
            out.push(tinted(
                self.square,
                Affine::translate((x, 0.0)) * Affine::scale_non_uniform(w, 1.7),
                c,
            ));
        }
        // Снеговики: шары снизу вверх, потом шляпа (поля и тулья).
        let sway = 0.03 * (t * 0.9).sin();
        let snow = Color::from_srgb8(235, 242, 255);
        let hat = Color::from_srgb8(30, 30, 45);
        let body = |x: f64, alpha: f32| {
            let c = |base: Color| Color {
                a: base.a * alpha,
                ..base
            };
            [
                tinted(
                    self.disk,
                    Affine::translate((x, -0.5)) * Affine::scale(0.24),
                    c(snow),
                ),
                tinted(
                    self.disk,
                    Affine::translate((x + sway * 0.5, -0.2)) * Affine::scale(0.17),
                    c(snow),
                ),
            ]
        };
        let head = |x: f64, alpha: f32| {
            let c = |base: Color| Color {
                a: base.a * alpha,
                ..base
            };
            let x = x + sway;
            [
                tinted(
                    self.disk,
                    Affine::translate((x, 0.04)) * Affine::scale(0.12),
                    c(snow),
                ),
                tinted(
                    self.square,
                    Affine::translate((x, 0.13)) * Affine::scale_non_uniform(0.3, 0.04),
                    c(hat),
                ),
                tinted(
                    self.square,
                    Affine::translate((x, 0.23)) * Affine::scale_non_uniform(0.17, 0.18),
                    c(hat),
                ),
            ]
        };
        // Левый: каждая часть с прозрачностью 0.5.
        out.extend(body(-1.32, 0.5));
        out.extend(head(-1.32, 0.5));
        // Правый: непрозрачные части в слое 0.5, голова во вложенном слое.
        out.push(SceneItem::layer(0.5));
        out.extend(body(-0.58, 1.0));
        out.push(SceneItem::layer(1.0));
        out.extend(head(-0.58, 1.0));
        out.push(SceneItem::EndLayer);
        out.push(SceneItem::EndLayer);

        // Справа: фон и капсулы с режимами смешивания.
        let (bx, by, _, _) = BACKDROP;
        out.push(shape(self.backdrop, Affine::translate((bx, by))));
        let modes = [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Add,
        ];
        for (i, blend) in modes.into_iter().enumerate() {
            let k = i as f64;
            let x = bx + (k - 1.5) * 0.38;
            let y = by + 0.12 * (t * 0.8 + k).sin();
            out.push(SceneItem::blend(blend, 1.0));
            out.push(shape(self.pill, Affine::translate((x, y))));
            // Синий кружок внутри: слой накладывается вместе с ним как одно целое.
            out.push(tinted(
                self.disk,
                Affine::translate((x, y + 0.3)) * Affine::scale(0.08),
                Color::from_srgb8(70, 120, 255),
            ));
            out.push(SceneItem::EndLayer);
        }
        out
    }

    /// Сцена «перетекание» в момент `time`: сетка маленьких фигур, у каждой свой темп, и
    /// большая фигура в центре поверх них. Каждый кадр меняется только число `morph` в
    /// экземплярах: смешивает контуры видеокарта.
    pub fn morph_instances(&self, time: f32) -> Vec<Instance> {
        let (cols, rows) = MORPH_GRID;
        let last = 3.0; // номер последнего ключевого контура
        // Туда и обратно: 0 → 3 → 0, плавно замедляясь у краёв.
        let swing = |phase: f32| last * (0.5 - 0.5 * (time * 0.7 + phase).cos());
        let mut out = Vec::with_capacity(cols * rows + 1);
        for r in 0..rows {
            for c in 0..cols {
                let x = (c as f64 - (cols - 1) as f64 / 2.0) * 0.2;
                let y = (r as f64 - (rows - 1) as f64 / 2.0) * 0.2;
                let k = (c + r) as f32 / (cols + rows) as f32;
                let [red, green, blue] = soft_hue(k);
                out.push(
                    Instance::new(self.morph, Affine::translate((x, y)) * Affine::scale(0.2))
                        .with_tint(Color::linear(red, green, blue, 0.55))
                        .with_morph(swing(k * TAU as f32)),
                );
            }
        }
        out.push(
            Instance::new(self.morph, Affine::scale(1.4))
                .with_tint(Color::from_srgb8(242, 193, 78))
                .with_morph(swing(0.0)),
        );
        out
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

/// Прямоугольник `w`×`h` с центром в нуле.
fn rect(w: f64, h: f64) -> BezPath {
    use reyvec_vector::kurbo::Shape as _;
    Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0).to_path(1e-6)
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

/// Звезда с `points` лучами: радиус лучей `outer`, впадин `inner`.
fn star(points: usize, outer: f64, inner: f64) -> BezPath {
    let mut path = BezPath::new();
    for i in 0..points * 2 {
        let r = if i % 2 == 0 { outer } else { inner };
        let a = FRAC_PI_2 + i as f64 * TAU / (points * 2) as f64;
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
        // У всех фигур есть треугольники, кроме перетекающей: она только трафаретом.
        assert!(
            demo.library
                .iter()
                .all(|(id, _, m)| (m.triangle_count() > 0) != (id == demo.morph))
        );
        assert_eq!(demo.library.method(demo.heart), Some(FillMethod::Stencil));
        assert_eq!(demo.library.morph_keyframes(demo.morph).len(), 3);
        let morphs = demo.morph_instances(1.0);
        assert_eq!(morphs.len(), MORPH_GRID.0 * MORPH_GRID.1 + 1);
        assert!(morphs.iter().all(|i| (0.0..=3.0).contains(&i.morph)));
        let instances = demo.instances(1.0);
        let petals = instances.iter().filter(|i| i.shape == demo.petal).count();
        assert_eq!(petals, PETALS);
        // Кадры отличаются только экземплярами: фигуры не пересчитываются.
        let rev = demo.library.revision();
        let _ = demo.instances(2.0);
        let _ = demo.layer_items(2.0);
        assert_eq!(demo.library.revision(), rev);
        // Сцена «слои»: скобки слоёв парные, у правого снеговика вложенный слой.
        let items = demo.layer_items(1.0);
        let mut depth = 0i32;
        let mut deepest = 0;
        for item in &items {
            match item {
                SceneItem::BeginLayer { .. } => depth += 1,
                SceneItem::EndLayer => depth -= 1,
                SceneItem::Shape(_) => {}
            }
            assert!(depth >= 0);
            deepest = deepest.max(depth);
        }
        assert_eq!((depth, deepest), (0, 2));
    }
}
