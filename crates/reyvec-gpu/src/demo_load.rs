//! Загрузка в фоне в песочнице (этап 5.4): файл сцены, уровни детализации и проверочная
//! «картина» сцены «из файла».
//!
//! Картина — нарочно тяжёлый рисунок SVG (мозаика из нескольких тысяч фигур), который
//! каждый раз рисуется заново, чтобы его нельзя было взять готовым. Клавиша U грузит
//! её в фоне, Shift+U — по-старому, прямо в кадре: на графике времени кадра видно
//! разницу.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::Instant;

use reyvec_scene::{LoadJob, LoadResult, Loader, ShapeCache};
use reyvec_svg::{SvgDrawing, SvgOptions, load_svg};
use reyvec_vector::{Affine, SceneItem, ShapeId, ShapeLibrary};

/// Где висит картина в сцене «из файла»: центр и высота, единицы сцены.
const PICTURE_AT: (f64, f64) = (-1.2, 0.5);
const PICTURE_HEIGHT: f64 = 0.62;
/// Мозаика картины: клеток по ширине и высоте.
const MOSAIC: (usize, usize) = (72, 54);

/// Что показывает строка «Загрузка» на панели.
#[derive(Clone, Debug, Default)]
pub struct LoadStats {
    /// Грузится ли в фоне (в окне да, в картинках тестов нет).
    pub background: bool,
    /// Заданий в работе.
    pub pending: usize,
    /// Последняя законченная загрузка файла или рисунка (картина, файл сцены).
    pub last: Option<LastLoad>,
    /// Последняя пачка уровней детализации (своя строка: они догружаются после
    /// каждой картины и при приближении камеры и не должны прятать цифры картины).
    pub details: Option<LastLoad>,
    /// Сколько фигур в наборе и сколько они занимают на видеокарте (5.5б).
    pub memory: ShapeMemory,
}

/// Фигуры в наборе и их место на видеокарте: видно, что ненужные удаляются (5.5б).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShapeMemory {
    /// Фигур в наборе сейчас.
    pub live: usize,
    /// Удалено за всё время.
    pub removed: usize,
    /// Место на видеокарте.
    pub gpu: crate::vector::MeshMemory,
}

/// Последняя законченная загрузка.
#[derive(Clone, Debug)]
pub struct LastLoad {
    /// Что грузили: «картина», «файл сцены», «уровни детализации».
    pub what: &'static str,
    /// Сколько фигур нарезано.
    pub shapes: usize,
    /// Сколько работа шла в фоновом потоке, мс (0, если грузили в кадре).
    pub background_ms: f64,
    /// Сколько из неё досталось потоку кадра, мс.
    pub frame_ms: f64,
}

/// Чем закончилась загрузка файла сцены.
pub(crate) enum SceneLoad {
    /// Готов мир сцены.
    World(Box<reyvec_scene::SceneWorld>),
    /// Ошибка в файле.
    Error(String),
}

/// Загрузка песочницы.
#[derive(Default)]
pub(crate) struct DemoLoading {
    loader: Option<Loader>,
    /// Последнее задание файла сцены: результаты старых сохранений не нужны.
    scene_job: Option<u64>,
    picture_job: Option<u64>,
    /// Уровни детализации, которые сейчас режутся в фоне.
    details: HashSet<(ShapeId, i32)>,
    picture: Option<SvgDrawing>,
    pictures: u32,
    stats: LoadStats,
}

impl DemoLoading {
    /// Включает поток загрузки (окно). Без него всё грузится прямо в кадре.
    pub fn start(&mut self, library: &ShapeLibrary) {
        if self.loader.is_none() {
            self.loader = Some(Loader::start(library.tessellator()));
            self.stats.background = true;
            tracing::info!("Загрузка: свой поток (файл сцены, картина, уровни детализации)");
        }
    }

    pub fn is_background(&self) -> bool {
        self.loader.is_some()
    }

    pub fn stats(&self) -> LoadStats {
        LoadStats {
            pending: self.loader.as_ref().map_or(0, Loader::pending),
            ..self.stats.clone()
        }
    }

    /// Картина для сцены «из файла», если загружена.
    pub fn picture_items(&self) -> impl Iterator<Item = SceneItem> + '_ {
        self.picture
            .iter()
            .flat_map(|d| d.placed(Affine::translate(PICTURE_AT)))
    }

    /// Отдаёт файл сцены в фон. `false`, если фона нет (грузить в кадре).
    pub fn submit_scene(
        &mut self,
        text: &str,
        dir: Option<&std::path::Path>,
        cache: &ShapeCache,
    ) -> bool {
        let Some(loader) = self.loader.as_mut() else {
            return false;
        };
        self.scene_job = Some(loader.submit(LoadJob::Scene {
            text: text.to_owned(),
            dir: dir.map(std::path::Path::to_path_buf),
            known: cache.keys(),
        }));
        true
    }

    /// Новая картина вместо старой: фигуры старой (с уровнями детализации) удаляются из
    /// набора (5.5б), иначе каждая картина навсегда оставалась бы в памяти.
    fn replace_picture(&mut self, library: &mut ShapeLibrary, drawing: SvgDrawing) {
        if let Some(old) = self.picture.replace(drawing) {
            let removed: usize = old.shapes().map(|id| library.remove(id)).sum();
            tracing::debug!("старая картина удалена: {removed} фигур");
        }
    }

    /// Новая картина: в фоне (`background`, если есть поток) или прямо здесь.
    pub fn load_picture(&mut self, library: &mut ShapeLibrary, background: bool) {
        self.pictures += 1;
        let svg = picture_svg(self.pictures);
        let options = SvgOptions {
            height: PICTURE_HEIGHT,
        };
        match self.loader.as_mut() {
            Some(loader) if background => {
                self.picture_job = Some(loader.submit(LoadJob::Svg {
                    name: "картина".into(),
                    data: svg.into_bytes().into(),
                    options,
                }));
            }
            _ => {
                let _zone = crate::cpu_zone!("картина в кадре");
                let started = Instant::now();
                match load_svg(svg.as_bytes(), library, options) {
                    Ok(d) => {
                        let ms = started.elapsed().as_secs_f64() * 1000.0;
                        tracing::info!(
                            "Картина №{} в кадре: {} фигур, {ms:.1} мс",
                            self.pictures,
                            d.shape_count
                        );
                        self.stats.last = Some(LastLoad {
                            what: "картина в кадре",
                            shapes: d.shape_count,
                            background_ms: 0.0,
                            frame_ms: ms,
                        });
                        self.replace_picture(library, d);
                    }
                    Err(e) => tracing::warn!("картина: {e}"),
                }
            }
        }
    }

    /// Уровни детализации, которых не хватило кадру, — в фон (те, что уже режутся,
    /// второй раз не отдаются). `false`, если фона нет.
    pub fn submit_details(&mut self, library: &ShapeLibrary, requests: &[(ShapeId, i32)]) -> bool {
        let Some(loader) = self.loader.as_mut() else {
            return false;
        };
        let jobs: Vec<_> = requests
            .iter()
            .filter(|r| self.details.insert(**r))
            .filter_map(|&(id, level)| library.detail_job(id, level).ok())
            .collect();
        if !jobs.is_empty() {
            loader.submit(LoadJob::Details(jobs));
        }
        true
    }

    /// Забирает всё готовое из фона и кладёт в набор. Возвращает загруженный файл
    /// сцены, если он пришёл.
    pub fn poll(
        &mut self,
        library: &mut ShapeLibrary,
        cache: &mut ShapeCache,
    ) -> Option<SceneLoad> {
        let mut scene = None;
        while let Some(done) = self.loader.as_mut().and_then(Loader::poll) {
            let started = Instant::now();
            let background_ms = done.took.as_secs_f64() * 1000.0;
            let (what, shapes) = match done.result {
                LoadResult::Scene(_) if Some(done.job) != self.scene_job => continue,
                LoadResult::Scene(Ok(prepared)) => {
                    let shapes = prepared.new_shapes();
                    scene = Some(match prepared.finish(library, cache) {
                        Ok(world) => SceneLoad::World(Box::new(world)),
                        Err(e) => SceneLoad::Error(e.to_string()),
                    });
                    ("файл сцены", shapes)
                }
                LoadResult::Scene(Err(e)) => {
                    scene = Some(SceneLoad::Error(e.to_string()));
                    continue;
                }
                LoadResult::Svg { result, .. } if Some(done.job) == self.picture_job => {
                    match result {
                        Ok((mut drawing, batch)) => {
                            let first = library.append(batch);
                            drawing.offset_shapes(first);
                            let shapes = drawing.shape_count;
                            self.replace_picture(library, drawing);
                            ("картина", shapes)
                        }
                        Err(e) => {
                            tracing::warn!("картина: {e}");
                            continue;
                        }
                    }
                }
                LoadResult::Svg { .. } | LoadResult::Motif { .. } => continue,
                LoadResult::Details(details) => {
                    let shapes = details.len();
                    for d in details {
                        // Если фигуру, пока резался уровень, удалили, уровень не нужен.
                        self.details.remove(&(d.id(), d.level()));
                        library.insert_detail(d);
                    }
                    ("уровни детализации", shapes)
                }
            };
            let frame_ms = started.elapsed().as_secs_f64() * 1000.0;
            tracing::debug!(
                "Загружено в фоне: {what}, фигур {shapes}, в фоне {background_ms:.2} мс, в кадре {frame_ms:.3} мс"
            );
            let load = LastLoad {
                what,
                shapes,
                background_ms,
                frame_ms,
            };
            if what == "уровни детализации" {
                self.stats.details = Some(load);
            } else {
                tracing::info!(
                    "Загружено в фоне: {what}, фигур {shapes}, в фоне {background_ms:.1} мс, в кадре {frame_ms:.2} мс"
                );
                self.stats.last = Some(load);
            }
        }
        scene
    }
}

/// Псевдослучайное число от 0 до 1 для клетки `i` (одно и то же для одних и тех же чисел).
fn noise(i: u64, salt: u64) -> f64 {
    let mut x = i
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(salt.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    x ^= x >> 31;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// Тяжёлая картина `n`: закат над морем мозаикой из кривых камешков. Каждая картина своя
/// (камешки другой формы), поэтому нарезать её приходится заново.
pub(crate) fn picture_svg(n: u32) -> String {
    let (cols, rows) = MOSAIC;
    let (w, h) = (cols as f64 * 10.0, rows as f64 * 10.0);
    let mut s = String::with_capacity(cols * rows * 160);
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"##
    );
    let _ = write!(s, r##"<rect width="{w}" height="{h}" fill="#20181c"/>"##);
    let salt = u64::from(n);
    for r in 0..rows {
        for c in 0..cols {
            let i = (r * cols + c) as u64;
            let (cx, cy) = (c as f64 * 10.0 + 5.0, r as f64 * 10.0 + 5.0);
            let horizon = rows as f64 * 0.55;
            let y = r as f64;
            // Небо: от фиолетового сверху к оранжевому у горизонта; солнце; море.
            let sun = ((c as f64 - cols as f64 * 0.62).powi(2) + (y - horizon + 6.0).powi(2))
                .sqrt()
                < 7.0;
            let (hue, sat, val) = if sun {
                (45.0, 0.85, 1.0)
            } else if y < horizon {
                let t = y / horizon;
                (280.0 + 100.0 * t, 0.55 + 0.3 * t, 0.45 + 0.5 * t)
            } else {
                let t = (y - horizon) / (rows as f64 - horizon);
                (200.0 + 20.0 * t, 0.7, 0.75 - 0.45 * t)
            };
            let jitter = noise(i, salt) * 16.0 - 8.0;
            let (red, green, blue) = hsv((hue + jitter).rem_euclid(360.0), sat, val);
            let _ = write!(s, r##"<path fill="#{red:02x}{green:02x}{blue:02x}" d=""##);
            // Камешек: шесть точек по кругу со своим радиусом, гладкие кубические кривые.
            let points: Vec<(f64, f64)> = (0..6)
                .map(|k| {
                    let a =
                        (k as f64 + noise(i * 6 + k, salt + 1) * 0.5) * std::f64::consts::TAU / 6.0;
                    let rad = 3.2 + 1.6 * noise(i * 6 + k, salt + 2);
                    (cx + rad * a.cos(), cy + rad * a.sin())
                })
                .collect();
            let mid = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            let start = mid(points[5], points[0]);
            let _ = write!(s, "M{:.2} {:.2}", start.0, start.1);
            for k in 0..6 {
                let p = points[k];
                let end = mid(p, points[(k + 1) % 6]);
                let _ = write!(s, "Q{:.2} {:.2} {:.2} {:.2}", p.0, p.1, end.0, end.1);
            }
            s.push_str(r#"Z"/>"#);
        }
    }
    // Рама.
    let _ = write!(
        s,
        r##"<rect x="2" y="2" width="{}" height="{}" fill="none" stroke="#5a3a22" stroke-width="5"/></svg>"##,
        w - 4.0,
        h - 4.0
    );
    s
}

fn hsv(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let byte = |t: f64| ((t + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (byte(r), byte(g), byte(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_is_heavy_and_new_every_time() {
        let a = picture_svg(1);
        assert_ne!(a, picture_svg(2));
        let mut lib = ShapeLibrary::new(0.0005);
        let started = Instant::now();
        let d = load_svg(
            a.as_bytes(),
            &mut lib,
            SvgOptions {
                height: PICTURE_HEIGHT,
            },
        )
        .unwrap();
        eprintln!(
            "картина: {} фигур, {:.1} мс",
            d.shape_count,
            started.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(d.shape_count, MOSAIC.0 * MOSAIC.1 + 2);
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    }

    #[test]
    fn background_loading_lands_in_the_library() {
        let mut lib = ShapeLibrary::new(0.0005);
        let mut cache = ShapeCache::new();
        let mut loading = DemoLoading::default();
        loading.start(&lib);
        loading.load_picture(&mut lib, true);
        assert!(lib.is_empty(), "в фоне набор не трогается");
        assert!(loading.submit_scene(
            reyvec_scene::EXAMPLE,
            Some(std::path::Path::new(reyvec_scene::EXAMPLE_DIR)),
            &cache
        ));
        let mut scene = None;
        let started = Instant::now();
        while loading.stats().pending > 0 || scene.is_none() {
            if let Some(s) = loading.poll(&mut lib, &mut cache) {
                scene = Some(s);
            }
            assert!(started.elapsed().as_secs() < 30, "загрузка зависла");
            std::thread::yield_now();
        }
        assert!(matches!(scene, Some(SceneLoad::World(_))));
        let items: Vec<_> = loading.picture_items().collect();
        assert_eq!(items.len(), MOSAIC.0 * MOSAIC.1 + 2);
        for item in items {
            if let SceneItem::Shape(s) = item {
                assert!(lib.mesh(s.shape).is_some());
            }
        }
        assert!(!cache.is_empty());
    }
}
