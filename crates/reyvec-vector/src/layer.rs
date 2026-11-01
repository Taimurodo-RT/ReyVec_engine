//! Слои: группа фигур, которая сначала рисуется отдельно, а потом целиком накладывается
//! на то, что под ней, с общей прозрачностью и режимом смешивания.
//!
//! Зачем: если сделать полупрозрачными по отдельности части персонажа, их перекрытия
//! станут темнее и сквозь тело будет видна рука. Слой с прозрачностью 0.5 делает
//! полупрозрачным персонажа целиком, как в редакторах («непрозрачность группы»).
//!
//! Кадр описывается плоским списком [`SceneItem`]: фигуры и скобки «начало слоя» и
//! «конец слоя». Слои можно вкладывать друг в друга.

use crate::Instance;

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

/// Элемент кадра векторного слоя по порядку рисования.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneItem {
    /// Фигура.
    Shape(Instance),
    /// Начало слоя: всё до парного [`SceneItem::EndLayer`] рисуется в слой.
    BeginLayer { opacity: f32, blend: BlendMode },
    /// Конец слоя. Лишний конец пропускается, незакрытые слои закрываются в конце кадра.
    EndLayer,
}

impl From<Instance> for SceneItem {
    fn from(i: Instance) -> Self {
        Self::Shape(i)
    }
}

impl SceneItem {
    /// Начало слоя с прозрачностью `opacity` и обычным наложением.
    pub fn layer(opacity: f32) -> Self {
        Self::BeginLayer {
            opacity,
            blend: BlendMode::Normal,
        }
    }

    /// Начало слоя с режимом смешивания `blend`.
    pub fn blend(blend: BlendMode, opacity: f32) -> Self {
        Self::BeginLayer { opacity, blend }
    }
}
