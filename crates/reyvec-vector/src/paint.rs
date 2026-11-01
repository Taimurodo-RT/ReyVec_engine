//! Чем закрашивается фигура: один цвет или градиент.
//!
//! Градиент задаётся в координатах фигуры (как `gradientUnits="userSpaceOnUse"` в SVG),
//! поэтому он двигается, поворачивается и перетекает вместе с фигурой. Считает его
//! пиксельный шейдер, процессор каждый кадр ничего не делает.
//!
//! Цвета между точками градиента смешиваются в sRGB, без умножения на прозрачность, как в
//! SVG и в `resvg` (по нему будем сверять импорт SVG в 3.5). Так градиент выглядит так же,
//! как в редакторе, где его нарисовали.

use kurbo::{Affine, Point};

use crate::Color;

/// Закраска фигуры.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    /// Один цвет.
    Solid(Color),
    /// Градиент.
    Gradient(Gradient),
}

impl From<Color> for Paint {
    fn from(c: Color) -> Self {
        Self::Solid(c)
    }
}

impl From<Gradient> for Paint {
    fn from(g: Gradient) -> Self {
        Self::Gradient(g)
    }
}

/// Форма градиента.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientKind {
    /// Цвет меняется вдоль прямой от `start` (доля 0) к `end` (доля 1).
    Linear { start: Point, end: Point },
    /// Цвет меняется от точки `focus` (доля 0) к окружности с центром `center` и
    /// радиусом `radius` (доля 1). Обычно `focus` = `center`; сдвинутый фокус даёт
    /// «блик» сбоку, как `fx`/`fy` в SVG. Фокус вне круга подтягивается внутрь, как в SVG.
    Radial {
        center: Point,
        radius: f64,
        focus: Point,
    },
}

/// Что за пределами доли от 0 до 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Spread {
    /// Крайние цвета тянутся дальше (как `pad` в SVG).
    #[default]
    Pad,
    /// Градиент повторяется туда и обратно (`reflect`).
    Reflect,
    /// Градиент повторяется (`repeat`).
    Repeat,
}

/// Точка градиента: доля от 0 до 1 и цвет в ней.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

/// Градиент: форма, точки цвета, что за краями и дополнительное преобразование (как
/// `gradientTransform` в SVG: например, сплющенный радиальный градиент).
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    /// По возрастанию доли. Если пусто, фигура прозрачная.
    pub stops: Vec<GradientStop>,
    pub spread: Spread,
    pub transform: Affine,
}

impl Gradient {
    /// Линейный градиент от `start` к `end`.
    pub fn linear(start: impl Into<Point>, end: impl Into<Point>) -> Self {
        Self::new(GradientKind::Linear {
            start: start.into(),
            end: end.into(),
        })
    }

    /// Радиальный градиент из центра `center` радиусом `radius`.
    pub fn radial(center: impl Into<Point>, radius: f64) -> Self {
        let center = center.into();
        Self::new(GradientKind::Radial {
            center,
            radius,
            focus: center,
        })
    }

    fn new(kind: GradientKind) -> Self {
        Self {
            kind,
            stops: Vec::new(),
            spread: Spread::Pad,
            transform: Affine::IDENTITY,
        }
    }

    /// С ещё одной точкой цвета.
    pub fn stop(mut self, offset: f32, color: Color) -> Self {
        self.stops.push(GradientStop { offset, color });
        self
    }

    /// С другим поведением за краями.
    pub fn with_spread(self, spread: Spread) -> Self {
        Self { spread, ..self }
    }

    /// Со сдвинутым фокусом (только у радиального).
    pub fn with_focus(mut self, focus: impl Into<Point>) -> Self {
        if let GradientKind::Radial { focus: f, .. } = &mut self.kind {
            *f = focus.into();
        }
        self
    }

    /// С преобразованием градиента внутри фигуры.
    pub fn with_transform(self, transform: Affine) -> Self {
        Self { transform, ..self }
    }

    /// Точки цвета, готовые для видеокарты: доли по возрастанию в пределах 0..1, фокус
    /// внутри круга. Цвета в sRGB без умножения на прозрачность.
    pub fn normalized_stops(&self) -> Vec<(f32, [f32; 4])> {
        let mut last = 0.0f32;
        self.stops
            .iter()
            .map(|s| {
                // Как в SVG: доля не меньше предыдущей.
                last = s.offset.clamp(0.0, 1.0).max(last);
                (last, s.color.to_srgb_array())
            })
            .collect()
    }

    /// Фокус радиального градиента, подтянутый внутрь круга (чуть внутрь, иначе на
    /// краю формула делит на ноль).
    pub fn clamped_focus(&self) -> Option<Point> {
        let GradientKind::Radial {
            center,
            radius,
            focus,
        } = self.kind
        else {
            return None;
        };
        let d = focus - center;
        let max = radius * 0.999;
        Some(if d.hypot() > max && d.hypot() > 0.0 {
            center + d * (max / d.hypot())
        } else {
            focus
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_are_sorted_and_clamped_like_svg() {
        let g = Gradient::linear((0.0, 0.0), (1.0, 0.0))
            .stop(-0.5, Color::WHITE)
            .stop(0.6, Color::WHITE)
            .stop(0.3, Color::WHITE)
            .stop(2.0, Color::from_srgb8(255, 0, 0));
        let offsets: Vec<f32> = g.normalized_stops().iter().map(|s| s.0).collect();
        assert_eq!(offsets, [0.0, 0.6, 0.6, 1.0]);
        let red = g.normalized_stops()[3].1;
        assert!((red[0] - 1.0).abs() < 1e-3 && red[1].abs() < 1e-6);
    }

    #[test]
    fn focus_outside_is_pulled_in() {
        let g = Gradient::radial((0.0, 0.0), 1.0).with_focus((3.0, 0.0));
        let f = g.clamped_focus().unwrap();
        assert!(f.x < 1.0 && f.x > 0.99 && f.y == 0.0);
        let inside = Gradient::radial((0.0, 0.0), 1.0).with_focus((0.2, 0.1));
        assert_eq!(inside.clamped_focus(), Some(Point::new(0.2, 0.1)));
        assert_eq!(
            Gradient::linear((0.0, 0.0), (1.0, 0.0)).clamped_focus(),
            None
        );
    }
}
