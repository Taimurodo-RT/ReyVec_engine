//! Описание фигуры: контур, способ закраски и цвет.

use kurbo::BezPath;

use crate::Paint;

/// Какие точки считать внутри контура, если он пересекает сам себя или в нём есть дырки.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillRule {
    /// Внутри всё, вокруг чего контур обходит хотя бы раз в одну сторону. Звезда,
    /// нарисованная одной линией, закрашена целиком. Правило по умолчанию в SVG.
    #[default]
    NonZero,
    /// Внутри то, что обведено нечётное число раз: у такой звезды центр остаётся пустым,
    /// а круг внутри круга даёт кольцо.
    EvenOdd,
}

/// Как видеокарта рисует залитую фигуру. Выбирается для каждой фигуры отдельно при
/// добавлении в [`crate::ShapeLibrary`]; у обводок не важен (они всегда треугольниками).
/// Почему по умолчанию треугольники: `docs/decisions/0009-vector-layer.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillMethod {
    /// Заранее разбитые треугольники: все такие фигуры рисуются одной общей командой.
    /// Дёшево на любой видеокарте, но изгибы при сильном приближении становятся ломаными.
    #[default]
    Triangles,
    /// Трафарет, кривые точной формулой в шейдере: гладко при любом приближении, но две
    /// команды на каждую фигуру в кадре. Перетекающие фигуры рисуются только так.
    Stencil,
}

impl FillMethod {
    /// Название для панели и лога.
    pub fn label(self) -> &'static str {
        match self {
            FillMethod::Triangles => "треугольники",
            FillMethod::Stencil => "трафарет",
        }
    }
}

/// Как соединяются отрезки обводки в углах.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineJoin {
    /// Острый угол (если он слишком острый, срезается, см. [`Stroke::miter_limit`]).
    #[default]
    Miter,
    /// Скруглённый угол.
    Round,
    /// Срезанный угол.
    Bevel,
}

/// Как выглядят концы незамкнутой обводки.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineCap {
    /// Обрывается ровно в конце линии.
    #[default]
    Butt,
    /// Полукруг за концом линии.
    Round,
    /// Квадрат за концом линии.
    Square,
}

/// Обводка контура линией.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    /// Толщина линии в единицах фигуры.
    pub width: f64,
    pub join: LineJoin,
    pub cap: LineCap,
    /// Во сколько раз острый угол может вылезти дальше половины толщины, прежде чем его
    /// срежут. Как в SVG, по умолчанию 4.
    pub miter_limit: f64,
}

impl Stroke {
    /// Обводка толщиной `width` с острыми углами и обрезанными концами (как в SVG).
    pub fn new(width: f64) -> Self {
        Self {
            width,
            join: LineJoin::Miter,
            cap: LineCap::Butt,
            miter_limit: 4.0,
        }
    }

    /// С другим соединением отрезков.
    pub fn with_join(self, join: LineJoin) -> Self {
        Self { join, ..self }
    }

    /// С другими концами линии.
    pub fn with_cap(self, cap: LineCap) -> Self {
        Self { cap, ..self }
    }
}

/// Чем закрашивается фигура: внутренность контура или линия вдоль него.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Fill(FillRule),
    Stroke(Stroke),
}

/// Векторная фигура: контур, способ закраски и цвет.
///
/// Координаты контура свои у каждой фигуры (обычно с центром в нуле). Где фигура стоит
/// на сцене, задаёт [`crate::Instance`]: одну фигуру можно показать много раз.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub path: BezPath,
    pub style: Style,
    /// Цвет или градиент.
    pub paint: Paint,
}

impl Shape {
    /// Залитая фигура (правило [`FillRule::NonZero`]).
    pub fn fill(path: BezPath, paint: impl Into<Paint>) -> Self {
        Self {
            path,
            style: Style::Fill(FillRule::NonZero),
            paint: paint.into(),
        }
    }

    /// Залитая фигура с заданным правилом заливки.
    pub fn fill_with(path: BezPath, rule: FillRule, paint: impl Into<Paint>) -> Self {
        Self {
            path,
            style: Style::Fill(rule),
            paint: paint.into(),
        }
    }

    /// Обводка контура.
    pub fn stroke(path: BezPath, stroke: Stroke, paint: impl Into<Paint>) -> Self {
        Self {
            path,
            style: Style::Stroke(stroke),
            paint: paint.into(),
        }
    }
}
