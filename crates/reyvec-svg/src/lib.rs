//! Загрузка SVG в векторный слой ReyVec.
//!
//! SVG разбирает и упрощает `usvg`: стили, `use`, единицы, `viewBox`, проценты и
//! `objectBoundingBox` он уже раскрывает, остаётся дерево групп и контуров. Здесь это
//! дерево один раз превращается в фигуры [`ShapeLibrary`] и список кадра
//! [`SceneItem`]: контур с заливкой и обводкой даёт одну или две фигуры, группа с
//! прозрачностью, режимом смешивания, обрезкой или маской становится слоем
//! (`BeginLayer`… `EndLayer`), обрезка и маска становятся масками слоя.
//!
//! Каждый кадр игре остаётся только поставить рисунок на место:
//! [`SvgDrawing::placed`] умножает преобразования экземпляров, кривые не трогаются.
//!
//! Чего пока нет (пишется в [`SvgDrawing::warnings`]): текст (будет в 3.6), картинки,
//! узоры (`pattern`), фильтры, режимы смешивания кроме обычного, умножения, экрана и
//! сложения, фокусный радиус радиального градиента (`fr`).

use std::collections::BTreeSet;

use kurbo::{Affine, BezPath, PathEl, Point};
use reyvec_vector::{
    BlendMode, Color, FillRule, Gradient, LineCap, LineJoin, MaskMode, Paint, SceneItem, Shape,
    ShapeLibrary, Spread, Stroke,
};

/// Ошибка загрузки SVG.
#[derive(Debug, thiserror::Error)]
pub enum SvgError {
    #[error("SVG не разобрать: {0}")]
    Parse(#[from] usvg::Error),
}

/// Как ставить рисунок.
#[derive(Clone, Copy, Debug)]
pub struct SvgOptions {
    /// Высота рисунка в единицах сцены (высота кадра = 2). По ней выбирается точность
    /// разбиения кривых: показывать рисунок намного крупнее значит видеть грани
    /// (`docs/vector-bottlenecks.md`, пункт 1).
    pub height: f64,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self { height: 1.0 }
    }
}

/// Загруженный рисунок: его фигуры уже в наборе, здесь список кадра.
#[derive(Clone, Debug)]
pub struct SvgDrawing {
    /// Элементы кадра в координатах рисунка: центр рисунка в нуле, y вверх, высота
    /// [`SvgOptions::height`].
    pub items: Vec<SceneItem>,
    /// Ширина и высота рисунка в тех же координатах.
    pub size: (f64, f64),
    /// Чего движок в этом рисунке не показал (по одной строке на вид).
    pub warnings: Vec<String>,
    /// Сколько фигур добавлено в набор.
    pub shape_count: usize,
}

impl SvgDrawing {
    /// Элементы кадра для рисунка, поставленного преобразованием `place` (из координат
    /// рисунка в координаты сцены).
    pub fn placed(&self, place: Affine) -> impl Iterator<Item = SceneItem> + '_ {
        self.items.iter().map(move |item| item.placed(place))
    }
}

/// Разбирает SVG `data`, добавляет его фигуры в `library` и возвращает список кадра.
pub fn load_svg(
    data: &[u8],
    library: &mut ShapeLibrary,
    options: SvgOptions,
) -> Result<SvgDrawing, SvgError> {
    let tree = usvg::Tree::from_data(data, &usvg::Options::default())?;
    let size = tree.size();
    let (w, h) = (f64::from(size.width()), f64::from(size.height()));
    let s = options.height / h;
    // Из координат SVG (y вниз, начало в левом верхнем углу) в координаты рисунка.
    let drawing = Affine::new([s, 0.0, 0.0, -s, -s * w / 2.0, s * h / 2.0]);
    let mut builder = Builder {
        library,
        items: Vec::new(),
        warnings: BTreeSet::new(),
        shape_count: 0,
    };
    builder.nodes(tree.root(), drawing);
    Ok(SvgDrawing {
        items: builder.items,
        size: (w * s, options.height),
        warnings: builder.warnings.into_iter().collect(),
        shape_count: builder.shape_count,
    })
}

struct Builder<'a> {
    library: &'a mut ShapeLibrary,
    items: Vec<SceneItem>,
    warnings: BTreeSet<String>,
    shape_count: usize,
}

impl Builder<'_> {
    fn warn(&mut self, what: impl Into<String>) {
        self.warnings.insert(what.into());
    }

    fn nodes(&mut self, parent: &usvg::Group, t: Affine) {
        for node in parent.children() {
            match node {
                usvg::Node::Group(g) => self.group(g, t),
                usvg::Node::Path(p) => self.path(p, t),
                usvg::Node::Image(_) => self.warn("картинки внутри SVG не показываются"),
                usvg::Node::Text(_) => self.warn("текст в SVG пока не показывается (этап 3.6)"),
            }
        }
    }

    /// Группа как в `resvg`: если её нужно рисовать отдельно (прозрачность, режим,
    /// обрезка, маска), она становится слоем. У слоя движка одна маска, поэтому маска,
    /// обрезка и их вложенные маски получают по своему слою внутри.
    fn group(&mut self, g: &usvg::Group, parent: Affine) {
        let t = parent * affine(g.transform());
        if !g.should_isolate() {
            self.nodes(g, t);
            return;
        }
        if !g.filters().is_empty() {
            self.warn("фильтры SVG (размытие, тени) пока не поддерживаются");
        }
        let blend = match g.blend_mode() {
            usvg::BlendMode::Normal => BlendMode::Normal,
            usvg::BlendMode::Multiply => BlendMode::Multiply,
            usvg::BlendMode::Screen => BlendMode::Screen,
            usvg::BlendMode::PlusLighter => BlendMode::Add,
            other => {
                self.warn(format!(
                    "режим смешивания «{other}» пока не поддерживается, вместо него обычный"
                ));
                BlendMode::Normal
            }
        };
        let start = self.items.len();
        let opacity = g.opacity().get();
        self.items.push(SceneItem::BeginLayer { opacity, blend });
        let mut open = 1;
        // Маска и её маски.
        let mut mask = g.mask();
        let mut first = true;
        while let Some(m) = mask {
            if !first {
                self.items.push(SceneItem::layer(1.0));
                open += 1;
            }
            first = false;
            self.mask(m, t);
            mask = m.mask();
        }
        // Обрезка и её обрезки.
        let mut clip = g.clip_path();
        while let Some(c) = clip {
            if !first {
                self.items.push(SceneItem::layer(1.0));
                open += 1;
            }
            first = false;
            self.items.push(SceneItem::clip());
            self.clip_nodes(c.root(), t * affine(c.transform()));
            self.items.push(SceneItem::EndMask);
            clip = c.clip_path();
        }
        self.nodes(g, t);
        for _ in 0..open {
            self.items.push(SceneItem::EndLayer);
        }
        // Прозрачная группа из одной фигуры: слой не нужен, хватит прозрачности в оттенке
        // фигуры (картинка та же, а проход «слой» на встроенной видеокарте стоит около 0,7 мс).
        if let [
            SceneItem::BeginLayer {
                blend: BlendMode::Normal,
                ..
            },
            SceneItem::Shape(instance),
            SceneItem::EndLayer,
        ] = &self.items[start..]
        {
            let mut instance = *instance;
            instance.tint.a *= opacity;
            self.items.truncate(start);
            self.items.push(SceneItem::Shape(instance));
        }
    }

    /// Маска SVG: её содержимое, обрезанное прямоугольником маски (`mask.rect`).
    fn mask(&mut self, m: &usvg::Mask, t: Affine) {
        let mode = match m.kind() {
            usvg::MaskType::Luminance => MaskMode::Luminance,
            usvg::MaskType::Alpha => MaskMode::Alpha,
        };
        self.items.push(SceneItem::mask(mode, false));
        self.items.push(SceneItem::layer(1.0));
        self.items.push(SceneItem::clip());
        let r = m.rect();
        let rect = kurbo::Rect::new(
            f64::from(r.left()),
            f64::from(r.top()),
            f64::from(r.right()),
            f64::from(r.bottom()),
        );
        self.fill_shape(
            kurbo::Shape::to_path(&rect, 1e-6),
            FillRule::NonZero,
            Color::WHITE.into(),
            t,
        );
        self.items.push(SceneItem::EndMask);
        self.nodes(m.root(), t);
        self.items.push(SceneItem::EndLayer);
        self.items.push(SceneItem::EndMask);
    }

    /// Содержимое обрезки: только силуэты контуров (белая заливка по их правилу), как
    /// в `resvg`. Обводки и цвета в обрезке не участвуют.
    fn clip_nodes(&mut self, parent: &usvg::Group, t: Affine) {
        for node in parent.children() {
            match node {
                usvg::Node::Path(p) => {
                    let Some(fill) = p.fill() else { continue };
                    if !p.is_visible() {
                        continue;
                    }
                    self.fill_shape(path(p), rule(fill.rule()), Color::WHITE.into(), t);
                }
                usvg::Node::Group(g) => {
                    let t = t * affine(g.transform());
                    if let Some(c) = g.clip_path() {
                        // Часть обрезки со своей обрезкой: отдельный слой.
                        self.items.push(SceneItem::layer(1.0));
                        self.items.push(SceneItem::clip());
                        self.clip_nodes(c.root(), t * affine(c.transform()));
                        self.items.push(SceneItem::EndMask);
                        self.clip_nodes(g, t);
                        self.items.push(SceneItem::EndLayer);
                    } else {
                        self.clip_nodes(g, t);
                    }
                }
                usvg::Node::Text(_) => self.warn("текст в SVG пока не показывается (этап 3.6)"),
                usvg::Node::Image(_) => {}
            }
        }
    }

    fn path(&mut self, p: &usvg::Path, t: Affine) {
        if !p.is_visible() {
            return;
        }
        let fill_first = matches!(p.paint_order(), usvg::PaintOrder::FillAndStroke);
        if fill_first {
            self.path_fill(p, t);
            self.path_stroke(p, t);
        } else {
            self.path_stroke(p, t);
            self.path_fill(p, t);
        }
    }

    fn path_fill(&mut self, p: &usvg::Path, t: Affine) {
        let Some(fill) = p.fill() else { return };
        let geometry = path(p);
        // У линий и ломаных SVG заливка по умолчанию есть, но площади нет.
        if flat(&geometry) {
            return;
        }
        let Some(paint) = self.paint(fill.paint(), fill.opacity().get()) else {
            return;
        };
        self.fill_shape(geometry, rule(fill.rule()), paint, t);
    }

    fn path_stroke(&mut self, p: &usvg::Path, t: Affine) {
        let Some(stroke) = p.stroke() else { return };
        let Some(paint) = self.paint(stroke.paint(), stroke.opacity().get()) else {
            return;
        };
        let mut geometry = path(p);
        if let Some(dashes) = stroke.dasharray() {
            let dashes: Vec<f64> = dashes.iter().map(|&d| f64::from(d)).collect();
            geometry = BezPath::from_iter(kurbo::dash(
                geometry.into_iter(),
                f64::from(stroke.dashoffset()),
                &dashes,
            ));
        }
        let style = Stroke {
            width: f64::from(stroke.width().get()),
            join: match stroke.linejoin() {
                usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => LineJoin::Miter,
                usvg::LineJoin::Round => LineJoin::Round,
                usvg::LineJoin::Bevel => LineJoin::Bevel,
            },
            cap: match stroke.linecap() {
                usvg::LineCap::Butt => LineCap::Butt,
                usvg::LineCap::Round => LineCap::Round,
                usvg::LineCap::Square => LineCap::Square,
            },
            miter_limit: f64::from(stroke.miterlimit().get()),
        };
        let Some(k) = unit_scale(t) else { return };
        // Обводка режется в координатах контура (так неравномерное растяжение группы
        // меняет толщину линии, как в SVG), но в масштабе сцены, чтобы точность
        // разбиения была под тот размер, в котором рисунок показывают.
        let stroke = Stroke {
            width: style.width * k,
            ..style
        };
        let shape = Shape::stroke(Affine::scale(k) * geometry, stroke, scale_paint(paint, k));
        self.add(shape, t * Affine::scale(1.0 / k));
    }

    /// Заливка контура `geometry` (в координатах контура) с преобразованием `t`.
    fn fill_shape(&mut self, geometry: BezPath, rule: FillRule, paint: Paint, t: Affine) {
        let Some(k) = unit_scale(t) else { return };
        let shape = Shape::fill_with(Affine::scale(k) * geometry, rule, scale_paint(paint, k));
        self.add(shape, t * Affine::scale(1.0 / k));
    }

    fn add(&mut self, shape: Shape, transform: Affine) {
        match self.library.add(shape) {
            Ok(id) => {
                self.shape_count += 1;
                self.items
                    .push(SceneItem::from(reyvec_vector::Instance::new(id, transform)));
            }
            Err(e) => self.warn(format!("контур пропущен: {e}")),
        }
    }

    /// Закраска движка или `None`, если рисовать нечем (узор, прозрачно).
    fn paint(&mut self, paint: &usvg::Paint, opacity: f32) -> Option<Paint> {
        let stops = |base: &usvg::BaseGradient| -> Gradient {
            let mut g = Gradient::linear((0.0, 0.0), (1.0, 0.0));
            for s in base.stops() {
                g = g.stop(
                    s.offset().get(),
                    color(s.color(), s.opacity().get() * opacity),
                );
            }
            g.with_spread(match base.spread_method() {
                usvg::SpreadMethod::Pad => Spread::Pad,
                usvg::SpreadMethod::Reflect => Spread::Reflect,
                usvg::SpreadMethod::Repeat => Spread::Repeat,
            })
            .with_transform(affine(base.transform()))
        };
        match paint {
            usvg::Paint::Color(c) => Some(color(*c, opacity).into()),
            usvg::Paint::LinearGradient(l) => {
                let mut g = stops(l);
                g.kind = reyvec_vector::GradientKind::Linear {
                    start: Point::new(f64::from(l.x1()), f64::from(l.y1())),
                    end: Point::new(f64::from(l.x2()), f64::from(l.y2())),
                };
                Some(g.into())
            }
            usvg::Paint::RadialGradient(r) => {
                if r.fr().get() > 0.0 {
                    self.warn("фокусный радиус радиального градиента (fr) не поддерживается");
                }
                let mut g = stops(r);
                g.kind = reyvec_vector::GradientKind::Radial {
                    center: Point::new(f64::from(r.cx()), f64::from(r.cy())),
                    radius: f64::from(r.r().get()),
                    focus: Point::new(f64::from(r.fx()), f64::from(r.fy())),
                };
                Some(g.into())
            }
            usvg::Paint::Pattern(_) => {
                self.warn("узоры SVG (pattern) пока не поддерживаются");
                None
            }
        }
    }
}

/// Насколько единица контура велика в сцене (корень из площади преобразования).
/// `None`, если преобразование сплющивает контур в линию или точку.
fn unit_scale(t: Affine) -> Option<f64> {
    let k = t.determinant().abs().sqrt();
    (k.is_finite() && k > 1e-12).then_some(k)
}

/// Градиент контура, увеличенного в `k` раз: градиент задан в координатах контура.
fn scale_paint(paint: Paint, k: f64) -> Paint {
    match paint {
        Paint::Gradient(g) => Paint::Gradient(Gradient {
            transform: Affine::scale(k) * g.transform,
            ..g
        }),
        solid => solid,
    }
}

fn affine(t: usvg::Transform) -> Affine {
    Affine::new([t.sx, t.ky, t.kx, t.sy, t.tx, t.ty].map(f64::from))
}

fn color(c: usvg::Color, alpha: f32) -> Color {
    Color::from_srgb8(c.red, c.green, c.blue).with_alpha(alpha)
}

fn rule(r: usvg::FillRule) -> FillRule {
    match r {
        usvg::FillRule::NonZero => FillRule::NonZero,
        usvg::FillRule::EvenOdd => FillRule::EvenOdd,
    }
}

/// Все части контура лежат на прямых (вместе с управляющими точками кривых): у такого
/// контура нет площади, заливать нечего.
fn flat(path: &BezPath) -> bool {
    let mut points: Vec<kurbo::Point> = Vec::new();
    let mut flat_so_far = true;
    let check = |points: &mut Vec<kurbo::Point>| {
        let Some(&a) = points.first() else {
            return true;
        };
        let scale = points
            .iter()
            .map(|p| (*p - a).hypot())
            .fold(0.0_f64, f64::max);
        let Some(&b) = points.iter().find(|p| (**p - a).hypot() > scale * 1e-9) else {
            return true;
        };
        let d = b - a;
        let ok = points
            .iter()
            .all(|p| d.cross(*p - a).abs() <= 1e-9 * scale * scale);
        points.clear();
        ok
    };
    for el in path.elements() {
        match *el {
            kurbo::PathEl::MoveTo(p) => {
                flat_so_far &= check(&mut points);
                points.push(p);
            }
            kurbo::PathEl::LineTo(p) => points.push(p),
            kurbo::PathEl::QuadTo(p1, p2) => points.extend([p1, p2]),
            kurbo::PathEl::CurveTo(p1, p2, p3) => points.extend([p1, p2, p3]),
            kurbo::PathEl::ClosePath => {}
        }
    }
    flat_so_far && check(&mut points)
}

/// Контур `usvg` как контур kurbo.
fn path(p: &usvg::Path) -> BezPath {
    let pt = |p: usvg::tiny_skia_path::Point| Point::new(f64::from(p.x), f64::from(p.y));
    let mut out = BezPath::new();
    for seg in p.data().segments() {
        use usvg::tiny_skia_path::PathSegment as S;
        out.push(match seg {
            S::MoveTo(a) => PathEl::MoveTo(pt(a)),
            S::LineTo(a) => PathEl::LineTo(pt(a)),
            S::QuadTo(a, b) => PathEl::QuadTo(pt(a), pt(b)),
            S::CubicTo(a, b, c) => PathEl::CurveTo(pt(a), pt(b), pt(c)),
            S::Close => PathEl::ClosePath,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(svg: &str) -> (SvgDrawing, ShapeLibrary) {
        let mut library = ShapeLibrary::new(0.0005);
        let d = load_svg(svg.as_bytes(), &mut library, SvgOptions::default()).unwrap();
        (d, library)
    }

    fn kinds(items: &[SceneItem]) -> String {
        items
            .iter()
            .map(|i| match i {
                SceneItem::Shape(_) => "S",
                SceneItem::Text(_) => "T",
                SceneItem::BeginLayer { .. } => "L(",
                SceneItem::EndLayer => ")",
                SceneItem::BeginMask { .. } => "M[",
                SceneItem::EndMask => "]",
            })
            .collect()
    }

    #[test]
    fn fill_and_stroke_become_shapes_placed_y_up() {
        let (d, lib) = load(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
                 <rect x="0" y="0" width="100" height="100" fill="red" stroke="blue" stroke-width="4"/>
               </svg>"#,
        );
        assert_eq!(kinds(&d.items), "SS");
        assert_eq!(d.size, (2.0, 1.0));
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        assert_eq!((d.shape_count, lib.len()), (2, 2));
        // Левый верхний угол прямоугольника (0, 0) в SVG → (−1, 0.5) в рисунке.
        let SceneItem::Shape(fill) = d.items[0] else {
            unreachable!()
        };
        let shape = lib.shape(fill.shape).unwrap();
        let corner = shape
            .path
            .elements()
            .iter()
            .find_map(|e| match e {
                PathEl::MoveTo(p) => Some(*p),
                _ => None,
            })
            .unwrap();
        let p = fill.transform * corner;
        assert!(
            (p.x + 1.0).abs() < 1e-9 && (p.y - 0.5).abs() < 1e-9,
            "{p:?}"
        );
        // Фигура в масштабе сцены: высота рисунка 1, поэтому квадрат 100 единиц SVG
        // (вся высота) стал 1.
        let b = kurbo::Shape::bounding_box(&shape.path);
        assert!((b.width() - 1.0).abs() < 1e-9, "{b:?}");
        // Обводка 4 единицы SVG = 0.04 сцены.
        let SceneItem::Shape(stroke) = d.items[1] else {
            unreachable!()
        };
        match &lib.shape(stroke.shape).unwrap().style {
            reyvec_vector::Style::Stroke(s) => assert!((s.width - 0.04).abs() < 1e-9),
            other => panic!("{other:?}"),
        }
        // Сдвиг рисунка двигает все фигуры.
        let moved: Vec<_> = d.placed(Affine::translate((3.0, 0.0))).collect();
        let SceneItem::Shape(m) = moved[0] else {
            unreachable!()
        };
        assert!(((m.transform * corner).x - 2.0).abs() < 1e-9);
    }

    #[test]
    fn groups_clips_and_masks_become_layers() {
        let (d, _) = load(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                 <defs>
                   <clipPath id="c"><circle cx="50" cy="50" r="40"/></clipPath>
                   <mask id="m"><rect width="100" height="100" fill="white"/></mask>
                 </defs>
                 <g opacity="0.5"><rect width="10" height="10"/><rect x="20" width="10" height="10"/></g>
                 <g clip-path="url(#c)"><rect width="100" height="100" fill="green"/></g>
                 <g mask="url(#m)" clip-path="url(#c)"><rect width="100" height="100"/></g>
                 <g style="mix-blend-mode:multiply"><rect width="10" height="10"/></g>
                 <g opacity="0.25"><g opacity="0.5"><rect width="10" height="10"/></g></g>
               </svg>"##,
        );
        // Прозрачная группа; обрезка; маска (её прямоугольник — обрезка внутри маски) и
        // обрезка вложенным слоем; умножение; прозрачные группы из одной фигуры без слоя.
        assert_eq!(kinds(&d.items), "L(SS)L(M[S]S)L(M[L(M[S]S)]L(M[S]S))L(S)S");
        let Some(SceneItem::Shape(last)) = d.items.last() else {
            unreachable!()
        };
        assert_eq!(last.tint.a, 0.125);
        let SceneItem::BeginLayer { opacity, .. } = d.items[0] else {
            unreachable!()
        };
        assert_eq!(opacity, 0.5);
        assert!(d.items.contains(&SceneItem::BeginLayer {
            opacity: 1.0,
            blend: BlendMode::Multiply
        }));
        assert!(
            d.items
                .contains(&SceneItem::mask(MaskMode::Luminance, false))
        );
    }

    #[test]
    fn flat_paths_have_nothing_to_fill() {
        let p = |d: &str| BezPath::from_svg(d).unwrap();
        assert!(flat(&p("M 160 100 V 125 M 150 112 H 170")));
        assert!(flat(&p("M 0 0 L 10 10 L 5 5 Z")));
        assert!(!flat(&p("M 0 0 L 10 0 L 10 10 Z")));
        assert!(!flat(&p("M 0 0 C 0 10 10 10 10 0")));
        assert!(!flat(&p("M 0 0 L 1 0 M 0 0 L 10 0 L 10 10 Z")));
    }

    #[test]
    fn gradients_dashes_and_unsupported_parts() {
        let (d, lib) = load(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                 <defs>
                   <linearGradient id="g" x1="0" y1="0" x2="1" y2="0">
                     <stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff" stop-opacity="0.5"/>
                   </linearGradient>
                   <pattern id="p" width="10" height="10"><rect width="5" height="5"/></pattern>
                 </defs>
                 <rect width="100" height="50" fill="url(#g)" fill-opacity="0.5"/>
                 <line x1="0" y1="80" x2="100" y2="80" stroke="black" stroke-dasharray="10 5"/>
                 <rect y="60" width="10" height="10" fill="url(#p)"/>
                 <g style="mix-blend-mode:overlay"><rect width="1" height="1"/></g>
               </svg>"##,
        );
        let SceneItem::Shape(inst) = d.items[0] else {
            unreachable!()
        };
        let Paint::Gradient(g) = &lib.shape(inst.shape).unwrap().paint else {
            panic!("ждали градиент")
        };
        assert_eq!(g.stops.len(), 2);
        assert!((g.stops[1].color.a - 0.25).abs() < 1e-6, "0.5 × 0.5");
        // Штрихи: контур линии разбит на 7 отрезков (100 / 15). Пустая заливка линии
        // (у SVG она есть по умолчанию) пропущена.
        let SceneItem::Shape(line) = d.items[1] else {
            unreachable!()
        };
        let moves = lib
            .shape(line.shape)
            .unwrap()
            .path
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::MoveTo(_)))
            .count();
        assert_eq!(moves, 7, "{:?}", lib.shape(line.shape).unwrap().path);
        assert_eq!(d.warnings.len(), 2, "{:?}", d.warnings);
        assert!(
            load_svg(
                b"not svg",
                &mut ShapeLibrary::new(0.001),
                SvgOptions::default()
            )
            .is_err()
        );
    }
}
