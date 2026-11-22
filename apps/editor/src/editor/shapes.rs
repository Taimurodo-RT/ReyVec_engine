//! «Формы» (М.2, `docs/models-3d.md`): модель формы `models/…/<имя>.shape.ron` на
//! подставке. Двойной щелчок по файлу модели в «Ассетах» открывает её вместо сцены: движок
//! рисует сцену «3D», которую собирает этот модуль (пол, круглая подставка, две лампы и
//! сама модель через `Use`). Камера ходит вокруг модели: мышь поворачивает, колесо
//! приближает, без мыши модель медленно вращается. Сохранили файл модели (в блокноте
//! или другой программе) — подставка перечитывает его сама.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use reyvec_sdf::{Camera3D, SpaceScene};

/// Окончание файла модели формы.
pub(crate) const EXTENSION: &str = ".shape.ron";
/// Как часто смотреть, не сохранили ли файл модели.
const CHECK_EVERY: Duration = Duration::from_millis(300);
/// Скорость вращения на подставке, радиан в секунду (полный оборот за ~18 с).
const SPIN: f32 = 0.35;
/// Наклон камеры при открытии: чуть сверху.
const HOME_PITCH: f32 = 0.35;
/// Поворот камеры при открытии: спереди и чуть справа.
const HOME_YAW: f32 = 0.6;
/// Наклон не доходит до вертикали (как у камеры движка) и не уходит под пол.
const PITCH_RANGE: (f32, f32) = (-0.15, 1.45);
/// Радиан поворота на точку перетаскивания мышью.
const ORBIT_PER_POINT: f32 = 0.008;

/// Файл модели формы.
pub(crate) fn is_shape(name: &str) -> bool {
    name.ends_with(EXTENSION)
}

/// Где искать модель и как её назвать в `Use`: папка, в которой лежит `models`, и путь
/// внутри `models` без окончания (`склад/стеллаж`). `None` — файл не в папке `models`.
pub(crate) fn model_of(path: &Path) -> Option<(PathBuf, String)> {
    let file = path.file_name()?.to_str()?;
    let stem = file.strip_suffix(EXTENSION)?;
    let mut parts = vec![stem.to_owned()];
    let mut dir = path.parent()?;
    loop {
        let name = dir.file_name()?.to_str()?;
        if name == "models" {
            parts.reverse();
            return Some((dir.parent()?.to_owned(), parts.join("/")));
        }
        parts.push(name.to_owned());
        dir = dir.parent()?;
    }
}

/// Открытая на подставке модель.
pub(crate) struct ShapeView {
    /// Файл модели.
    pub path: PathBuf,
    /// Имя модели в `Use` (`склад/стеллаж`).
    pub name: String,
    /// Папка, где лежит `models`.
    pub root: PathBuf,
    /// Время записи файла при последнем чтении.
    modified: Option<SystemTime>,
    checked: Instant,
    /// Камера: поворот вокруг вертикали, наклон, расстояние до середины модели.
    yaw: f32,
    pitch: f32,
    distance: f32,
    /// Середина модели на подставке и радиус шара вокруг неё.
    target: [f32; 3],
    radius: f32,
    /// Модель вращается сама.
    pub spin: bool,
    last: Instant,
    /// Ошибка в файле модели (тогда на подставке остаётся прошлая).
    pub error: Option<String>,
    /// Модель ещё ни разу не прочиталась: камера встанет по ней, когда прочитается.
    framed: bool,
    /// Текст файла модели при последнем чтении или записи (М.3: из него дерево частей).
    pub text: String,
}

impl ShapeView {
    /// Модель из файла `path`. Ошибка — файл не в папке `models`.
    pub(crate) fn open(path: PathBuf) -> Result<Self, String> {
        let (root, name) = model_of(&path).ok_or_else(|| {
            format!(
                "{} — не модель формы: файл *.shape.ron должен лежать в папке models",
                path.display()
            )
        })?;
        Ok(Self {
            path,
            name,
            root,
            modified: None,
            checked: Instant::now(),
            yaw: HOME_YAW,
            pitch: HOME_PITCH,
            distance: 3.0,
            target: [0.0, 0.5, 0.0],
            radius: 1.0,
            spin: true,
            last: Instant::now(),
            error: None,
            framed: false,
            text: String::new(),
        })
    }

    /// Читает модель и собирает сцену подставки. `None` — в файле ошибка
    /// ([`ShapeView::error`]), движку отдавать нечего.
    pub(crate) fn load(&mut self) -> Option<String> {
        self.modified = modified(&self.path);
        match std::fs::read_to_string(&self.path) {
            Ok(t) => self.text = t,
            Err(e) => {
                self.error = Some(format!("не прочитать файл: {e}"));
                return None;
            }
        }
        self.checked = Instant::now();
        // Сначала модель в начале координат: узнать, какого она размера.
        let probe = stand_text(&self.name, [0.0; 3], 1.0, None);
        let scene = match SpaceScene::parse_in(&probe, Some(&self.root)) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e);
                return None;
            }
        };
        let frame = scene.frame(0.0);
        let Some((lo, hi)) = frame.shapes.last().and_then(|s| s.bounds_box()) else {
            self.error = Some("модель бесконечная (плоскость): на подставку не встаёт".into());
            return None;
        };
        let size = hi - lo;
        // Модель встаёт серединой на ось подставки, низом на её верх.
        let shift = [-(lo.x + hi.x) * 0.5, -lo.y, -(lo.z + hi.z) * 0.5];
        let radius = (size * 0.5).length().max(0.05);
        let stand = (size.x.max(size.z) * 0.5 * 1.15).max(0.15);
        self.target = [0.0, size.y * 0.5, 0.0];
        if !self.framed || (radius - self.radius).abs() > self.radius * 0.5 {
            // Камера по размеру: модель целиком в кадре с полями (угол обзора 60°).
            self.distance = radius * 2.4;
            self.framed = true;
        }
        self.radius = radius;
        self.error = None;
        Some(stand_text(&self.name, shift, stand, Some(radius)))
    }

    /// Записать новый текст модели (правка из редактора, М.3) и перечитать её; ответ —
    /// как у [`ShapeView::load`]. Не записалось — текст остаётся прежним, ошибка в
    /// [`ShapeView::error`].
    pub(crate) fn write(&mut self, text: &str) -> Option<String> {
        if let Err(e) = std::fs::write(&self.path, text) {
            self.error = Some(format!("не записать файл: {e}"));
            return None;
        }
        self.load()
    }

    /// Сохранили ли файл модели с прошлого чтения (смотрит не чаще [`CHECK_EVERY`]).
    pub(crate) fn changed(&mut self) -> bool {
        if self.checked.elapsed() < CHECK_EVERY {
            return false;
        }
        self.checked = Instant::now();
        modified(&self.path) != self.modified
    }

    /// Камера этого кадра (с вращением подставки).
    pub(crate) fn camera(&mut self) -> Camera3D {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        if self.spin {
            self.yaw = (self.yaw + SPIN * dt) % std::f32::consts::TAU;
        }
        let [x, y, z] = self.target;
        let target = glam::Vec3::new(x, y, z);
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let eye = target + glam::Vec3::new(sy * cp, sp, cy * cp) * self.distance;
        let mut camera = Camera3D::looking_at(eye, target);
        camera.near = (self.distance * 0.01).max(0.005);
        camera.far = (self.distance * 20.0).max(50.0);
        camera
    }

    /// Повернуть камеру мышью на `dx`, `dy` точек интерфейса. Вращение подставки
    /// останавливается: модель смотрят с этой стороны.
    pub(crate) fn orbit(&mut self, dx: f32, dy: f32) {
        self.spin = false;
        self.yaw -= dx * ORBIT_PER_POINT;
        self.pitch = (self.pitch + dy * ORBIT_PER_POINT).clamp(PITCH_RANGE.0, PITCH_RANGE.1);
    }

    /// Приблизить (`steps` > 0) или отдалить колесом.
    pub(crate) fn zoom(&mut self, steps: f32) {
        let r = self.radius;
        self.distance = (self.distance * 0.85_f32.powf(steps)).clamp(r * 0.6, r * 12.0);
    }

    /// Камера как при открытии, вращение снова идёт.
    pub(crate) fn home(&mut self) {
        self.yaw = HOME_YAW;
        self.pitch = HOME_PITCH;
        self.distance = self.radius * 2.4;
        self.spin = true;
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Текст сцены подставки: пол, круглая подставка радиусом `stand`, модель `name`,
/// сдвинутая на `shift`, и лампы по размеру модели `radius` (нет — для замера размера,
/// без ламп).
fn stand_text(name: &str, shift: [f32; 3], stand: f32, radius: Option<f32>) -> String {
    let lamps = radius.map_or_else(String::new, |r| {
        let r = r.max(0.3);
        let key = [r * 1.6, r * 2.6, r * 2.0];
        let fill = [-r * 2.2, r * 1.4, -r * 1.2];
        format!(
            "(at: ({:.4}, {:.4}, {:.4}), color: (1.0, 0.95, 0.88), power: 6.0, range: {:.4}),\n        \
             (at: ({:.4}, {:.4}, {:.4}), color: (0.75, 0.85, 1.0), power: 2.5, range: {:.4}),",
            key[0],
            key[1],
            key[2],
            r * 8.0,
            fill[0],
            fill[1],
            fill[2],
            r * 7.0,
        )
    });
    let [x, y, z] = shift;
    format!(
        r#"// Подставка «Форм» редактора (М.2): собрана редактором, в файл не пишется.
(
    format: "reyvec-space",
    version: 1,
    materials: [
        (name: "пол", color: (0.26, 0.27, 0.29), grid: true),
        (name: "подставка", color: (0.42, 0.43, 0.46)),
        (name: "форма", color: (0.80, 0.80, 0.78)),
    ],
    lamps: [
        {lamps}
    ],
    objects: [
        (name: "пол", material: "пол", shape: Plane(normal: (0.0, 1.0, 0.0), offset: -0.03)),
        (name: "подставка", material: "подставка", at: (0.0, -0.015, 0.0),
            shape: Cylinder(height: 0.03, radius: {stand:.4}, round: 0.005)),
        (name: "модель", material: "форма", at: ({x:.4}, {y:.4}, {z:.4}),
            shape: Use(model: "{name}")),
    ],
)
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenes() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenes/3d")
    }

    #[test]
    fn model_name_comes_from_the_models_folder() {
        let (root, name) =
            model_of(Path::new("/p/scenes/3d/models/склад/стеллаж.shape.ron")).expect("модель");
        assert_eq!(root, Path::new("/p/scenes/3d"));
        assert_eq!(name, "склад/стеллаж");
        assert_eq!(
            model_of(Path::new("/p/models/шар.shape.ron")).map(|m| m.1),
            Some("шар".into())
        );
        assert!(model_of(Path::new("/p/scenes/шар.shape.ron")).is_none());
        assert!(model_of(Path::new("/p/models/шар.ron")).is_none());
        assert!(ShapeView::open("/p/шар.shape.ron".into()).is_err());
    }

    #[test]
    fn rack_stands_on_the_stand() {
        let path = scenes().join("models/склад/стеллаж.shape.ron");
        let mut view = ShapeView::open(path).expect("модель");
        assert_eq!(view.name, "склад/стеллаж");
        let text = view.load().expect("сцена подставки");
        assert!(view.error.is_none());
        let scene = SpaceScene::parse_in(&text, Some(&view.root)).expect("сцена читается");
        let frame = scene.frame(0.0);
        let (lo, hi) = frame.shapes.last().and_then(|s| s.bounds_box()).unwrap();
        // Низ модели на верху подставки, середина на её оси.
        assert!(lo.y.abs() < 1e-3, "{lo}");
        assert!((lo.x + hi.x).abs() < 1e-3 && (lo.z + hi.z).abs() < 1e-3);
        assert_eq!(frame.lamps.len(), 2);
        // Камера видит модель целиком: дальше её радиуса и смотрит на середину.
        let camera = view.camera();
        let target = glam::Vec3::from(view.target);
        assert!((camera.eye - target).length() > view.radius * 2.0);
        assert!((camera.forward() - (target - camera.eye).normalize()).length() < 1e-3);
        assert!(!view.changed(), "файл не меняли");
    }

    #[test]
    fn orbit_stops_spin_and_zoom_is_limited() {
        let mut view = ShapeView::open(scenes().join("models/кузница/молот.shape.ron")).unwrap();
        view.load().expect("молот читается");
        assert!(view.spin);
        view.orbit(100.0, 1000.0);
        assert!(!view.spin);
        assert!(view.pitch <= PITCH_RANGE.1);
        for _ in 0..100 {
            view.zoom(1.0);
        }
        assert!(view.distance >= view.radius * 0.6 - 1e-4);
        view.home();
        assert!(view.spin && (view.pitch - HOME_PITCH).abs() < 1e-6);
    }

    #[test]
    fn broken_model_reports_error() {
        let dir = std::env::temp_dir().join(format!("reyvec-shapes-{}", std::process::id()));
        let models = dir.join("models");
        std::fs::create_dir_all(&models).unwrap();
        let file = models.join("сломан.shape.ron");
        std::fs::write(&file, "(format: \"reyvec-shape\", version: 1, shape: Шар)").unwrap();
        let mut view = ShapeView::open(file).unwrap();
        assert!(view.load().is_none());
        assert!(view.error.as_deref().is_some_and(|e| !e.is_empty()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
