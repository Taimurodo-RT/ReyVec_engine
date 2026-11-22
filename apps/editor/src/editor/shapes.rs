//! «Формы» (М.2, `docs/models-3d.md`): модель формы `models/…/<имя>.shape.ron` на
//! подставке. Двойной щелчок по файлу модели в «Ассетах» открывает её вместо сцены: движок
//! рисует сцену «3D», которую собирает этот модуль (пол, круглая подставка, две лампы и
//! сама модель через `Use`). Камера ходит вокруг модели: мышь поворачивает, колесо
//! приближает, без мыши модель медленно вращается. Есть готовые виды (спереди, сбоку,
//! сверху), показ одной детали крупно и свободный полёт клавишами. Сохранили файл
//! модели (в блокноте или другой программе) — подставка перечитывает его сама.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use glam::Vec3;
use reyvec_sdf::shape_model::ModelFile;
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
    shift: Vec3,
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
            target: Vec3::new(0.0, 0.5, 0.0),
            center: Vec3::new(0.0, 0.5, 0.0),
            radius: 1.0,
            near: 1.0,
            shift: Vec3::ZERO,
            goal: None,
            free: false,
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
    fn camera_now(&self) -> Camera3D {
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
        self.yaw -= dx * ORBIT_PER_POINT;
        let (lo, hi) = if self.free {
            (-FREE_PITCH, FREE_PITCH)
        } else {
            PITCH_RANGE
        };
        self.pitch = (self.pitch + dy * ORBIT_PER_POINT).clamp(lo, hi);
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
        let speed = (self.radius * 1.6).max(0.3);
        self.target += (c.right() * d.x + Vec3::Y * d.y + c.forward() * d.z) * speed * seconds;
    }

    /// Приблизить (`steps` > 0) или отдалить колесом. В полёте колесо двигает камеру
    /// вперёд-назад.
    pub(crate) fn zoom(&mut self, steps: f32) {
        if self.free {
            let forward = -self.offset();
            self.goal = None;
            self.target += forward * steps * (self.radius * 0.15).max(0.02);
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

    /// Камера как при открытии, вращение снова идёт.
    pub(crate) fn home(&mut self) {
        self.view(ShapeAngle::Overview);
        self.spin = true;
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

/// Кратчайший поворот от угла `from` до `to` (от −π до π).
fn angle_to(from: f32, to: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (to - from + PI).rem_euclid(TAU) - PI
}

/// Коробка детали по пути `path` в дереве частей (`shape_edit::parse`) в координатах
/// модели: со всеми сдвигами вокруг неё. Копии зеркала и повтора не входят — коробка
/// самой детали. `None` — модель не читается или у детали нет размера (ссылка `Use`).
pub(crate) fn part_bounds(text: &str, path: &[usize]) -> Option<(Vec3, Vec3)> {
    let file = ModelFile::parse(text).ok()?;
    let mut moves = Vec::new();
    let mut node = &file.shape;
    for &i in path {
        node = unwrap(node, &mut moves);
        node = child(node, i, &mut moves)?;
    }
    let mut shape = node.clone();
    for &(at, turn, scale) in moves.iter().rev() {
        shape = Shape3::Move {
            at,
            turn,
            scale,
            shape: Box::new(shape),
        };
    }
    // Ссылка на другую модель здесь не раскрыта: у неё коробка в точку.
    shape
        .bounds_box()
        .filter(|(lo, hi)| (*hi - *lo).max_element() > 1e-6)
}

/// Сдвиг одной части: место, поворот, масштаб.
type Placing = ([f32; 3], [f32; 3], f32);

/// Снимает со части её сдвиг и цвет (по одному, как дерево `shape_edit`), запоминая сдвиг.
fn unwrap<'a>(mut node: &'a Shape3, moves: &mut Vec<Placing>) -> &'a Shape3 {
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
                moves.push((*at, *turn, *scale));
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
fn child<'a>(node: &'a Shape3, i: usize, moves: &mut Vec<Placing>) -> Option<&'a Shape3> {
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
            moves.push((*at, *turn, *scale));
            Some(shape)
        }
        Shape3::Paint { shape, .. }
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
        view.home();
        assert!(view.spin);
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
}
