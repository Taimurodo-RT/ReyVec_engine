//! Редактор ReyVec (Р.1, каркас): отдельная программа на том же движке.
//!
//! Окно открывается на весь экран без рамки системы: свои кнопки «свернуть, развернуть,
//! закрыть» стоят в верхней полосе, окно тащат за пустое место полосы и растягивают за
//! края. Посередине — настоящий кадр движка со сценой из файла, вокруг — панели egui
//! (раскладка описана данными в [`layout`]). «Играть» запускает игру прямо в окне сцены,
//! «Стоп» возвращает сцену к началу.
//!
//! В режиме правки (Р.4) предметы выбирают щелчком, двигают, поворачивают и меняют в
//! размере ручками или числами в инспекторе; Ctrl+S пишет файл сцены. Правка меняет в
//! тексте файла только нужное число ([`ron_edit`]), поэтому комментарии и порядок полей
//! в файле остаются, а отмена — это просто прошлый текст ([`doc`]).
//!
//! Слабое место: движок рисует сцену на всё окно, а панели лежат поверх неё. Камера
//! сдвинута так, чтобы середина сцены была посередине свободного места. Отдельная
//! картинка сцены точно под размер середины — следующий шаг.

mod arrange;
mod assets;
mod clay_edit;
mod code;
mod contour;
mod convert;
mod cost;
mod dialog_graph;
mod doc;
mod endless;
mod explorer;
pub(crate) mod files;
mod geom;
mod labels;
mod layout;
mod luau_text;
mod model_params;
mod outline;
mod overlay;
mod parts;
pub(crate) mod project;
mod project_files;
mod ron_edit;
mod shape_edit;
mod shapes;
mod space_edit;
mod story_play;
mod story_refs;
mod story_ui;
mod theme;
mod thumbs;
mod ui;
mod viewport;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use reyvec_gpu::{DemoScene, FrameStatus, GpuOptions, RendererOptions, UiFrame, WindowRenderer};
use reyvec_vector::{Camera2D, Point, Vec2};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use doc::SceneDoc;
use reyvec_app::for_editor::{Controls, Input, controls_path};
use reyvec_app::for_editor::{input_of, to_screen};
use reyvec_app::{AppError, RunSummary};
use ui::{Action, EditorUi, Info, PlayState};

/// Как часто пересчитывать счётчик кадров.
const STATS_INTERVAL: Duration = Duration::from_millis(500);
/// Во сколько раз приближает один щелчок колеса мыши.
const ZOOM_STEP: f64 = 1.2;
/// Сторона кадра сцены, пока её закрывает проводник «Ассетов», пикселей.
const HIDDEN_SCENE_PX: u32 = 64;
/// Пока тянут ручку, сцена в движке пересобирается не чаще этого.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(60);
/// Как часто перечитывать список уровней проекта (их могли создать в проводнике).
const LEVELS_EVERY: Duration = Duration::from_secs(3);

/// Настройки запуска редактора.
#[derive(Clone, Debug)]
pub struct EditorConfig {
    /// Заголовок окна (виден на панели задач).
    pub title: String,
    /// Видеокарта и валидация.
    pub gpu: GpuOptions,
    /// Вертикальная синхронизация.
    pub vsync: bool,
    /// Окно без рамки системы (по умолчанию). `false` — обычная рамка Windows, запасной
    /// путь, если без рамки что-то не так.
    pub frameless: bool,
    /// Файл сцены, который открыть.
    pub scene_file: Option<PathBuf>,
    /// Модель, которую сразу открыть во «Формах» (`--shape`).
    pub shape: Option<PathBuf>,
    /// Сразу открыть «Ассеты» с проводником (`--assets`, для проверок).
    pub assets: bool,
    /// Папка проекта (в ней `assets` и `scenes`), для вкладки «Ассеты».
    pub project: Option<PathBuf>,
    /// Закрыть окно через столько секунд (для автопроверок).
    pub max_seconds: Option<f64>,
    /// Закрыть окно после стольких кадров (для автопроверок).
    pub max_frames: Option<u64>,
    /// Показать окно «Проекты» сразу после запуска (П.1).
    pub start_screen: bool,
    /// Сохранить снимок окна в этот файл и закрыть окно (для проверки в CI).
    pub snapshot: Option<PathBuf>,
    /// Через сколько секунд после открытия делать снимок.
    pub snapshot_after: f64,
}

/// Чем закончилась работа редактора.
pub struct EditorExit {
    pub summary: RunSummary,
    /// Открыть этот проект: редактор перезапускается на его папке (П.1).
    pub reopen: Option<PathBuf>,
}

/// Открывает редактор и работает, пока его не закроют.
pub fn run_editor(config: EditorConfig) -> Result<EditorExit, AppError> {
    let event_loop = EventLoop::new().map_err(|e| AppError::Window(e.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let scene_text = config
        .scene_file
        .as_ref()
        .and_then(|p| match std::fs::read_to_string(p) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::error!("Не прочитать файл сцены «{}»: {e}", p.display());
                None
            }
        });
    let doc = scene_text.map(|t| SceneDoc::new(config.scene_file.clone(), t));
    let restore = config
        .scene_file
        .as_ref()
        .zip(doc.as_ref())
        .and_then(|(p, d)| files::pending_autosave(p, d.text()));
    if restore.is_some() {
        tracing::warn!(
            "Есть автосохранение с несохранёнными правками: «Файл → Восстановить автосохранение»"
        );
    }
    let scenes = levels_dir(&config)
        .map(|d| files::list_levels(&d))
        .unwrap_or_default();
    let mut ui = EditorUi::new(config.project.as_deref());
    if config.start_screen {
        ui.open_projects(true);
    }
    let mut editor = Editor {
        reopen: None,
        ui,
        config,
        window: None,
        renderer: None,
        egui_ctx: egui::Context::default(),
        egui_state: None,
        play: PlayState::Edit,
        doc,
        engine_stale: false,
        engine_sent: Instant::now(),
        clipboard: None,
        hidden: HashSet::new(),
        ejected: false,
        simulating: false,
        time_scale: 1.0,
        watcher: None,
        controls: Controls::load(&controls_path()),
        start: Instant::now(),
        anim: AnimClock::default(),
        frames: 0,
        stats_at: Instant::now(),
        stats_frames: 0,
        fps: 0.0,
        frame_ms: 0.0,
        gpu_ms: None,
        gpu_avg: reyvec_gpu::GpuTimingAverage::default(),
        gpu_passes: Vec::new(),
        scene_post_aa: None,
        adapter: String::new(),
        error: None,
        closing: false,
        closed_at: None,
        minimized: false,
        centered: false,
        cursor: None,
        endless: false,
        endless_cursor: endless::EndlessCursor::default(),
        snapshot_requested: false,
        scenes,
        restore,
        autosave_at: Instant::now(),
        autosaved: String::new(),
        shape: None,
        parked: None,
        shape_in_assets: false,
        levels_at: Instant::now(),
        shape_undo: Vec::new(),
        shape_redo: Vec::new(),
        clay_stroke: None,
        clay_written: Instant::now(),
        shape_drag: None,
        shape_pending: None,
        shape_written: Instant::now(),
    };
    event_loop
        .run_app(&mut editor)
        .map_err(|e| AppError::Window(e.to_string()))?;
    if let Some(e) = editor.error.take() {
        return Err(e);
    }
    let end = editor.closed_at.unwrap_or_else(Instant::now);
    let seconds = end.duration_since(editor.start).as_secs_f64();
    Ok(EditorExit {
        summary: RunSummary {
            frames: editor.frames,
            adapter: editor.adapter.clone(),
            average_fps: if seconds > 0.0 {
                editor.frames as f64 / seconds
            } else {
                0.0
            },
            gpu: None,
        },
        reopen: editor.reopen.take(),
    })
}

/// Модель или уровень 3D, отложенные, пока открыта другая вкладка: с их историей правок.
struct ShapeSlot {
    view: shapes::ShapeView,
    undo: Vec<clay_edit::ShapeStep>,
    redo: Vec<clay_edit::ShapeStep>,
}

/// Папка уровней проекта: `scenes` проекта, иначе папка открытой сцены.
fn levels_dir(config: &EditorConfig) -> Option<PathBuf> {
    config
        .project
        .as_ref()
        .map(|p| p.join("scenes"))
        .filter(|d| d.is_dir())
        .or_else(|| {
            config
                .scene_file
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        })
}

struct Editor {
    config: EditorConfig,
    /// Проект, который открыть после закрытия (перезапуском).
    reopen: Option<PathBuf>,
    ui: EditorUi,
    renderer: Option<WindowRenderer>,
    window: Option<Arc<Window>>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    play: PlayState,
    /// Открытая сцена: текст с правками, разбор, отмена. «Стоп» начинает сцену с него.
    doc: Option<SceneDoc>,
    /// Текст сцены поменялся, а движок его ещё не получил (правку тянут мышью).
    engine_stale: bool,
    /// Когда движок последний раз получил текст сцены.
    engine_sent: Instant,
    /// Предмет, скопированный в редакторе (текст из файла сцены).
    clipboard: Option<String>,
    /// Скрытые в редакторе предметы: движку они не отдаются, в файле остаются.
    hidden: HashSet<Vec<usize>>,
    /// Камера отцеплена от героя во время игры (F8).
    ejected: bool,
    /// Симуляция: игра идёт, но герой не слушает клавиш, камера свободна.
    simulating: bool,
    /// Скорость игры (замедление).
    time_scale: f64,
    watcher: Option<reyvec_scene::SceneWatcher>,
    /// Клавиши героя из настроек игрока (`controls.ron`), как в песочнице.
    controls: Controls,
    start: Instant,
    /// Время анимации сцены: идёт только в игре и симуляции.
    anim: AnimClock,
    frames: u64,
    stats_at: Instant,
    stats_frames: u32,
    fps: f64,
    frame_ms: f64,
    gpu_ms: Option<f64>,
    /// Замеры видеокарты за секунду и самые долгие проходы (весь кадр первым).
    gpu_avg: reyvec_gpu::GpuTimingAverage,
    gpu_passes: Vec<(&'static str, f64)>,
    /// Сглаживание по картинке у сцены до открытия «Форм» (на подставке оно включается,
    /// после закрытия возвращается как было).
    scene_post_aa: Option<bool>,
    adapter: String,
    error: Option<AppError>,
    closing: bool,
    closed_at: Option<Instant>,
    minimized: bool,
    /// Камеру уже поставили серединой сцены в середину свободного места.
    centered: bool,
    cursor: Option<PhysicalPosition<f64>>,
    /// Тянут число или камеру без края экрана (по прошлому кадру интерфейса).
    endless: bool,
    endless_cursor: endless::EndlessCursor,
    snapshot_requested: bool,
    /// Уровни проекта (меню «Файл → Открыть уровень»).
    scenes: Vec<files::Level>,
    /// Когда последний раз перечитывали список уровней.
    levels_at: Instant,
    /// Автосохранение с правками, которых нет в файле (можно восстановить).
    restore: Option<String>,
    /// Когда последний раз проверяли, не пора ли автосохранить.
    autosave_at: Instant,
    /// Что записано в автосохранение последним.
    autosaved: String,
    /// Что показано в 3D сейчас: модель во «Формах» «Ассетов» (М.2, на подставке) или
    /// уровень 3D в «Мире» (Р.12) — смотря какая сторона открыта, см. `shape_in_assets`.
    shape: Option<shapes::ShapeView>,
    /// Модель или уровень 3D другой стороны: в «Ассетах» здесь ждёт уровень «Мира»,
    /// в «Мире» — модель «Ассетов». Переключили вкладку — они меняются местами.
    parked: Option<ShapeSlot>,
    /// `shape` — модель «Ассетов» (иначе — уровень 3D «Мира»).
    shape_in_assets: bool,
    /// История правок модели (М.3): тексты файла до правки.
    shape_undo: Vec<clay_edit::ShapeStep>,
    shape_redo: Vec<clay_edit::ShapeStep>,
    /// Проход кистью по глине, который ещё тянут (М.9б).
    clay_stroke: Option<clay_edit::Stroke>,
    clay_written: Instant,
    /// Текст модели до правки, которую ещё тянут мышью.
    shape_drag: Option<String>,
    /// Правка модели, которую ещё не записали (пишется не чаще [`PREVIEW_INTERVAL`]).
    shape_pending: Option<String>,
    shape_written: Instant,
}

impl Editor {
    fn close(&mut self, event_loop: &ActiveEventLoop) {
        if !self.closing {
            self.closed_at = Some(Instant::now());
        }
        self.closing = true;
        event_loop.exit();
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: AppError) {
        self.error = Some(error);
        self.close(event_loop);
    }

    /// Отдаёт движку текст сцены из документа: предметы на местах из него. `now: false`
    /// — правку ещё тянут, движок получит её не чаще [`PREVIEW_INTERVAL`].
    fn send_scene(&mut self, now: bool) {
        self.engine_stale = true;
        if !now && self.engine_sent.elapsed() < PREVIEW_INTERVAL {
            return;
        }
        let (Some(r), Some(doc)) = (self.renderer.as_mut(), &self.doc) else {
            return;
        };
        let camera = r.camera();
        r.switch_scene_file(&engine_text(doc.text(), &self.hidden));
        r.set_camera(camera);
        self.engine_stale = false;
        self.engine_sent = Instant::now();
        self.ui.scene_changed(doc.file());
    }

    /// Правка, отмена или повтор поменяли текст сцены.
    fn doc_changed(&mut self, now: bool) {
        if let Some(doc) = &self.doc {
            self.ui.scene_changed(doc.file());
        }
        self.send_scene(now);
    }

    /// Открыть модель формы на подставке (М.2) во «Ассетах» или уровень 3D во «Мире»
    /// (Р.12): уровень заменяет прежний уровень «Мира».
    fn open_shape(&mut self, path: PathBuf) {
        if self.play != PlayState::Edit {
            tracing::warn!("Во время игры модель не открыть: сначала «Стоп»");
            return;
        }
        let level = files::level_kind(&path) == Some(true);
        if level {
            return self.open_level(path, false);
        }
        let shape = match shapes::ShapeView::open(path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("{e}");
                return;
            }
        };
        self.ui.open_workspace(layout::ASSETS_WORKSPACE);
        self.sync_side();
        self.put_shape(shape);
    }

    /// Модель или уровень 3D — в окно сцены вместо того, что там было на этой стороне.
    fn put_shape(&mut self, shape: shapes::ShapeView) {
        self.flush_shape();
        self.shape = None;
        self.shape_undo.clear();
        self.shape_redo.clear();
        if self.show_shape(shape) {
            let s = self.shape.as_ref().map(|s| (s.scene, s.name.clone()));
            if let Some((scene, name)) = s {
                if scene {
                    tracing::info!("Мир: уровень 3D {name}");
                } else {
                    tracing::info!("Формы: модель {name}");
                }
            }
        }
    }

    /// Показать модель или уровень 3D: движок рисует сцену «3D».
    fn show_shape(&mut self, mut shape: shapes::ShapeView) -> bool {
        let Some(r) = self.renderer.as_mut() else {
            return false;
        };
        // Вернулись с другой вкладки — из текста в памяти (у уровня там несохранённые
        // правки), впервые — из файла.
        let scene = if shape.text.is_empty() {
            shape.load()
        } else {
            shape.rebuild()
        };
        match scene {
            Some(text) => r.set_space_file_in(&text, Some(shape.root.clone())),
            None => tracing::warn!(
                "Модель {} не прочиталась: {}",
                shape.name,
                shape.error.as_deref().unwrap_or("")
            ),
        }
        r.set_scene(DemoScene::Space);
        r.set_space_camera(shape.camera());
        // Края лучами — самый дорогой проход на подставке: сглаживаем по картинке.
        if self.scene_post_aa.is_none() {
            self.scene_post_aa = Some(r.post_aa());
            r.set_post_aa(true);
        }
        self.shape = Some(shape);
        true
    }

    /// Снова сцена 2D (модели или уровня 3D на этой стороне нет).
    fn show_scene_2d(&mut self) {
        if let Some(r) = self.renderer.as_mut() {
            r.set_scene(DemoScene::File);
            if let Some(on) = self.scene_post_aa.take() {
                r.set_post_aa(on);
            }
        }
    }

    /// Дописать недописанную правку модели (кисть, перетаскивание) в её файл.
    fn flush_shape(&mut self) {
        self.clay_end();
        if let Some(t) = self.shape_pending.take() {
            self.write_shape(&t);
        }
        self.shape_drag = None;
        self.clay_stroke = None;
    }

    /// Вкладку переключили между «Ассетами» и остальными: модель «Ассетов» и уровень 3D
    /// «Мира» меняются местами. Во время игры ничего не меняется (играют в «Мире»).
    fn sync_side(&mut self) {
        let assets = self.ui.in_assets();
        if assets == self.shape_in_assets || self.play != PlayState::Edit {
            return;
        }
        self.flush_shape();
        let current = self.shape.take().map(|view| ShapeSlot {
            view,
            undo: std::mem::take(&mut self.shape_undo),
            redo: std::mem::take(&mut self.shape_redo),
        });
        let had = current.is_some();
        let next = std::mem::replace(&mut self.parked, current);
        self.shape_in_assets = assets;
        match next {
            Some(slot) => {
                self.shape_undo = slot.undo;
                self.shape_redo = slot.redo;
                self.show_shape(slot.view);
            }
            None if had => self.show_scene_2d(),
            None => {}
        }
    }

    /// Уровень 3D «Мира» (открыт сейчас или отложен, пока открыты «Ассеты»).
    fn level_3d(&self) -> Option<&shapes::ShapeView> {
        if self.shape_in_assets {
            self.parked.as_ref().map(|s| &s.view)
        } else {
            self.shape.as_ref()
        }
    }

    /// В уровне 3D «Мира» есть несохранённые правки.
    fn level_dirty(&self) -> bool {
        self.level_3d().is_some_and(shapes::ShapeView::dirty)
    }

    /// Сохранить уровень 3D «Мира» (Ctrl+S, Р.13). `false` — не записался.
    fn save_level(&mut self) -> bool {
        let level = if self.shape_in_assets {
            self.parked.as_mut().map(|s| &mut s.view)
        } else {
            self.shape.as_mut()
        };
        let Some(level) = level.filter(|l| l.dirty()) else {
            return true;
        };
        match level.save() {
            Ok(()) => {
                files::drop_autosave(&level.path);
                tracing::info!("Уровень сохранён: {}", level.name);
                self.restore = None;
                true
            }
            Err(e) => {
                tracing::warn!("Уровень {} не сохранён: {e}", level.name);
                false
            }
        }
    }

    /// Открыть уровень (сцену 2D или 3D) во «Мире» вместо прежнего (Р.12). Несохранённые
    /// правки сцены 2D — сначала вопрос, `force` — уже спросили.
    fn open_level(&mut self, path: PathBuf, force: bool) {
        if self.play != PlayState::Edit {
            tracing::warn!("Во время игры другой уровень не открыть: сначала «Стоп»");
            return;
        }
        if !force && (self.doc.as_ref().is_some_and(SceneDoc::dirty) || self.level_dirty()) {
            self.ui.ask_open(path);
            return;
        }
        let Some(three_d) = files::level_kind(&path) else {
            tracing::warn!("«{}» — не уровень (не сцена 2D или 3D)", path.display());
            return;
        };
        self.ui.open_workspace("Мир");
        self.sync_side();
        if three_d {
            let shape = match shapes::ShapeView::open(path.clone()) {
                Ok(s) => s,
                Err(e) => return tracing::warn!("{e}"),
            };
            self.put_shape(shape);
            let text = self
                .shape
                .as_ref()
                .map(|s| s.text.clone())
                .unwrap_or_default();
            self.restore = files::pending_autosave(&path, &text);
            self.autosaved.clear();
            if self.restore.is_some() {
                tracing::warn!(
                    "Есть автосохранение с несохранёнными правками: «Файл → Восстановить автосохранение»"
                );
            }
        } else {
            if !self.open_scene(path.clone()) {
                return;
            }
            if self.shape.is_some() {
                // Прежний уровень 3D уступает место сцене 2D.
                self.flush_shape();
                self.shape = None;
                self.shape_undo.clear();
                self.shape_redo.clear();
                self.show_scene_2d();
            }
        }
        self.remember_level(&path);
        self.refresh_levels();
    }

    /// Запомнить уровень: в следующий раз проект откроется на нём.
    fn remember_level(&self, path: &Path) {
        let Some(project) = &self.config.project else {
            return;
        };
        let at = project::recent_path();
        let mut recent = project::Recent::load(&at);
        recent.set_level(project, path);
        recent.save(&at);
    }

    fn refresh_levels(&mut self) {
        self.scenes = levels_dir(&self.config)
            .map(|d| files::list_levels(&d))
            .unwrap_or_default();
        self.levels_at = Instant::now();
    }

    /// Правки сцены 2D выбросить: документ снова как файл на диске.
    fn revert_doc(&mut self) {
        let Some(path) = self.config.scene_file.clone() else {
            return;
        };
        if let Ok(text) = std::fs::read_to_string(&path) {
            self.doc = Some(SceneDoc::new(Some(path), text));
            self.hidden.clear();
            self.ui.structure_changed();
            self.ui.select(None);
            self.doc_changed(true);
        }
    }

    /// Правка модели из инспектора «Форм», её отмена и повтор (М.3). Правка сразу
    /// пишется в файл модели: подставка показывает её, блокнот видит её.
    fn shape_action(&mut self, action: Action) {
        let Some(shape) = self.shape.as_ref() else {
            return;
        };
        // Во время просмотра уровень не правится (как сцена 2D во время игры).
        if self.play != PlayState::Edit {
            return;
        }
        let current = shape.text.clone();
        let next = match action {
            Action::ShapeText {
                text,
                commit: false,
            } => {
                if self.shape_drag.is_none() {
                    self.shape_drag = Some(current);
                }
                self.shape_pending = Some(text);
                if self.shape_written.elapsed() < PREVIEW_INTERVAL {
                    return;
                }
                self.shape_pending.take()
            }
            Action::ShapeText { text, commit: true } => {
                self.shape_undo.push(clay_edit::ShapeStep::Text(
                    self.shape_drag.take().unwrap_or(current),
                ));
                self.shape_redo.clear();
                self.shape_pending = None;
                Some(text)
            }
            Action::EndEdit => match self.shape_drag.take() {
                Some(before) => {
                    self.shape_undo.push(clay_edit::ShapeStep::Text(before));
                    self.shape_redo.clear();
                    self.shape_pending.take()
                }
                None => None,
            },
            Action::CancelEdit => {
                self.shape_pending = None;
                self.shape_drag.take()
            }
            Action::Undo => {
                let Some(before) = self.shape_undo.pop() else {
                    return;
                };
                let back = self.shape_step(before, current);
                self.shape_redo.push(back);
                None
            }
            Action::Redo => {
                let Some(after) = self.shape_redo.pop() else {
                    return;
                };
                let back = self.shape_step(after, current);
                self.shape_undo.push(back);
                None
            }
            Action::ToClay { path, room } => {
                self.turn_to_clay(&path, room);
                None
            }
            Action::ClayGrow { file, room } => {
                self.clay_grow(&file, room);
                None
            }
            Action::ClayDab(dab) => {
                self.clay_dab(&dab);
                None
            }
            Action::ClayEnd => {
                self.clay_end();
                None
            }
            _ => None,
        };
        if let Some(text) = next {
            self.write_shape(&text);
        }
    }

    /// Записать текст модели в её файл и показать на подставке.
    fn write_shape(&mut self, text: &str) {
        let (Some(shape), Some(r)) = (self.shape.as_mut(), self.renderer.as_mut()) else {
            return;
        };
        self.shape_written = Instant::now();
        match shape.edit(text) {
            Some(scene) => r.set_space_file_in(&scene, Some(shape.root.clone())),
            None => tracing::warn!(
                "Модель {}: {}",
                shape.name,
                shape.error.as_deref().unwrap_or("")
            ),
        }
    }

    /// Модель на подставке: перечитать сохранённый файл, повернуть камеру.
    fn update_shape(&mut self) {
        if self.shape_written.elapsed() >= PREVIEW_INTERVAL
            && let Some(t) = self.shape_pending.take()
        {
            self.write_shape(&t);
        }
        let (Some(shape), Some(r)) = (self.shape.as_mut(), self.renderer.as_mut()) else {
            return;
        };
        if shape.changed() {
            if let Some(text) = shape.load() {
                r.set_space_file_in(&text, Some(shape.root.clone()));
                tracing::info!("Модель {} перечитана", shape.name);
            } else {
                tracing::warn!(
                    "Модель {}: {}",
                    shape.name,
                    shape.error.as_deref().unwrap_or("")
                );
            }
        }
        r.set_space_camera(shape.camera());
    }

    /// Камера сцены в точках интерфейса.
    fn view(&self) -> Option<viewport::View> {
        let (r, w) = (self.renderer.as_ref()?, self.window.as_ref()?);
        let size = w.inner_size();
        let ppp = f64::from(self.egui_ctx.pixels_per_point());
        let c = r.camera();
        Some(viewport::View {
            center: [c.center.x, c.center.y],
            zoom: c.zoom,
            size: [f64::from(size.width) / ppp, f64::from(size.height) / ppp],
        })
    }

    fn scene_name(&self) -> String {
        self.config
            .scene_file
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(
                || "встроенный пример".to_owned(),
                |n| n.to_string_lossy().into_owned(),
            )
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), AppError> {
        let attributes = Window::default_attributes()
            .with_title(&self.config.title)
            .with_decorations(!self.config.frameless)
            .with_inner_size(LogicalSize::new(1600.0, 900.0))
            .with_min_inner_size(LogicalSize::new(960.0, 600.0))
            .with_maximized(true);
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| AppError::Window(e.to_string()))?,
        );
        let size = window.inner_size();
        let files = reyvec_gpu::UserFiles {
            scene: self.doc.as_ref().map(|d| d.text().into()),
            scene_dir: self
                .config
                .scene_file
                .as_ref()
                .and_then(|p| p.parent().map(ToOwned::to_owned)),
            ..Default::default()
        };
        let mut renderer = WindowRenderer::new(
            window.clone(),
            [size.width, size.height],
            &RendererOptions {
                gpu: self.config.gpu.clone(),
                vsync: self.config.vsync,
                hot_reload: false,
                files,
            },
        )?;
        self.adapter = renderer.adapter().name.clone();
        renderer.set_scene(DemoScene::File);
        let msaa = renderer.msaa_default();
        renderer.set_msaa(msaa);
        renderer.set_game_paused(true);
        renderer.set_follow(true);
        tracing::info!(
            "Редактор открыт: {}×{}, {}, {}",
            size.width,
            size.height,
            renderer.present_mode_name(),
            if self.config.frameless {
                "окно без рамки"
            } else {
                "окно с рамкой системы"
            }
        );
        if let Some(path) = &self.config.scene_file {
            match reyvec_scene::SceneWatcher::new(path) {
                Ok(w) => self.watcher = Some(w),
                Err(e) => {
                    tracing::warn!("Не удалось следить за файлом сцены {}: {e}", path.display())
                }
            }
        }
        self.egui_ctx.set_fonts(reyvec_app::for_editor::fonts());
        theme::apply(&self.egui_ctx);
        self.egui_state = Some(egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            Some(winit::window::Theme::Dark),
            Some(4096),
        ));
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.start = Instant::now();
        self.stats_at = self.start;
        Ok(())
    }

    /// Сдвигает камеру так, чтобы середина кадра была посередине свободного места.
    fn center_on_viewport(&mut self) {
        let (Some(r), Some(w), Some(view)) =
            (self.renderer.as_mut(), &self.window, self.ui.viewport())
        else {
            return;
        };
        let size = w.inner_size();
        let ppp = self.egui_ctx.pixels_per_point() as f64;
        let c = view.center();
        let at = PhysicalPosition::new(f64::from(c.x) * ppp, f64::from(c.y) * ppp);
        let mid = PhysicalPosition::new(f64::from(size.width) / 2.0, f64::from(size.height) / 2.0);
        let mut camera = r.camera();
        camera.pan_screen(to_screen(at, size) - to_screen(mid, size));
        r.set_camera(camera);
    }

    fn apply(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        if self.shape.is_some() {
            match action {
                Action::ShapeText { .. }
                | Action::Undo
                | Action::Redo
                | Action::EndEdit
                | Action::CancelEdit
                | Action::ToClay { .. }
                | Action::ClayGrow { .. }
                | Action::ClayDab(_)
                | Action::ClayEnd => return self.shape_action(action),
                // Ctrl+S: уровень 3D сохраняется, как сцена 2D (модель пишется сразу).
                Action::Save => {
                    if !self.shape_in_assets {
                        self.save_level();
                    }
                    return;
                }
                // Правки сцены, пока открыта модель, сцену не трогают (Delete, Ctrl+C…).
                Action::Edit { .. }
                | Action::Copy { .. }
                | Action::Paste { .. }
                | Action::Duplicate(_)
                | Action::Delete(_)
                | Action::Move { .. }
                | Action::DropAsset { .. }
                | Action::MoveMany(_)
                | Action::Group(_)
                | Action::Ungroup(_)
                | Action::AddPart { .. }
                | Action::RemovePart { .. }
                | Action::SavePrefab(_) => return,
                _ => {}
            }
        }
        let level_open = self.level_3d().is_some();
        let (Some(r), Some(w)) = (self.renderer.as_mut(), self.window.clone()) else {
            return;
        };
        match action {
            Action::ShapeText { .. }
            | Action::ToClay { .. }
            | Action::ClayGrow { .. }
            | Action::ClayDab(_)
            | Action::ClayEnd => {}
            Action::Play | Action::Simulate => {
                if let Some(level) = self.shape.as_ref() {
                    // Уровень 3D (Р.14): героя в 3D у движка пока нет — идут анимации,
                    // камера летает. Спрятанное в редакторе в игре видно.
                    r.set_space_file_in(&level.text, Some(level.root.clone()));
                    self.play = PlayState::Playing;
                    tracing::info!(
                        "Просмотр уровня 3D: W A S D — лететь, Q и E — вниз и вверх, \
                         мышь — оглядеться, Esc или F5 — назад к правке"
                    );
                    return;
                }
                let simulate = action == Action::Simulate;
                let camera = r.camera();
                if !self.hidden.is_empty()
                    && let Some(doc) = &self.doc
                {
                    // Скрытое — только в редакторе: в игре сцена целиком.
                    r.switch_scene_file(doc.text());
                }
                self.play = PlayState::Playing;
                self.ejected = false;
                r.set_game_paused(false);
                r.set_follow(!simulate);
                if simulate {
                    // Камера остаётся, где была: смотрим, как падают и катятся предметы.
                    r.set_camera(camera);
                }
                self.simulating = simulate;
                self.ejected = simulate;
                if let Some(logic) = r.scene_logic() {
                    logic.set_debug_paused(false);
                    logic.set_time_scale(self.time_scale);
                }
                if simulate {
                    tracing::info!("Симуляция: физика и логика идут, герой стоит");
                } else {
                    tracing::info!("Игра запущена");
                }
            }
            Action::Pause => {
                self.play = PlayState::Paused;
                if let Some(logic) = r.scene_logic() {
                    logic.set_debug_paused(false);
                }
                r.release_game_input();
                r.set_game_paused(true);
            }
            Action::Resume => {
                self.play = PlayState::Playing;
                if let Some(logic) = r.scene_logic() {
                    logic.set_debug_paused(false);
                }
                r.set_game_paused(false);
            }
            Action::Stop => {
                self.play = PlayState::Edit;
                self.ejected = false;
                self.simulating = false;
                if let Some(level) = self.shape.as_ref() {
                    // Уровень 3D с начала: анимации на месте из файла, спрятанное снова
                    // спрятано. Камера остаётся, где летала.
                    self.anim = AnimClock::default();
                    r.set_space_file_in(&level.stand_scene(), Some(level.root.clone()));
                    tracing::info!("Просмотр остановлен, уровень с начала");
                    return;
                }
                if let Some(logic) = r.scene_logic() {
                    logic.set_debug_paused(false);
                }
                r.release_game_input();
                let camera = r.camera();
                if let Some(doc) = &self.doc {
                    // Сцена с начала: герой, тела и враги на местах из файла (с правками).
                    r.switch_scene_file(&engine_text(doc.text(), &self.hidden));
                }
                r.set_game_paused(true);
                r.set_follow(false);
                r.set_camera(camera);
                tracing::info!("Игра остановлена, сцена с начала");
            }
            Action::Minimize => w.set_minimized(true),
            Action::ToggleMaximize => w.set_maximized(!w.is_maximized()),
            Action::Close => self.close_or_ask(event_loop),
            Action::ShowProjects => self.ui.open_projects(false),
            Action::OpenProject(dir) => {
                // Окно «Проекты» само не даёт открыть другой проект с несохранённым.
                tracing::info!("Открываю проект «{}»: перезапуск редактора", dir.display());
                self.reopen = Some(dir);
                self.close(event_loop);
            }
            Action::ForceClose => self.close(event_loop),
            Action::SaveAndClose => {
                if let Err(e) = self.ui.save_code() {
                    tracing::warn!("{e}, окно остаётся");
                    return;
                }
                let saved = self.doc.as_mut().filter(|d| d.dirty()).map(SceneDoc::save);
                match saved {
                    Some(Err(e)) => tracing::warn!("Сцена не сохранена, окно остаётся: {e}"),
                    _ if !self.save_level() => {}
                    _ => self.close(event_loop),
                }
            }
            Action::DragWindow => {
                if let Err(e) = w.drag_window() {
                    tracing::debug!("Окно не тащится: {e}");
                }
            }
            Action::Resize(dir) => {
                if let Err(e) = w.drag_resize_window(dir) {
                    tracing::debug!("Окно не растягивается: {e}");
                }
            }
            Action::OpenShape(path) => self.open_shape(path),
            Action::OpenFile(path) => open_in_system(&path, false),
            Action::ShowInFolder(path) => open_in_system(&path, true),
            Action::CloseShape => {
                // Уровень «Мира» не закрывают: вместо него открывают другой.
                if !self.shape_in_assets {
                    return;
                }
                self.flush_shape();
                self.shape_undo.clear();
                self.shape_redo.clear();
                if self.shape.take().is_some() {
                    self.show_scene_2d();
                    tracing::info!("Формы закрыты, снова проводник");
                }
            }
            Action::ShapeHide(path, hide) => {
                if let Some(shape) = self.shape.as_mut() {
                    let text = shape.set_hidden(&path, hide);
                    r.set_space_file_in(&text, Some(shape.root.clone()));
                }
            }
            Action::ShapePreview(values) => {
                if let Some(shape) = self.shape.as_mut() {
                    let text = shape.set_preview(values);
                    r.set_space_file_in(&text, Some(shape.root.clone()));
                }
            }
            Action::SpaceScale(scale) => {
                r.set_space_scale(scale);
                tracing::info!("Разрешение 3D: {}", scale.title());
            }
            Action::PostAa(on) => {
                r.set_post_aa(on);
                tracing::info!(
                    "Сглаживание 3D: {}",
                    if on {
                        "по картинке"
                    } else {
                        "лучами"
                    }
                );
            }
            Action::SpaceTriangles(on) => {
                r.set_space_triangles(on);
                tracing::info!("Треугольники: {}", if on { "вкл" } else { "выкл" });
            }
            Action::ShapeCamera(c) => {
                if let Some(shape) = self.shape.as_mut() {
                    shape_camera(shape, c);
                }
            }
            Action::Pan(d) => {
                let size = w.inner_size();
                let ppp = f64::from(self.egui_ctx.pixels_per_point());
                let h = f64::from(size.height.max(1));
                let delta = Vec2::new(
                    2.0 * f64::from(d.x) * ppp / h,
                    -2.0 * f64::from(d.y) * ppp / h,
                );
                let mut camera = r.camera();
                camera.pan_screen(delta);
                r.set_camera(camera);
                r.set_follow(false);
            }
            Action::ResetCamera => {
                r.set_follow(false);
                r.set_camera(Camera2D::default());
                self.center_on_viewport();
            }
            Action::ReloadScene => {
                let Some(path) = self.config.scene_file.clone() else {
                    return;
                };
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        if let Some(doc) = self.doc.as_mut() {
                            doc.file_changed(text);
                        } else {
                            self.doc = Some(SceneDoc::new(Some(path.clone()), text));
                        }
                        self.doc_changed(true);
                        tracing::info!("Сцена перечитана: {}", path.display());
                    }
                    Err(e) => tracing::warn!("Не прочитать файл сцены: {e}"),
                }
            }
            Action::SetCamera { center, zoom } => {
                r.set_follow(false);
                r.set_camera(Camera2D {
                    center: Point::new(center[0], center[1]),
                    zoom,
                });
            }
            Action::Edit { .. }
            | Action::CancelEdit
            | Action::EndEdit
            | Action::Undo
            | Action::Redo
            | Action::Save
            | Action::Copy { .. }
            | Action::Paste { .. }
            | Action::Duplicate(_)
            | Action::Delete(_)
            | Action::Move { .. }
            | Action::DropAsset { .. }
            | Action::MoveMany(_)
            | Action::Group(_)
            | Action::Ungroup(_)
            | Action::AddPart { .. }
            | Action::RemovePart { .. } => self.apply_doc(action),
            Action::SavePrefab(path) => self.save_prefab(&path),
            Action::OpenScene(path) => self.open_level(path, false),
            Action::SaveAndOpen(path) => {
                match self.doc.as_mut().filter(|d| d.dirty()).map(SceneDoc::save) {
                    Some(Err(e)) => tracing::warn!("Сцена не сохранена: {e}"),
                    _ if !self.save_level() => {}
                    _ => self.open_level(path, true),
                }
            }
            Action::DiscardAndOpen(path) => {
                self.revert_doc();
                self.open_level(path, true);
            }
            Action::NewScene => {
                let Some(dir) = self.config.scene_file.as_deref().and_then(|p| p.parent()) else {
                    return;
                };
                let path = files::new_scene_path(dir);
                match std::fs::write(&path, files::NEW_SCENE) {
                    Ok(()) => self.open_level(path, false),
                    Err(e) => tracing::warn!("Не создать сцену «{}»: {e}", path.display()),
                }
            }
            Action::RestoreAutosave if level_open => {
                if let Some(t) = self.restore.take() {
                    self.ui.open_workspace("Мир");
                    self.sync_side();
                    self.shape_action(Action::ShapeText {
                        text: t,
                        commit: true,
                    });
                    tracing::info!("Правки из автосохранения вернулись (Ctrl+Z — отменить)");
                }
            }
            Action::RestoreAutosave => {
                if let (Some(t), Some(doc)) = (self.restore.take(), self.doc.as_mut()) {
                    match doc.change(|_| Ok(t)) {
                        Ok(()) => {
                            self.hidden.clear();
                            self.ui.structure_changed();
                            self.ui.select(None);
                            self.doc_changed(true);
                            tracing::info!(
                                "Правки из автосохранения вернулись (Ctrl+Z — отменить)"
                            );
                        }
                        Err(e) => tracing::warn!("Автосохранение не читается: {e}"),
                    }
                }
            }
            Action::Eject => {
                if self.play != PlayState::Edit {
                    self.ejected = !self.ejected;
                    r.set_follow(!self.ejected);
                }
            }
            Action::Step => {
                if self.play == PlayState::Paused
                    && let Some(logic) = r.scene_logic()
                {
                    // Пауза консоли умеет шагать: обычную снимаем, шаг — один.
                    logic.set_debug_paused(true);
                    logic.set_paused(false);
                    logic.step(1);
                }
            }
            Action::SetTimeScale(k) => {
                self.time_scale = match r.scene_logic() {
                    Some(logic) => logic.set_time_scale(k),
                    None => k,
                };
            }
            Action::ToggleHidden(path) => {
                if !self.hidden.remove(&path) {
                    self.hidden.insert(path);
                }
                self.send_scene(true);
            }
        }
    }

    /// Закрыть окно; если есть несохранённые правки — сначала спросить.
    /// Несохранённые правки раз в минуту — в `.autosave` рядом со сценой (сам файл сцены
    /// не трогается). Сохранили сцену — автосохранение убирается.
    fn autosave(&mut self) {
        const EVERY: Duration = Duration::from_secs(60);
        if self.autosave_at.elapsed() < EVERY {
            return;
        }
        self.autosave_at = Instant::now();
        // Уровень 3D — его правки, иначе сцена 2D.
        let (path, text) = match (self.level_3d(), &self.doc, &self.config.scene_file) {
            (Some(l), _, _) if l.dirty() => (l.path.clone(), l.text.clone()),
            (Some(_), _, _) => return,
            (None, Some(doc), Some(path)) if doc.dirty() => (path.clone(), doc.text().to_owned()),
            _ => return,
        };
        if text == self.autosaved {
            return;
        }
        match files::write_autosave(&path, &text) {
            Ok(()) => self.autosaved = text,
            Err(e) => tracing::warn!("Автосохранение не записалось: {e}"),
        }
    }

    /// Открыть другую сцену 2D (правки прошлой уже сохранены или выброшены). Её папка
    /// может быть любой: пути скриптов и картинок считаются от неё.
    fn open_scene(&mut self, path: PathBuf) -> bool {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("Не прочитать сцену «{}»: {e}", path.display());
                return false;
            }
        };
        self.restore = files::pending_autosave(&path, &text);
        self.doc = Some(SceneDoc::new(Some(path.clone()), text));
        self.watcher = reyvec_scene::SceneWatcher::new(&path)
            .map_err(|e| tracing::warn!("Не следить за файлом сцены: {e}"))
            .ok();
        self.config.scene_file = Some(path.clone());
        self.hidden.clear();
        self.autosaved.clear();
        self.ui.structure_changed();
        self.ui.select(None);
        // Камера встанет на героя новой сцены и потом — серединой в свободное место.
        if let Some(r) = self.renderer.as_mut() {
            r.set_follow(true);
            r.set_scene_dir(path.parent().map(Path::to_path_buf));
        }
        self.centered = false;
        self.doc_changed(true);
        tracing::info!("Сцена открыта: {}", self.scene_name());
        if self.restore.is_some() {
            tracing::warn!(
                "Есть автосохранение с несохранёнными правками: «Файл → Восстановить автосохранение»"
            );
        }
        true
    }

    /// Предмет — в файл `assets/prefabs/<имя>.ron` (имя не занимает чужой файл).
    fn save_prefab(&mut self, path: &[usize]) {
        let (Some(doc), Some(project)) = (&self.doc, self.config.project.clone()) else {
            tracing::warn!("Заготовку некуда записать: нет папки проекта");
            return;
        };
        let text = match doc.node_text(path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("Не записать заготовку: {e}");
                return;
            }
        };
        let name = doc
            .file()
            .and_then(|f| outline::node_at(f, path))
            .map(|n| n.name.clone())
            .unwrap_or_default();
        let dir = project.join("assets").join(assets::PREFABS);
        let base = parts::prefab_file_name(&name);
        let mut file = dir.join(format!("{base}.ron"));
        let mut k = 2;
        while file.exists() {
            file = dir.join(format!("{base} {k}.ron"));
            k += 1;
        }
        let written =
            std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&file, format!("{text}\n")));
        match written {
            Ok(()) => {
                tracing::info!("Заготовка записана: {}", file.display());
                self.ui.rescan_assets(Some(&project));
            }
            Err(e) => tracing::warn!("Не записать заготовку «{}»: {e}", file.display()),
        }
    }

    fn close_or_ask(&mut self, event_loop: &ActiveEventLoop) {
        if self.doc.as_ref().is_some_and(SceneDoc::dirty)
            || self.level_dirty()
            || self.ui.code_dirty()
        {
            self.ui.ask_close();
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        } else {
            self.close(event_loop);
        }
    }

    /// Правки сцены: только в режиме правки, когда сцена открыта.
    fn apply_doc(&mut self, action: Action) {
        if self.play != PlayState::Edit {
            return;
        }
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let shape_before = doc.file().map(shape);
        let mut select = None;
        let structural = |r: Result<Vec<usize>, ron_edit::EditError>,
                          select: &mut Option<Vec<usize>>| match r {
            Ok(p) => {
                *select = Some(p);
                true
            }
            Err(e) => {
                tracing::warn!("Не вышло: {e}");
                false
            }
        };
        let now = match action {
            Action::Copy { path, cut } => {
                match doc.node_text(&path) {
                    Ok(t) => {
                        self.egui_ctx.copy_text(t.clone());
                        self.clipboard = Some(t);
                    }
                    Err(e) => {
                        tracing::warn!("Не скопировать: {e}");
                        return;
                    }
                }
                if !cut || !structural(doc.delete(&path).map(|()| Vec::new()), &mut select) {
                    return;
                }
                select = None;
                true
            }
            Action::Paste { after, text } => {
                let Some(item) = text.or_else(|| self.clipboard.clone()) else {
                    return;
                };
                let (parent, index) = match after.as_deref().and_then(<[usize]>::split_last) {
                    Some((i, p)) => (p.to_vec(), i + 1),
                    None => (Vec::new(), usize::MAX),
                };
                if !structural(doc.paste(&item, &parent, index), &mut select) {
                    return;
                }
                true
            }
            Action::MoveMany(moves) => {
                if let Err(e) = doc.move_many(&moves) {
                    tracing::warn!("Не выровнять: {e}");
                    return;
                }
                true
            }
            Action::Group(paths) => {
                if !structural(doc.group(&paths), &mut select) {
                    return;
                }
                true
            }
            Action::Ungroup(path) => {
                if !structural(doc.ungroup(&path), &mut select) {
                    return;
                }
                true
            }
            Action::DropAsset { file, at, after } => {
                let prefab = file
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("ron"));
                let item = if prefab {
                    let made = std::fs::read_to_string(&file)
                        .map_err(|e| ron_edit::EditError(e.to_string()))
                        .and_then(|t| parts::prefab_item(&t, at));
                    match made {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::warn!("Заготовка «{}» не читается: {e}", file.display());
                            return;
                        }
                    }
                } else {
                    let dir = doc.dir().unwrap_or(std::path::Path::new(".")).to_owned();
                    parts::picture_item(&dir, &file, at)
                };
                let (parent, index) = (Vec::new(), after.map_or(usize::MAX, |p| p[0] + 1));
                if !structural(doc.paste(&item, &parent, index), &mut select) {
                    return;
                }
                true
            }
            Action::Duplicate(path) => {
                if !structural(doc.duplicate(&path), &mut select) {
                    return;
                }
                true
            }
            Action::Delete(path) => {
                if let Err(e) = doc.delete(&path) {
                    tracing::warn!("Не удалить: {e}");
                    return;
                }
                true
            }
            Action::Move {
                from,
                parent,
                index,
            } => {
                if !structural(doc.move_node(&from, &parent, index), &mut select) {
                    return;
                }
                true
            }
            Action::AddPart { path, field, item } => {
                let r = doc.change(|t| ron_edit::insert_item(t, &path, field, usize::MAX, &item));
                if let Err(e) = r {
                    tracing::warn!("Часть не добавилась: {e}");
                    return;
                }
                true
            }
            Action::RemovePart { path, field, index } => {
                if let Err(e) = doc.change(|t| ron_edit::remove_item(t, &path, field, index)) {
                    tracing::warn!("Часть не убралась: {e}");
                    return;
                }
                true
            }
            Action::Edit { edit, commit } => {
                let done = if commit {
                    doc.commit(&edit)
                } else {
                    doc.preview(&edit)
                };
                if let Err(e) = done {
                    tracing::warn!("Правка не легла в файл сцены: {e}");
                    return;
                }
                commit
            }
            Action::CancelEdit => {
                doc.cancel_preview();
                true
            }
            Action::EndEdit => {
                doc.finish_preview();
                true
            }
            Action::Undo => {
                if !doc.undo() {
                    return;
                }
                true
            }
            Action::Redo => {
                if !doc.redo() {
                    return;
                }
                true
            }
            Action::Save => {
                match doc.save() {
                    Ok(()) => {
                        if let Some(p) = &self.config.scene_file {
                            files::drop_autosave(p);
                        }
                        self.restore = None;
                        tracing::info!("Сцена сохранена: {}", self.scene_name());
                    }
                    Err(e) => tracing::warn!("Сцена не сохранена: {e}"),
                }
                return;
            }
            _ => return,
        };
        if doc.file().map(shape) != shape_before {
            // Номера предметов сдвинулись: скрытые, закреплённые и раскрытые — заново.
            self.hidden.clear();
            self.ui.structure_changed();
            self.ui.select(select);
        }
        self.doc_changed(now);
    }

    /// Файл сцены поменялся на диске: сохранили в блокноте или это наше же сохранение.
    fn file_saved_outside(&mut self, text: String) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        if doc.dirty() && text != doc.text() {
            // Свои правки не теряем: файл с диска можно взять через «Файл → Перечитать».
            tracing::warn!(
                "Файл сцены поменяли на диске, а в редакторе есть несохранённые правки. \
                 Оставляю правки редактора; «ReyVec → Файл → Перечитать сцену» возьмёт файл с диска"
            );
            return;
        }
        if doc.file_changed(text) {
            tracing::info!("Файл сцены сохранён, читаю заново");
            if self.play == PlayState::Edit {
                self.doc_changed(true);
            } else if let Some(r) = self.renderer.as_mut() {
                // Во время игры герой остаётся, где был (как в песочнице).
                r.set_scene_file(doc.text());
            }
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.closing {
            return;
        }
        self.sync_side();
        if self.levels_at.elapsed() > LEVELS_EVERY {
            self.refresh_levels();
        }
        for action in self.ui.take_actions() {
            self.apply(action, event_loop);
            if self.closing {
                return;
            }
        }
        if let Some(watcher) = self.watcher.as_mut() {
            match watcher.poll() {
                Some(Ok(text)) => self.file_saved_outside(text),
                Some(Err(e)) => tracing::warn!("Файл сцены: {e}"),
                None => {}
            }
        }
        if self.engine_stale {
            self.send_scene(false);
        }
        self.update_shape();
        self.autosave();
        if self.frames >= 2 && !self.centered && self.ui.viewport().is_some() {
            // Камера уже встала на героя: дальше она не идёт за ним, пока не нажмут «Играть».
            if let Some(r) = self.renderer.as_mut() {
                r.set_follow(false);
            }
            self.center_on_viewport();
            self.centered = true;
            if let Some(path) = self.config.shape.clone() {
                self.open_shape(path);
            }
            if self.config.assets {
                self.ui.open_workspace(layout::ASSETS_WORKSPACE);
            }
        }
        self.ui.push_log(reyvec_app::for_editor::take_log());

        let view = self.view();
        let level_3d = self.level_3d().map(|s| s.path.clone());
        let level_dirty = self.level_dirty();
        let (Some(window), Some(renderer), Some(state)) =
            (&self.window, &mut self.renderer, &mut self.egui_state)
        else {
            return;
        };
        if self.play == PlayState::Playing
            && let Some(logic) = renderer.scene_logic()
        {
            // Точка прицела (6.12): курсор над сценой в координатах сцены.
            let size = window.inner_size();
            let camera = renderer.camera();
            let aim = self.cursor.filter(|_| self.ui.viewport_hovered()).map(|c| {
                let p = camera.to_scene(to_screen(c, size).to_point());
                [p.x, p.y]
            });
            logic.set_aim(aim);
        }
        let scene_name = level_3d
            .as_ref()
            .or(self.config.scene_file.as_ref())
            .and_then(|p| p.file_name());
        let scene_name = scene_name.map_or_else(
            || "встроенный пример".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let engine_error = renderer
            .shader_error()
            .map(|e| format!("шейдер: {e}"))
            .or_else(|| renderer.scene_error());
        let shape_error = self.shape.as_ref().and_then(|_| renderer.space_error());
        let doc = self.doc.as_ref();
        let error = doc
            .and_then(SceneDoc::error)
            .map(ToOwned::to_owned)
            .or(engine_error);
        if let Some(s) = self.shape.as_mut() {
            s.cost.tick();
        }
        let info = Info {
            play: self.play,
            maximized: window.is_maximized(),
            frameless: self.config.frameless,
            fps: self.fps,
            frame_ms: self.frame_ms,
            gpu_ms: self.gpu_ms,
            adapter: &self.adapter,
            scene: doc.and_then(SceneDoc::file),
            scene_name: &scene_name,
            error: error.as_deref(),
            view,
            dirty: if level_3d.is_some() {
                level_dirty
            } else {
                doc.is_some_and(SceneDoc::dirty)
            },
            can_undo: doc.is_some_and(SceneDoc::can_undo),
            can_redo: doc.is_some_and(SceneDoc::can_redo),
            can_save: self.config.scene_file.is_some(),
            hidden: &self.hidden,
            can_paste: self.clipboard.is_some(),
            scenes: &self.scenes,
            level: level_3d.as_deref().or(self.config.scene_file.as_deref()),
            can_restore: self.restore.is_some(),
            scene_path: self.config.scene_file.as_deref(),
            ejected: self.ejected,
            time_scale: self.time_scale,
            simulating: self.simulating,
            shape: self.shape.as_ref().map(|s| ui::ShapeInfo {
                name: &s.name,
                spin: s.spin,
                free: s.free,
                hidden: &s.hidden,
                scale: renderer.space_scale(),
                triangles: renderer.space_triangles(),
                post_aa: renderer.post_aa(),
                in_view: renderer.view_rect_supported(),
                passes: &self.gpu_passes,
                camera: s.camera_now(),
                shift: s.shift.into(),
                error: s.error.as_deref().or(shape_error.as_deref()),
                text: &s.text,
                preview: &s.preview,
                cost: s.cost.result(),
                scene: s.scene,
                root: &s.root,
            }),
        };
        let input = state.take_egui_input(window);
        let editor_ui = &mut self.ui;
        let out = self.egui_ctx.run_ui(input, |ui| editor_ui.draw(ui, &info));
        self.endless = ui::take_endless_drag(&self.egui_ctx);
        state.handle_platform_output(window, out.platform_output);
        let primitives = self.egui_ctx.tessellate(out.shapes, out.pixels_per_point);
        let ui_frame = UiFrame {
            primitives,
            textures_delta: out.textures_delta,
            pixels_per_point: out.pixels_per_point,
        };

        if let Some(at) = self.config.snapshot.as_ref()
            && !self.snapshot_requested
            && self.start.elapsed().as_secs_f64() >= self.config.snapshot_after
        {
            self.snapshot_requested = true;
            if !renderer.request_snapshot() {
                tracing::error!(
                    "Эта видеокарта не умеет копировать картинку окна: снимка {} не будет",
                    at.display()
                );
                self.error = Some(AppError::Window("снимок окна не поддерживается".into()));
                return self.close(event_loop);
            }
        }

        // На подставке 3D рисуется только в окне просмотра, не под панелями.
        let ppp = out.pixels_per_point;
        let view_rect = self.shape.as_ref().and(self.ui.viewport()).map(|r| {
            let px = |v: f32| (v * ppp).round().max(0.0) as u32;
            let (x, y) = (px(r.left()), px(r.top()));
            [
                x,
                y,
                px(r.right()).saturating_sub(x),
                px(r.bottom()).saturating_sub(y),
            ]
        });
        // Во «Ассетах» сцену закрывает проводник: рисовать её маленькой, чтобы видеокарта
        // не работала зря (кадр сцены всё равно никто не видит).
        let view_rect = view_rect.or(self.ui.scene_hidden().then_some([
            0,
            0,
            HIDDEN_SCENE_PX,
            HIDDEN_SCENE_PX,
        ]));
        renderer.set_view_rect(view_rect);
        let size = window.inner_size();
        // В правке и на паузе анимация сцены стоит: кадр тот же, движку меньше работы.
        let time = self.anim.tick(self.play == PlayState::Playing);
        match renderer.render([size.width, size.height], time, Some(ui_frame)) {
            Ok(FrameStatus::Presented) => {
                self.frames += 1;
                self.stats_frames += 1;
                if let Some(t) = renderer.gpu_timings() {
                    self.gpu_ms = Some(t.total_ms);
                    self.gpu_avg.add(t);
                }
                let now = Instant::now();
                let since = now.duration_since(self.stats_at);
                if since >= STATS_INTERVAL {
                    let s = since.as_secs_f64();
                    self.fps = f64::from(self.stats_frames) / s;
                    self.frame_ms = s * 1000.0 / f64::from(self.stats_frames.max(1));
                    self.stats_frames = 0;
                    self.stats_at = now;
                    if let Some(avg) = self.gpu_avg.average() {
                        let mut passes: Vec<_> =
                            avg.passes.iter().map(|p| (p.name, p.ms)).collect();
                        passes.sort_by(|a, b| b.1.total_cmp(&a.1));
                        passes.truncate(5);
                        passes.insert(0, ("кадр", avg.total_ms));
                        self.gpu_passes = passes;
                    }
                    self.gpu_avg.clear();
                }
                if let (Some(path), Some(image)) =
                    (self.config.snapshot.clone(), renderer.take_snapshot())
                {
                    match reyvec_app::snapshot::save_png(&path, &image) {
                        Ok(()) => tracing::info!(
                            "Снимок редактора {}×{} сохранён: {}",
                            image.width,
                            image.height,
                            path.display()
                        ),
                        Err(e) => {
                            tracing::error!("Снимок не сохранён: {e}");
                            self.error = Some(AppError::Window(e.to_string()));
                        }
                    }
                    return self.close(event_loop);
                }
                if self.config.max_frames.is_some_and(|max| self.frames >= max) {
                    tracing::info!("Нарисовано {} кадров, закрываю редактор", self.frames);
                    return self.close(event_loop);
                }
                if self
                    .config
                    .max_seconds
                    .is_some_and(|max| self.start.elapsed().as_secs_f64() >= max)
                {
                    tracing::info!("Время вышло, закрываю редактор");
                    return self.close(event_loop);
                }
            }
            Ok(FrameStatus::Skipped) => {}
            Err(e) => return self.fail(event_loop, e.into()),
        }
        if !self.minimized {
            window.request_redraw();
        }
    }

    /// Клавиши и кнопки мыши во время игры — герою.
    fn game_input(&mut self, event: &WindowEvent) {
        if self.play != PlayState::Playing || self.simulating {
            return;
        }
        let Some((input, down, repeat)) = input_of(event) else {
            return;
        };
        if repeat {
            return;
        }
        if self.shape.is_some() {
            // Во «Формах» мышь и клавиши — камере подставки, не герою.
            return;
        }
        let over_scene = self.ui.viewport_hovered();
        match input {
            Input::Key(_) if self.egui_ctx.egui_wants_keyboard_input() => return,
            // Камера отцеплена: мышью двигают сцену, а не бьют.
            Input::Mouse(_) | Input::Wheel(_) if self.ejected => return,
            Input::Mouse(_) | Input::Wheel(_) if !over_scene && down => return,
            _ => {}
        }
        let Some(action) = self.controls.action_of(input) else {
            return;
        };
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        let aimed = down
            && input.is_mouse()
            && action == reyvec_scene::game::Action::Attack
            && self.controls.attack_toward_cursor();
        if aimed && let Some(logic) = r.scene_logic() {
            logic.press_aimed(action);
        } else {
            r.game_action(action, down);
        }
        if matches!(input, Input::Wheel(_)) {
            r.game_action(action, false);
        }
    }
}

impl ApplicationHandler for Editor {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match self.create_window(event_loop) {
            Ok(()) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                tracing::info!("Сцена: {}", self.scene_name());
            }
            Err(e) => self.fail(event_loop, e),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let event = match event {
            WindowEvent::CursorMoved {
                device_id,
                position,
            } => WindowEvent::CursorMoved {
                device_id,
                position: self.endless_move(position),
            },
            e => e,
        };
        let mut egui_took = false;
        if let (Some(state), Some(window)) = (self.egui_state.as_mut(), &self.window) {
            egui_took = state.on_window_event(window, &event).consumed;
        }
        self.game_input(&event);
        match event {
            WindowEvent::CloseRequested => self.close_or_ask(event_loop),
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::Resized(size) => {
                self.minimized = size.width == 0 || size.height == 0;
                if let Some(r) = &mut self.renderer {
                    r.resize();
                }
                if let (false, Some(w)) = (self.minimized, &self.window) {
                    w.request_redraw();
                }
            }
            WindowEvent::Focused(false) => {
                if let Some(r) = &mut self.renderer {
                    r.release_game_input();
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.cursor = Some(position),
            WindowEvent::CursorLeft { .. } => self.cursor = None,
            WindowEvent::MouseWheel { delta, .. }
                if (self.play == PlayState::Edit || self.ejected)
                    && self.ui.viewport_hovered()
                    && !egui_took =>
            {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                // Во «Формах» колесо читает само окно подставки (ui.rs).
                if self.shape.is_none()
                    && let (Some(r), Some(w)) = (self.renderer.as_mut(), &self.window)
                {
                    let size = w.inner_size();
                    let at = self
                        .cursor
                        .map_or(Point::ZERO, |c| to_screen(c, size).to_point());
                    let mut camera = r.camera();
                    camera.zoom_at(at, ZOOM_STEP.powf(steps));
                    r.set_camera(camera);
                }
            }
            _ => {}
        }
    }
}

impl Editor {
    /// Курсор без края экрана (`endless.rs`): у края окна переносится на другую сторону,
    /// а интерфейс видит положение «дальше края».
    fn endless_move(&mut self, real: PhysicalPosition<f64>) -> PhysicalPosition<f64> {
        let Some(w) = &self.window else { return real };
        let size = w.inner_size();
        let (shown, warp) = self.endless_cursor.moved(
            [real.x, real.y],
            [f64::from(size.width), f64::from(size.height)],
            self.endless,
        );
        if let Some([x, y]) = warp
            && let Err(e) = w.set_cursor_position(PhysicalPosition::new(x, y))
        {
            tracing::debug!("Курсор не переносится: {e}");
            self.endless = false;
        }
        PhysicalPosition::new(shown[0], shown[1])
    }
}

/// Действие с камерой подставки «Форм».
fn shape_camera(shape: &mut shapes::ShapeView, c: ui::ShapeCam) {
    use ui::ShapeCam;
    match c {
        ShapeCam::Orbit(d) => shape.orbit(d.x, d.y),
        ShapeCam::Pan(d) => shape.pan(d.x, d.y),
        ShapeCam::Zoom(steps) => shape.zoom(steps),
        ShapeCam::Fly(dir, dt) => shape.fly(dir, dt),
        ShapeCam::View(a) => shape.view(a),
        ShapeCam::Focus(path) => {
            if !shape.focus(&path) {
                tracing::info!("У этой детали не узнать размер: показываю всю модель");
                shape.view(shapes::ShapeAngle::Overview);
            }
        }
        ShapeCam::Free(on) => shape.set_free(on),
        ShapeCam::Spin(on) => shape.spin = on,
    }
}

/// Форма дерева предметов: сколько вложенных у каждого по порядку. Поменялась — значит,
/// предметы добавили, убрали или переставили.
fn shape(file: &reyvec_scene::SceneFile) -> Vec<usize> {
    fn walk(nodes: &[reyvec_scene::NodeDesc], out: &mut Vec<usize>) {
        out.push(nodes.len());
        for n in nodes {
            walk(&n.children, out);
        }
    }
    let mut out = Vec::new();
    walk(&file.nodes, &mut out);
    out
}

/// Текст сцены для движка: без скрытых в редакторе предметов.
fn engine_text(text: &str, hidden: &HashSet<Vec<usize>>) -> String {
    let mut paths: Vec<&Vec<usize>> = hidden
        .iter()
        // Вложенный в скрытого уйдёт вместе с ним.
        .filter(|p| !(1..p.len()).any(|k| hidden.contains(&p[..k])))
        .collect();
    // С конца: вырезанный предмет не сдвигает номера тех, что ещё впереди.
    paths.sort_unstable_by(|a, b| b.cmp(a));
    let mut out = text.to_owned();
    for p in paths {
        match ron_edit::remove_node(&out, p) {
            Ok(t) => out = t,
            Err(e) => tracing::debug!("Скрытый предмет не убран: {e}"),
        }
    }
    out
}

/// Часы анимации сцены (покачивания, круги, вращения): идут, только пока играют или
/// симулируют. В правке сцена замирает на последнем кадре игры, а движок не пересчитывает
/// движущиеся предметы каждый кадр.
#[derive(Debug)]
struct AnimClock {
    time: f32,
    last: Instant,
}

impl Default for AnimClock {
    fn default() -> Self {
        Self {
            time: 0.0,
            last: Instant::now(),
        }
    }
}

impl AnimClock {
    /// Время для этого кадра; `running` — идёт ли сейчас время сцены.
    fn tick(&mut self, running: bool) -> f32 {
        self.tick_at(Instant::now(), running)
    }

    fn tick_at(&mut self, now: Instant, running: bool) -> f32 {
        if running {
            // Долгий кадр (окно тянули, отладчик) не перескакивает анимацию.
            let dt = now
                .saturating_duration_since(self.last)
                .as_secs_f32()
                .min(0.25);
            self.time += dt;
        }
        self.last = now;
        self.time
    }
}

/// Открыть файл программой Windows или показать его в проводнике (`reveal`).
fn open_in_system(path: &std::path::Path, reveal: bool) {
    let started = {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let mut c = std::process::Command::new("explorer.exe");
            // Кавычки вокруг пути — сами: «/select,» должен стоять вне них.
            if reveal {
                c.raw_arg(format!("/select,\"{}\"", path.display()));
            } else {
                c.raw_arg(format!("\"{}\"", path.display()));
            }
            c.spawn()
        }
        #[cfg(not(windows))]
        {
            let target = if reveal {
                path.parent().unwrap_or(path)
            } else {
                path
            };
            std::process::Command::new("xdg-open").arg(target).spawn()
        }
    };
    match started {
        Ok(_) => tracing::info!("Открываю «{}»", path.display()),
        Err(e) => tracing::warn!("Не открыть «{}»: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_animation_stands_still_while_editing() {
        let mut c = AnimClock::default();
        let t0 = c.last;
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        assert_eq!(c.tick_at(at(100), false), 0.0);
        assert!((c.tick_at(at(200), true) - 0.1).abs() < 1e-6);
        assert!((c.tick_at(at(300), true) - 0.2).abs() < 1e-6);
        // Стоп: время замирает и после не прыгает на простой.
        assert!((c.tick_at(at(5000), false) - 0.2).abs() < 1e-6);
        assert!((c.tick_at(at(5100), true) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn hidden_nodes_leave_engine_text() {
        let text = reyvec_scene::EXAMPLE;
        let file = reyvec_scene::SceneFile::parse(text).unwrap();
        let hero = file.nodes.iter().position(|n| n.player.is_some()).unwrap();
        let hidden: HashSet<Vec<usize>> = [vec![0], vec![hero], vec![hero, 0], vec![2]].into();
        let t = engine_text(text, &hidden);
        let f = reyvec_scene::SceneFile::parse(&t).unwrap();
        let gone = if hero == 0 || hero == 2 { 2 } else { 3 };
        assert_eq!(f.nodes.len(), file.nodes.len() - gone);
        assert!(f.nodes.iter().all(|n| n.player.is_none()));
        assert_eq!(shape(&file)[0], file.nodes.len());
    }
}
