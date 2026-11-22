//! Миниатюры моделей и сцен 3D для проводника «Ассетов» (Э.3).
//!
//! Модель снимается «общим видом» (сверху-сбоку, как на подставке) лучами на процессоре в
//! своём потоке: картинка 128×128 и сглаживание четырьмя лучами на точку. Готовая
//! миниатюра лежит в скрытой папке `.thumbs` проекта рядом со списком файлов, из которых
//! модель собрана (вложенные модели, глина). Снимок обновляется только когда один из этих
//! файлов сохранили позже снимка; пока новый считается, видна старая миниатюра.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::SystemTime;

use glam::Vec3;
use reyvec_sdf::Shape3;
use reyvec_sdf::shape_model::{ModelFile, Resolver};
use reyvec_sdf::shape3d::compile_with;
use reyvec_sdf::space_file::{MaterialEntry, SpaceFile};

use super::shapes::{child_mut, model_of};

/// Сторона миниатюры, пикселей.
pub(crate) const SIZE: u32 = 128;
/// Скрытая папка миниатюр в проекте.
const DIR: &str = ".thumbs";
/// Как часто для видимого файла проверять, не сохранили ли его, секунд.
const CHECK_S: f64 = 2.0;
/// Шагов луча не больше.
const STEPS: usize = 160;

/// Картинка миниатюры: пиксели RGBA подряд.
#[derive(Clone, Debug)]
pub(crate) struct Picture {
    pub size: u32,
    pub rgba: Vec<u8>,
}

/// Где лежит миниатюра файла: `.thumbs/<путь от проекта>.png`.
fn thumb_path(project: &Path, file: &Path) -> Option<PathBuf> {
    let rel = file.strip_prefix(project).ok()?;
    let mut p = project.join(DIR).join(rel);
    let name = format!("{}.png", p.file_name()?.to_string_lossy());
    p.set_file_name(name);
    Some(p)
}

/// Файлы, из которых собрана модель (лежат рядом с миниатюрой, по строке на файл).
fn deps_path(thumb: &Path) -> PathBuf {
    thumb.with_extension("deps")
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Миниатюра свежая: она новее файла и всех, из которых он собран.
fn fresh(file: &Path, thumb: &Path) -> bool {
    let Some(made) = modified(thumb) else {
        return false;
    };
    let deps = std::fs::read_to_string(deps_path(thumb)).unwrap_or_default();
    std::iter::once(file.to_path_buf())
        .chain(deps.lines().filter(|l| !l.is_empty()).map(PathBuf::from))
        .all(|f| modified(&f).is_none_or(|m| m <= made))
}

/// Что снимать: форма целиком, по чему ставить кадр (у сцены — без пола), материалы и
/// файлы, из которых она собрана.
struct Loaded {
    shape: Shape3,
    frame: Option<(Vec3, Vec3)>,
    materials: Vec<MaterialEntry>,
    deps: Vec<PathBuf>,
}

/// Глине нужна картинка видеокарты, иначе программа рисует вместо неё точку. Картинки
/// здесь нет, но процессору она и не нужна: номер-заглушка включает чтение сетки.
fn bricks_on_cpu(shape: &mut Shape3) {
    if let Shape3::Brick { brick } = shape {
        if brick.0.texture().is_none() {
            brick.0.set_texture(0);
        }
        return;
    }
    let mut i = 0;
    while let Some(c) = child_mut(shape, i) {
        bricks_on_cpu(c);
        i += 1;
    }
}

fn load(path: &Path) -> Result<Loaded, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let parent = path.parent().unwrap_or(Path::new("."));
    // У сцены кадр ставится по предметам без сетки (пол с сеткой огромный).
    let mut framing: Vec<usize> = Vec::new();
    let (mut shape, mut materials, dir) = if let Ok(f) = ModelFile::parse(&text) {
        // Модель из папки `models`: вложенные ссылки считаются от папки над `models`.
        let dir = model_of(path).map_or_else(|| parent.to_path_buf(), |(root, _)| root);
        (f.shape, f.materials, dir)
    } else {
        let f: SpaceFile = ron::from_str(&text).map_err(|e| e.to_string())?;
        let materials = f.materials;
        let shapes = f
            .objects
            .into_iter()
            .enumerate()
            .map(|(k, o)| {
                let index = materials
                    .iter()
                    .position(|m| m.name == o.material)
                    .unwrap_or(0);
                if !materials.get(index).is_some_and(|m| m.grid) {
                    framing.push(k);
                }
                Shape3::Move {
                    at: o.at,
                    turn: o.turn,
                    scale: 1.0,
                    shape: Box::new(o.shape),
                }
                .paint(index as u32)
            })
            .collect();
        let shape = Shape3::Union {
            shapes,
            smooth: 0.0,
        };
        (shape, materials, parent.to_path_buf())
    };
    let mut resolver = Resolver::new(Some(&dir), &mut materials);
    resolver.resolve(&mut shape)?;
    let deps = std::mem::take(&mut resolver.files);
    bricks_on_cpu(&mut shape);
    let frame = match &shape {
        Shape3::Union { shapes, .. } if !framing.is_empty() && framing.len() < shapes.len() => {
            framing
                .iter()
                .filter_map(|&k| shapes.get(k)?.bounds_box())
                .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
        }
        _ => None,
    };
    Ok(Loaded {
        shape,
        frame,
        materials,
        deps,
    })
}

/// Снять миниатюру: общий вид сверху-сбоку, прозрачный фон.
pub(crate) fn render(path: &Path) -> Result<(Picture, Vec<PathBuf>), String> {
    let Loaded {
        shape,
        frame,
        materials,
        deps,
    } = load(path)?;
    let program = compile_with(&shape, true).map_err(|e| e.to_string())?;
    let (center, radius) = match frame.or(program.bounds_box) {
        Some((lo, hi)) => ((lo + hi) * 0.5, ((hi - lo) * 0.5).length().max(0.05)),
        None => (Vec3::ZERO, 1.0),
    };
    let colors: Vec<Vec3> = materials
        .iter()
        .map(|m| Vec3::from(m.color.linear().unwrap_or([0.6; 3])))
        .collect();
    let clay = Vec3::new(0.62, 0.55, 0.50);
    // Общий вид: как «вид сверху-сбоку» подставки, модель целиком в кадре.
    let dir = Vec3::new(0.55, -0.45, -0.70).normalize();
    let fov = 30f32.to_radians();
    let dist = radius / (fov * 0.5).sin() * 1.02;
    let eye = center - dir * dist;
    let right = dir.cross(Vec3::Y).normalize();
    let up = right.cross(dir);
    let light = Vec3::new(-0.45, 0.85, 0.35).normalize();
    let fill = Vec3::new(0.6, 0.25, -0.5).normalize();
    let half = (fov * 0.5).tan();
    let n = SIZE as usize;
    let far = dist + radius * 2.0;
    let sdf = |p: Vec3| program.distance(p);
    let normal = |p: Vec3| {
        let e = radius * 0.002;
        let k = [
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(1.0, 1.0, 1.0),
        ];
        k.iter()
            .map(|&k| k * sdf(p + k * e).0)
            .sum::<Vec3>()
            .normalize_or_zero()
    };
    let mut rgba = vec![0u8; n * n * 4];
    // Четыре луча на точку: края без лесенки.
    let offsets = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
    for y in 0..n {
        for x in 0..n {
            let mut sum = Vec3::ZERO;
            let mut cover = 0.0;
            for (ox, oy) in offsets {
                let u = ((x as f32 + ox) / n as f32 * 2.0 - 1.0) * half;
                let v = (1.0 - (y as f32 + oy) / n as f32 * 2.0) * half;
                let ray = (dir + right * u + up * v).normalize();
                let mut t = (dist - radius * 1.1).max(0.0);
                let mut hit = None;
                for _ in 0..STEPS {
                    let p = eye + ray * t;
                    let (d, m) = sdf(p);
                    if d < radius * 0.0015 {
                        hit = Some((p, m));
                        break;
                    }
                    t += d.max(radius * 0.0005);
                    if t > far {
                        break;
                    }
                }
                let Some((p, m)) = hit else {
                    continue;
                };
                let nrm = normal(p);
                let base = colors.get(m as usize).copied().unwrap_or(clay);
                let key = nrm.dot(light).max(0.0);
                let back = nrm.dot(fill).max(0.0) * 0.35;
                let sky = 0.25 + 0.15 * nrm.y.max(0.0);
                // Край к камере чуть светлее: форма читается на тёмной плитке.
                let rim = (1.0 + nrm.dot(ray)).clamp(0.0, 1.0).powi(3) * 0.25;
                sum += base * (key * 0.95 + back + sky) + Vec3::splat(rim);
                cover += 1.0;
            }
            if cover == 0.0 {
                continue;
            }
            let c = sum / cover;
            let a = cover / offsets.len() as f32;
            let i = (y * n + x) * 4;
            let srgb = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
            rgba[i..i + 4].copy_from_slice(&[srgb(c.x), srgb(c.y), srgb(c.z), (a * 255.0) as u8]);
        }
    }
    let mut deps = deps;
    deps.retain(|d| d != path);
    Ok((Picture { size: SIZE, rgba }, deps))
}

fn save(thumb: &Path, pic: &Picture, deps: &[PathBuf]) -> Result<(), String> {
    if let Some(dir) = thumb.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(thumb).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), pic.size, pic.size);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&pic.rgba).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    let list: String = deps.iter().map(|d| format!("{}\n", d.display())).collect();
    std::fs::write(deps_path(thumb), list).map_err(|e| e.to_string())
}

fn read_png(path: &Path) -> Option<Picture> {
    let file = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.width != info.height {
        return None;
    }
    buf.truncate(info.buffer_size());
    Some(Picture {
        size: info.width,
        rgba: buf,
    })
}

/// Работа для потока: прочитать готовую миниатюру или снять новую.
enum Job {
    /// Прочитать с диска (свежая или старая, пока считается новая).
    Read(PathBuf, PathBuf),
    /// Снять заново и записать.
    Render(PathBuf, PathBuf),
}

/// Миниатюры проводника: просит поток снять или прочитать, отдаёт готовые текстуры.
pub(crate) struct Thumbs {
    project: Option<PathBuf>,
    jobs: Sender<Job>,
    done: Receiver<(PathBuf, Option<Picture>)>,
    textures: HashMap<PathBuf, egui::TextureHandle>,
    /// Уже попросили у потока.
    asked: HashSet<PathBuf>,
    /// Когда последний раз смотрели, не сохранили ли файл.
    checked: HashMap<PathBuf, f64>,
}

impl Thumbs {
    pub(crate) fn new(project: Option<&Path>) -> Self {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel();
        // Один поток на все миниатюры: редактор не тормозит, процессор не занят целиком.
        let spawned = std::thread::Builder::new()
            .name("миниатюры".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let (file, pic) = match job {
                        Job::Read(file, thumb) => {
                            let pic = read_png(&thumb);
                            (file, pic)
                        }
                        Job::Render(file, thumb) => match render(&file) {
                            Ok((pic, deps)) => {
                                if let Err(e) = save(&thumb, &pic, &deps) {
                                    tracing::warn!(
                                        "Миниатюра «{}» не записалась: {e}",
                                        thumb.display()
                                    );
                                }
                                (file, Some(pic))
                            }
                            Err(e) => {
                                tracing::debug!("Миниатюра «{}» не снята: {e}", file.display());
                                (file, None)
                            }
                        },
                    };
                    if tx.send((file, pic)).is_err() {
                        break;
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("Поток миниатюр не запустился: {e}");
        }
        Self {
            project: project.map(Path::to_path_buf),
            jobs,
            done,
            textures: HashMap::new(),
            asked: HashSet::new(),
            checked: HashMap::new(),
        }
    }

    /// Забрать готовые картинки из потока.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((file, pic)) = self.done.try_recv() {
            self.asked.remove(&file);
            if let Some(pic) = pic {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [pic.size as usize, pic.size as usize],
                    &pic.rgba,
                );
                let name = format!("миниатюра {}", file.display());
                let tex = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
                self.textures.insert(file, tex);
                ctx.request_repaint();
            }
        }
    }

    /// Миниатюра файла, если она есть; заодно просит снять новую, если файл сохранили.
    pub(crate) fn get(&mut self, file: &Path, now: f64) -> Option<egui::TextureId> {
        let project = self.project.as_ref()?;
        let have = self.textures.get(file).map(egui::TextureHandle::id);
        let due = self.checked.get(file).is_none_or(|t| now - t > CHECK_S);
        if due && !self.asked.contains(file) {
            self.checked.insert(file.to_path_buf(), now);
            if let Some(thumb) = thumb_path(project, file) {
                let is_fresh = fresh(file, &thumb);
                let job = if !is_fresh {
                    // Пока снимается новая, видна старая (если она была).
                    if have.is_none() && thumb.is_file() {
                        let _ = self.jobs.send(Job::Read(file.to_path_buf(), thumb.clone()));
                    }
                    Some(Job::Render(file.to_path_buf(), thumb))
                } else if have.is_none() {
                    Some(Job::Read(file.to_path_buf(), thumb))
                } else {
                    None
                };
                if let Some(job) = job {
                    self.asked.insert(file.to_path_buf());
                    let _ = self.jobs.send(job);
                }
            }
        }
        have
    }

    /// Файл переименовали или перенесли: старую картинку забыть.
    pub(crate) fn forget(&mut self) {
        self.textures.clear();
        self.checked.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Стеллаж склада снимается: модель в середине кадра, по краям пусто.
    #[test]
    fn rack_thumbnail_has_model_in_middle() {
        let file = project().join("scenes/3d/models/склад/стеллаж.shape.ron");
        let (pic, _) = render(&file).unwrap();
        let n = pic.size as usize;
        let alpha = |x: usize, y: usize| pic.rgba[(y * n + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "угол пустой");
        let covered = (0..n * n).filter(|i| pic.rgba[i * 4 + 3] > 0).count();
        assert!(covered > n * n / 10, "модель видна: {covered} точек");
        assert!(alpha(n / 2, n / 2) > 0 || covered > n * n / 4);
    }

    /// Модель с вложенными моделями и глиной помнит, из чего собрана.
    #[test]
    fn forge_billet_lists_clay_and_scene_renders() {
        let p = project();
        let billet = p.join("scenes/3d/models/кузница/заготовка.shape.ron");
        let (_, deps) = render(&billet).unwrap();
        assert!(
            deps.iter().any(|d| d.to_string_lossy().ends_with(".clay")),
            "{deps:?}"
        );
        let (pic, deps) = render(&p.join("scenes/3d/forge.ron")).unwrap();
        assert!(pic.rgba.chunks(4).any(|c| c[3] > 0));
        assert!(!deps.is_empty());
    }

    /// Миниатюры для просмотра глазами: `target/editor-preview/thumb-*.png`.
    #[test]
    #[ignore = "пишет картинки для просмотра глазами"]
    fn thumbs_preview() {
        let p = project();
        let out = p.join("target/editor-preview");
        for (name, file) in [
            ("стеллаж", "scenes/3d/models/склад/стеллаж.shape.ron"),
            ("конвейер", "scenes/3d/models/завод/конвейер.shape.ron"),
            ("заготовка", "scenes/3d/models/кузница/заготовка.shape.ron"),
            (
                "наковальня",
                "scenes/3d/models/кузница/наковальня.shape.ron",
            ),
            ("кузница", "scenes/3d/forge.ron"),
        ] {
            let started = std::time::Instant::now();
            let (pic, deps) = render(&p.join(file)).unwrap();
            println!("{name}: {:?}", started.elapsed());
            save(&out.join(format!("thumb-{name}.png")), &pic, &deps).unwrap();
        }
    }

    #[test]
    fn saved_thumbnail_is_fresh_until_file_changes() {
        let root = std::env::temp_dir().join(format!("reyvec-thumbs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let model = root.join("assets/куб.shape.ron");
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::write(
            &model,
            "(format: \"reyvec-shape\", version: 1, shape: Box(size: (1.0, 1.0, 1.0)))",
        )
        .unwrap();
        let thumb = thumb_path(&root, &model).unwrap();
        assert!(thumb.starts_with(root.join(DIR)));
        assert!(!fresh(&model, &thumb));
        let (pic, deps) = render(&model).unwrap();
        save(&thumb, &pic, &deps).unwrap();
        assert!(fresh(&model, &thumb));
        let back = read_png(&thumb).unwrap();
        assert_eq!((back.size, back.rgba.len()), (SIZE, pic.rgba.len()));
        // Сохранили модель позже снимка — миниатюра устарела.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        let f = std::fs::File::options().write(true).open(&model).unwrap();
        f.set_modified(later).unwrap();
        assert!(!fresh(&model, &thumb));
        let _ = std::fs::remove_dir_all(&root);
    }
}
