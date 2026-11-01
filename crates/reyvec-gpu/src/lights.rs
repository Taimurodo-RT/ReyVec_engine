//! Источники света сцены «лампы и тени» (этап 4, задача 4.3).
//!
//! Свет считает видеокарта (`shaders/light.slang`) в разрешении поля расстояний: для
//! каждой точки и каждого источника шаги по полю к источнику дают мягкую тень. Процессор
//! каждый кадр пишет только короткий список источников (до [`MAX_LIGHTS`]).

/// Сколько источников света видеокарта берёт за кадр.
pub(crate) const MAX_LIGHTS: usize = 16;
/// Размер записи источника в буфере (как `struct Light` в `light.slang`, std430).
pub(crate) const LIGHT_SIZE: usize = 48;

/// Вид источника.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LightKind {
    /// Лампа в точке `a` или светящийся отрезок от `a` до `b` (неоновая трубка): светит во
    /// все стороны и гаснет к расстоянию `reach`.
    Local,
    /// Свет издалека (солнце, луна) в направлении на источник `a`: не гаснет, тени длиной
    /// до `reach`.
    Directional,
}

/// Источник света в координатах сцены.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Light {
    pub kind: LightKind,
    pub a: [f32; 2],
    pub b: [f32; 2],
    /// Цвет, умноженный на яркость (линейный).
    pub color: [f32; 3],
    /// Дальность света (у направленного — длина теней), единицы сцены.
    pub reach: f32,
    /// Размер источника: чем больше, тем мягче край тени (единицы сцены).
    pub softness: f32,
}

impl Light {
    /// Лампа в точке.
    pub fn point(at: [f32; 2], color: [f32; 3], reach: f32, softness: f32) -> Self {
        Self {
            kind: LightKind::Local,
            a: at,
            b: at,
            color,
            reach,
            softness,
        }
    }

    /// Светящийся отрезок (неоновая трубка).
    pub fn tube(a: [f32; 2], b: [f32; 2], color: [f32; 3], reach: f32, softness: f32) -> Self {
        Self {
            kind: LightKind::Local,
            a,
            b,
            color,
            reach,
            softness,
        }
    }

    /// Свет издалека, идущий со стороны `towards` (направление на источник).
    pub fn directional(towards: [f32; 2], color: [f32; 3], reach: f32, softness: f32) -> Self {
        let len = towards[0].hypot(towards[1]).max(1e-6);
        Self {
            kind: LightKind::Directional,
            a: [towards[0] / len, towards[1] / len],
            b: [0.0, 0.0],
            color,
            reach,
            softness,
        }
    }

    pub fn to_bytes(self) -> [u8; LIGHT_SIZE] {
        let kind = match self.kind {
            LightKind::Local => 0u32,
            LightKind::Directional => 1,
        };
        let words: [u32; LIGHT_SIZE / 4] = [
            self.a[0].to_bits(),
            self.a[1].to_bits(),
            self.b[0].to_bits(),
            self.b[1].to_bits(),
            self.color[0].to_bits(),
            self.color[1].to_bits(),
            self.color[2].to_bits(),
            self.reach.to_bits(),
            self.softness.to_bits(),
            kind,
            0,
            0,
        ];
        let mut out = [0u8; LIGHT_SIZE];
        for (i, w) in words.iter().enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&w.to_ne_bytes());
        }
        out
    }
}

/// Общий тусклый свет, который есть везде, даже в тени (линейный цвет).
pub(crate) const AMBIENT: [f32; 3] = [0.06, 0.065, 0.08];

/// Источники сцены «лампы и тени» в момент `t`: тёплая лампа кружит по сцене, холодная
/// ходит вдоль нижнего ряда, розовая неоновая трубка над верхним рядом и слабый лунный
/// свет сверху слева с длинными тенями.
pub(crate) fn demo_lights(t: f32) -> Vec<Light> {
    vec![
        Light::point(
            [1.0 * (t * 0.45).cos(), 0.05 + 0.55 * (t * 0.45).sin()],
            [1.7, 1.2, 0.7],
            1.8,
            0.05,
        ),
        Light::point([1.4 * (t * 0.3).sin(), -0.68], [0.45, 0.7, 1.4], 1.2, 0.03),
        Light::tube([-1.0, 0.9], [0.1, 0.9], [1.2, 0.3, 0.9], 0.9, 0.06),
        Light::directional([-0.6, 0.8], [0.12, 0.15, 0.25], 1.0, 0.12),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_layout_matches_shader() {
        let l = Light::directional([3.0, 4.0], [1.0, 2.0, 3.0], 5.0, 0.5);
        let b = l.to_bytes();
        let f = |i: usize| f32::from_ne_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
        assert_eq!((f(0), f(1)), (0.6, 0.8), "направление единичное");
        assert_eq!((f(4), f(5), f(6), f(7), f(8)), (1.0, 2.0, 3.0, 5.0, 0.5));
        assert_eq!(u32::from_ne_bytes(b[36..40].try_into().unwrap()), 1);
        assert!(demo_lights(0.0).len() <= MAX_LIGHTS);
    }
}
