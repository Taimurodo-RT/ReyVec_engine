//! «Формы» (М.2, `docs/models-3d.md`): модель формы `models/…/<имя>.shape.ron` на
//! подставке. Двойной щелчок по файлу модели в «Ассетах» открывает её вместо сцены: движок
//! рисует сцену «3D», которую собирает этот модуль (пол, круглая подставка, две лампы и
//! сама модель через `Use`). Камера ходит вокруг модели: мышь поворачивает, колесо
//! приближает, без мыши модель медленно вращается. Есть готовые виды (спереди, сбоку,
//! сверху), показ одной детали крупно и свободный полёт клавишами. Сохранили файл
//! модели (в блокноте или другой программе) — подставка перечитывает его сама.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use glam::Vec3;
use reyvec_sdf::shape_model::{ModelFile, Resolver};
use reyvec_sdf::space_file::SpaceFile;
use reyvec_sdf::{Camera3D, Shape3, SpaceScene};

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
/// Наклон вокруг модели не доходит до вертикали (как у камеры движка) и не уходит под
/// пол.
const PITCH_RANGE: (f32, f32) = (-0.15, 1.5);
/// В полёте можно смотреть и снизу вверх.
const FREE_PITCH: f32 = 1.5;
/// Радиан поворота на точку перетаскивания мышью.
const ORBIT_PER_POINT: f32 = 0.008;
/// Сдвиг середины на точку перетаскивания, в долях расстояния до неё.
const PAN_PER_POINT: f32 = 0.0016;
/// Сцена 3D (М.8): поворот камеры на точку мыши — спокойнее, чем у модели.
const SCENE_TURN: f32 = 0.004;
/// Сцена 3D: скорость полёта клавишами, метров в секунду (Shift — втрое).
const SCENE_FLY: f32 = 3.0;
/// Сцена 3D: шаг колеса в полёте, метров.
const SCENE_WHEEL: f32 = 0.6;
/// Во сколько раз дальше радиуса шара вокруг модели (или детали) встаёт камера, чтобы
/// всё было в кадре с полями (угол обзора 60°).
const FRAME: f32 = 2.4;
/// Как быстро камера доезжает до нового вида: доля пути за секунду в показателе.
const EASE: f32 = 12.0;

/// Готовый вид на модель: имя для подсказки, поворот и наклон камеры.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShapeAngle {
    Front,
    Back,
    Left,
    Right,
    Top,
    /// Как при открытии: спереди-справа и чуть сверху.
    Overview,
}

impl ShapeAngle {
    pub(crate) const ALL: [Self; 6] = [
        Self::Overview,
        Self::Front,
        Self::Back,
        Self::Left,
        Self::Right,
        Self::Top,
    ];

    /// Подпись при наведении.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Front => "Спереди",
            Self::Back => "Сзади",
            Self::Left => "Слева",
            Self::Right => "Справа",
            Self::Top => "Сверху",
            Self::Overview => "Общий вид (как при открытии)",
        }
    }

    /// Поворот и наклон камеры. Спереди — со стороны +Z (камера смотрит вдоль −Z, как в
    /// сценах движка), справа — со стороны +X.
    fn angles(self) -> (f32, f32) {
        use std::f32::consts::{FRAC_PI_2, PI};
        match self {
            Self::Front => (0.0, 0.0),
            Self::Back => (PI, 0.0),
            Self::Left => (-FRAC_PI_2, 0.0),
            Self::Right => (FRAC_PI_2, 0.0),
            Self::Top => (0.0, PITCH_RANGE.1),
            Self::Overview => (HOME_YAW, HOME_PITCH),
        }
    }
}

/// Куда плавно едет камера.
#[derive(Clone, Copy, Debug)]
struct Goal {
    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,
}

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
    /// Точка, вокруг которой ходит камера (середина модели или показанной детали).
    target: Vec3,
    /// Середина модели на подставке и радиус шара вокруг неё.
    center: Vec3,
    radius: f32,
    /// Радиус того, на что смотрим (модель или показанная деталь): ближе камера не
    /// подходит.
    near: f32,
    /// Сдвиг модели на подставке (из её координат в координаты подставки).
    pub shift: Vec3,
    /// Камера плавно едет к готовому виду или к детали.
    goal: Option<Goal>,
    /// Свободный полёт: мышь крутит голову камеры на месте, клавиши её двигают.
    pub free: bool,
    /// Модель вращается сама.
    pub spin: bool,
    last: Instant,
    /// Ошибка в файле модели (тогда на подставке остаётся прошлая).
    pub error: Option<String>,
    /// Модель ещё ни разу не прочиталась: камера встанет по ней, когда прочитается.
    framed: bool,
    /// Текст файла модели при последнем чтении или записи (М.3: из него дерево частей).
    pub text: String,
    /// Детали, спрятанные «глазом» в дереве (пути в дереве `shape_edit`): на подставке их
    /// нет, в файле они остаются.
    pub hidden: Vec<Vec<usize>>,
    /// Ручки модели на подставке не как в файле (М.6, «Посмотреть»): в файл не пишется.
    pub preview: BTreeMap<String, f32>,
    /// Замер цены модели (М.7) в своём потоке.
    pub cost: super::cost::CostJob,
    /// Открыта сцена 3D (М.8), а не модель: рисуется сама сцена, без подставки.
    pub scene: bool,
    /// Радиус подставки.
    stand: f32,
}

impl ShapeView {
    /// Модель из файла `path`. Ошибка — файл не в папке `models`.
    pub(crate) fn open(path: PathBuf) -> Result<Self, String> {
        let file = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
        let scene = !is_shape(file);
        let (root, name) = if scene {
            // Сцена 3D (М.8): модели ищутся в папке models рядом с её файлом.
            let dir = path
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            (dir, file.trim_end_matches(".ron").to_owned())
        } else {
            model_of(&path).ok_or_else(|| {
                format!(
                    "{} — не модель формы: файл *.shape.ron должен лежать в папке models",
                    path.display()
                )
            })?
        };
        Ok(Self {
            path,
            name,
            root,
            modified: None,
            checked: Instant::now(),
            yaw: HOME_YAW,
            pitch: HOME_PITCH,
            distance: 3.0,
            target: Vec3::new(0.0, 0.5, 0.0),
            center: Vec3::new(0.0, 0.5, 0.0),
            radius: 1.0,
            near: 1.0,
            shift: Vec3::ZERO,
            goal: None,
            // Сцену смотрят изнутри: камера поворачивается на месте и летает, как в
            // других редакторах (модель — вокруг её середины).
            free: scene,
            spin: !scene,
            last: Instant::now(),
            error: None,
            framed: false,
            text: String::new(),
            hidden: Vec::new(),
            preview: BTreeMap::new(),
            cost: super::cost::CostJob::default(),
            scene,
            stand: 0.15,
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
        if self.scene {
            return self.load_scene();
        }
        // Глина деталей (М.9б): для выбора мышью и кисти.
        super::clay_edit::remember(&self.text, &self.root);
        // Сначала модель в начале координат: узнать, какого она размера.
        let probe = stand_text(
            &use_model(&self.name, &BTreeMap::new()),
            "",
            [0.0; 3],
            1.0,
            None,
        );
        let scene = match SpaceScene::parse_in(&probe, Some(&self.root)) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e);
                return None;
            }
        };
        let frame = scene.frame(0.0);
        if let Some(model) = frame.shapes.last() {
            self.cost.start(model.clone());
        }
        let Some((lo, hi)) = frame.shapes.last().and_then(|s| s.bounds_box()) else {
            self.error = Some("модель бесконечная (плоскость): на подставку не встаёт".into());
            return None;
        };
        let size = hi - lo;
        // Модель встаёт серединой на ось подставки, низом на её верх.
        let shift = [-(lo.x + hi.x) * 0.5, -lo.y, -(lo.z + hi.z) * 0.5];
        let radius = (size * 0.5).length().max(0.05);
        let stand = (size.x.max(size.z) * 0.5 * 1.15).max(0.15);
        self.center = Vec3::new(0.0, size.y * 0.5, 0.0);
        self.shift = Vec3::from(shift);
        if !self.framed || (radius - self.radius).abs() > self.radius * 0.5 {
            // Камера по размеру: модель целиком в кадре с полями.
            self.distance = radius * FRAME;
            self.target = self.center;
            self.near = radius;
            self.goal = None;
            self.framed = true;
        }
        self.radius = radius;
        self.stand = stand;
        self.error = None;
        self.hidden.retain(|p| {
            super::shape_edit::parse(&self.text).is_ok_and(|r| r.at(p).is_some_and(|x| x.removable))
        });
        Some(self.stand_scene())
    }

    /// Сцена 3D (М.8): прочитать, раскрыть модели для выбора мышью, поставить камеру
    /// по размеру сцены.
    fn load_scene(&mut self) -> Option<String> {
        let scene = match SpaceScene::parse_in(&self.text, Some(&self.root)) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e);
                return None;
            }
        };
        remember_models(&self.text, &self.root);
        // Коробка всех конечных предметов (пол-плоскость не в счёт).
        let frame = scene.frame(0.0);
        let mut bounds: Option<(Vec3, Vec3)> = None;
        for b in frame.shapes.iter().filter_map(Shape3::bounds_box) {
            bounds = Some(bounds.map_or(b, |(lo, hi)| (lo.min(b.0), hi.max(b.1))));
        }
        let (lo, hi) = bounds.unwrap_or((Vec3::splat(-1.0), Vec3::splat(1.0)));
        let radius = ((hi - lo) * 0.5).length().max(0.5);
        self.center = (lo + hi) * 0.5;
        self.shift = Vec3::ZERO;
        if !self.framed {
            self.distance = radius * FRAME * 0.6;
            self.target = self.center;
            self.near = radius * 0.05;
            self.goal = None;
            self.framed = true;
        }
        self.radius = radius;
        self.error = None;
        self.hidden.retain(|p| {
            super::shape_edit::parse(&self.text).is_ok_and(|r| r.at(p).is_some_and(|x| x.removable))
        });
        Some(self.stand_scene())
    }

    /// Сцена подставки с моделью как сейчас (без спрятанных деталей). У сцены 3D — она
    /// сама без спрятанных предметов и деталей.
    pub(crate) fn stand_scene(&self) -> String {
        if self.scene {
            return without_in_text(&self.text, &self.hidden);
        }
        let values = self.preview_values();
        let (model, materials) = if self.hidden.is_empty() {
            (use_model(&self.name, &values), String::new())
        } else {
            match without(&self.text, &self.hidden, &values) {
                Some(m) => m,
                None => (use_model(&self.name, &values), String::new()),
            }
        };
        stand_text(
            &model,
            &materials,
            self.shift.into(),
            self.stand,
            Some(self.radius),
        )
    }

    /// Значения ручек для подставки: только ручки, которые есть в файле, и не равные
    /// значению из файла.
    pub(crate) fn preview_values(&self) -> BTreeMap<String, f32> {
        let params = super::model_params::read(&self.text);
        self.preview
            .iter()
            .filter(|(n, v)| params.iter().any(|p| &p.name == *n && p.value != **v))
            .map(|(n, v)| (n.clone(), *v))
            .collect()
    }

    /// Поставить ручки на подставке (М.6). Ответ — новая сцена подставки.
    pub(crate) fn set_preview(&mut self, values: BTreeMap<String, f32>) -> String {
        self.preview = values;
        self.stand_scene()
    }

    /// Спрятать деталь на подставке или показать снова. Ответ — новая сцена подставки.
    pub(crate) fn set_hidden(&mut self, path: &[usize], hide: bool) -> String {
        self.hidden.retain(|p| p != path);
        if hide {
            self.hidden.push(path.to_vec());
        }
        self.stand_scene()
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

    /// Камера этого кадра (с вращением подставки и плавным переездом к новому виду).
    pub(crate) fn camera(&mut self) -> Camera3D {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.step(dt);
        self.camera_now()
    }

    /// Время идёт на `dt` секунд: вращение подставки и переезд камеры.
    fn step(&mut self, dt: f32) {
        if let Some(g) = self.goal {
            let t = 1.0 - (-EASE * dt).exp();
            self.yaw += angle_to(self.yaw, g.yaw) * t;
            self.pitch += (g.pitch - self.pitch) * t;
            self.distance += (g.distance - self.distance) * t;
            self.target += (g.target - self.target) * t;
            let close = angle_to(self.yaw, g.yaw).abs() < 1e-3
                && (g.pitch - self.pitch).abs() < 1e-3
                && (g.distance - self.distance).abs() < g.distance * 1e-3
                && (g.target - self.target).length() < g.distance * 1e-3;
            if close {
                (self.yaw, self.pitch) = (g.yaw, g.pitch);
                (self.distance, self.target) = (g.distance, g.target);
                self.goal = None;
            }
        } else if self.spin && !self.free {
            self.yaw = (self.yaw + SPIN * dt) % std::f32::consts::TAU;
        }
    }

    /// Камера без хода времени.
    pub(crate) fn camera_now(&self) -> Camera3D {
        let eye = self.eye();
        let mut camera = Camera3D::looking_at(eye, self.target);
        camera.near = (self.distance.min(self.near * 2.0) * 0.01).max(0.002);
        camera.far = (self.distance * 20.0).max(self.radius * 40.0).max(50.0);
        camera
    }

    /// Направление от середины к камере.
    fn offset(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, cy * cp)
    }

    fn eye(&self) -> Vec3 {
        self.target + self.offset() * self.distance
    }

    /// Повернуть камеру мышью на `dx`, `dy` точек интерфейса: вокруг середины, а в полёте —
    /// на месте (камера оглядывается). Вращение подставки останавливается: модель
    /// смотрят с этой стороны.
    pub(crate) fn orbit(&mut self, dx: f32, dy: f32) {
        self.spin = false;
        self.goal = None;
        let eye = self.eye();
        let per = if self.scene {
            SCENE_TURN
        } else {
            ORBIT_PER_POINT
        };
        self.yaw -= dx * per;
        let (lo, hi) = if self.free {
            (-FREE_PITCH, FREE_PITCH)
        } else {
            PITCH_RANGE
        };
        self.pitch = (self.pitch + dy * per).clamp(lo, hi);
        if self.free {
            self.target = eye - self.offset() * self.distance;
        }
    }

    /// Сдвинуть камеру вбок и вверх-вниз вслед за мышью (правая или средняя кнопка).
    pub(crate) fn pan(&mut self, dx: f32, dy: f32) {
        self.spin = false;
        self.goal = None;
        let c = self.camera_now();
        let k = self.distance * PAN_PER_POINT;
        self.target += (-c.right() * dx + c.up() * dy) * k;
    }

    /// Полёт клавишами: `dir` — вправо, вверх, вперёд (от −1 до 1) за `seconds` секунд.
    /// Скорость по размеру модели: её поперечник примерно за секунду.
    pub(crate) fn fly(&mut self, dir: [f32; 3], seconds: f32) {
        let d = Vec3::from(dir);
        if d == Vec3::ZERO {
            return;
        }
        self.spin = false;
        self.goal = None;
        let c = self.camera_now();
        // У сцены скорость своя, в метрах: по её размеру (десятки метров) камера
        // проскакивала предметы.
        let speed = if self.scene {
            SCENE_FLY
        } else {
            (self.radius * 1.6).max(0.3)
        };
        self.target += (c.right() * d.x + Vec3::Y * d.y + c.forward() * d.z) * speed * seconds;
    }

    /// Приблизить (`steps` > 0) или отдалить колесом. В полёте колесо двигает камеру
    /// вперёд-назад.
    pub(crate) fn zoom(&mut self, steps: f32) {
        if self.free {
            let forward = -self.offset();
            self.goal = None;
            let step = if self.scene {
                SCENE_WHEEL
            } else {
                (self.radius * 0.15).max(0.02)
            };
            self.target += forward * steps * step;
            return;
        }
        let g = self.goal.take();
        if let Some(g) = g {
            // Колесо во время переезда: доехать сразу и приблизить оттуда.
            (self.yaw, self.pitch, self.target) = (g.yaw, g.pitch, g.target);
            self.distance = g.distance;
        }
        let (lo, hi) = (self.near * 0.6, self.radius * 12.0);
        self.distance = (self.distance * 0.85_f32.powf(steps)).clamp(lo.min(hi), hi);
    }

    /// Готовый вид на всю модель (вращение останавливается, полёт выключается).
    pub(crate) fn view(&mut self, angle: ShapeAngle) {
        let (yaw, pitch) = angle.angles();
        self.spin = false;
        self.free = false;
        self.near = self.radius;
        self.goal = Some(Goal {
            yaw,
            pitch,
            distance: self.radius * FRAME,
            target: self.center,
        });
    }

    /// Показать крупно деталь по пути `path` в дереве частей (`shape_edit`): камера
    /// плавно встаёт так, чтобы деталь была целиком в кадре, и ходит вокруг неё. Пустой
    /// путь — вся модель. `false` — у детали не узнать размер (ссылка на другую модель).
    pub(crate) fn focus(&mut self, path: &[usize]) -> bool {
        let Some((lo, hi)) = part_bounds(&self.text, path) else {
            return false;
        };
        let r = ((hi - lo) * 0.5).length().max(0.01);
        self.spin = false;
        self.free = false;
        self.near = r;
        let pitch = self.pitch.clamp(PITCH_RANGE.0, PITCH_RANGE.1);
        self.goal = Some(Goal {
            yaw: self.yaw,
            pitch,
            distance: r * FRAME,
            target: (lo + hi) * 0.5 + self.shift,
        });
        true
    }

    /// Включить или выключить свободный полёт.
    pub(crate) fn set_free(&mut self, free: bool) {
        if let Some(g) = self.goal.take() {
            (self.yaw, self.pitch, self.target) = (g.yaw, g.pitch, g.target);
            self.distance = g.distance;
        }
        self.free = free;
        self.spin = false;
        if !free {
            self.pitch = self.pitch.clamp(PITCH_RANGE.0, PITCH_RANGE.1);
        }
    }
}

thread_local! {
    /// Модели сцены 3D, раскрытые при её чтении (М.8): ссылка `Use` текстом → дерево.
    /// По ним выбор мышью и рамки находят детали моделей в сцене.
    static MODELS: RefCell<HashMap<String, Shape3>> = RefCell::new(HashMap::new());
    /// Последний текст и его форма целиком: стрелки спрашивают её несколько раз за кадр.
    static WHOLE: RefCell<Option<(String, Option<Shape3>)>> = const { RefCell::new(None) };
}

/// Раскрыть и запомнить модели, на которые ссылается сцена `text` из папки `dir`.
fn remember_models(text: &str, dir: &Path) {
    let Ok(file) = ron::from_str::<SpaceFile>(text) else {
        return;
    };
    let mut uses = Vec::new();
    for o in &file.objects {
        collect_uses(&o.shape, &mut uses);
    }
    let mut materials = Vec::new();
    MODELS.with_borrow_mut(|m| {
        for u in uses {
            let Ok(key) = ron::to_string(&u) else {
                continue;
            };
            if m.contains_key(&key) {
                continue;
            }
            let mut shape = u.clone();
            let mut r = Resolver::new(Some(dir), &mut materials);
            if r.resolve(&mut shape).is_ok() {
                m.insert(key, shape);
            }
        }
    });
    WHOLE.with_borrow_mut(|w| *w = None);
}

fn collect_uses(shape: &Shape3, out: &mut Vec<Shape3>) {
    if matches!(shape, Shape3::Use { .. }) {
        out.push(shape.clone());
        return;
    }
    let mut i = 0;
    while let Some(c) = child(shape, i, &mut Vec::new()) {
        collect_uses(c, out);
        i += 1;
    }
}

/// Ссылки на модели заменяются запомненными деревьями.
fn with_models(shape: &mut Shape3) {
    if matches!(shape, Shape3::Use { .. }) {
        let key = ron::to_string(&*shape).unwrap_or_default();
        if let Some(m) = MODELS.with_borrow(|m| m.get(&key).cloned()) {
            *shape = m;
        }
        return;
    }
    let mut i = 0;
    while let Some(c) = child_mut(shape, i) {
        with_models(c);
        i += 1;
    }
}

/// Вся форма файла: у модели — её дерево, у сцены 3D (М.8) — объединение предметов,
/// каждый в своём сдвиге (место и поворот предмета), ссылки на модели раскрыты.
fn whole(text: &str) -> Option<Shape3> {
    if let Some(w) = WHOLE.with_borrow(|w| {
        w.as_ref()
            .filter(|(t, _)| t == text)
            .map(|(_, s)| s.clone())
    }) {
        return w;
    }
    let shape = if let Ok(mut f) = ModelFile::parse(text) {
        with_clay(&mut f.shape);
        Some(f.shape)
    } else {
        ron::from_str::<SpaceFile>(text)
            .ok()
            .map(|f| Shape3::Union {
                shapes: f
                    .objects
                    .into_iter()
                    .map(|o| {
                        let mut shape = o.shape;
                        with_models(&mut shape);
                        Shape3::Move {
                            at: o.at,
                            turn: o.turn,
                            scale: 1.0,
                            shape: Box::new(shape),
                        }
                    })
                    .collect(),
                smooth: 0.0,
            })
    };
    WHOLE.with_borrow_mut(|w| *w = Some((text.to_owned(), shape.clone())));
    shape
}

/// Забыть форму целиком: глина поменялась, а текст тот же (М.9б).
pub(crate) fn forget_whole() {
    WHOLE.with_borrow_mut(|w| *w = None);
}

/// Глина (М.9) заменяется сеткой из памяти `clay_edit`: её видят выбор мышью и кисть.
fn with_clay(shape: &mut Shape3) {
    if let Shape3::Clay { file } = shape {
        if let Some(brick) = super::clay_edit::brick(file) {
            *shape = Shape3::Brick { brick };
        }
        return;
    }
    let mut i = 0;
    while let Some(c) = child_mut(shape, i) {
        with_clay(c);
        i += 1;
    }
}

/// Сцена 3D без спрятанных предметов и деталей (текстом).
fn without_in_text(text: &str, hidden: &[Vec<usize>]) -> String {
    let mut paths = hidden.to_vec();
    paths.sort_unstable_by(|a, b| b.cmp(a));
    let mut out = text.to_owned();
    for p in &paths {
        let Ok(root) = super::shape_edit::parse(&out) else {
            break;
        };
        if let Some(part) = root.at(p)
            && let Ok(t) = super::shape_edit::remove(&out, part)
        {
            out = t;
        }
    }
    out
}

/// Материалов у подставки до материалов модели (пол, подставка, форма).
const STAND_MATERIALS: u32 = 3;

/// Форма модели ссылкой на её файл.
fn use_model(name: &str, values: &BTreeMap<String, f32>) -> String {
    if values.is_empty() {
        return format!("Use(model: \"{name}\")");
    }
    let values = ron::to_string(values).unwrap_or_default();
    format!("Use(model: \"{name}\", params: {values})")
}

/// Модель без деталей `hidden` прямо текстом формы (номера цветов сдвинуты за материалы
/// подставки) и её материалы для списка подставки. `None` — модель не читается.
fn without(
    text: &str,
    hidden: &[Vec<usize>],
    values: &BTreeMap<String, f32>,
) -> Option<(String, String)> {
    let mut file = ModelFile::parse(text).ok()?;
    reyvec_sdf::shape_param::apply(&mut file.shape, &file.params, values).ok()?;
    let mut paths = hidden.to_vec();
    // С конца: сначала поздние и вложенные, тогда номера ранних не съезжают.
    paths.sort_unstable_by(|a, b| b.cmp(a));
    for p in &paths {
        remove_part(&mut file.shape, p);
    }
    shift_paint(&mut file.shape, STAND_MATERIALS);
    let shape = ron::to_string(&file.shape).ok()?;
    let materials = super::shape_edit::material_items(text).join(",\n        ");
    let materials = if materials.is_empty() {
        materials
    } else {
        materials + ","
    };
    Some((shape, materials))
}

/// Убирает деталь по пути (последний в списке своей части не убирается: пустое
/// объединение не нарисовать).
fn remove_part(root: &mut Shape3, path: &[usize]) {
    let Some((&last, up)) = path.split_last() else {
        return;
    };
    let mut node = root;
    for &i in up {
        node = unwrap_mut(node);
        let Some(next) = child_mut(node, i) else {
            return;
        };
        node = next;
    }
    match unwrap_mut(node) {
        Shape3::Union { shapes, .. } | Shape3::Intersect { shapes, .. }
            if shapes.len() > 1 && last < shapes.len() =>
        {
            shapes.remove(last);
        }
        Shape3::Subtract { cut, .. } if last >= 1 && last - 1 < cut.len() => {
            cut.remove(last - 1);
        }
        _ => {}
    }
}

/// [`unwrap`] для правки.
fn unwrap_mut(node: &mut Shape3) -> &mut Shape3 {
    fn go(node: &mut Shape3, moved: bool, painted: bool) -> &mut Shape3 {
        let what = match node {
            Shape3::Move { .. } if !moved => 1,
            Shape3::Paint { .. } if !painted => 2,
            _ => 0,
        };
        match (what, node) {
            (1, Shape3::Move { shape, .. }) => go(shape, true, painted),
            (2, Shape3::Paint { shape, .. }) => go(shape, moved, true),
            (_, n) => n,
        }
    }
    go(node, false, false)
}

/// [`child`] для правки.
pub(crate) fn child_mut(node: &mut Shape3, i: usize) -> Option<&mut Shape3> {
    match node {
        Shape3::Union { shapes, .. } | Shape3::Intersect { shapes, .. } => shapes.get_mut(i),
        Shape3::Subtract { base, cut, .. } => {
            if i == 0 {
                Some(base)
            } else {
                cut.get_mut(i - 1)
            }
        }
        Shape3::Move { shape, .. }
        | Shape3::Paint { shape, .. }
        | Shape3::Mirror { shape, .. }
        | Shape3::Repeat { shape, .. }
        | Shape3::Animate { shape, .. }
            if i == 0 =>
        {
            Some(shape)
        }
        _ => None,
    }
}

/// Номера цветов `Paint` модели сдвигаются на `base` (материалы модели стоят в сцене
/// подставки после её своих).
fn shift_paint(shape: &mut Shape3, base: u32) {
    match shape {
        Shape3::Paint { material, shape } => {
            *material += base;
            shift_paint(shape, base);
        }
        Shape3::Union { shapes, .. } | Shape3::Intersect { shapes, .. } => {
            shapes.iter_mut().for_each(|s| shift_paint(s, base));
        }
        Shape3::Subtract { base: b, cut, .. } => {
            shift_paint(b, base);
            cut.iter_mut().for_each(|s| shift_paint(s, base));
        }
        Shape3::Move { shape, .. }
        | Shape3::Animate { shape, .. }
        | Shape3::Mirror { shape, .. }
        | Shape3::Repeat { shape, .. } => shift_paint(shape, base),
        _ => {}
    }
}

/// Кратчайший поворот от угла `from` до `to` (от −π до π).
fn angle_to(from: f32, to: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (to - from + PI).rem_euclid(TAU) - PI
}

/// Что лежит вокруг части по пути к ней: сдвиги, зеркала, повторы.
#[derive(Clone, Copy, Debug)]
enum Around {
    Move(Placing),
    Mirror(reyvec_sdf::shape3d::Axis),
    Repeat(u32, [f32; 3]),
}

/// Часть по пути `path` (со своими сдвигом и цветом) и что вокруг неё, снаружи внутрь.
fn walk<'a>(shape: &'a Shape3, path: &[usize]) -> Option<(&'a Shape3, Vec<Around>)> {
    let mut around = Vec::new();
    let mut node = shape;
    for &i in path {
        node = unwrap(node, &mut around);
        node = child(node, i, &mut around)?;
    }
    Some((node, around))
}

/// Форма части в координатах модели: обёрнута во всё, что вокруг неё (`copies` — с
/// копиями зеркал и повторов).
fn placed(node: &Shape3, around: &[Around], copies: bool) -> Shape3 {
    let mut shape = node.clone();
    for a in around.iter().rev() {
        shape = match *a {
            Around::Move((at, turn, scale)) => Shape3::Move {
                at,
                turn,
                scale,
                shape: Box::new(shape),
            },
            Around::Mirror(axis) if copies => Shape3::Mirror {
                axis,
                smooth: 0.0,
                shape: Box::new(shape),
            },
            Around::Repeat(count, step) if copies => Shape3::Repeat {
                count,
                step,
                smooth: 0.0,
                shape: Box::new(shape),
            },
            _ => shape,
        };
    }
    shape
}

/// Коробка детали по пути `path` в дереве частей (`shape_edit::parse`) в координатах
/// модели: со всеми сдвигами вокруг неё. Копии зеркала и повтора не входят — коробка
/// самой детали. `None` — модель не читается или у детали нет размера (ссылка `Use`).
pub(crate) fn part_bounds(text: &str, path: &[usize]) -> Option<(Vec3, Vec3)> {
    let shape = whole(text)?;
    let (node, around) = walk(&shape, path)?;
    // Ссылка на другую модель здесь не раскрыта: у неё коробка в точку.
    placed(node, &around, false)
        .bounds_box()
        .filter(|(lo, hi)| (*hi - *lo).max_element() > 1e-6)
}

/// Переход из координат самой детали (внутри её сдвига) в координаты модели: в них
/// лежит глина детали (М.9б).
pub(crate) fn part_frame(text: &str, path: &[usize]) -> Option<glam::Affine3A> {
    let shape = whole(text)?;
    let (node, mut around) = walk(&shape, path)?;
    unwrap(node, &mut around);
    Some(frame_of(&around))
}

/// Переход из координат родителя детали (где записано её `at`) в координаты модели.
pub(crate) fn parent_frame(text: &str, path: &[usize]) -> Option<glam::Affine3A> {
    let shape = whole(text)?;
    let (_, around) = walk(&shape, path)?;
    Some(frame_of(&around))
}

/// Все сдвиги снаружи внутрь одним переходом.
fn frame_of(around: &[Around]) -> glam::Affine3A {
    let mut m = glam::Affine3A::IDENTITY;
    for a in around {
        if let Around::Move((at, turn, scale)) = *a {
            let r = glam::Mat3A::from(reyvec_sdf::shape3d::rotation(turn));
            m *= glam::Affine3A {
                matrix3: r * scale,
                translation: Vec3::from(at).into(),
            };
        }
    }
    m
}

/// Точка, где луч `origin + dir·t` встречает модель или сцену без спрятанных деталей
/// (М.8б: куда встаёт модель, брошенная из «Ассетов»). `None` — луч мимо.
pub(crate) fn hit(
    text: &str,
    hidden: &[Vec<usize>],
    origin: Vec3,
    dir: Vec3,
    far: f32,
) -> Option<Vec3> {
    march(&whole(text)?, hidden, origin, dir, far)
}

/// Шаги луча по форме `shape` без спрятанных частей `hidden`.
fn march(shape: &Shape3, hidden: &[Vec<usize>], origin: Vec3, dir: Vec3, far: f32) -> Option<Vec3> {
    let mut sorted = hidden.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let mut whole = shape.clone();
    for p in &sorted {
        remove_part(&mut whole, p);
    }
    // У сцены с полом-плоскостью коробки нет: точность — как у сцены в десятки метров.
    let eps = whole
        .bounds_box()
        .map_or(2e-3, |(lo, hi)| ((hi - lo).max_element() * 2e-4).max(1e-5));
    let mut t = 0.0;
    for _ in 0..400 {
        let p = origin + dir * t;
        let d = whole.distance(p);
        if d.abs() < eps {
            return Some(p);
        }
        t += d.max(eps * 0.5);
        if t > far {
            break;
        }
    }
    None
}

/// Какую деталь видно по лучу из `origin` вдоль `dir` (координаты модели): луч идёт до
/// поверхности модели без спрятанных деталей, там берётся ближайшая деталь без вложенных
/// (с копиями зеркал и повторов). `None` — луч мимо модели.
pub(crate) fn pick(
    text: &str,
    hidden: &[Vec<usize>],
    origin: Vec3,
    dir: Vec3,
    far: f32,
) -> Option<Vec<usize>> {
    let shape = whole(text)?;
    let p = march(&shape, hidden, origin, dir, far)?;
    let tree = super::shape_edit::parse(text).ok()?;
    let mut best: Option<(f32, Vec<usize>)> = None;
    let mut stack = vec![(Vec::new(), &tree)];
    while let Some((path, part)) = stack.pop() {
        if hidden.iter().any(|h| path.starts_with(h)) {
            continue;
        }
        if part.children.is_empty() {
            if let Some((node, around)) = walk(&shape, &path) {
                let d = placed(node, &around, true).distance(p).abs();
                if d < f32::MAX && best.as_ref().is_none_or(|(b, _)| d < *b) {
                    best = Some((d, path.clone()));
                }
            }
            continue;
        }
        for (i, c) in part.children.iter().enumerate() {
            let mut q = path.clone();
            q.push(i);
            stack.push((q, c));
        }
    }
    best.map(|(_, p)| p)
}

/// Сдвиг одной части: место, поворот, масштаб.
type Placing = ([f32; 3], [f32; 3], f32);

/// Снимает со части её сдвиг и цвет (по одному, как дерево `shape_edit`), запоминая сдвиг.
fn unwrap<'a>(mut node: &'a Shape3, around: &mut Vec<Around>) -> &'a Shape3 {
    let (mut moved, mut painted) = (false, false);
    loop {
        match node {
            Shape3::Move {
                at,
                turn,
                scale,
                shape,
            } if !moved => {
                moved = true;
                around.push(Around::Move((*at, *turn, *scale)));
                node = shape;
            }
            Shape3::Paint { shape, .. } if !painted => {
                painted = true;
                node = shape;
            }
            _ => return node,
        }
    }
}

/// Вложенная часть номер `i` (по порядку, как в дереве `shape_edit`).
fn child<'a>(node: &'a Shape3, i: usize, around: &mut Vec<Around>) -> Option<&'a Shape3> {
    match node {
        Shape3::Union { shapes, .. } | Shape3::Intersect { shapes, .. } => shapes.get(i),
        Shape3::Subtract { base, cut, .. } => {
            if i == 0 {
                Some(base)
            } else {
                cut.get(i - 1)
            }
        }
        Shape3::Move {
            at,
            turn,
            scale,
            shape,
        } if i == 0 => {
            around.push(Around::Move((*at, *turn, *scale)));
            Some(shape)
        }
        Shape3::Mirror { axis, shape, .. } if i == 0 => {
            around.push(Around::Mirror(*axis));
            Some(shape)
        }
        Shape3::Repeat {
            count, step, shape, ..
        } if i == 0 => {
            around.push(Around::Repeat(*count, *step));
            Some(shape)
        }
        Shape3::Paint { shape, .. } | Shape3::Animate { shape, .. } if i == 0 => Some(shape),
        _ => None,
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Текст сцены подставки: пол, круглая подставка радиусом `stand`, модель `name`,
/// сдвинутая на `shift`, и лампы по размеру модели `radius` (нет — для замера размера,
/// без ламп).
fn stand_text(
    model: &str,
    materials: &str,
    shift: [f32; 3],
    stand: f32,
    radius: Option<f32>,
) -> String {
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
        (name: "подставка: пол", color: (0.26, 0.27, 0.29), grid: true),
        (name: "подставка: круг", color: (0.42, 0.43, 0.46)),
        (name: "подставка: форма", color: (0.80, 0.80, 0.78)),
        {materials}
    ],
    lamps: [
        {lamps}
    ],
    objects: [
        (name: "пол", material: "подставка: пол", shape: Plane(normal: (0.0, 1.0, 0.0), offset: -0.03)),
        (name: "подставка", material: "подставка: круг", at: (0.0, -0.015, 0.0),
            shape: Cylinder(height: 0.03, radius: {stand:.4}, round: 0.005)),
        (name: "модель", material: "подставка: форма", at: ({x:.4}, {y:.4}, {z:.4}),
            shape: {model}),
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
    fn space_scene_opens_picks_and_moves_objects() {
        let path = scenes().join("forge.ron");
        let mut view = ShapeView::open(path).expect("сцена");
        assert!(view.scene && !view.spin);
        assert_eq!(view.name, "forge");
        let text = view.load().expect("сцена читается");
        assert!(view.error.is_none());
        // Камера сцены поворачивается на месте и летает 3 м/с, а не по размеру сцены.
        assert!(view.free);
        let eye = view.camera_now().eye;
        view.fly([0.0, 0.0, 1.0], 1.0);
        let moved = (view.camera_now().eye - eye).length();
        assert!((moved - SCENE_FLY).abs() < 1e-3, "{moved}");
        view.orbit(100.0, 0.0);
        assert!((view.camera_now().eye - eye).length() - moved < 1e-3);
        assert_eq!(text, view.text, "сцена рисуется как есть");
        let root = super::super::shape_edit::parse(&view.text).unwrap();
        assert!(super::super::shape_edit::is_scene(&root));
        let file: SpaceFile = ron::from_str(&view.text).unwrap();
        assert_eq!(root.children.len(), file.objects.len());
        // Наковальня — ссылка на модель: у неё есть рамка (модель раскрыта), и луч сверху
        // в её середину выбирает её.
        let i = file
            .objects
            .iter()
            .position(
                |o| matches!(&o.shape, Shape3::Use { model, .. } if model.ends_with("наковальня")),
            )
            .expect("наковальня");
        assert_eq!(root.children[i].kind, "Use");
        assert_eq!(
            root.children[i].name.as_deref(),
            Some(file.objects[i].name.as_str())
        );
        let (lo, hi) = part_bounds(&view.text, &[i]).expect("рамка модели");
        let mid = (lo + hi) * 0.5;
        let top = Vec3::new(mid.x, hi.y + 3.0, mid.z);
        // На наковальне лежит заготовка: прячем её, чтобы луч дошёл до наковальни.
        let billet = file
            .objects
            .iter()
            .position(
                |o| matches!(&o.shape, Shape3::Use { model, .. } if model.ends_with("заготовка")),
            )
            .expect("заготовка");
        let hit = pick(&view.text, &[vec![billet]], top, Vec3::NEG_Y, 50.0).expect("попали");
        assert_eq!(hit[0], i, "{hit:?}");
        // Сдвиг предмета пишется в его at, сцена читается.
        let part = root.at(&[i]).unwrap();
        let moved = super::super::shape_edit::set_moved(&view.text, part, "at", "(1.5, 0.0, -2.0)")
            .unwrap();
        let back: SpaceFile = ron::from_str(&moved).unwrap();
        assert_eq!(back.objects[i].at, [1.5, 0.0, -2.0]);
        // Спрятанный предмет пропадает из сцены, остальные на месте.
        view.hidden = vec![vec![i]];
        let without: SpaceFile = ron::from_str(&view.stand_scene()).unwrap();
        assert_eq!(without.objects.len(), file.objects.len() - 1);
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
        let target = view.target;
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
        view.view(ShapeAngle::Overview);
        assert!(!view.spin, "общий вид вращение не включает");
        view.step(1.0);
        assert!((view.pitch - HOME_PITCH).abs() < 1e-3);
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

    #[test]
    fn views_focus_and_free_flight() {
        let mut view = ShapeView::open(scenes().join("models/склад/стеллаж.shape.ron")).unwrap();
        view.load().expect("стеллаж читается");
        // Вид спереди: камера на +Z от середины, смотрит вдоль −Z, вращение стоит.
        view.view(ShapeAngle::Front);
        assert!(!view.spin);
        view.step(1.0);
        let c = view.camera_now();
        assert!(
            (c.forward() - Vec3::NEG_Z).length() < 1e-2,
            "{}",
            c.forward()
        );
        view.step(1.0);
        assert!(view.goal.is_none(), "доехала");
        view.view(ShapeAngle::Right);
        view.step(1.0);
        assert!((view.camera_now().forward() - Vec3::NEG_X).length() < 1e-2);
        view.view(ShapeAngle::Top);
        view.step(1.0);
        assert!(view.camera_now().forward().y < -0.95);
        // Деталь крупно: камера ходит вокруг середины детали и подходит ближе.
        let whole = view.distance;
        let root = super::super::shape_edit::parse(&view.text).unwrap();
        let path = (0..root.children.len())
            .map(|i| vec![i])
            .find(|p| part_bounds(&view.text, p).is_some())
            .expect("у детали есть коробка");
        let (lo, hi) = part_bounds(&view.text, &path).unwrap();
        assert!(view.focus(&path));
        view.step(1.0);
        view.step(1.0);
        assert!((view.target - ((lo + hi) * 0.5 + view.shift)).length() < 1e-3);
        assert!(view.distance < whole, "{} < {whole}", view.distance);
        // Колесо подходит ближе, чем позволял размер всей модели.
        for _ in 0..40 {
            view.zoom(1.0);
        }
        assert!(view.distance < view.radius * 0.6);
        // Полёт: оглядывается на месте, клавиши двигают камеру вперёд.
        view.set_free(true);
        let eye = view.eye();
        view.orbit(120.0, -40.0);
        assert!((view.eye() - eye).length() < 1e-4, "глаз на месте");
        let forward = view.camera_now().forward();
        view.fly([0.0, 0.0, 1.0], 0.5);
        let moved = view.eye() - eye;
        assert!(moved.dot(forward) > 0.0 && moved.length() > 0.1, "{moved}");
        // Сдвиг вокруг модели: середина уходит вбок.
        view.set_free(false);
        let before = view.target;
        view.pan(50.0, 0.0);
        assert!((view.target - before).length() > 1e-3);
    }

    #[test]
    fn part_bounds_follow_moves() {
        let text = r#"(format: "reyvec-shape", version: 1, shape: Move(at: (1.0, 0.0, 0.0), shape: Union(shapes: [
            Sphere(radius: 0.1),
            Paint(material: 0, shape: Move(at: (0.0, 2.0, 0.0), shape: Box(size: (0.2, 0.2, 0.2)))),
            Use(model: "другая"),
        ])))"#;
        let text = text.replace(
            "version: 1,",
            "version: 1, materials: [(name: \"м\", color: (1.0, 1.0, 1.0))],",
        );
        let (lo, hi) = part_bounds(&text, &[1]).expect("коробка");
        assert!((lo - Vec3::new(0.9, 1.9, -0.1)).length() < 1e-4, "{lo}");
        assert!((hi - Vec3::new(1.1, 2.1, 0.1)).length() < 1e-4, "{hi}");
        let (lo, _) = part_bounds(&text, &[0]).unwrap();
        assert!((lo - Vec3::new(0.9, -0.1, -0.1)).length() < 1e-4);
        assert!(part_bounds(&text, &[2]).is_none(), "у ссылки нет размера");
        assert!(part_bounds(&text, &[5]).is_none());
    }

    #[test]
    fn hidden_parts_leave_the_stand_but_stay_in_the_file() {
        let path = scenes().join("models/склад/стеллаж.shape.ron");
        let mut view = ShapeView::open(path.clone()).unwrap();
        let full = view.load().expect("стеллаж");
        let dir = view.root.clone();
        let count = |text: &str| {
            let scene =
                SpaceScene::parse_in(text, Some(&dir)).unwrap_or_else(|e| panic!("{e}\n{text}"));
            let frame = scene.frame(0.0);
            let shape = frame.shapes.last().unwrap().clone();
            (format!("{shape:?}").matches("Box {").count(), scene)
        };
        let (boxes, _) = count(&full);
        let root = super::super::shape_edit::parse(&view.text).unwrap();
        let first = (0..root.children.len())
            .find(|&i| root.children[i].removable)
            .expect("деталь в списке");
        let hidden = view.set_hidden(&[first], true);
        let (fewer, scene) = count(&hidden);
        assert!(fewer < boxes, "{fewer} < {boxes}");
        // Цвета модели на месте: материалы подставки и модели вместе.
        let model_materials = super::super::shape_edit::materials(&view.text).len();
        assert_eq!(scene.materials.len(), 3 + model_materials);
        // Показать снова — сцена как была; файл не тронут.
        let back = view.set_hidden(&[first], false);
        assert_eq!(back, full);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), view.text);
    }

    const PICK: &str = r#"(format: "reyvec-shape", version: 1, shape: Union(shapes: [
        Sphere(radius: 0.3),
        Mirror(axis: X, shape: Move(at: (1.0, 0.0, 0.0), shape: Box(size: (0.4, 0.4, 0.4)))),
    ]))"#;

    #[test]
    fn click_picks_the_part_under_the_cursor() {
        let back = Vec3::NEG_Z;
        let at = |x: f32| Vec3::new(x, 0.0, 5.0);
        assert_eq!(pick(PICK, &[], at(0.0), back, 50.0), Some(vec![0]));
        // Коробка и её отражение — одна деталь.
        assert_eq!(pick(PICK, &[], at(1.0), back, 50.0), Some(vec![1, 0]));
        assert_eq!(pick(PICK, &[], at(-1.0), back, 50.0), Some(vec![1, 0]));
        assert_eq!(pick(PICK, &[], at(3.0), back, 50.0), None, "мимо");
        // Спрятанный шар не мешает: луч проходит насквозь.
        assert_eq!(pick(PICK, &[vec![0]], at(0.0), back, 50.0), None);
    }

    #[test]
    fn parent_frame_follows_enclosing_moves() {
        let text = r#"(format: "reyvec-shape", version: 1, shape: Move(at: (1.0, 0.0, 0.0), scale: 2.0,
            shape: Union(shapes: [Move(at: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.1))])))"#;
        let f = parent_frame(text, &[0]).expect("место родителя");
        assert!(
            (f.transform_point3(Vec3::new(0.0, 1.0, 0.0)) - Vec3::new(1.0, 2.0, 0.0)).length()
                < 1e-5
        );
        assert!((f.transform_vector3(Vec3::X).length() - 2.0).abs() < 1e-5);
    }
}
