//! Сцены песочницы этапа 3: векторные фигуры, перетекание, слои, маски, SVG, Motif и текст.
//!
//! Все фигуры готовятся один раз, при создании сцены. Каждый кадр меняются только их
//! положения, повороты, масштабы, оттенки и форма перетекания ([`Instance`]), так что
//! процессор не пересчитывает ни одной кривой.
//!
//! Сердце, кольцо и волна закрашены градиентами (радиальный с бликом, линейный и
//! повторяющийся вдоль обводки), остальные фигуры одним цветом.
//!
//! Способы заливки выбраны для каждой фигуры: всё треугольниками, кроме сердца. Его
//! показывает лупа (клавиша Z), а при сильном приближении треугольники видны ломаной,
//! поэтому сердце рисуется трафаретом. Перетекающие фигуры всегда рисуются трафаретом.

use std::f64::consts::{FRAC_PI_2, TAU};

use reyvec_motif::{MorphShape, load_morph_file};
use reyvec_svg::{SvgDrawing, SvgOptions, load_svg};
use reyvec_text::{Align, TextBlock, TextLibrary};
use reyvec_vector::{
    Affine, BezPath, BlendMode, Circle, Color, FillMethod, FillRule, Gradient, Instance, LineCap,
    LineJoin, MaskMode, Rect, SceneItem, Shape, ShapeId, ShapeLibrary, Spread, Stroke, TextId,
    TextInstance,
};

/// Допуск разбиения кривых в единицах сцены (высота кадра = 2 единицы). В окне высотой
/// 1080 пикселей это около четверти пикселя.
const TOLERANCE: f64 = 0.0005;
/// Лепестков у цветка в центре: одна фигура, показанная 12 раз.
const PETALS: usize = 12;
/// Сетка маленьких перетекающих фигур в сцене «перетекание»: столбцов и строк.
const MORPH_GRID: (usize, usize) = (17, 9);
/// Фон режимов смешивания в сцене «слои»: центр x, y, ширина, высота.
const BACKDROP: (f64, f64, f64, f64) = (0.95, 0.0, 1.6, 1.7);
/// Размер панели в сцене «маски»: ширина, высота.
const PANEL: (f64, f64) = (1.0, 1.4);

pub(crate) struct VectorDemo {
    pub library: ShapeLibrary,
    star_nonzero: ShapeId,
    star_evenodd: ShapeId,
    heart: ShapeId,
    ring: ShapeId,
    petal: ShapeId,
    core: ShapeId,
    joins: [ShapeId; 3],
    wave: ShapeId,
    /// Круг → звезда → сердце → кольцо.
    morph: ShapeId,
    /// Сцена «слои»: белый круг радиуса 1 и белый квадрат 1×1 (размер и цвет задают
    /// экземпляры), фон с градиентом и оранжевая капсула.
    disk: ShapeId,
    square: ShapeId,
    backdrop: ShapeId,
    pill: ShapeId,
    /// Сцена «маски»: белая звезда (силуэт обрезки) и прямоугольник панели с градиентом
    /// от белого к чёрному (маска по яркости).
    mask_star: ShapeId,
    fade: ShapeId,
    /// Сцена «текст»: квадрат 1×1, белый слева, чёрный справа, с переходом посередине
    /// (маска по яркости, в которой тают буквы).
    text_fade: ShapeId,
    /// Сцена «SVG»: рисунки, загруженные один раз. `user`: показан свой файл.
    svgs: Vec<SvgDrawing>,
    user_svg: bool,
    /// Сцена «Motif»: перетекающие фигуры из файла (пример или свой).
    motif: Vec<MorphShape>,
    /// Сцена «поток»: фигуры, добавленные во время работы (по одной каждые
    /// 1 / [`STREAM_RATE`] секунды), и когда каждая появилась.
    stream: Vec<(ShapeId, f32)>,
    /// Блоки текста всех сцен и атлас их букв.
    pub text: TextLibrary,
    /// Сцена «текст».
    text_scene: TextScene,
    /// Сцена «лаборатория мага» (4.6).
    pub lab: crate::lab::LabShapes,
}

/// Векторные фигуры сцены «свет» стоят ниже этой высоты (единицы сцены при камере по
/// умолчанию, низ экрана −1). Выше только SDF-фигуры: их сверяет с процессором тест
/// `light_scene_matches_cpu`.
pub const LIGHT_VECTOR_TOP: f32 = -0.74;

/// Фигур в сцене «стресс» и их сетка: столбцов, строк.
pub(crate) const STRESS_COUNT: usize = 10_000;
const STRESS_GRID: (usize, usize) = (125, 80);

/// Сколько новых фигур в секунду добавляет сцена «поток».
const STREAM_RATE: f32 = 10.0;
/// Сетка сцены «поток»: столбцов, строк. Когда она заполнится, фигуры больше не добавляются.
const STREAM_GRID: (usize, usize) = (20, 10);
/// Радиус фигуры потока в единицах сцены (примерно её размер в кадре 16:9): под него
/// подобран допуск разбиения кривых.
const STREAM_SIZE: f64 = 0.08;

/// Блоки сцены «текст».
struct TextScene {
    title: TextId,
    /// Подписи колонок и сами колонки: по левому краю, по центру, по правому, по ширине.
    labels: [TextId; 4],
    columns: [TextId; 4],
    /// Строки разного размера в пикселях (при высоте окна 1080).
    sizes: Vec<TextId>,
    /// Текст в слое с маской по яркости.
    masked: TextId,
    /// Крупное слово, которое вращается и меняет размер.
    spin: TextId,
}

/// Сцена «текст» рассчитана на область шириной `TEXT_AREA.0` и высотой `TEXT_AREA.1`
/// (единицы сцены) и уменьшается целиком, если окно уже.
const TEXT_AREA: (f64, f64) = (3.4, 2.0);
/// Ширина колонки и расстояние между колонками.
const TEXT_COLUMN: (f32, f32) = (0.74, 0.11);
/// Текст колонок: кириллица и латиница, длинные слова, цифры и знаки.
const TEXT_PARAGRAPH: &str = "Съешь же ещё этих мягких французских булок, да выпей чаю. \
Широкая электрификация южных губерний даст мощный толчок подъёму сельского хозяйства. \
The quick brown fox jumps over the lazy dog — 0123456789, «кавычки» и (скобки)!";
/// Размер строк в пикселях при высоте окна 1080.
const TEXT_SIZES: [f32; 6] = [10.0, 12.0, 14.0, 18.0, 24.0, 32.0];

/// Пример файла от Motif: круг → звезда → сердце, «плей» → «пауза», рот.
const MOTIF_EXAMPLE: &[u8] = include_bytes!("../../../assets/motif/morph.reyvec.json");
/// Доля клетки, которую занимает фигура Motif.
const MOTIF_FIT: f64 = 0.8;

/// Тестовые рисунки SVG сцены «SVG». Те же файлы сверяются с `resvg` в тестах.
pub(crate) const TEST_SVGS: [(&str, &[u8]); 6] = [
    ("shapes", include_bytes!("../../../assets/svg/shapes.svg")),
    (
        "gradients",
        include_bytes!("../../../assets/svg/gradients.svg"),
    ),
    ("groups", include_bytes!("../../../assets/svg/groups.svg")),
    (
        "clip-mask",
        include_bytes!("../../../assets/svg/clip-mask.svg"),
    ),
    (
        "transforms",
        include_bytes!("../../../assets/svg/transforms.svg"),
    ),
    (
        "illustration",
        include_bytes!("../../../assets/svg/illustration.svg"),
    ),
];
/// Сетка тестовых рисунков: столбцов, строк.
const SVG_GRID: (usize, usize) = (3, 2);
/// Доля клетки или кадра, которую занимает рисунок.
const SVG_FIT: f64 = 0.95;

impl VectorDemo {
    /// `files`: свой рисунок для сцены «SVG» и свои фигуры для сцены «Motif».
    pub fn new(files: &crate::UserFiles) -> Self {
        let mut library = ShapeLibrary::new(TOLERANCE);
        let mut add = |shape| {
            library
                .add(shape)
                .expect("фигуры сцены песочницы корректны (проверено тестом)")
        };
        let gold = Color::from_srgb8(242, 193, 78);
        let star_nonzero = add(Shape::fill(pentagram(0.32), gold));
        let star_evenodd = add(Shape::fill_with(
            pentagram(0.32),
            FillRule::EvenOdd,
            Color::from_srgb8(78, 205, 196),
        ));
        let mut ring_path = circle(0.28);
        ring_path.extend(circle(0.16));
        // Кольцо: линейный градиент сверху вниз.
        let ring = add(Shape::fill_with(
            ring_path,
            FillRule::EvenOdd,
            Gradient::linear((0.0, 0.28), (0.0, -0.28))
                .stop(0.0, Color::from_srgb8(160, 140, 255))
                .stop(1.0, Color::from_srgb8(60, 40, 170)),
        ));
        let petal = add(Shape::fill(petal(), Color::WHITE));
        let core = add(Shape::fill(circle(0.07), Color::from_srgb8(255, 233, 168)));
        let blue = Color::from_srgb8(154, 209, 245);
        let joins = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel].map(|join| {
            add(Shape::stroke(
                zigzag(),
                Stroke::new(0.06).with_join(join),
                blue,
            ))
        });
        // Волна: обводка с повторяющимся градиентом вдоль.
        let wave = add(Shape::stroke(
            wave(),
            Stroke::new(0.035).with_cap(LineCap::Round),
            Gradient::linear((-0.7, 0.0), (-0.35, 0.0))
                .with_spread(Spread::Reflect)
                .stop(0.0, Color::from_srgb8(245, 163, 199))
                .stop(1.0, Color::from_srgb8(120, 200, 255)),
        ));
        // Сердце трафаретом: его показывает лупа.
        // Сердце: радиальный градиент с бликом слева сверху.
        let heart_shape = Shape::fill(
            heart(0.3),
            Gradient::radial((0.0, 0.05), 0.42)
                .with_focus((-0.12, 0.15))
                .stop(0.0, Color::from_srgb8(255, 170, 180))
                .stop(0.45, Color::from_srgb8(232, 74, 95))
                .stop(1.0, Color::from_srgb8(120, 20, 45)),
        );
        let morph_keyframes = [circle(0.4), star(5, 0.45, 0.2), heart(0.4), {
            let mut ring = circle(0.4);
            ring.extend(circle(0.22).reverse_subpaths());
            ring
        }];
        let disk = add(Shape::fill(circle(1.0), Color::WHITE));
        let square = add(Shape::fill(rect(1.0, 1.0), Color::WHITE));
        // Фон режимов смешивания: светлый сверху, бирюзовый в середине, тёмный снизу.
        let backdrop = add(Shape::fill(
            rect(BACKDROP.2, BACKDROP.3),
            Gradient::linear((0.0, BACKDROP.3 / 2.0), (0.0, -BACKDROP.3 / 2.0))
                .stop(0.0, Color::from_srgb8(245, 245, 240))
                .stop(0.5, Color::from_srgb8(50, 140, 170))
                .stop(1.0, Color::from_srgb8(15, 15, 25)),
        ));
        let pill = add(Shape::fill(
            {
                use reyvec_vector::kurbo::{RoundedRect, Shape as _};
                RoundedRect::new(-0.13, -0.6, 0.13, 0.6, 0.13).to_path(1e-6)
            },
            Color::from_srgb8(255, 140, 40),
        ));
        let mask_star = add(Shape::fill(star(5, 0.5, 0.22), Color::WHITE));
        let fade = add(Shape::fill(
            rect(PANEL.0, PANEL.1),
            Gradient::linear((-PANEL.0 / 2.0, 0.0), (PANEL.0 / 2.0, 0.0))
                .stop(0.0, Color::WHITE)
                .stop(1.0, Color::from_srgb8(0, 0, 0)),
        ));
        let text_fade = add(Shape::fill(
            rect(1.0, 1.0),
            Gradient::linear((-0.5, 0.0), (0.5, 0.0))
                .stop(0.4, Color::WHITE)
                .stop(0.6, Color::from_srgb8(0, 0, 0)),
        ));
        let heart = library
            .add_with(heart_shape, FillMethod::Stencil)
            .expect("сердце корректно (проверено тестом)");
        let morph = library
            .add_morph(&morph_keyframes, FillRule::NonZero, Color::WHITE)
            .expect("контуры перетекания корректны (проверено тестом)");
        let lab = crate::lab::LabShapes::new(&mut library);
        let (text, text_scene) = text_scene();
        Self {
            library,
            star_nonzero,
            star_evenodd,
            heart,
            ring,
            petal,
            core,
            joins,
            wave,
            morph,
            disk,
            square,
            backdrop,
            pill,
            mask_star,
            fade,
            text_fade,
            svgs: Vec::new(),
            user_svg: false,
            motif: Vec::new(),
            stream: Vec::new(),
            text,
            text_scene,
            lab,
        }
        .with_svgs(files.svg.as_deref())
        .with_motif(files.motif.as_deref())
    }

    /// Загружает фигуры сцены «Motif»: свой файл, а если его нет или он не читается,
    /// пример. Ошибки и предупреждения пишутся в лог.
    fn with_motif(mut self, user: Option<&[u8]>) -> Self {
        let mut load = |name: &str, data: &[u8]| {
            let _zone = crate::cpu_zone!("загрузка файла Motif");
            let started = std::time::Instant::now();
            match load_morph_file(data, &mut self.library) {
                Ok(f) => {
                    tracing::info!(
                        "Motif «{name}»: {} перетекающих фигур за {:.1} мс",
                        f.shapes.len(),
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                    for w in &f.warnings {
                        tracing::warn!("Motif «{name}»: {w}");
                    }
                    Some(f.shapes)
                }
                Err(e) => {
                    tracing::error!("Motif «{name}»: {e}");
                    None
                }
            }
        };
        self.motif = user
            .and_then(|data| load("свой файл", data))
            .or_else(|| load("пример", MOTIF_EXAMPLE))
            .unwrap_or_default();
        self
    }

    /// Сцена «поток» в момент `time`: добавляет в набор фигуры, которым пора появиться
    /// (каждая новая: своя форма из кривых, свой цвет, у каждой третьей градиент, у каждой
    /// пятой обводка), и раскладывает все сеткой. Новые фигуры режутся на треугольники
    /// здесь же, а на видеокарту попадают внутри кадра без ожидания
    /// (`VectorRenderer::stream`). Появившись, фигура за 0,3 с вырастает из точки.
    pub fn stream_items(&mut self, time: f32, aspect: f32) -> Vec<SceneItem> {
        let (cols, rows) = STREAM_GRID;
        let wanted = ((time.max(0.0) * STREAM_RATE) as usize + 1).min(cols * rows);
        while self.stream.len() < wanted {
            let i = self.stream.len();
            let shape = stream_shape(i);
            match self.library.add(shape) {
                Ok(id) => self.stream.push((id, i as f32 / STREAM_RATE)),
                Err(e) => {
                    tracing::error!("фигура потока №{i}: {e}");
                    break;
                }
            }
        }
        let aspect = f64::from(aspect);
        let (cw, ch) = (2.0 * aspect / cols as f64, 2.0 / rows as f64);
        self.stream
            .iter()
            .enumerate()
            .map(|(i, &(id, born))| {
                let (c, r) = (i % cols, i / cols);
                let center = (-aspect + cw * (c as f64 + 0.5), 1.0 - ch * (r as f64 + 0.5));
                let grow = ((time - born) / 0.3).clamp(0.0, 1.0);
                let k = cw.min(ch) * 0.45 / STREAM_SIZE * f64::from(grow * (2.0 - grow));
                SceneItem::Shape(Instance::new(
                    id,
                    Affine::translate(center)
                        * Affine::rotate(f64::from(time) * 0.3 + i as f64)
                        * Affine::scale(k),
                ))
            })
            .collect()
    }

    /// Сцена «Motif» в момент `time`: фигуры в ряд (до четырёх в ряду), каждая по
    /// первой своей анимации из Motif (без анимации стоит на своём значении `morph`).
    /// Каждый кадр процессор только берёт значение анимации, формы смешивает видеокарта.
    pub fn motif_items(&self, time: f32, aspect: f32) -> Vec<SceneItem> {
        let n = self.motif.len();
        if n == 0 {
            return Vec::new();
        }
        let aspect = f64::from(aspect);
        let cols = n.min(4);
        let rows = n.div_ceil(cols);
        let (cw, ch) = (2.0 * aspect / cols as f64, 2.0 / rows as f64);
        self.motif
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let (c, r) = (i % cols, i / cols);
                let center = (-aspect + cw * (c as f64 + 0.5), 1.0 - ch * (r as f64 + 0.5));
                let (w, h) = m.size;
                let k = (ch * MOTIF_FIT / h.max(1e-6)).min(cw * MOTIF_FIT / w.max(1e-6));
                let morph = m.animations.first().map_or(m.morph, |a| a.sample(time));
                SceneItem::Shape(
                    Instance::new(m.shape, Affine::translate(center) * Affine::scale(k))
                        .with_morph(morph),
                )
            })
            .collect()
    }

    /// Сцена «текст» в момент `time`: заголовок, четыре колонки с разным выравниванием,
    /// строки от 10 до 32 пикселей, текст в слое с маской и вращающееся слово. Текст
    /// разложен один раз при создании сцены: каждый кадр здесь только места блоков.
    pub fn text_items(&self, time: f32, aspect: f32) -> Vec<SceneItem> {
        let t = f64::from(time);
        let s = &self.text_scene;
        // Вся сцена уменьшается, если окно уже 3.4 × 2.
        let fit = (2.0 * f64::from(aspect) / TEXT_AREA.0)
            .min(2.0 / TEXT_AREA.1)
            .min(1.0);
        let place = Affine::scale(fit);
        let left = -TEXT_AREA.0 / 2.0 + 0.05;
        let mut out = Vec::with_capacity(32);
        let mut put =
            |id: TextId, x: f64, y: f64| out.push(SceneItem::from(TextInstance::at(id, x, y)));

        // Заголовок по центру сверху.
        let (w, _) = self.text_size(s.title);
        put(s.title, -w / 2.0, 0.97);
        // Колонки.
        let step = f64::from(TEXT_COLUMN.0 + TEXT_COLUMN.1);
        let columns_width = 4.0 * step - f64::from(TEXT_COLUMN.1);
        for i in 0..4 {
            let x = -columns_width / 2.0 + step * i as f64;
            put(s.labels[i], x, 0.72);
            put(s.columns[i], x, 0.64);
        }
        // Строки разного размера слева внизу.
        let mut y = -0.05;
        for &id in &s.sizes {
            put(id, left, y);
            y -= self.text_size(id).1 + 0.02;
        }

        // В середине внизу: текст в слое с маской по яркости (белое видно, чёрное нет):
        // слева буквы видны, справа нет, а граница ходит туда-сюда.
        let (mw, mh) = self.text_size(s.masked);
        let (mx, my) = (-0.25, -0.12);
        out.push(SceneItem::layer(1.0));
        out.push(SceneItem::mask(MaskMode::Luminance, false));
        let sway = 0.45 * (t * 0.8).sin();
        out.push(SceneItem::from(Instance::new(
            self.text_fade,
            Affine::translate((mx + mw / 2.0 + sway * mw, my - mh / 2.0))
                * Affine::scale_non_uniform(2.0 * mw, mh * 1.2),
        )));
        out.push(SceneItem::EndMask);
        out.push(SceneItem::from(TextInstance::at(s.masked, mx, my)));
        out.push(SceneItem::EndLayer);

        // Справа внизу: слово вращается и меняет размер (буквы остаются чёткими).
        let (sw, sh) = self.text_size(s.spin);
        let k = 1.0 + 0.35 * (t * 0.7).sin();
        let mut spin = TextInstance::at(s.spin, 0.0, 0.0);
        spin.transform = Affine::translate((1.15, -0.55))
            * Affine::rotate(t * 0.4)
            * Affine::scale(k)
            * Affine::translate((-sw / 2.0, sh / 2.0));
        let hue = soft_hue((time * 0.05).rem_euclid(1.0));
        spin.tint = Color::linear(hue[0], hue[1], hue[2], 1.0);
        out.push(spin.into());

        out.into_iter().map(|i| i.placed(place)).collect()
    }

    /// Ширина и высота блока текста в единицах сцены.
    fn text_size(&self, id: TextId) -> (f64, f64) {
        self.text.get(id).map_or((0.0, 0.0), |l| {
            let (w, h) = l.size();
            (f64::from(w), f64::from(h))
        })
    }

    /// Загружает рисунки сцены «SVG»: свой файл или тестовые. Ошибки пишутся в лог.
    fn with_svgs(mut self, user: Option<&[u8]>) -> Self {
        let mut load = |name: &str, data: &[u8], height: f64| {
            let _zone = crate::cpu_zone!("загрузка SVG");
            let started = std::time::Instant::now();
            match load_svg(data, &mut self.library, SvgOptions { height }) {
                Ok(d) => {
                    tracing::info!(
                        "SVG «{name}»: {} фигур за {:.1} мс",
                        d.shape_count,
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                    for w in &d.warnings {
                        tracing::warn!("SVG «{name}»: {w}");
                    }
                    Some(d)
                }
                Err(e) => {
                    tracing::error!("SVG «{name}»: {e}");
                    None
                }
            }
        };
        if let Some(d) = user.and_then(|data| load("свой файл", data, 2.0 * SVG_FIT)) {
            self.svgs.push(d);
            self.user_svg = true;
            return self;
        }
        let cell = 2.0 / SVG_GRID.1 as f64 * SVG_FIT;
        for (name, data) in TEST_SVGS {
            if let Some(d) = load(name, data, cell) {
                self.svgs.push(d);
            }
        }
        self
    }

    /// Сцена «SVG» для кадра с соотношением сторон `aspect`: свой рисунок по центру во
    /// весь кадр или тестовые рисунки сеткой. Рисунки не двигаются: каждый кадр только
    /// копируется готовый список.
    pub fn svg_items(&self, aspect: f32) -> Vec<SceneItem> {
        let aspect = f64::from(aspect);
        if self.user_svg {
            let d = &self.svgs[0];
            let (w, h) = d.size;
            let k = (2.0 * SVG_FIT / h).min(2.0 * aspect * SVG_FIT / w);
            return d.placed(Affine::scale(k)).collect();
        }
        let (cols, rows) = SVG_GRID;
        let (cw, ch) = (2.0 * aspect / cols as f64, 2.0 / rows as f64);
        let mut out = Vec::new();
        for (i, d) in self.svgs.iter().enumerate() {
            let (c, r) = (i % cols, i / cols);
            let center = (-aspect + cw * (c as f64 + 0.5), 1.0 - ch * (r as f64 + 0.5));
            let (w, h) = d.size;
            let k = (ch * SVG_FIT / h).min(cw * SVG_FIT / w);
            out.extend(d.placed(Affine::translate(center) * Affine::scale(k)));
        }
        out
    }

    /// Сцена «маски» в момент `time`: три панели.
    ///
    /// - Слева обрезка: косые цветные полосы видны только внутри вращающейся звезды
    ///   (маска по прозрачности).
    /// - В середине маска по яркости: сетка цветных кругов плавно гаснет слева направо
    ///   под градиентом от белого к чёрному.
    /// - Справа обратная маска: в бирюзовой панели со звездой движется круглая дырка,
    ///   сквозь неё видны полосы позади панели.
    pub fn mask_items(&self, time: f32) -> Vec<SceneItem> {
        let t = f64::from(time);
        let mut out = Vec::with_capacity(96);
        let shape = |id, transform| SceneItem::from(Instance::new(id, transform));
        let tinted =
            |id, transform, c: Color| SceneItem::from(Instance::new(id, transform).with_tint(c));
        let (w, h) = PANEL;
        let centers = [-1.15, 0.0, 1.15];
        // Тёмные подложки панелей, чтобы были видны их границы.
        for x in centers {
            out.push(tinted(
                self.square,
                Affine::translate((x, 0.0)) * Affine::scale_non_uniform(w, h),
                Color::from_srgb8(32, 36, 58),
            ));
        }

        // Слева: обрезка по звезде.
        let x = centers[0];
        out.push(SceneItem::layer(1.0));
        out.push(SceneItem::clip());
        out.push(shape(
            self.mask_star,
            Affine::translate((x, 0.0)) * Affine::rotate(t * 0.3),
        ));
        out.push(SceneItem::EndMask);
        let step = 0.12;
        let shift = (t * 0.1).rem_euclid(step);
        for i in 0..16 {
            let k = f64::from(i);
            let [r, g, b] = soft_hue(i as f32 / 16.0);
            out.push(tinted(
                self.square,
                Affine::translate((x - 0.9 + k * step + shift, 0.0))
                    * Affine::rotate(0.5)
                    * Affine::scale_non_uniform(0.07, 2.2),
                Color::linear(r, g, b, 1.0),
            ));
        }
        out.push(SceneItem::EndLayer);

        // В середине: маска по яркости.
        let x = centers[1];
        out.push(SceneItem::layer(1.0));
        out.push(SceneItem::mask(MaskMode::Luminance, false));
        out.push(shape(self.fade, Affine::translate((x, 0.0))));
        out.push(SceneItem::EndMask);
        let (cols, rows) = (5, 7);
        for row in 0..rows {
            for col in 0..cols {
                let k = (row * cols + col) as f32 / (rows * cols) as f32;
                let [r, g, b] = soft_hue(k);
                let pulse = 1.0 + 0.15 * (t * 2.0 + f64::from(row + col)).sin();
                out.push(tinted(
                    self.disk,
                    Affine::translate((
                        x + (f64::from(col) - 2.0) * 0.19,
                        (f64::from(row) - 3.0) * 0.19,
                    )) * Affine::scale(0.075 * pulse),
                    Color::linear(r, g, b, 1.0),
                ));
            }
        }
        out.push(SceneItem::EndLayer);

        // Справа: полосы позади панели и панель с обратной маской (дыркой).
        let x = centers[2];
        let stripes = 7;
        let sw = w / f64::from(stripes);
        for i in 0..stripes {
            let c = if i % 2 == 0 {
                Color::from_srgb8(200, 60, 80)
            } else {
                Color::from_srgb8(240, 220, 120)
            };
            out.push(tinted(
                self.square,
                Affine::translate((x - w / 2.0 + sw * (f64::from(i) + 0.5), 0.0))
                    * Affine::scale_non_uniform(sw, h),
                c,
            ));
        }
        out.push(SceneItem::layer(1.0));
        out.push(SceneItem::mask(MaskMode::Alpha, true));
        let a = t * 0.8;
        out.push(shape(
            self.disk,
            Affine::translate((x + 0.25 * a.cos(), 0.3 * a.sin())) * Affine::scale(0.22),
        ));
        out.push(SceneItem::EndMask);
        out.push(tinted(
            self.square,
            Affine::translate((x, 0.0)) * Affine::scale_non_uniform(w, h),
            Color::from_srgb8(40, 150, 150),
        ));
        out.push(tinted(
            self.mask_star,
            Affine::translate((x, 0.0)) * Affine::scale(0.7),
            Color::from_srgb8(242, 193, 78),
        ));
        out.push(SceneItem::EndLayer);
        out
    }

    /// Сцена «слои» в момент `time`.
    ///
    /// Слева два снеговика на полосатом фоне. У левого каждая часть полупрозрачная
    /// отдельно: перекрытия светлее, сквозь тело видны края шаров и шляпы. Правый
    /// целиком в слое с прозрачностью 0.5: он ровно полупрозрачный, как в редакторах.
    /// Его голова со шляпой лежат во вложенном слое (проверка вложенности: если она
    /// сломана, головы не будет).
    ///
    /// Справа фон с градиентом и четыре капсулы, каждая в своём слое с режимом
    /// смешивания: обычное, умножение, экран, сложение (слева направо).
    pub fn layer_items(&self, time: f32) -> Vec<SceneItem> {
        let t = f64::from(time);
        let mut out = Vec::with_capacity(64);
        let shape = |id, transform| SceneItem::from(Instance::new(id, transform));
        let tinted =
            |id, transform, c: Color| SceneItem::from(Instance::new(id, transform).with_tint(c));
        // Полосатый фон под снеговиками.
        let (x0, x1) = (-1.7, -0.2);
        let stripes = 10;
        let w = (x1 - x0) / f64::from(stripes);
        for i in 0..stripes {
            let c = if i % 2 == 0 {
                Color::from_srgb8(200, 60, 80)
            } else {
                Color::from_srgb8(40, 50, 90)
            };
            let x = x0 + w * (f64::from(i) + 0.5);
            out.push(tinted(
                self.square,
                Affine::translate((x, 0.0)) * Affine::scale_non_uniform(w, 1.7),
                c,
            ));
        }
        // Снеговики: шары снизу вверх, потом шляпа (поля и тулья).
        let sway = 0.03 * (t * 0.9).sin();
        let snow = Color::from_srgb8(235, 242, 255);
        let hat = Color::from_srgb8(30, 30, 45);
        let body = |x: f64, alpha: f32| {
            let c = |base: Color| Color {
                a: base.a * alpha,
                ..base
            };
            [
                tinted(
                    self.disk,
                    Affine::translate((x, -0.5)) * Affine::scale(0.24),
                    c(snow),
                ),
                tinted(
                    self.disk,
                    Affine::translate((x + sway * 0.5, -0.2)) * Affine::scale(0.17),
                    c(snow),
                ),
            ]
        };
        let head = |x: f64, alpha: f32| {
            let c = |base: Color| Color {
                a: base.a * alpha,
                ..base
            };
            let x = x + sway;
            [
                tinted(
                    self.disk,
                    Affine::translate((x, 0.04)) * Affine::scale(0.12),
                    c(snow),
                ),
                tinted(
                    self.square,
                    Affine::translate((x, 0.13)) * Affine::scale_non_uniform(0.3, 0.04),
                    c(hat),
                ),
                tinted(
                    self.square,
                    Affine::translate((x, 0.23)) * Affine::scale_non_uniform(0.17, 0.18),
                    c(hat),
                ),
            ]
        };
        // Левый: каждая часть с прозрачностью 0.5.
        out.extend(body(-1.32, 0.5));
        out.extend(head(-1.32, 0.5));
        // Правый: непрозрачные части в слое 0.5, голова во вложенном слое.
        out.push(SceneItem::layer(0.5));
        out.extend(body(-0.58, 1.0));
        out.push(SceneItem::layer(1.0));
        out.extend(head(-0.58, 1.0));
        out.push(SceneItem::EndLayer);
        out.push(SceneItem::EndLayer);

        // Справа: фон и капсулы с режимами смешивания.
        let (bx, by, _, _) = BACKDROP;
        out.push(shape(self.backdrop, Affine::translate((bx, by))));
        let modes = [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Add,
        ];
        for (i, blend) in modes.into_iter().enumerate() {
            let k = i as f64;
            let x = bx + (k - 1.5) * 0.38;
            let y = by + 0.12 * (t * 0.8 + k).sin();
            out.push(SceneItem::blend(blend, 1.0));
            out.push(shape(self.pill, Affine::translate((x, y))));
            // Синий кружок внутри: слой накладывается вместе с ним как одно целое.
            out.push(tinted(
                self.disk,
                Affine::translate((x, y + 0.3)) * Affine::scale(0.08),
                Color::from_srgb8(70, 120, 255),
            ));
            out.push(SceneItem::EndLayer);
        }
        out
    }

    /// Сцена «перетекание» в момент `time`: сетка маленьких фигур, у каждой свой темп, и
    /// большая фигура в центре поверх них. Каждый кадр меняется только число `morph` в
    /// экземплярах: смешивает контуры видеокарта.
    pub fn morph_instances(&self, time: f32) -> Vec<Instance> {
        let (cols, rows) = MORPH_GRID;
        let last = 3.0; // номер последнего ключевого контура
        // Туда и обратно: 0 → 3 → 0, плавно замедляясь у краёв.
        let swing = |phase: f32| last * (0.5 - 0.5 * (time * 0.7 + phase).cos());
        let mut out = Vec::with_capacity(cols * rows + 1);
        for r in 0..rows {
            for c in 0..cols {
                let x = (c as f64 - (cols - 1) as f64 / 2.0) * 0.2;
                let y = (r as f64 - (rows - 1) as f64 / 2.0) * 0.2;
                let k = (c + r) as f32 / (cols + rows) as f32;
                let [red, green, blue] = soft_hue(k);
                out.push(
                    Instance::new(self.morph, Affine::translate((x, y)) * Affine::scale(0.2))
                        .with_tint(Color::linear(red, green, blue, 0.55))
                        .with_morph(swing(k * TAU as f32)),
                );
            }
        }
        out.push(
            Instance::new(self.morph, Affine::scale(1.4))
                .with_tint(Color::from_srgb8(242, 193, 78))
                .with_morph(swing(0.0)),
        );
        out
    }

    /// Сцена «стресс» в момент `time`: [`STRESS_COUNT`] фигур сеткой на весь кадр, каждая
    /// кружит на своём месте и вращается со своей скоростью. `morph`: все фигуры
    /// перетекающие и меняют форму каждый кадр (каждая рисуется трафаретом), иначе пять
    /// разных обычных фигур вперемешку (треугольниками). Каждый кадр процессор только
    /// считает положения: кривые не пересчитываются.
    pub fn stress_instances(&self, time: f32, aspect: f32, morph: bool) -> Vec<Instance> {
        let (cols, rows) = STRESS_GRID;
        let t = f64::from(time);
        // Сетка на весь кадр с небольшим полем.
        let width = 2.0 * f64::from(aspect) * 0.96;
        let height = 2.0 * 0.96;
        let (cw, ch) = (width / cols as f64, height / rows as f64);
        let cell = cw.min(ch);
        // Обычные фигуры и во сколько раз их уменьшить до размера клетки (радиус ~1).
        let shapes = [
            (self.star_nonzero, 1.0 / 0.32),
            (self.ring, 1.0 / 0.28),
            (self.petal, 1.0 / 0.36),
            (self.core, 1.0 / 0.07),
            (self.star_evenodd, 1.0 / 0.32),
        ];
        let mut out = Vec::with_capacity(STRESS_COUNT);
        for i in 0..STRESS_COUNT {
            let (c, r) = (i % cols, i / cols);
            let x = -width / 2.0 + (c as f64 + 0.5) * cw;
            let y = -height / 2.0 + (r as f64 + 0.5) * ch;
            let speed = 0.5 + 1.5 * noise(i, 1);
            let phase = noise(i, 2) * TAU;
            let orbit = 0.25 * cell;
            let a = t * speed + phase;
            let place = Affine::translate((x + orbit * a.cos(), y + orbit * a.sin()));
            let k = (c as f32 / cols as f32 + r as f32 / rows as f32) * 0.5;
            let [red, green, blue] = soft_hue(k);
            let tint = Color::linear(red, green, blue, 0.9);
            out.push(if morph {
                // Туда и обратно по всем четырём контурам, у каждой свой темп.
                let swing = 3.0 * (0.5 - 0.5 * (t * 0.8 * speed + phase).cos());
                Instance::new(
                    self.morph,
                    place * Affine::rotate(a * 0.5) * Affine::scale(0.4 * cell / 0.45),
                )
                .with_tint(tint)
                .with_morph(swing as f32)
            } else {
                // Фигуры вперемешку: соседние почти всегда разные, как в настоящей сцене.
                let pick = (noise(i, 3) * shapes.len() as f64) as usize;
                let (shape, unit) = shapes[pick.min(shapes.len() - 1)];
                Instance::new(
                    shape,
                    place * Affine::rotate(a) * Affine::scale(0.4 * cell * unit),
                )
                .with_tint(tint)
            });
        }
        out
    }

    /// Фигуры на сцене в момент `time` (секунды), в порядке рисования.
    /// Векторные фигуры сцены «свет» (задача 4.2б): ряд под SDF-фигурами, ниже
    /// [`LIGHT_VECTOR_TOP`]. Звезда, сердце и кольцо стоят в слое «отбрасывает тень» и
    /// попадают в поле расстояний, вторая звезда справа без отметки: поле её не видит.
    pub fn light_items(&self, time: f32) -> Vec<SceneItem> {
        let t = f64::from(time);
        let y = -0.88;
        let size = 0.3;
        let beat = 1.0 + 0.08 * (t * 3.0).sin().max(0.0);
        let squash = 0.2 * (t * 1.5).sin();
        vec![
            SceneItem::shadow_caster(),
            Instance::new(
                self.star_nonzero,
                Affine::translate((-1.2, y)) * Affine::rotate(t * 0.4) * Affine::scale(size),
            )
            .into(),
            Instance::new(
                self.heart,
                Affine::translate((-0.4, y)) * Affine::scale(size * beat),
            )
            .into(),
            Instance::new(
                self.ring,
                Affine::translate((0.4, y))
                    * Affine::scale_non_uniform(size * (1.0 + squash), size * (1.0 - squash)),
            )
            .into(),
            SceneItem::EndLayer,
            Instance::new(
                self.star_nonzero,
                Affine::translate((1.2, y)) * Affine::rotate(-t * 0.4) * Affine::scale(size),
            )
            .into(),
        ]
    }

    pub fn instances(&self, time: f32) -> Vec<Instance> {
        let t = f64::from(time);
        let mut out = Vec::with_capacity(10 + PETALS);
        // Слева: одна и та же звезда одной линией с разными правилами заливки.
        out.push(Instance::new(
            self.star_nonzero,
            Affine::translate((-1.2, 0.42)) * Affine::rotate(t * 0.4),
        ));
        out.push(Instance::new(
            self.star_evenodd,
            Affine::translate((-1.2, -0.42)) * Affine::rotate(-t * 0.4),
        ));
        // Справа: сердце бьётся (масштаб), кольцо сплющивается (разный масштаб по осям).
        let beat = 1.0 + 0.08 * (t * 3.0).sin().max(0.0);
        out.push(Instance::new(
            self.heart,
            Affine::translate((1.2, 0.42)) * Affine::scale(beat),
        ));
        let squash = 0.2 * (t * 1.5).sin();
        out.push(Instance::new(
            self.ring,
            Affine::translate((1.2, -0.42)) * Affine::scale_non_uniform(1.0 + squash, 1.0 - squash),
        ));
        // В центре: цветок из 12 копий одного лепестка с разными оттенками.
        for i in 0..PETALS {
            let k = i as f64 / PETALS as f64;
            let [r, g, b] = soft_hue(k as f32);
            out.push(
                Instance::new(
                    self.petal,
                    Affine::translate((0.0, 0.05)) * Affine::rotate(k * TAU + t * 0.5),
                )
                .with_tint(Color::linear(r, g, b, 0.9)),
            );
        }
        out.push(Instance::new(self.core, Affine::translate((0.0, 0.05))));
        // Сверху: углы обводки (острый, скруглённый, срезанный).
        for (i, &shape) in self.joins.iter().enumerate() {
            out.push(Instance::new(
                shape,
                Affine::translate((-0.55 + 0.55 * i as f64, 0.78)),
            ));
        }
        // Снизу: волна со скруглёнными концами покачивается.
        out.push(Instance::new(
            self.wave,
            Affine::translate((0.08 * (t * 1.2).sin(), -0.78)),
        ));
        out
    }
}

/// Число от 0 до 1 из номера `i` и соли `salt`: одинаковое при каждом запуске.
fn noise(i: usize, salt: u64) -> f64 {
    let mut x =
        (i as u64 ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_add(0x632B_E59B_D9B4_E019);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// Фигура потока №`i`: гладкое пятно радиуса около [`STREAM_SIZE`] из квадратичных кривых через
/// середины между случайными точками на окружности.
fn stream_shape(i: usize) -> Shape {
    let n = 5 + (noise(i, 1) * 5.0) as usize;
    let points: Vec<(f64, f64)> = (0..n)
        .map(|k| {
            let a = TAU * k as f64 / n as f64;
            let r = STREAM_SIZE * (0.55 + 0.45 * noise(i * 16 + k, 2));
            (r * a.cos(), r * a.sin())
        })
        .collect();
    let mid = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let mut path = BezPath::new();
    path.move_to(mid(points[n - 1], points[0]));
    for k in 0..n {
        path.quad_to(points[k], mid(points[k], points[(k + 1) % n]));
    }
    path.close_path();
    // Цвета по кругу оттенков с шагом «золотого угла»: соседние фигуры не похожи.
    let hue = (i as f64 * 0.618_034).fract();
    let color = hsv(hue, 0.65, 0.95);
    if i % 5 == 4 {
        return Shape::stroke(
            path,
            Stroke::new(STREAM_SIZE * 0.18).with_join(LineJoin::Round),
            color,
        );
    }
    if i % 3 == 2 {
        let gradient = Gradient::linear((0.0, -STREAM_SIZE), (0.0, STREAM_SIZE))
            .stop(0.0, color)
            .stop(1.0, hsv((hue + 0.15).fract(), 0.5, 1.0));
        return Shape::fill(path, gradient);
    }
    Shape::fill(path, color)
}

/// Цвет из оттенка, насыщенности и яркости (всё от 0 до 1), в sRGB.
fn hsv(h: f64, s: f64, v: f64) -> Color {
    let f = |n: f64| {
        let k = (n + h * 6.0) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    let byte = |x: f64| (x * 255.0).round() as u8;
    Color::from_srgb8(byte(f(5.0)), byte(f(3.0)), byte(f(1.0)))
}

/// Прямоугольник `w`×`h` с центром в нуле.
/// Готовит блоки сцены «текст»: раскладка и атлас букв один раз.
fn text_scene() -> (TextLibrary, TextScene) {
    let _zone = crate::cpu_zone!("раскладка текста");
    let started = std::time::Instant::now();
    let mut lib = TextLibrary::new();
    let white = Color::from_srgb8(236, 238, 245);
    let grey = Color::from_srgb8(150, 156, 180);
    let gold = Color::from_srgb8(242, 193, 78);
    let title = lib.add(TextBlock::new("ReyVec: текст на векторном слое", 0.11).color(white));
    let aligns = [Align::Left, Align::Center, Align::Right, Align::Justify];
    let labels = aligns.map(|a| {
        lib.add(
            TextBlock::new(a.label(), 0.04)
                .width(TEXT_COLUMN.0)
                .align(a)
                .color(gold),
        )
    });
    let columns = aligns.map(|a| {
        lib.add(
            TextBlock::new(TEXT_PARAGRAPH, 0.042)
                .width(TEXT_COLUMN.0)
                .align(a)
                .line_height(1.3)
                .color(white),
        )
    });
    // 1 пиксель при высоте окна 1080 = 2 / 1080 единиц сцены.
    let sizes = TEXT_SIZES
        .iter()
        .map(|&px| {
            lib.add(
                TextBlock::new(
                    format!("{px} px: Мелкий текст интерфейса — Small UI text 123"),
                    px * 2.0 / 1080.0,
                )
                .color(grey),
            )
        })
        .collect();
    let masked = lib.add(
        TextBlock::new("Текст в слое с маской: буквы гаснут и появляются", 0.075)
            .width(1.2)
            .color(Color::from_srgb8(120, 200, 255)),
    );
    let spin = lib.add(TextBlock::new("Чёткость", 0.12).color(Color::WHITE));
    tracing::info!(
        "Текст: {} блоков, {} разных букв в атласе за {:.1} мс",
        lib.len(),
        lib.atlas().len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    (
        lib,
        TextScene {
            title,
            labels,
            columns,
            sizes,
            masked,
            spin,
        },
    )
}

fn rect(w: f64, h: f64) -> BezPath {
    use reyvec_vector::kurbo::Shape as _;
    Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0).to_path(1e-6)
}

/// Круг радиуса `r` с центром в нуле (четыре кубические кривые).
fn circle(r: f64) -> BezPath {
    use reyvec_vector::kurbo::Shape as _;
    Circle::new((0.0, 0.0), r).to_path(1e-6)
}

/// Пятиконечная звезда одной линией (контур пересекает сам себя) радиуса `r`.
fn pentagram(r: f64) -> BezPath {
    let mut path = BezPath::new();
    for i in 0..5 {
        let a = FRAC_PI_2 + f64::from(i * 2) * TAU / 5.0;
        let p = (r * a.cos(), r * a.sin());
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    path.close_path();
    path
}

/// Сердце из четырёх кубических кривых Безье, высотой около `2 * size`.
fn heart(size: f64) -> BezPath {
    let mut path = BezPath::new();
    path.move_to((0.0, -1.0));
    path.curve_to((-0.2, -0.75), (-1.0, -0.35), (-1.0, 0.25));
    path.curve_to((-1.0, 0.75), (-0.45, 1.0), (0.0, 0.55));
    path.curve_to((0.45, 1.0), (1.0, 0.75), (1.0, 0.25));
    path.curve_to((1.0, -0.35), (0.2, -0.75), (0.0, -1.0));
    path.close_path();
    path.apply_affine(Affine::scale(size));
    path
}

/// Лепесток из двух квадратичных кривых: от центра вверх и обратно.
fn petal() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((0.0, 0.06));
    path.quad_to((0.11, 0.2), (0.0, 0.36));
    path.quad_to((-0.11, 0.2), (0.0, 0.06));
    path.close_path();
    path
}

/// Звезда с `points` лучами: радиус лучей `outer`, впадин `inner`.
fn star(points: usize, outer: f64, inner: f64) -> BezPath {
    let mut path = BezPath::new();
    for i in 0..points * 2 {
        let r = if i % 2 == 0 { outer } else { inner };
        let a = FRAC_PI_2 + i as f64 * TAU / (points * 2) as f64;
        let p = (r * a.cos(), r * a.sin());
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    path.close_path();
    path
}

/// Ломаная из трёх отрезков с острыми углами: на ней видна разница соединений обводки.
fn zigzag() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((-0.15, -0.06));
    path.line_to((-0.05, 0.07));
    path.line_to((0.05, -0.06));
    path.line_to((0.15, 0.07));
    path
}

/// Волна из кубических кривых шириной 1.4.
fn wave() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((-0.7, 0.0));
    path.curve_to((-0.55, 0.12), (-0.4, 0.12), (-0.35, 0.0));
    path.curve_to((-0.3, -0.12), (-0.05, -0.12), (0.0, 0.0));
    path.curve_to((0.05, 0.12), (0.3, 0.12), (0.35, 0.0));
    path.curve_to((0.4, -0.12), (0.55, -0.12), (0.7, 0.0));
    path
}

/// Мягкий цвет радуги: оттенок `h` от 0 до 1, разбавленный белым.
fn soft_hue(h: f32) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        1.0 - (k.min(4.0 - k).clamp(0.0, 1.0))
    };
    [f(5.0), f(3.0), f(1.0)].map(|c| 0.35 + 0.65 * c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_builds_and_reuses_petal_mesh() {
        let demo = VectorDemo::new(&crate::UserFiles::default());
        // У всех фигур есть треугольники, кроме перетекающих (своя и из примера Motif):
        // они только трафаретом.
        assert!(
            demo.library
                .iter()
                .all(|(id, _, m)| (m.triangle_count() > 0)
                    != !demo.library.morph_keyframes(id).is_empty())
        );
        assert_eq!(demo.motif.len(), 3);
        let mut demo = demo;
        let before = demo.library.len();
        assert_eq!(demo.stream_items(0.0, 1.0).len(), 1);
        assert_eq!(demo.stream_items(100.0, 1.0).len(), 200);
        assert_eq!(demo.library.len(), before + 200);
        // Повторный вызов не добавляет фигур.
        demo.stream_items(100.0, 1.0);
        assert_eq!(demo.library.len(), before + 200);
        let motif = demo.motif_items(1.0, 16.0 / 9.0);
        assert_eq!(motif.len(), 3);
        assert_eq!(demo.library.method(demo.heart), Some(FillMethod::Stencil));
        assert_eq!(demo.library.morph_keyframes(demo.morph).len(), 3);
        let morphs = demo.morph_instances(1.0);
        assert_eq!(morphs.len(), MORPH_GRID.0 * MORPH_GRID.1 + 1);
        assert!(morphs.iter().all(|i| (0.0..=3.0).contains(&i.morph)));
        let instances = demo.instances(1.0);
        let petals = instances.iter().filter(|i| i.shape == demo.petal).count();
        assert_eq!(petals, PETALS);
        // Кадры отличаются только экземплярами: фигуры не пересчитываются.
        let rev = demo.library.revision();
        let _ = demo.instances(2.0);
        let _ = demo.layer_items(2.0);
        assert_eq!(demo.library.revision(), rev);
        // Сцена «слои»: скобки слоёв парные, у правого снеговика вложенный слой.
        let items = demo.layer_items(1.0);
        let mut depth = 0i32;
        let mut deepest = 0;
        for item in &items {
            match item {
                SceneItem::BeginLayer { .. } => depth += 1,
                SceneItem::EndLayer => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0);
            deepest = deepest.max(depth);
        }
        assert_eq!((depth, deepest), (0, 2));
        // Сцена «маски»: три слоя, у каждого одна маска прямо внутри.
        let items = demo.mask_items(1.0);
        let masks = items
            .iter()
            .filter(|i| matches!(i, SceneItem::BeginMask { .. }))
            .count();
        let ends = items
            .iter()
            .filter(|i| matches!(i, SceneItem::EndMask))
            .count();
        assert_eq!((masks, ends), (3, 3));
        let _ = demo.mask_items(2.0);
        assert_eq!(demo.library.revision(), rev);
    }
}
