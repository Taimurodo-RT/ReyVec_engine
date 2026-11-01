//! Демо-сцена этапа 4 «лаборатория мага» (задача 4.6): всё, что умеет свет этапа, в
//! одной сцене.
//!
//! - Векторные предметы (стол, полки, книги, банки, котёл, подставка шара) стоят в слоях
//!   «отбрасывает тень» и дают мягкие тени от каждого источника.
//! - Стекло колб — векторные обводки без тени, под ним светящиеся SDF-зелья: жидкость и
//!   пузыри, которые поднимаются и сливаются, как при кипении.
//! - Неоновая пентаграмма в круге на стене и неоновая лента под полкой — светящиеся
//!   SDF-трубки, которые сами светят (с редким миганием, как у настоящего неона).
//! - Котёл с кипящим варевом, свеча с качающимся пламенем, магический шар с искрами.
//!
//! Фон (каменная стена и дощатый пол) рисует `sdf_demo.slang` (флаг лаборатории).
//! Процессор каждый кадр пишет только положения частей и источников: кривые векторов
//! разбиты на треугольники один раз при запуске.

use std::f32::consts::TAU;
use std::f64::consts::{FRAC_PI_2, PI};

use reyvec_vector::kurbo::{self, Shape as _};
use reyvec_vector::{
    Affine, BezPath, Color, Gradient, Instance, LineCap, LineJoin, SceneItem, Shape, ShapeId,
    ShapeLibrary, Stroke,
};

use crate::blobs::{Blob, OBJECT_EMISSIVE, SdfObject};
use crate::lights::Light;

/// Верх пола (низ стены) и верх стола, единицы сцены (кадр от −1 до 1 по высоте).
const FLOOR: f64 = -0.78;
const TABLE_TOP: f64 = -0.42;
/// Колбы на столе: круглая, стакан, коническая (x середины).
const ROUND_X: f64 = -1.25;
const BEAKER_X: f64 = -0.95;
const CONE_X: f64 = -0.65;
/// Круглая колба: центр шара над столом и его радиус.
const BULB_Y: f64 = 0.116;
const BULB_R: f64 = 0.11;
/// Магический шар на подставке.
const ORB_X: f64 = -0.2;
const STAND_H: f64 = 0.06;
const ORB_R: f32 = 0.085;
/// Свеча на стопке книг.
const CANDLE_X: f64 = 0.14;
const BOOKS_H: f64 = 0.135;
const CANDLE_H: f64 = 0.12;
/// Неоновый знак на стене: центр и радиус круга.
const SIGIL: [f32; 2] = [-0.65, 0.42];
const SIGIL_R: f32 = 0.25;
/// Полки справа: середина по x, ширина, верх верхней и нижней полки, толщина.
const SHELF_X: f64 = 1.17;
const SHELF_W: f64 = 0.9;
const SHELF_TOPS: [f64; 2] = [0.38, -0.02];
const SHELF_T: f64 = 0.04;
/// Неоновая лента под верхней полкой.
const STRIP_Y: f32 = 0.305;
/// Котёл на полу под полками: середина по x, радиус чаши, высота ножек.
const CAULDRON_X: f64 = 1.17;
const CAULDRON_R: f64 = 0.2;
const CAULDRON_LEGS: f64 = 0.05;
/// Толщина стекла колб.
const GLASS: f64 = 0.012;

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_srgb8(r, g, b)
}

/// Фигуры сцены. Большинство белые: цвет каждой копии задаёт оттенок.
pub(crate) struct LabShapes {
    /// Квадрат 1×1 с центром в нуле (доски, ножки, книги, свеча).
    square: ShapeId,
    /// Круг радиуса 1.
    disk: ShapeId,
    /// Банка: прямоугольник 1×1 со скруглёнными углами.
    jar: ShapeId,
    /// Уголок полки: прямоугольный треугольник 1×1 (прямой угол слева сверху).
    bracket: ShapeId,
    cauldron: ShapeId,
    stand: ShapeId,
    /// Стекло колб (обводки): круглая, стакан, коническая.
    round_glass: ShapeId,
    beaker_glass: ShapeId,
    cone_glass: ShapeId,
}

impl LabShapes {
    pub fn new(library: &mut ShapeLibrary) -> Self {
        let mut add = |shape| {
            library
                .add(shape)
                .expect("фигуры лаборатории корректны (проверено тестом)")
        };
        let square = add(Shape::fill(
            kurbo::Rect::new(-0.5, -0.5, 0.5, 0.5).to_path(1e-6),
            Color::WHITE,
        ));
        let disk = add(Shape::fill(
            kurbo::Circle::new((0.0, 0.0), 1.0).to_path(1e-6),
            Color::WHITE,
        ));
        let jar = add(Shape::fill(
            kurbo::RoundedRect::new(-0.5, -0.5, 0.5, 0.5, 0.18).to_path(1e-6),
            Color::WHITE,
        ));
        let bracket = add(Shape::fill(
            polygon(&[(0.0, 0.0), (1.0, 0.0), (0.0, -1.0)]),
            Color::WHITE,
        ));
        // Котёл: чугунная полусфера с бликом слева.
        let mut bowl = BezPath::new();
        bowl.move_to((-CAULDRON_R, 0.0));
        bowl.extend(
            kurbo::Arc::new((0.0, 0.0), (CAULDRON_R, CAULDRON_R), PI, PI, 0.0).append_iter(1e-5),
        );
        bowl.close_path();
        let cauldron = add(Shape::fill(
            bowl,
            Gradient::radial((-0.07, -0.06), 0.24)
                .stop(0.0, rgb(92, 96, 106))
                .stop(1.0, rgb(24, 25, 30)),
        ));
        let stand = add(Shape::fill(
            polygon(&[
                (-0.09, 0.0),
                (0.09, 0.0),
                (0.05, 0.03),
                (0.035, STAND_H),
                (-0.035, STAND_H),
                (-0.05, 0.03),
            ]),
            rgb(176, 124, 58),
        ));
        let glass = |path| {
            Shape::stroke(
                path,
                Stroke::new(GLASS)
                    .with_cap(LineCap::Round)
                    .with_join(LineJoin::Round),
                Color::linear(0.75, 0.88, 1.0, 0.6),
            )
        };
        let round_glass = add(glass(round_flask()));
        let beaker_glass = add(glass(polyline(&[
            (-0.062, 0.27),
            (-0.05, 0.258),
            (-0.05, GLASS / 2.0),
            (0.05, GLASS / 2.0),
            (0.05, 0.258),
            (0.062, 0.27),
        ])));
        let cone_glass = add(glass(polyline(&[
            (-0.042, 0.28),
            (-0.03, 0.27),
            (-0.03, 0.15),
            (-0.12, GLASS / 2.0),
            (0.12, GLASS / 2.0),
            (0.03, 0.15),
            (0.03, 0.27),
            (0.042, 0.28),
        ])));
        Self {
            square,
            disk,
            jar,
            bracket,
            cauldron,
            stand,
            round_glass,
            beaker_glass,
            cone_glass,
        }
    }

    /// Векторные предметы в момент `t`: два слоя «отбрасывает тень» (стол со всем, что на
    /// нём, и полки с котлом: у двух небольших слоёв картинки меньше, чем у одного на всю
    /// сцену) и стекло колб поверх зелий.
    pub fn items(&self, t: f32) -> Vec<SceneItem> {
        let t = f64::from(t);
        let block = |x: f64, y: f64, w: f64, h: f64, angle: f64, color: Color| -> SceneItem {
            Instance::new(
                self.square,
                Affine::translate((x, y)) * Affine::rotate(angle) * Affine::scale_non_uniform(w, h),
            )
            .with_tint(color)
            .into()
        };
        let jar = |x: f64, base: f64, w: f64, h: f64, body: Color| -> [SceneItem; 2] {
            [
                Instance::new(
                    self.jar,
                    Affine::translate((x, base + h / 2.0)) * Affine::scale_non_uniform(w, h),
                )
                .with_tint(body)
                .into(),
                block(x, base + h + 0.012, w * 0.8, 0.026, 0.0, rgb(92, 64, 40)),
            ]
        };
        let wood = rgb(118, 76, 46);
        let dark_wood = rgb(84, 52, 32);
        let mut out = Vec::with_capacity(48);

        // Стол и всё, что на нём (кроме стекла).
        out.push(SceneItem::shadow_caster());
        out.push(block(-0.6, TABLE_TOP - 0.025, 1.9, 0.05, 0.0, wood));
        for x in [-1.45, 0.25] {
            let h = TABLE_TOP - 0.05 - FLOOR;
            out.push(block(x, FLOOR + h / 2.0, 0.05, h, 0.0, dark_wood));
        }
        out.push(block(-0.6, FLOOR + 0.08, 1.7, 0.025, 0.0, dark_wood));
        out.push(
            Instance::new(self.stand, Affine::translate((ORB_X, TABLE_TOP)))
                .with_tint(Color::WHITE)
                .into(),
        );
        // Стопка книг и свеча на ней.
        let books = [
            (0.0, 0.26, 0.05, 0.0, rgb(130, 38, 40)),
            (0.015, 0.22, 0.045, -0.04, rgb(40, 74, 128)),
            (-0.01, 0.19, 0.04, 0.03, rgb(64, 110, 58)),
        ];
        let mut y = TABLE_TOP;
        for (dx, w, h, angle, color) in books {
            out.push(block(CANDLE_X + dx, y + h / 2.0, w, h, angle, color));
            y += h;
        }
        let candle_base = TABLE_TOP + BOOKS_H;
        out.push(block(
            CANDLE_X,
            candle_base + CANDLE_H / 2.0,
            0.035,
            CANDLE_H,
            0.0,
            rgb(236, 222, 190),
        ));
        out.push(SceneItem::EndLayer);

        // Полки, их содержимое и котёл.
        out.push(SceneItem::shadow_caster());
        for top in SHELF_TOPS {
            out.push(block(
                SHELF_X,
                top - SHELF_T / 2.0,
                SHELF_W,
                SHELF_T,
                0.0,
                wood,
            ));
            for (x, sx) in [
                (SHELF_X - SHELF_W / 2.0 + 0.04, 1.0),
                (SHELF_X + SHELF_W / 2.0 - 0.04, -1.0),
            ] {
                out.push(
                    Instance::new(
                        self.bracket,
                        Affine::translate((x, top - SHELF_T))
                            * Affine::scale_non_uniform(0.09 * sx, 0.09),
                    )
                    .with_tint(dark_wood)
                    .into(),
                );
            }
        }
        // Верхняя полка: книги стоят, одна наклонилась, банка и тёмная бутыль.
        let top = SHELF_TOPS[0];
        let standing = [
            (0.80, 0.045, 0.22, rgb(120, 34, 52)),
            (0.85, 0.05, 0.19, rgb(46, 84, 120)),
            (0.905, 0.05, 0.24, rgb(150, 116, 48)),
            (0.958, 0.045, 0.2, rgb(78, 52, 104)),
        ];
        for (x, w, h, color) in standing {
            out.push(block(x, top + h / 2.0, w, h, 0.0, color));
        }
        // Наклонённая книга стоит правым нижним углом на полке, левым верхним опирается
        // на последнюю стоящую книгу (её правый край на x = 0,98).
        let lean: f64 = 0.32;
        let (w, h) = (0.05, 0.21);
        let (sin, cos) = lean.sin_cos();
        let corner_x = 0.983 + w * cos + h * sin;
        out.push(block(
            corner_x - (w / 2.0) * cos - (h / 2.0) * sin,
            top + (h / 2.0) * cos - (w / 2.0) * sin,
            w,
            h,
            lean,
            rgb(60, 104, 84),
        ));
        out.extend(jar(1.27, top, 0.12, 0.14, rgb(176, 156, 124)));
        out.push(
            Instance::new(
                self.disk,
                Affine::translate((1.47, top + 0.058)) * Affine::scale(0.058),
            )
            .with_tint(rgb(52, 70, 62))
            .into(),
        );
        out.push(block(1.47, top + 0.135, 0.036, 0.07, 0.0, rgb(52, 70, 62)));
        out.push(block(1.47, top + 0.175, 0.044, 0.016, 0.0, rgb(92, 64, 40)));
        // Нижняя полка: три банки и две лежащие книги.
        let low = SHELF_TOPS[1];
        out.extend(jar(0.84, low, 0.1, 0.17, rgb(126, 136, 146)));
        out.extend(jar(1.01, low, 0.13, 0.12, rgb(160, 104, 70)));
        out.extend(jar(1.18, low, 0.09, 0.2, rgb(150, 150, 120)));
        out.push(block(1.44, low + 0.025, 0.25, 0.05, 0.0, rgb(110, 40, 36)));
        out.push(block(1.45, low + 0.07, 0.21, 0.04, 0.02, rgb(48, 64, 100)));
        // Котёл на ножках, обод поверх варева.
        let rim = FLOOR + CAULDRON_LEGS + CAULDRON_R;
        for dx in [-0.12, 0.12] {
            out.push(block(
                CAULDRON_X + dx,
                FLOOR + CAULDRON_LEGS / 2.0 + 0.01,
                0.04,
                CAULDRON_LEGS + 0.02,
                0.0,
                rgb(30, 31, 36),
            ));
        }
        out.push(
            Instance::new(self.cauldron, Affine::translate((CAULDRON_X, rim)))
                .with_tint(Color::WHITE)
                .into(),
        );
        out.push(
            Instance::new(
                self.jar,
                Affine::translate((CAULDRON_X, rim)) * Affine::scale_non_uniform(0.48, 0.045),
            )
            .with_tint(rgb(56, 58, 66))
            .into(),
        );
        out.push(SceneItem::EndLayer);

        // Стекло колб поверх зелий (прозрачное, тени не даёт). Колбы чуть покачиваются от
        // кипения.
        let wobble = |ph: f64| 0.004 * (t * 9.0 + ph).sin() * (t * 2.3 + ph).sin().max(0.0);
        for (shape, x, ph) in [
            (self.round_glass, ROUND_X, 0.0),
            (self.beaker_glass, BEAKER_X, 1.7),
            (self.cone_glass, CONE_X, 3.1),
        ] {
            out.push(
                Instance::new(
                    shape,
                    Affine::translate((x, TABLE_TOP)) * Affine::rotate(wobble(ph)),
                )
                .into(),
            );
        }
        out
    }
}

/// Замкнутый многоугольник.
fn polygon(points: &[(f64, f64)]) -> BezPath {
    let mut path = polyline(points);
    path.close_path();
    path
}

/// Ломаная (незамкнутая).
fn polyline(points: &[(f64, f64)]) -> BezPath {
    let mut path = BezPath::new();
    for (i, &p) in points.iter().enumerate() {
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    path
}

/// Круглая колба: горлышко с отогнутым краем и шар, одной линией (низ на нуле).
fn round_flask() -> BezPath {
    let neck = 0.03;
    let open = (neck / BULB_R).asin();
    let join = BULB_Y + BULB_R * open.cos();
    let mut path = polyline(&[(-neck - 0.012, 0.31), (-neck, 0.3), (-neck, join)]);
    path.extend(
        kurbo::Arc::new(
            (0.0, BULB_Y),
            (BULB_R, BULB_R),
            FRAC_PI_2 + open,
            2.0 * PI - 2.0 * open,
            0.0,
        )
        .append_iter(1e-5),
    );
    path.line_to((neck, 0.3));
    path.line_to((neck + 0.012, 0.31));
    path
}

/// Пузыри, поднимающиеся от `from` на высоту `rise` за `period` секунд: `count` штук
/// вразнобой, у каждого своё смещение вбок до `spread`. Пузырь растёт от `r / 2` до `r`
/// и в конце пути лопается (быстро сжимается).
#[allow(clippy::too_many_arguments)]
fn bubbles(
    t: f32,
    from: [f32; 2],
    rise: f32,
    spread: f32,
    r: f32,
    period: f32,
    count: usize,
    seed: f32,
) -> impl Iterator<Item = Blob> {
    (0..count).filter_map(move |i| {
        let k = i as f32;
        let ph = (t / period + k / count as f32 + 0.37 * (k * 1.7 + seed).sin()).rem_euclid(1.0);
        let x = from[0]
            + spread * (k * 2.3 + seed).sin()
            + 0.25 * spread * (t * 4.0 + k * 1.3).sin() * ph;
        let y = from[1] + rise * ph;
        let size = if ph < 0.85 {
            r * (0.5 + 0.5 * ph / 0.85)
        } else {
            r * (1.0 - (ph - 0.85) / 0.15)
        };
        (size > 0.002).then(|| Blob::circle([x, y], size))
    })
}

/// Мигание неона: обычно 1, раз в несколько секунд на долю секунды дрожит.
fn neon_flicker(t: f32) -> f32 {
    let ph = t.rem_euclid(7.3);
    if (4.0..4.35).contains(&ph) && ((ph * 37.0) as i32) % 3 != 0 {
        0.3
    } else {
        1.0
    }
}

/// Дрожание пламени свечи: 0,85…1,1.
fn candle_flicker(t: f32) -> f32 {
    0.97 + 0.08 * (t * 11.0).sin() * (t * 7.3).sin() + 0.04 * (t * 23.0).sin()
}

fn f(v: f64) -> f32 {
    v as f32
}

/// Точки сцены, общие для объектов и источников света.
struct Spots {
    bulb: [f32; 2],
    beaker: [f32; 2],
    cone: [f32; 2],
    orb: [f32; 2],
    flame: [f32; 2],
    brew_y: f32,
}

const SPOTS: Spots = Spots {
    bulb: [ROUND_X as f32, (TABLE_TOP + BULB_Y) as f32],
    beaker: [BEAKER_X as f32, (TABLE_TOP + 0.1) as f32],
    cone: [CONE_X as f32, (TABLE_TOP + 0.06) as f32],
    orb: [ORB_X as f32, (TABLE_TOP + STAND_H) as f32 + ORB_R + 0.005],
    flame: [
        CANDLE_X as f32,
        (TABLE_TOP + BOOKS_H + CANDLE_H) as f32 + 0.03,
    ],
    brew_y: (FLOOR + CAULDRON_LEGS + CAULDRON_R) as f32 + 0.012,
};

/// SDF-объекты сцены в момент `t` (все светятся сами).
pub(crate) fn objects(t: f32) -> Vec<SdfObject> {
    let glow = |color: [f32; 3], k: f32| [color[0] * k, color[1] * k, color[2] * k, 1.0];
    let mut out = Vec::with_capacity(8);
    let table = f(TABLE_TOP);

    // Зелёное зелье в круглой колбе: шар жидкости, пена поднимается по горлышку.
    let mut blobs = vec![Blob::circle(SPOTS.bulb, f(BULB_R - GLASS) - 0.006)];
    blobs.extend(bubbles(
        t,
        [SPOTS.bulb[0], SPOTS.bulb[1] + 0.07],
        0.12,
        0.006,
        0.017,
        1.3,
        4,
        0.0,
    ));
    out.push(SdfObject {
        blobs,
        k: 0.035,
        color: [0.3, 1.0, 0.4, 1.0],
        flags: OBJECT_EMISSIVE,
    });

    // Оранжевое в стакане: столбик жидкости, пузыри выскакивают над ним.
    let mut blobs = vec![Blob::capsule(
        [SPOTS.beaker[0], table + 0.05],
        [SPOTS.beaker[0], table + 0.15],
        0.04,
    )];
    blobs.extend(bubbles(
        t,
        [SPOTS.beaker[0], table + 0.16],
        0.09,
        0.02,
        0.016,
        0.9,
        5,
        2.0,
    ));
    out.push(SdfObject {
        blobs,
        k: 0.04,
        color: [1.0, 0.42, 0.1, 1.0],
        flags: OBJECT_EMISSIVE,
    });

    // Синее в конической колбе: дно шире, пузыри по горлышку.
    let (cx, base) = (SPOTS.cone[0], table);
    let mut blobs = vec![
        Blob::capsule(
            [cx - 0.075, base + 0.035],
            [cx + 0.075, base + 0.035],
            0.022,
        ),
        Blob::circle([cx, base + 0.07], 0.05),
    ];
    blobs.extend(bubbles(
        t,
        [cx, base + 0.1],
        0.16,
        0.004,
        0.014,
        1.6,
        4,
        4.0,
    ));
    out.push(SdfObject {
        blobs,
        k: 0.05,
        color: [0.25, 0.5, 1.0, 1.0],
        flags: OBJECT_EMISSIVE,
    });

    // Варево в котле: выпуклая поверхность над ободом, пузыри растут и лопаются.
    let x = f(CAULDRON_X);
    let mut blobs = vec![Blob::capsule(
        [x - 0.16, SPOTS.brew_y],
        [x + 0.16, SPOTS.brew_y],
        0.03,
    )];
    blobs.extend(bubbles(
        t,
        [x, SPOTS.brew_y + 0.01],
        0.07,
        0.12,
        0.035,
        1.1,
        6,
        1.0,
    ));
    blobs.extend(bubbles(
        t,
        [x, SPOTS.brew_y + 0.05],
        0.22,
        0.08,
        0.014,
        1.7,
        3,
        5.0,
    ));
    out.push(SdfObject {
        blobs,
        k: 0.05,
        color: [0.6, 1.0, 0.15, 1.0],
        flags: OBJECT_EMISSIVE,
    });

    // Магический шар на подставке и искры вокруг.
    let pulse = 1.0 + 0.15 * (t * 4.0).sin();
    let mut blobs = vec![Blob::circle(SPOTS.orb, ORB_R + 0.006 * (t * 4.0).sin())];
    for i in 0..5 {
        let a = t * 1.6 + i as f32 * TAU / 5.0;
        let r = 0.16 + 0.05 * (t * 1.1 + i as f32 * 1.7).sin();
        blobs.push(Blob::circle(
            [
                SPOTS.orb[0] + r * a.cos(),
                SPOTS.orb[1] + 0.02 + r * a.sin() * 0.55,
            ],
            0.022,
        ));
    }
    out.push(SdfObject {
        blobs,
        k: 0.09,
        color: glow([0.75, 0.35, 1.0], pulse),
        flags: OBJECT_EMISSIVE,
    });

    // Пламя свечи: капля, верхушка качается.
    let flame = SPOTS.flame;
    let sway = 0.006 * (t * 3.1).sin() + 0.003 * (t * 8.7).sin();
    out.push(SdfObject {
        blobs: vec![
            Blob::circle([flame[0], flame[1] - 0.008], 0.016),
            Blob::capsule(
                [flame[0], flame[1] - 0.004],
                [flame[0] + sway, flame[1] + 0.04],
                0.003,
            ),
        ],
        k: 0.03,
        color: glow([1.0, 0.55, 0.18], 1.3 * candle_flicker(t)),
        flags: OBJECT_EMISSIVE,
    });

    // Неоновый знак: круг из 16 отрезков и пентаграмма внутри.
    let neon = neon_flicker(t);
    let mut blobs = Vec::with_capacity(21);
    let point = |a: f32, r: f32| [SIGIL[0] + r * a.cos(), SIGIL[1] + r * a.sin()];
    for i in 0..16 {
        let a = i as f32 * TAU / 16.0;
        let b = (i + 1) as f32 * TAU / 16.0;
        blobs.push(Blob::capsule(point(a, SIGIL_R), point(b, SIGIL_R), 0.008));
    }
    let star_r = SIGIL_R * 0.9;
    for i in 0..5 {
        let a = std::f32::consts::FRAC_PI_2 + (2 * i) as f32 * TAU / 5.0;
        let b = std::f32::consts::FRAC_PI_2 + (2 * i + 2) as f32 * TAU / 5.0;
        blobs.push(Blob::capsule(point(a, star_r), point(b, star_r), 0.008));
    }
    out.push(SdfObject {
        blobs,
        k: 0.004,
        color: glow([1.0, 0.2, 0.75], 1.6 * neon),
        flags: OBJECT_EMISSIVE,
    });

    // Голубая неоновая лента под верхней полкой.
    let half = f(SHELF_W / 2.0) - 0.08;
    out.push(SdfObject {
        blobs: vec![Blob::capsule(
            [f(SHELF_X) - half, STRIP_Y],
            [f(SHELF_X) + half, STRIP_Y],
            0.009,
        )],
        k: 0.0,
        color: [0.25, 1.1, 1.6, 1.0],
        flags: OBJECT_EMISSIVE,
    });
    out
}

/// Источники света в момент `t`: зелья, шар, свеча, котёл и неон.
pub(crate) fn lights(t: f32) -> Vec<Light> {
    let pulse = 1.0 + 0.15 * (t * 4.0).sin();
    let neon = neon_flicker(t);
    let candle = candle_flicker(t);
    let brew = 1.0 + 0.1 * (t * 5.0).sin() * (t * 1.3).sin();
    let half = f(SHELF_W / 2.0) - 0.08;
    let x = f(CAULDRON_X);
    vec![
        Light::point(SIGIL, [1.2 * neon, 0.22 * neon, 0.85 * neon], 1.6, 0.2),
        Light::tube(
            [f(SHELF_X) - half, STRIP_Y - 0.005],
            [f(SHELF_X) + half, STRIP_Y - 0.005],
            [0.2, 0.85, 1.25],
            0.8,
            0.05,
        ),
        Light::point(SPOTS.bulb, [0.3, 1.15, 0.4], 0.75, 0.05),
        Light::point(SPOTS.beaker, [1.3, 0.55, 0.12], 0.7, 0.04),
        Light::point(SPOTS.cone, [0.25, 0.55, 1.5], 0.7, 0.05),
        Light::point(
            SPOTS.orb,
            [0.9 * pulse, 0.45 * pulse, 1.5 * pulse],
            1.2,
            0.05,
        ),
        Light::point(
            SPOTS.flame,
            [1.5 * candle, 0.85 * candle, 0.35 * candle],
            1.2,
            0.025,
        ),
        Light::tube(
            [x - 0.14, SPOTS.brew_y + 0.05],
            [x + 0.14, SPOTS.brew_y + 0.05],
            [0.6 * brew, 1.35 * brew, 0.2 * brew],
            1.1,
            0.08,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobs::{MAX_BLOBS, MAX_OBJECTS};
    use crate::lights::MAX_LIGHTS;

    #[test]
    fn shapes_build() {
        let mut library = ShapeLibrary::new(0.0005);
        let shapes = LabShapes::new(&mut library);
        let items = shapes.items(1.0);
        let layers = items
            .iter()
            .filter(|i| {
                matches!(
                    i,
                    SceneItem::BeginLayer {
                        casts_shadow: true,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(layers, 2, "стол и полки — два слоя с тенью");
    }

    #[test]
    fn counts_fit_the_buffers() {
        for t in [0.0f32, 1.0, 2.6, 4.1, 9.9] {
            let objects = objects(t);
            assert!(objects.len() <= MAX_OBJECTS);
            assert!(objects.iter().map(|o| o.blobs.len()).sum::<usize>() <= MAX_BLOBS);
            assert!(lights(t).len() <= MAX_LIGHTS);
        }
    }

    #[test]
    fn bounds_contain_the_shapes() {
        for t in [0.0f32, 0.7, 1.3, 2.5, 4.2] {
            for o in objects(t) {
                let (lo, hi) = o.bounds();
                for i in 0..=20 {
                    let s = i as f32 / 20.0;
                    for p in [
                        [lo[0] + (hi[0] - lo[0]) * s, lo[1]],
                        [lo[0] + (hi[0] - lo[0]) * s, hi[1]],
                        [lo[0], lo[1] + (hi[1] - lo[1]) * s],
                        [hi[0], lo[1] + (hi[1] - lo[1]) * s],
                    ] {
                        assert!(o.distance(p) > 0.0, "t {t}: край {p:?} внутри объекта");
                    }
                }
            }
        }
    }

    #[test]
    fn lights_are_outside_shadow_casters() {
        // Источник внутри предмета, отбрасывающего тень, был бы тёмным: луч к нему из
        // любой точки упирается в предмет. Источники стоят над столом, над ободом котла
        // и под полкой с запасом.
        let l = lights(1.0);
        let table = f(TABLE_TOP);
        for light in &l[2..7] {
            assert!(light.a[1] > table + 0.03, "{light:?} в столе");
        }
        assert!(l[7].a[1] > SPOTS.brew_y + 0.02, "свет котла над ободом");
        assert!(
            l[1].a[1] < f(SHELF_TOPS[0] - SHELF_T) - 0.02,
            "лента под полкой"
        );
    }
}
