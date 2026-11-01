//! Цвет фигур.

/// Цвет в линейном пространстве (так считает видеокарта), альфа от 0 до 1.
///
/// Цвета из редакторов и SVG записаны в sRGB: их переводит [`Color::from_srgb8`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    /// Белый: оттенок, который не меняет цвет фигуры.
    pub const WHITE: Self = Self::linear(1.0, 1.0, 1.0, 1.0);

    /// Цвет из линейных составляющих.
    pub const fn linear(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Цвет из sRGB по 8 бит (как `#RRGGBB` в редакторах), непрозрачный.
    pub fn from_srgb8(r: u8, g: u8, b: u8) -> Self {
        Self::linear(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), 1.0)
    }

    /// Тот же цвет с другой прозрачностью.
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Произведение цветов по составляющим: так оттенок экземпляра красит фигуру.
    pub fn multiply(self, other: Self) -> Self {
        Self::linear(
            self.r * other.r,
            self.g * other.g,
            self.b * other.b,
            self.a * other.a,
        )
    }

    /// Составляющие `[r, g, b, a]`.
    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Составляющие в sRGB (как в редакторах) от 0 до 1, альфа как есть.
    pub fn to_srgb_array(self) -> [f32; 4] {
        [
            linear_to_srgb(self.r),
            linear_to_srgb(self.g),
            linear_to_srgb(self.b),
            self.a,
        ]
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_conversion_keeps_ends_and_darkens_middle() {
        assert_eq!(Color::from_srgb8(0, 0, 0).r, 0.0);
        assert!((Color::from_srgb8(255, 255, 255).g - 1.0).abs() < 1e-6);
        // Серый 50% в sRGB в линейном пространстве около 0.214.
        assert!((Color::from_srgb8(128, 128, 128).b - 0.2158).abs() < 1e-3);
    }

    #[test]
    fn srgb_round_trip() {
        let c = Color::from_srgb8(10, 128, 250).to_srgb_array();
        for (got, want) in c.iter().zip([10.0, 128.0, 250.0]) {
            assert!((got * 255.0 - want).abs() < 0.01);
        }
    }

    #[test]
    fn multiply_by_white_keeps_color() {
        let c = Color::from_srgb8(10, 200, 30).with_alpha(0.5);
        assert_eq!(c.multiply(Color::WHITE), c);
    }
}
