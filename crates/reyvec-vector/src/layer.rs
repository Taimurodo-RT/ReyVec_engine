//! Слои: группа фигур, которая сначала рисуется отдельно, а потом целиком накладывается
//! на то, что под ней, с общей прозрачностью и режимом смешивания.
//!
//! Зачем: если сделать полупрозрачными по отдельности части персонажа, их перекрытия
//! станут темнее и сквозь тело будет видна рука. Слой с прозрачностью 0.5 делает
//! полупрозрачным персонажа целиком, как в редакторах («непрозрачность группы»).
//!
//! Кадр описывается плоским списком [`SceneItem`]: фигуры и скобки «начало слоя» и
//! «конец слоя». Слои можно вкладывать друг в друга.
//!
//! У слоя может быть маска: фигуры между [`SceneItem::BeginMask`] и
//! [`SceneItem::EndMask`] внутри слоя сами не видны, а решают, где слой виден. Маска
//! по прозрачности (обрезка по силуэту, как clip-path в SVG, или мягкий край градиентом)
//! или по яркости (белое видно, чёрное нет, как `mask` в SVG), можно обратную.

use crate::{Affine, Color, Instance};

/// Как слой накладывается на то, что под ним. Цвет слоя здесь уже умножен на его
/// прозрачность.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlendMode {
    /// Обычное наложение.
    #[default]
    Normal,
    /// Умножение: темнит, белый ничего не меняет (тени, краска поверх).
    Multiply,
    /// Экран: светлит, чёрный ничего не меняет (блики, свечение).
    Screen,
    /// Сложение: ярче экрана, быстро уходит в белый (огонь, искры, свет).
    Add,
}

impl BlendMode {
    /// Название для панели и лога.
    pub fn label(self) -> &'static str {
        match self {
            BlendMode::Normal => "обычное",
            BlendMode::Multiply => "умножение",
            BlendMode::Screen => "экран",
            BlendMode::Add => "сложение",
        }
    }
}

/// Что в маске решает, где слой виден.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaskMode {
    /// По прозрачности фигур маски: где они непрозрачны, слой виден. Обычная обрезка по
    /// силуэту (clip-path) и мягкие края.
    #[default]
    Alpha,
    /// По яркости фигур маски: белое видно, чёрное нет, прозрачное нет (как `mask` в SVG).
    Luminance,
}

impl MaskMode {
    /// Название для документов и лога.
    pub fn label(self) -> &'static str {
        match self {
            MaskMode::Alpha => "по прозрачности",
            MaskMode::Luminance => "по яркости",
        }
    }
}

/// Элемент кадра векторного слоя по порядку рисования.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneItem {
    /// Фигура.
    Shape(Instance),
    /// Готовый блок текста (см. крейт `reyvec-text`).
    Text(TextInstance),
    /// Начало слоя: всё до парного [`SceneItem::EndLayer`] рисуется в слой.
    /// `casts_shadow`: фигуры слоя отбрасывают тень (этап 4): их силуэт попадает в поле
    /// расстояний света. Силуэт берётся из картинки слоя, поэтому фигуры рисуются один раз;
    /// маска слоя и его прозрачность на силуэт не влияют.
    BeginLayer {
        opacity: f32,
        blend: BlendMode,
        casts_shadow: bool,
    },
    /// Конец слоя. Лишний конец пропускается, незакрытые слои закрываются в конце кадра.
    EndLayer,
    /// Начало маски слоя, в котором она стоит: фигуры до [`SceneItem::EndMask`] рисуются
    /// в маску, а не на экран. `inverted`: слой виден там, где маски нет (дырка).
    /// Маска вне слоя пропускается вместе с её фигурами. У слоя одна маска: если их
    /// несколько, действует последняя.
    BeginMask { mode: MaskMode, inverted: bool },
    /// Конец маски.
    EndMask,
}

/// Номер готового блока текста в библиотеке текста (`reyvec-text`). Здесь только номер:
/// раскладка строк и атлас букв живут в своём крейте, а сцене нужно знать лишь, какой блок
/// где стоит.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextId(pub u32);

/// Блок текста на сцене в этом кадре: как у фигуры, только место и оттенок. Буквы уже
/// разложены по строкам и лежат на видеокарте, процессор каждый кадр пишет одну запись на
/// блок, сколько бы в нём ни было букв.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextInstance {
    pub text: TextId,
    /// Из координат блока (левый верхний угол в нуле, y вверх, текст уходит вниз) в
    /// координаты сцены.
    pub transform: Affine,
    /// Множитель цвета текста (прозрачность в `a`).
    pub tint: Color,
}

impl TextInstance {
    /// Блок `text` с левым верхним углом в точке `(x, y)` сцены, без поворота.
    pub fn at(text: TextId, x: f64, y: f64) -> Self {
        Self {
            text,
            transform: Affine::translate((x, y)),
            tint: Color::WHITE,
        }
    }
}

impl From<TextInstance> for SceneItem {
    fn from(t: TextInstance) -> Self {
        Self::Text(t)
    }
}

impl From<Instance> for SceneItem {
    fn from(i: Instance) -> Self {
        Self::Shape(i)
    }
}

impl SceneItem {
    /// Тот же элемент, сдвинутый преобразованием `place` (из его координат сцены в новые):
    /// у фигур и текста оно ставится перед их собственным, скобки слоёв не меняются.
    #[must_use]
    pub fn placed(self, place: Affine) -> Self {
        match self {
            Self::Shape(mut i) => {
                i.transform = place * i.transform;
                Self::Shape(i)
            }
            Self::Text(mut t) => {
                t.transform = place * t.transform;
                Self::Text(t)
            }
            other => other,
        }
    }

    /// Начало слоя с прозрачностью `opacity` и обычным наложением.
    pub fn layer(opacity: f32) -> Self {
        Self::BeginLayer {
            opacity,
            blend: BlendMode::Normal,
            casts_shadow: false,
        }
    }

    /// Начало слоя, фигуры которого отбрасывают тень (непрозрачный, обычное наложение).
    pub fn shadow_caster() -> Self {
        Self::BeginLayer {
            opacity: 1.0,
            blend: BlendMode::Normal,
            casts_shadow: true,
        }
    }

    /// Начало маски по прозрачности: обрезка слоя по силуэту фигур маски.
    pub fn clip() -> Self {
        Self::BeginMask {
            mode: MaskMode::Alpha,
            inverted: false,
        }
    }

    /// Начало маски с режимом `mode`.
    pub fn mask(mode: MaskMode, inverted: bool) -> Self {
        Self::BeginMask { mode, inverted }
    }

    /// Начало слоя с режимом смешивания `blend`.
    pub fn blend(blend: BlendMode, opacity: f32) -> Self {
        Self::BeginLayer {
            opacity,
            blend,
            casts_shadow: false,
        }
    }
}
