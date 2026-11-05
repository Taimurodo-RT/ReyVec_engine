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
mod doc;
mod geom;
mod layout;
mod outline;
mod overlay;
mod parts;
mod ron_edit;
mod theme;
mod ui;
mod viewport;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reyvec_gpu::{DemoScene, FrameStatus, GpuOptions, RendererOptions, UiFrame, WindowRenderer};
use reyvec_vector::{Camera2D, Point, Vec2};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::app::{AppError, RunSummary, input_of, to_screen};
use crate::controls::{Controls, Input, controls_path};
use doc::SceneDoc;
use ui::{Action, EditorUi, Info, PlayState};

/// Как часто пересчитывать счётчик кадров.
const STATS_INTERVAL: Duration = Duration::from_millis(500);
/// Во сколько раз приближает один щелчок колеса мыши.
const ZOOM_STEP: f64 = 1.2;
/// Пока тянут ручку, сцена в движке пересобирается не чаще этого.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(60);

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
    /// Папка проекта (в ней `assets` и `scenes`), для вкладки «Ассеты».
    pub project: Option<PathBuf>,
    /// Закрыть окно через столько секунд (для автопроверок).
    pub max_seconds: Option<f64>,
    /// Закрыть окно после стольких кадров (для автопроверок).
    pub max_frames: Option<u64>,
    /// Сохранить снимок окна в этот файл и закрыть окно (для проверки в CI).
    pub snapshot: Option<PathBuf>,
    /// Через сколько секунд после открытия делать снимок.
    pub snapshot_after: f64,
}

/// Открывает редактор и работает, пока его не закроют.
pub fn run_editor(config: EditorConfig) -> Result<RunSummary, AppError> {
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
    let mut editor = Editor {
        ui: EditorUi::new(config.project.as_deref()),
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
        frames: 0,
        stats_at: Instant::now(),
        stats_frames: 0,
        fps: 0.0,
        frame_ms: 0.0,
        gpu_ms: None,
        adapter: String::new(),
        error: None,
        closing: false,
        closed_at: None,
        minimized: false,
        centered: false,
        cursor: None,
        snapshot_requested: false,
    };
    event_loop
        .run_app(&mut editor)
        .map_err(|e| AppError::Window(e.to_string()))?;
    if let Some(e) = editor.error.take() {
        return Err(e);
    }
    let end = editor.closed_at.unwrap_or_else(Instant::now);
    let seconds = end.duration_since(editor.start).as_secs_f64();
    Ok(RunSummary {
        frames: editor.frames,
        adapter: editor.adapter.clone(),
        average_fps: if seconds > 0.0 {
            editor.frames as f64 / seconds
        } else {
            0.0
        },
        gpu: None,
    })
}

struct Editor {
    config: EditorConfig,
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
    frames: u64,
    stats_at: Instant,
    stats_frames: u32,
    fps: f64,
    frame_ms: f64,
    gpu_ms: Option<f64>,
    adapter: String,
    error: Option<AppError>,
    closing: bool,
    closed_at: Option<Instant>,
    minimized: bool,
    /// Камеру уже поставили серединой сцены в середину свободного места.
    centered: bool,
    cursor: Option<PhysicalPosition<f64>>,
    snapshot_requested: bool,
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
        self.egui_ctx.set_fonts(crate::panel::fonts());
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
        let (Some(r), Some(w)) = (self.renderer.as_mut(), self.window.clone()) else {
            return;
        };
        match action {
            Action::Play | Action::Simulate => {
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
            Action::ForceClose => self.close(event_loop),
            Action::SaveAndClose => {
                let saved = self.doc.as_mut().map(SceneDoc::save);
                match saved {
                    Some(Err(e)) => tracing::warn!("Сцена не сохранена, окно остаётся: {e}"),
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
        if self.doc.as_ref().is_some_and(SceneDoc::dirty) {
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
                    Ok(()) => tracing::info!("Сцена сохранена: {}", self.scene_name()),
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
        if self.frames >= 2 && !self.centered && self.ui.viewport().is_some() {
            // Камера уже встала на героя: дальше она не идёт за ним, пока не нажмут «Играть».
            if let Some(r) = self.renderer.as_mut() {
                r.set_follow(false);
            }
            self.center_on_viewport();
            self.centered = true;
        }
        self.ui.push_log(crate::console::take_log());

        let view = self.view();
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
        let scene_name = self.config.scene_file.as_ref().and_then(|p| p.file_name());
        let scene_name = scene_name.map_or_else(
            || "встроенный пример".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let engine_error = renderer
            .shader_error()
            .map(|e| format!("шейдер: {e}"))
            .or_else(|| renderer.scene_error());
        let doc = self.doc.as_ref();
        let error = doc
            .and_then(SceneDoc::error)
            .map(ToOwned::to_owned)
            .or(engine_error);
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
            dirty: doc.is_some_and(SceneDoc::dirty),
            can_undo: doc.is_some_and(SceneDoc::can_undo),
            can_redo: doc.is_some_and(SceneDoc::can_redo),
            can_save: self.config.scene_file.is_some(),
            hidden: &self.hidden,
            can_paste: self.clipboard.is_some(),
            ejected: self.ejected,
            time_scale: self.time_scale,
            simulating: self.simulating,
        };
        let input = state.take_egui_input(window);
        let editor_ui = &mut self.ui;
        let out = self.egui_ctx.run_ui(input, |ui| editor_ui.draw(ui, &info));
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

        let size = window.inner_size();
        let time = self.start.elapsed().as_secs_f32();
        match renderer.render([size.width, size.height], time, Some(ui_frame)) {
            Ok(FrameStatus::Presented) => {
                self.frames += 1;
                self.stats_frames += 1;
                if let Some(t) = renderer.gpu_timings() {
                    self.gpu_ms = Some(t.total_ms);
                }
                let now = Instant::now();
                let since = now.duration_since(self.stats_at);
                if since >= STATS_INTERVAL {
                    let s = since.as_secs_f64();
                    self.fps = f64::from(self.stats_frames) / s;
                    self.frame_ms = s * 1000.0 / f64::from(self.stats_frames.max(1));
                    self.stats_frames = 0;
                    self.stats_at = now;
                }
                if let (Some(path), Some(image)) =
                    (self.config.snapshot.clone(), renderer.take_snapshot())
                {
                    match crate::snapshot::save_png(&path, &image) {
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
                if let (Some(r), Some(w)) = (self.renderer.as_mut(), &self.window) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
