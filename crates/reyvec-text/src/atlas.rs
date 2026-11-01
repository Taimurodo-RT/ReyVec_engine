//! Атлас букв: для каждой буквы, которая встретилась в тексте, один раз считается поле
//! расстояний MSDF (multi-channel signed distance field, Chlumský 2015) и кладётся в общую
//! картинку. Шейдер по этой картинке рисует букву чёткой при любом размере и повороте:
//! в каждом пикселе атласа хранится не цвет, а расстояние до края буквы (в трёх каналах
//! по-разному, чтобы острые углы оставались острыми, и в четвёртом обычное расстояние).

use std::collections::HashMap;

use fdsm::bezier::scanline::FillRule;
use fdsm::bezier::{Point, Segment};
use fdsm::correct_error::{ErrorCorrectionConfig, correct_error_mtsdf};
use fdsm::generate::generate_mtsdf;
use fdsm::render::correct_sign_mtsdf;
use fdsm::shape::{Contour, Shape};
use image::Rgba32FImage;
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::{LocationRef, Size};
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// Пикселей атласа на высоту буквы «em» (размер шрифта). Больше — точнее мелкие изгибы,
/// но больше памяти и времени на подготовку.
pub const EM_PIXELS: f32 = 40.0;
/// Сколько пикселей атласа охватывают расстояния поля (по `RANGE / 2` внутрь и наружу от
/// края). Шейдеру это число нужно, чтобы перевести расстояние в пиксели экрана.
pub const RANGE: f32 = 4.0;
/// Запас вокруг буквы в атласе: половина охвата поля и пиксель, чтобы соседние буквы не
/// просвечивали при сглаженном чтении.
const PAD: f32 = RANGE / 2.0 + 1.0;
/// Ширина атласа. Высота растёт вдвое, пока буквы не поместятся.
pub const ATLAS_WIDTH: u32 = 1024;
const START_HEIGHT: u32 = 256;
/// Больше атлас не растёт: 1024×4096 вмещает несколько тысяч разных букв.
pub const MAX_HEIGHT: u32 = 4096;

/// Буква в атласе.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtlasGlyph {
    /// Прямоугольник в пикселях атласа: левый, верхний, правый, нижний.
    pub uv: [f32; 4],
    /// Куда этот прямоугольник ложится относительно начала буквы на строке, в долях
    /// размера шрифта, y вверх: левый, верхний, правый, нижний.
    pub plane: [f32; 4],
}

/// Ключ буквы: шрифт (номер данных файла и номер в коллекции) и номер буквы в шрифте.
type Key = (u64, u32, u32);

/// Атлас букв в памяти процессора. На видеокарту уходит целиком, когда меняется.
pub struct GlyphAtlas {
    width: u32,
    height: u32,
    /// RGBA8: три канала MSDF и обычное расстояние, 0.5 — край буквы, больше — внутри.
    pixels: Vec<u8>,
    glyphs: HashMap<Key, Option<AtlasGlyph>>,
    /// Полки упаковки: верх текущей полки, её высота и занятая ширина.
    shelf_y: u32,
    shelf_height: u32,
    shelf_x: u32,
    /// Буквы, которые не поместились (атлас уже наибольший).
    overflow: usize,
}

impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new()
    }
}

impl GlyphAtlas {
    pub fn new() -> Self {
        Self {
            width: ATLAS_WIDTH,
            height: START_HEIGHT,
            pixels: vec![0; (ATLAS_WIDTH * START_HEIGHT * 4) as usize],
            glyphs: HashMap::new(),
            shelf_y: 0,
            shelf_height: 0,
            shelf_x: 0,
            overflow: 0,
        }
    }

    /// Ширина и высота в пикселях.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Пиксели RGBA8 построчно сверху вниз.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Сколько разных букв подготовлено (с пустыми, например пробелом).
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Сколько букв не поместилось в наибольший атлас (они не рисуются).
    pub fn overflow(&self) -> usize {
        self.overflow
    }

    /// Буква `glyph` шрифта `font` (данные файла, номер в коллекции, номер данных для
    /// ключа). Готовит её при первой встрече. `None` у букв без контура (пробел) и у
    /// непоместившихся.
    pub fn glyph(
        &mut self,
        font: &[u8],
        index: u32,
        font_key: u64,
        glyph: u32,
    ) -> Option<AtlasGlyph> {
        let key = (font_key, index, glyph);
        if let Some(g) = self.glyphs.get(&key) {
            return *g;
        }
        let made = self.make(font, index, glyph);
        self.glyphs.insert(key, made);
        made
    }

    fn make(&mut self, font: &[u8], index: u32, glyph: u32) -> Option<AtlasGlyph> {
        let font = FontRef::from_index(font, index).ok()?;
        let upem = f32::from(
            font.metrics(Size::unscaled(), LocationRef::default())
                .units_per_em,
        );
        let outline = font.outline_glyphs().get(GlyphId::new(glyph))?;
        let mut pen = Collect::default();
        outline
            .draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                &mut pen,
            )
            .ok()?;
        pen.finish();
        let [x0, y0, x1, y1] = pen.bounds()?;
        let scale = EM_PIXELS / upem;
        let w = ((x1 - x0) * scale + 2.0 * PAD).ceil() as u32;
        let h = ((y1 - y0) * scale + 2.0 * PAD).ceil() as u32;
        let (ax, ay) = self.place(w, h)?;

        // Контур в пикселях картинки буквы: y вниз, левый верхний угол в нуле.
        let to_px = |[x, y]: [f32; 2]| {
            Point::new(
                f64::from((x - x0) * scale + PAD),
                f64::from((y1 - y) * scale + PAD),
            )
        };
        let mut shape: Shape<Contour> = Shape::default();
        for contour in &pen.contours {
            let segments: Vec<Segment> = contour
                .iter()
                .map(|s| match *s {
                    Seg::Line(a, b) => Segment::line(to_px(a), to_px(b)),
                    Seg::Quad(a, b, c) => Segment::quad(to_px(a), to_px(b), to_px(c)),
                    Seg::Cubic(a, b, c, d) => {
                        Segment::cubic(to_px(a), to_px(b), to_px(c), to_px(d))
                    }
                })
                .collect();
            if !segments.is_empty() {
                shape.contours.push(Contour { segments });
            }
        }
        // Углы острее ~170° делят край на разные цвета (как msdfgen по умолчанию).
        let colored = Shape::edge_coloring_simple(shape, 3.0f64.sin(), 0x5265_7956);
        let prepared = colored.prepare();
        let mut field = Rgba32FImage::new(w, h);
        generate_mtsdf(&prepared, f64::from(RANGE), &mut field);
        correct_error_mtsdf(
            &mut field,
            &colored,
            &prepared,
            f64::from(RANGE),
            &ErrorCorrectionConfig::default(),
        );
        correct_sign_mtsdf(&mut field, &prepared, FillRule::Nonzero);

        for (x, y, p) in field.enumerate_pixels() {
            let at = (((ay + y) * self.width + ax + x) * 4) as usize;
            for c in 0..4 {
                self.pixels[at + c] = (p.0[c].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
        let em = |v: f32| v / upem;
        let left = em(x0) - PAD / EM_PIXELS;
        let top = em(y1) + PAD / EM_PIXELS;
        Some(AtlasGlyph {
            uv: [ax as f32, ay as f32, (ax + w) as f32, (ay + h) as f32],
            plane: [
                left,
                top,
                left + w as f32 / EM_PIXELS,
                top - h as f32 / EM_PIXELS,
            ],
        })
    }

    /// Место под картинку `w`×`h`: полками слева направо, сверху вниз. Если места нет,
    /// атлас растёт вдвое по высоте (старые буквы остаются на своих местах).
    fn place(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.width {
            self.overflow += 1;
            return None;
        }
        if self.shelf_x + w > self.width {
            self.shelf_y += self.shelf_height;
            self.shelf_x = 0;
            self.shelf_height = 0;
        }
        while self.shelf_y + h > self.height {
            if self.height >= MAX_HEIGHT {
                self.overflow += 1;
                return None;
            }
            self.height *= 2;
            self.pixels
                .resize((self.width * self.height * 4) as usize, 0);
        }
        let at = (self.shelf_x, self.shelf_y);
        self.shelf_x += w;
        self.shelf_height = self.shelf_height.max(h);
        Some(at)
    }
}

/// Отрезок контура буквы в единицах шрифта (y вверх).
#[derive(Clone, Copy, Debug)]
enum Seg {
    Line([f32; 2], [f32; 2]),
    Quad([f32; 2], [f32; 2], [f32; 2]),
    Cubic([f32; 2], [f32; 2], [f32; 2], [f32; 2]),
}

/// Собирает контуры буквы из skrifa. Замыкает каждый контур прямой, если он не замкнут, и
/// пропускает отрезки нулевой длины (на них поле расстояний не считается).
#[derive(Default)]
struct Collect {
    contours: Vec<Vec<Seg>>,
    start: [f32; 2],
    last: [f32; 2],
    open: bool,
}

impl Collect {
    fn close_current(&mut self) {
        if self.open && self.last != self.start {
            let (a, b) = (self.last, self.start);
            if let Some(c) = self.contours.last_mut() {
                c.push(Seg::Line(a, b));
            }
        }
        self.last = self.start;
        self.open = false;
    }

    fn finish(&mut self) {
        self.close_current();
        self.contours.retain(|c| !c.is_empty());
    }

    fn push(&mut self, seg: Seg, end: [f32; 2]) {
        if !self.open {
            self.contours.push(Vec::new());
            self.open = true;
        }
        if let Some(c) = self.contours.last_mut() {
            c.push(seg);
        }
        self.last = end;
    }

    fn bounds(&self) -> Option<[f32; 4]> {
        let mut b = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        let mut add = |p: [f32; 2]| {
            b = [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ];
        };
        for s in self.contours.iter().flatten() {
            match *s {
                Seg::Line(a, c) => [a, c].into_iter().for_each(&mut add),
                Seg::Quad(a, c, d) => [a, c, d].into_iter().for_each(&mut add),
                Seg::Cubic(a, c, d, e) => [a, c, d, e].into_iter().for_each(&mut add),
            }
        }
        (b[0] < b[2] && b[1] < b[3]).then_some(b)
    }
}

impl OutlinePen for Collect {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close_current();
        self.start = [x, y];
        self.last = [x, y];
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = [x, y];
        if p != self.last {
            self.push(Seg::Line(self.last, p), p);
        }
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let p = [x, y];
        if p == self.last && [cx, cy] == p {
            return;
        }
        self.push(Seg::Quad(self.last, [cx, cy], p), p);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let p = [x, y];
        if p == self.last && [cx0, cy0] == p && [cx1, cy1] == p {
            return;
        }
        self.push(Seg::Cubic(self.last, [cx0, cy0], [cx1, cy1], p), p);
    }

    fn close(&mut self) {
        self.close_current();
    }
}
