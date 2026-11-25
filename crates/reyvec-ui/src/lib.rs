//! `reyvec-ui`: интерфейс игры ReyVec без видеокарты (решение 0022, `docs/game-ui.md`).
//!
//! Экран пишется в файле `.rui` в виде SwiftUI:
//!
//! ```text
//! screen Demo {
//!     Column(spacing: 16, align: center) {
//!         Text("ReyVec").font(48)
//!         Rect().size(240, 12).background(#7FD36B).radius(6)
//!     }
//!     .padding(32)
//!     .background(#171A21E0)
//!     .radius(16)
//! }
//! ```
//!
//! Путь от файла до рисования:
//!
//! 1. [`syntax`] разбирает текст в дерево блоков с местами в файле;
//! 2. [`model`] проверяет имена и значения и строит элементы с раскладкой и оформлением;
//! 3. [`UiScreen::draw`] раскладывает элементы библиотекой `taffy` (flexbox и сетка, как
//!    в вебе) под размер окна и отдаёт список прямоугольников и букв ([`UiDraw`]);
//! 4. видеокарта (`reyvec-gpu`, проход «интерфейс») рисует список одной командой.
//!
//! Процессор раскладывает экран заново, только когда меняется размер окна или файл:
//! неподвижный экран каждый кадр ничего не пересчитывает.
//!
//! Единицы: пиксели экрана 1920×1080. На другом окне всё умножается на высоту / 1080,
//! поэтому экран выглядит одинаково на любом размере.

pub mod model;
pub mod syntax;

use reyvec_text::{GlyphAtlas, TextBlock, TextLibrary};
use reyvec_vector::{Color, TextId};
use taffy::prelude::*;

use model::{Edge, Element, Justify, Kind, Len};
pub use syntax::Pos;

/// Высота экрана, под которую пишутся размеры в `.rui`.
pub const DESIGN_HEIGHT: f32 = 1080.0;

/// Ошибка файла `.rui`: место и что не так, по-русски.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum UiError {
    /// Текст не складывается в блоки (скобка, кавычка, знак).
    #[error("{pos}: {message}")]
    Syntax { pos: Pos, message: String },
    /// Блоки сложились, но имя или значение непонятно.
    #[error("{pos}: {message}")]
    Meaning { pos: Pos, message: String },
    /// В файле нет ни одного экрана.
    #[error("в файле нет ни одного экрана: начните с «screen Имя {{ ... }}»")]
    Empty,
}

/// Что рисовать: прямоугольник со скруглением и обводкой или букву из атласа.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QuadKind {
    Block {
        /// Скругление углов в пикселях окна.
        radius: f32,
        /// Толщина обводки в пикселях окна (0 — без обводки).
        border: f32,
        /// Цвет обводки: линейный, прозрачность не умножена.
        border_color: [f32; 4],
    },
    Glyph {
        /// Прямоугольник буквы в пикселях атласа: левый, верхний, правый, нижний.
        uv: [f32; 4],
    },
}

/// Один прямоугольник списка рисования.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiQuad {
    /// В пикселях окна, (0, 0) — левый верхний угол: левый, верхний, правый, нижний.
    pub rect: [f32; 4],
    /// Цвет заливки или буквы: линейный, прозрачность не умножена.
    pub color: [f32; 4],
    pub kind: QuadKind,
}

/// Список рисования экрана: в порядке «снизу вверх».
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiDraw {
    pub quads: Vec<UiQuad>,
    /// Размер окна, под который разложен экран.
    pub viewport: [f32; 2],
}

/// Экран из файла `.rui`, готовый к рисованию.
pub struct UiScreen {
    screens: Vec<model::Screen>,
    current: usize,
    text: TextLibrary,
    /// Блоки текста элементов `Text` в порядке обхода дерева.
    text_ids: Vec<TextId>,
    draw: UiDraw,
    /// Разложен ли экран под `draw.viewport` после последней правки.
    fresh: bool,
}

impl UiScreen {
    /// Разбирает файл. Ошибка — с местом в файле; прежний экран при этом не трогается
    /// (его хранит тот, кто вызывает).
    pub fn parse(src: &str) -> Result<Self, UiError> {
        let doc = syntax::parse(src)?;
        if doc.screens.is_empty() {
            return Err(UiError::Empty);
        }
        let screens = doc
            .screens
            .iter()
            .map(model::screen)
            .collect::<Result<Vec<_>, _>>()?;
        let mut text = TextLibrary::new();
        let mut text_ids = Vec::new();
        for s in &screens {
            collect_text(&s.root, &mut text, &mut text_ids);
        }
        Ok(Self {
            screens,
            current: 0,
            text,
            text_ids,
            draw: UiDraw::default(),
            fresh: false,
        })
    }

    /// Имена экранов файла по порядку.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.screens.iter().map(|s| s.name.as_str())
    }

    /// Какой экран рисовать. `false`, если такого нет.
    pub fn show(&mut self, name: &str) -> bool {
        match self.screens.iter().position(|s| s.name == name) {
            Some(i) => {
                if i != self.current {
                    self.current = i;
                    self.fresh = false;
                }
                true
            }
            None => false,
        }
    }

    /// Атлас букв всех текстов файла: его загружает видеокарта. Меняется только при
    /// разборе файла.
    pub fn atlas(&self) -> &GlyphAtlas {
        self.text.atlas()
    }

    /// Список рисования под окно `viewport` (пиксели). Раскладка считается заново,
    /// только если размер окна поменялся.
    pub fn draw(&mut self, viewport: [f32; 2]) -> &UiDraw {
        if !self.fresh || self.draw.viewport != viewport {
            self.draw = self.lay_out(viewport);
            self.fresh = true;
        }
        &self.draw
    }

    /// То же, что [`Self::draw`], вместе с атласом букв: для того, кто отдаёт оба
    /// видеокарте сразу.
    pub fn draw_with_atlas(&mut self, viewport: [f32; 2]) -> (&UiDraw, &GlyphAtlas) {
        self.draw(viewport);
        (&self.draw, self.text.atlas())
    }

    fn lay_out(&self, viewport: [f32; 2]) -> UiDraw {
        let mut out = UiDraw {
            quads: Vec::new(),
            viewport,
        };
        if viewport[0] <= 0.0 || viewport[1] <= 0.0 {
            return out;
        }
        let scale = viewport[1] / DESIGN_HEIGHT;
        // Тексты экранов до текущего: номера их блоков идут раньше.
        let mut first_text = 0;
        for s in &self.screens[..self.current] {
            first_text += count_text(&s.root);
        }
        let root = &self.screens[self.current].root;
        let mut tree: TaffyTree<usize> = TaffyTree::new();
        let mut next_text = first_text;
        let Ok(node) = build(&mut tree, root, None, &mut next_text) else {
            return out;
        };
        let sizes: Vec<(f32, f32)> = self
            .text_ids
            .iter()
            .map(|id| self.text.get(*id).map_or((0.0, 0.0), |t| t.size()))
            .collect();
        let available = Size {
            width: AvailableSpace::Definite(viewport[0] / scale),
            height: AvailableSpace::Definite(DESIGN_HEIGHT),
        };
        let measured =
            tree.compute_layout_with_measure(node, available, |inputs, _, context, style| {
                taffy::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, _| {
                        let (w, h) = context.map_or((0.0, 0.0), |i| sizes[*i]);
                        Size {
                            width: known.width.unwrap_or(w),
                            height: known.height.unwrap_or(h),
                        }
                    },
                )
            });
        if measured.is_err() {
            return out;
        }
        let mut next_text = first_text;
        self.paint(
            &tree,
            node,
            root,
            [0.0, 0.0],
            1.0,
            scale,
            &mut next_text,
            &mut out,
        );
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &self,
        tree: &TaffyTree<usize>,
        node: NodeId,
        el: &Element,
        origin: [f32; 2],
        opacity: f32,
        scale: f32,
        next_text: &mut usize,
        out: &mut UiDraw,
    ) {
        let Ok(layout) = tree.layout(node) else {
            return;
        };
        let x = origin[0] + layout.location.x;
        let y = origin[1] + layout.location.y;
        let (w, h) = (layout.size.width, layout.size.height);
        let opacity = opacity * el.look.opacity;
        let rect = [x * scale, y * scale, (x + w) * scale, (y + h) * scale];
        let border = el.look.border.map_or((0.0, [0.0; 4]), |(c, width)| {
            (width * scale, faded(c, opacity))
        });
        if (el.look.background.is_some() || border.0 > 0.0) && w > 0.0 && h > 0.0 {
            out.quads.push(UiQuad {
                rect,
                color: el.look.background.map_or([0.0; 4], |c| faded(c, opacity)),
                kind: QuadKind::Block {
                    radius: el.look.radius * scale,
                    border: border.0,
                    border_color: border.1,
                },
            });
        }
        if let Kind::Text(spec) = &el.kind {
            let id = self.text_ids[*next_text];
            *next_text += 1;
            if let Some(laid) = self.text.get(id) {
                let color = faded(spec.color, opacity);
                let left = x + layout.padding.left;
                let top = y + layout.padding.top;
                for g in &laid.glyphs {
                    let [l, t, r, b] = g.rect;
                    // Буквы разложены с y вверх от верха блока: на экране y вниз.
                    out.quads.push(UiQuad {
                        rect: [
                            (left + l) * scale,
                            (top - t) * scale,
                            (left + r) * scale,
                            (top - b) * scale,
                        ],
                        color,
                        kind: QuadKind::Glyph { uv: g.uv },
                    });
                }
            }
        }
        let Ok(children) = tree.children(node) else {
            return;
        };
        for (child, el) in children.into_iter().zip(&el.children) {
            self.paint(tree, child, el, [x, y], opacity, scale, next_text, out);
        }
    }
}

fn faded(c: Color, opacity: f32) -> [f32; 4] {
    [c.r, c.g, c.b, c.a * opacity]
}

fn collect_text(el: &Element, text: &mut TextLibrary, ids: &mut Vec<TextId>) {
    if let Kind::Text(spec) = &el.kind {
        ids.push(text.add(TextBlock::new(spec.text.clone(), spec.size).color(spec.color)));
    }
    for c in &el.children {
        collect_text(c, text, ids);
    }
}

fn count_text(el: &Element) -> usize {
    usize::from(matches!(el.kind, Kind::Text(_)))
        + el.children.iter().map(count_text).sum::<usize>()
}

fn dimension(len: Len) -> Dimension {
    match len {
        Len::Auto => Dimension::auto(),
        Len::Px(px) => Dimension::length(px),
        Len::Percent(p) => Dimension::percent(p / 100.0),
    }
}

fn self_align(edge: Edge) -> AlignSelf {
    match edge {
        Edge::Start => AlignSelf::START,
        Edge::Center => AlignSelf::CENTER,
        Edge::End => AlignSelf::END,
        Edge::Stretch => AlignSelf::STRETCH,
    }
}

/// Какой родитель у элемента: от этого зависит, что значит `.align`.
#[derive(Clone, Copy)]
enum Parent {
    Column,
    Row,
    Stack,
}

fn build(
    tree: &mut TaffyTree<usize>,
    el: &Element,
    parent: Option<Parent>,
    next_text: &mut usize,
) -> taffy::TaffyResult<NodeId> {
    let p = &el.placement;
    let [pt, pr, pb, pl] = p.padding;
    let [mt, mr, mb, ml] = p.margin;
    let mut style = Style {
        size: Size {
            width: dimension(p.width),
            height: dimension(p.height),
        },
        padding: Rect {
            left: LengthPercentage::length(pl),
            right: LengthPercentage::length(pr),
            top: LengthPercentage::length(pt),
            bottom: LengthPercentage::length(pb),
        },
        margin: Rect {
            left: LengthPercentageAuto::length(ml),
            right: LengthPercentageAuto::length(mr),
            top: LengthPercentageAuto::length(mt),
            bottom: LengthPercentageAuto::length(mb),
        },
        flex_grow: p.grow,
        ..Style::default()
    };
    let (h, v) = p.align;
    match parent {
        Some(Parent::Column) => style.align_self = h.map(self_align),
        Some(Parent::Row) => style.align_self = v.map(self_align),
        Some(Parent::Stack) => {
            // Как ZStack у SwiftUI: без `.align` — в середину.
            style.justify_self = Some(self_align(h.unwrap_or(Edge::Center)));
            style.align_self = Some(self_align(v.unwrap_or(Edge::Center)));
            style.grid_row = Line {
                start: line(1),
                end: span(1),
            };
            style.grid_column = Line {
                start: line(1),
                end: span(1),
            };
        }
        None => {}
    }
    let me = match &el.kind {
        Kind::Column | Kind::Row => {
            style.display = Display::Flex;
            style.flex_direction = if matches!(el.kind, Kind::Column) {
                FlexDirection::Column
            } else {
                FlexDirection::Row
            };
            style.gap = Size {
                width: LengthPercentage::length(p.spacing),
                height: LengthPercentage::length(p.spacing),
            };
            style.align_items = match p.cross {
                Edge::Start => AlignItems::START,
                Edge::Center => AlignItems::CENTER,
                Edge::End => AlignItems::END,
                Edge::Stretch => AlignItems::STRETCH,
            };
            style.justify_content = match p.justify {
                Justify::Start => JustifyContent::START,
                Justify::Center => JustifyContent::CENTER,
                Justify::End => JustifyContent::END,
                Justify::Between => JustifyContent::SPACE_BETWEEN,
                Justify::Around => JustifyContent::SPACE_AROUND,
            };
            Some(if matches!(el.kind, Kind::Column) {
                Parent::Column
            } else {
                Parent::Row
            })
        }
        Kind::Stack => {
            style.display = Display::Grid;
            style.grid_template_columns = vec![fr(1.0)];
            style.grid_template_rows = vec![fr(1.0)];
            Some(Parent::Stack)
        }
        Kind::Text(_) => {
            let index = *next_text;
            *next_text += 1;
            return tree.new_leaf_with_context(style, index);
        }
        Kind::Spacer => None,
    };
    let mut children = Vec::with_capacity(el.children.len());
    for c in &el.children {
        children.push(build(tree, c, me, next_text)?);
    }
    tree.new_with_children(style, &children)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(draw: &UiDraw) -> Vec<&UiQuad> {
        draw.quads
            .iter()
            .filter(|q| matches!(q.kind, QuadKind::Block { .. }))
            .collect()
    }

    #[test]
    fn centered_panel_scales_with_window() {
        let mut ui =
            UiScreen::parse("screen A { Rect().size(400, 200).background(#FFFFFF).radius(10) }")
                .unwrap();
        let full = *blocks(ui.draw([1920.0, 1080.0]))[0];
        assert_eq!(full.rect, [760.0, 440.0, 1160.0, 640.0]);
        let QuadKind::Block { radius, .. } = full.kind else {
            unreachable!()
        };
        assert_eq!(radius, 10.0);
        // Вдвое меньшее окно — вдвое меньше и панель, всё так же в середине.
        let half = *blocks(ui.draw([960.0, 540.0]))[0];
        assert_eq!(half.rect, [380.0, 220.0, 580.0, 320.0]);
    }

    #[test]
    fn column_stacks_children_with_spacing_and_padding() {
        let mut ui = UiScreen::parse(
            r##"screen A {
                Column(spacing: 10) {
                    Rect().size(100, 20).background(#FF0000)
                    Rect().size(50, 30).background(#00FF00)
                }
                .padding(5)
                .background(#000000)
                .align(top_left)
            }"##,
        )
        .unwrap();
        let draw = ui.draw([1920.0, 1080.0]).clone();
        let b = blocks(&draw);
        assert_eq!(b.len(), 3);
        // Фон столбца: 5 + 100 + 5 по ширине, 5 + 20 + 10 + 30 + 5 по высоте.
        assert_eq!(b[0].rect, [0.0, 0.0, 110.0, 70.0]);
        assert_eq!(b[1].rect, [5.0, 5.0, 105.0, 25.0]);
        assert_eq!(b[2].rect, [5.0, 35.0, 55.0, 65.0]);
    }

    #[test]
    fn row_with_spacer_pushes_to_the_edges() {
        let mut ui = UiScreen::parse(
            r##"screen A {
                Row {
                    Rect().size(10, 10).background(#FF0000)
                    Spacer()
                    Rect().size(10, 10).background(#00FF00)
                }
                .width(fill)
                .align(top)
            }"##,
        )
        .unwrap();
        let draw = ui.draw([1920.0, 1080.0]).clone();
        let b = blocks(&draw);
        assert_eq!(b[0].rect[0], 0.0);
        assert_eq!(b[1].rect[2], 1920.0);
    }

    #[test]
    fn anchored_hud_bar_sits_in_the_corner() {
        let mut ui = UiScreen::parse(
            "layer Hud { Rect().size(260, 10).background(#7FD36B).anchor(bottom_left, margin: 24) }",
        )
        .unwrap();
        let draw = ui.draw([1920.0, 1080.0]).clone();
        assert_eq!(blocks(&draw)[0].rect, [24.0, 1046.0, 284.0, 1056.0]);
    }

    #[test]
    fn text_is_measured_and_its_letters_sit_inside() {
        let mut ui = UiScreen::parse(
            r##"screen A {
                Column {
                    Text("Играть").font(40).color(#F5A54A)
                }
                .padding(20)
                .background(#000000)
                .align(top_left)
                .opacity(0.5)
            }"##,
        )
        .unwrap();
        let draw = ui.draw([1920.0, 1080.0]).clone();
        let panel = blocks(&draw)[0].rect;
        let glyphs: Vec<_> = draw
            .quads
            .iter()
            .filter(|q| matches!(q.kind, QuadKind::Glyph { .. }))
            .collect();
        assert_eq!(glyphs.len(), 6);
        // Панель обнимает текст: ширина текста плюс два отступа, высота строки 40 × 1.25.
        assert!(panel[2] > 100.0 && panel[2] < 300.0, "{panel:?}");
        assert!((panel[3] - (40.0 + 50.0)).abs() < 1.0, "{panel:?}");
        for g in &glyphs {
            // С запасом поля вокруг буквы (несколько пикселей).
            assert!(g.rect[0] > 15.0 && g.rect[2] < panel[2], "{:?}", g.rect);
            assert!(g.rect[1] > 10.0 && g.rect[3] < panel[3], "{:?}", g.rect);
            assert!((g.color[3] - 0.5).abs() < 1e-6, "прозрачность родителя");
        }
        assert!(!ui.atlas().is_empty());
    }

    #[test]
    fn layout_is_cached_until_the_window_changes() {
        let mut ui =
            UiScreen::parse("screen A { Rect().size(10, 10).background(#FFFFFF) }").unwrap();
        let a = ui.draw([800.0, 600.0]) as *const UiDraw;
        let b = ui.draw([800.0, 600.0]) as *const UiDraw;
        assert_eq!(a, b);
        assert!(ui.fresh);
    }

    #[test]
    fn second_screen_keeps_its_own_texts() {
        let mut ui = UiScreen::parse(
            r#"screen A { Text("один") }
               screen B { Text("два слова") }"#,
        )
        .unwrap();
        assert_eq!(ui.names().collect::<Vec<_>>(), ["A", "B"]);
        let a = ui.draw([1920.0, 1080.0]).quads.len();
        assert!(ui.show("B"));
        let b = ui.draw([1920.0, 1080.0]).quads.len();
        assert_eq!((a, b), (4, 8));
        assert!(!ui.show("C"));
    }

    #[test]
    fn empty_file_is_an_error() {
        assert_eq!(UiScreen::parse("// пусто").err(), Some(UiError::Empty));
    }
}
