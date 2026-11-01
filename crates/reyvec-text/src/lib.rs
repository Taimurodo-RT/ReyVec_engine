//! `reyvec-text`: текст векторного слоя ReyVec без видеокарты.
//!
//! Как и с фигурами, процессор работает с текстом один раз, а не каждый кадр:
//!
//! - когда блок текста появляется или меняется, [`TextLibrary`] раскладывает его по
//!   строкам (библиотека `parley`: формы букв, переносы по словам, выравнивание) и
//!   превращает в список прямоугольников букв ([`GlyphQuad`]);
//! - каждая новая буква один раз попадает в атлас ([`GlyphAtlas`]): поле расстояний MSDF
//!   (библиотека `fdsm`), по которому шейдер рисует её чёткой при любом размере;
//! - каждый кадр на сцене стоит только запись «блок N здесь, такого цвета»
//!   ([`reyvec_vector::TextInstance`]), сколько бы букв в блоке ни было.
//!
//! Шрифт по умолчанию — Inter (лицензия OFL, тот же, что у панели отладки): латиница и
//! кириллица. Свои шрифты добавляются через [`TextLibrary::add_font`]. Системные шрифты
//! не читаются: игра должна выглядеть одинаково на любом компьютере.

mod atlas;

use parley::fontique::Blob;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, Layout, LayoutContext, LineHeight,
    PositionedLayoutItem, StyleProperty,
};
use reyvec_vector::{Color, TextId};
use std::sync::Arc;

pub use atlas::{ATLAS_WIDTH, AtlasGlyph, EM_PIXELS, GlyphAtlas, MAX_HEIGHT, RANGE};

/// Встроенный шрифт Inter (OFL-1.1, файл лицензии рядом со шрифтом).
const INTER: &[u8] = include_bytes!("../../reyvec-app/assets/fonts/Inter-Regular.otf");
/// Имя встроенного шрифта.
pub const DEFAULT_FAMILY: &str = "Inter";
/// Размер шрифта, в котором parley раскладывает строки. Потом всё масштабируется до
/// размера блока: так раскладка не зависит от мелких чисел сцены (высота экрана — 2).
const LAYOUT_SIZE: f32 = 64.0;

/// Ошибки текста.
#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("в файле шрифта нет ни одного шрифта, который можно прочитать")]
    BadFont,
}

/// Выравнивание строк в блоке.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    /// По ширине: пробелы растягиваются до краёв блока (последняя строка абзаца по
    /// левому краю, как в книгах).
    Justify,
}

impl Align {
    /// Название для панели и лога.
    pub fn label(self) -> &'static str {
        match self {
            Align::Left => "по левому краю",
            Align::Center => "по центру",
            Align::Right => "по правому краю",
            Align::Justify => "по ширине",
        }
    }
}

/// Блок текста: что написать и как разложить.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    /// Текст. `\n` — новый абзац.
    pub text: String,
    /// Размер шрифта (высота «em») в единицах сцены: высота экрана — 2.
    pub size: f32,
    /// Ширина блока в единицах сцены: длинные строки переносятся по словам. `None` — без
    /// переносов, только по `\n`.
    pub max_width: Option<f32>,
    pub align: Align,
    /// Расстояние между строками в долях размера шрифта.
    pub line_height: f32,
    /// Цвет (линейный, с прозрачностью). На сцене умножается на оттенок экземпляра.
    pub color: Color,
    /// Шрифт по имени семейства. `None` — Inter.
    pub family: Option<String>,
}

impl TextBlock {
    /// Белый текст Inter размером `size`, без переносов, по левому краю.
    pub fn new(text: impl Into<String>, size: f32) -> Self {
        Self {
            text: text.into(),
            size,
            max_width: None,
            align: Align::Left,
            line_height: 1.25,
            color: Color::WHITE,
            family: None,
        }
    }

    #[must_use]
    pub fn width(mut self, max_width: f32) -> Self {
        self.max_width = Some(max_width);
        self
    }

    #[must_use]
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    #[must_use]
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    #[must_use]
    pub fn line_height(mut self, line_height: f32) -> Self {
        self.line_height = line_height;
        self
    }

    #[must_use]
    pub fn family(mut self, family: impl Into<String>) -> Self {
        self.family = Some(family.into());
        self
    }
}

/// Буква блока, готовая к рисованию.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphQuad {
    /// Прямоугольник в координатах блока (единицы сцены, y вверх, левый верхний угол
    /// блока в нуле): левый, верхний, правый, нижний.
    pub rect: [f32; 4],
    /// Тот же прямоугольник в пикселях атласа: левый, верхний, правый, нижний.
    pub uv: [f32; 4],
}

/// Разложенный блок.
#[derive(Clone, Debug, PartialEq)]
pub struct LaidOutText {
    pub glyphs: Vec<GlyphQuad>,
    /// Рамка блока в его координатах: левый, нижний, правый, верхний (0). Ширина — ширина
    /// блока (или самой длинной строки, если ширина не задана).
    pub bounds: [f32; 4],
    /// Сколько получилось строк.
    pub lines: usize,
    pub color: Color,
}

impl LaidOutText {
    /// Ширина и высота блока в единицах сцены.
    pub fn size(&self) -> (f32, f32) {
        (
            self.bounds[2] - self.bounds[0],
            self.bounds[3] - self.bounds[1],
        )
    }
}

/// Все блоки текста и атлас их букв.
pub struct TextLibrary {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    atlas: GlyphAtlas,
    blocks: Vec<(TextBlock, LaidOutText)>,
    revision: u64,
}

impl Default for TextLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl TextLibrary {
    /// Пустая библиотека со встроенным шрифтом Inter.
    pub fn new() -> Self {
        let mut lib = Self {
            fonts: FontContext::new(),
            layouts: LayoutContext::new(),
            atlas: GlyphAtlas::new(),
            blocks: Vec::new(),
            revision: 0,
        };
        lib.add_font(INTER.to_vec())
            .expect("встроенный шрифт Inter читается");
        lib
    }

    /// Добавляет шрифт из файла (OTF, TTF или коллекция). Возвращает имена его семейств
    /// для [`TextBlock::family`].
    pub fn add_font(&mut self, data: Vec<u8>) -> Result<Vec<String>, TextError> {
        let families = self
            .fonts
            .collection
            .register_fonts(Blob::new(Arc::new(data)), None);
        let names: Vec<String> = families
            .iter()
            .filter_map(|(id, _)| self.fonts.collection.family_name(*id).map(str::to_owned))
            .collect();
        if names.is_empty() {
            return Err(TextError::BadFont);
        }
        Ok(names)
    }

    /// Раскладывает блок и добавляет его в библиотеку.
    pub fn add(&mut self, block: TextBlock) -> TextId {
        let laid = self.lay_out(&block);
        self.blocks.push((block, laid));
        self.revision += 1;
        TextId(self.blocks.len() as u32 - 1)
    }

    /// Меняет блок `id` (новый текст, размер, ширина…) и раскладывает его заново. Делать
    /// это стоит, когда текст правда изменился, а не каждый кадр: раскладка и загрузка на
    /// видеокарту стоят времени процессора. Неизвестный `id` пропускается.
    pub fn set(&mut self, id: TextId, block: TextBlock) {
        if self
            .blocks
            .get(id.0 as usize)
            .is_none_or(|(b, _)| *b == block)
        {
            return;
        }
        let laid = self.lay_out(&block);
        self.blocks[id.0 as usize] = (block, laid);
        self.revision += 1;
    }

    /// Разложенный блок `id`.
    pub fn get(&self, id: TextId) -> Option<&LaidOutText> {
        self.blocks.get(id.0 as usize).map(|(_, l)| l)
    }

    /// Описание блока `id`.
    pub fn block(&self, id: TextId) -> Option<&TextBlock> {
        self.blocks.get(id.0 as usize).map(|(b, _)| b)
    }

    /// Все блоки по порядку номеров.
    pub fn iter(&self) -> impl Iterator<Item = (TextId, &LaidOutText)> {
        self.blocks
            .iter()
            .enumerate()
            .map(|(i, (_, l))| (TextId(i as u32), l))
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Атлас букв всех блоков.
    pub fn atlas(&self) -> &GlyphAtlas {
        &self.atlas
    }

    /// Растёт при каждом изменении блоков или атласа: по нему видеокарта понимает, что
    /// пора загрузить текст заново.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn lay_out(&mut self, block: &TextBlock) -> LaidOutText {
        let scale = block.size / LAYOUT_SIZE;
        let family = block.family.as_deref().unwrap_or(DEFAULT_FAMILY);
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, &block.text, 1.0, false);
        builder.push_default(StyleProperty::FontSize(LAYOUT_SIZE));
        builder.push_default(StyleProperty::FontFamily(FontFamily::named(family)));
        builder.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(
            block.line_height,
        )));
        let mut layout: Layout<()> = builder.build(&block.text);
        let max = block.max_width.filter(|w| *w > 0.0).map(|w| w / scale);
        layout.break_all_lines(max);
        // Без заданной ширины выравниваем внутри самой длинной строки.
        let width = match max {
            Some(w) => w,
            None => {
                let w = layout.width();
                layout.break_all_lines(Some(w + 0.5));
                w
            }
        };
        let alignment = match block.align {
            Align::Left => Alignment::Left,
            Align::Center => Alignment::Center,
            Align::Right => Alignment::Right,
            Align::Justify => Alignment::Justify,
        };
        layout.align(alignment, AlignmentOptions::default());

        let mut glyphs = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let font = run.run().font();
                let em = run.run().font_size();
                let data = font.data.as_ref();
                let key = font.data.id();
                for g in run.positioned_glyphs() {
                    let Some(a) = self.atlas.glyph(data, font.index, key, g.id) else {
                        continue;
                    };
                    // parley: y вниз от верха блока; сцена: y вверх.
                    let [l, t, r, b] = a.plane;
                    glyphs.push(GlyphQuad {
                        rect: [
                            (g.x + l * em) * scale,
                            -(g.y - t * em) * scale,
                            (g.x + r * em) * scale,
                            -(g.y - b * em) * scale,
                        ],
                        uv: a.uv,
                    });
                }
            }
        }
        // Атлас мог вырасти: блокам нужна новая загрузка, даже если этот пуст.
        self.revision += 1;
        LaidOutText {
            glyphs,
            bounds: [0.0, -layout.height() * scale, width * scale, 0.0],
            lines: layout.len(),
            color: block.color,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_and_cyrillic_have_glyphs() {
        let mut lib = TextLibrary::new();
        let id = lib.add(TextBlock::new("Привет, Hello!", 0.1));
        let laid = lib.get(id).unwrap();
        // Пробел без контура: 13 букв и знаков.
        assert_eq!(laid.glyphs.len(), 13);
        assert_eq!(laid.lines, 1);
        // Буквы идут слева направо и стоят на одной строке.
        for pair in laid.glyphs.windows(2) {
            assert!(pair[0].rect[0] < pair[1].rect[0]);
        }
        let (w, h) = laid.size();
        assert!(w > 0.5 && w < 1.0, "ширина {w}");
        assert!((h - 0.125).abs() < 1e-3, "высота строки {h}");
        // Все буквы внутри атласа.
        let (aw, ah) = lib.atlas().size();
        for g in &laid.glyphs {
            assert!(g.uv[2] <= aw as f32 && g.uv[3] <= ah as f32);
        }
        // Заглавная «П» выше строчной «р»: верх буквы ближе к верху блока.
        assert!(laid.glyphs[0].rect[1] > laid.glyphs[1].rect[1]);
    }

    #[test]
    fn same_letters_share_atlas_space() {
        let mut lib = TextLibrary::new();
        lib.add(TextBlock::new("ааааа", 0.1));
        assert_eq!(lib.atlas().len(), 1);
        lib.add(TextBlock::new("аб", 0.3));
        assert_eq!(lib.atlas().len(), 2);
    }

    #[test]
    fn wraps_and_aligns() {
        let text = "Съешь же ещё этих мягких французских булок, да выпей чаю.";
        let mut lib = TextLibrary::new();
        let wide = lib.add(TextBlock::new(text, 0.1));
        let left = lib.add(TextBlock::new(text, 0.1).width(1.0));
        let right = lib.add(TextBlock::new(text, 0.1).width(1.0).align(Align::Right));
        let center = lib.add(TextBlock::new(text, 0.1).width(1.0).align(Align::Center));
        let justify = lib.add(TextBlock::new(text, 0.1).width(1.0).align(Align::Justify));
        assert_eq!(lib.get(wide).unwrap().lines, 1);
        let lines = lib.get(left).unwrap().lines;
        assert!(lines >= 3, "строк {lines}");
        for id in [left, right, center, justify] {
            let laid = lib.get(id).unwrap();
            assert_eq!(laid.lines, lines);
            assert_eq!(laid.size().0, 1.0);
            for g in &laid.glyphs {
                // Буквы не вылезают за ширину блока (с запасом поля вокруг буквы).
                assert!(g.rect[0] > -0.02 && g.rect[2] < 1.02, "{:?}", g.rect);
            }
        }
        let first = |id| lib.get(id).unwrap().glyphs[0].rect[0];
        let last_right = |id: TextId| {
            let laid = lib.get(id).unwrap();
            // Правый край первой строки: последняя буква перед переходом вниз.
            let y = laid.glyphs[0].rect[3];
            laid.glyphs
                .iter()
                .filter(|g| (g.rect[3] - y).abs() < 0.03)
                .map(|g| g.rect[2])
                .fold(f32::MIN, f32::max)
        };
        assert!(first(left) < 0.01);
        assert!(first(right) > first(center) && first(center) > first(left));
        // По ширине первая строка от левого края до правого.
        assert!(first(justify) < 0.01);
        assert!(last_right(justify) > 0.97, "{}", last_right(justify));
        assert!(last_right(left) < last_right(justify));
    }

    #[test]
    fn newline_makes_paragraphs_and_set_relays() {
        let mut lib = TextLibrary::new();
        let id = lib.add(TextBlock::new("один\nдва\nтри", 0.1));
        assert_eq!(lib.get(id).unwrap().lines, 3);
        let before = lib.revision();
        lib.set(id, TextBlock::new("один\nдва\nтри", 0.1));
        assert_eq!(
            lib.revision(),
            before,
            "тот же текст не раскладывается заново"
        );
        lib.set(id, TextBlock::new("один", 0.2));
        assert!(lib.revision() > before);
        let laid = lib.get(id).unwrap();
        assert_eq!(laid.lines, 1);
        assert!((laid.size().1 - 0.25).abs() < 1e-3);
    }

    #[test]
    fn atlas_field_is_inside_where_the_letter_is() {
        let mut lib = TextLibrary::new();
        let id = lib.add(TextBlock::new("I", 1.0));
        let g = lib.get(id).unwrap().glyphs[0];
        let (aw, _) = lib.atlas().size();
        let px = |x: f32, y: f32| {
            let at = ((y as u32 * aw + x as u32) * 4) as usize;
            let p = &lib.atlas().pixels()[at..at + 4];
            let mut rgb = [p[0], p[1], p[2]];
            rgb.sort_unstable();
            (rgb[1], p[3])
        };
        // Середина палочки «I» внутри (больше половины), угол картинки снаружи.
        let mid = ((g.uv[0] + g.uv[2]) / 2.0, (g.uv[1] + g.uv[3]) / 2.0);
        let (median, sdf) = px(mid.0, mid.1);
        assert!(median > 140 && sdf > 140, "внутри {median} {sdf}");
        let (median, sdf) = px(g.uv[0] + 0.5, g.uv[1] + 0.5);
        assert!(median < 100 && sdf < 100, "снаружи {median} {sdf}");
    }

    #[test]
    fn custom_font_is_rejected_when_broken() {
        let mut lib = TextLibrary::new();
        assert!(lib.add_font(b"not a font".to_vec()).is_err());
    }
}
