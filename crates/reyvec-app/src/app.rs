//! Окно и цикл кадра на winit.

use std::sync::Arc;
use std::time::{Duration, Instant};

use reyvec_gpu::{
    DemoScene, FrameStatus, GpuError, GpuFrameTimings, GpuOptions, GpuTimingAverage, Msaa,
    RendererOptions, WindowRenderer,
};
use reyvec_vector::{Camera2D, Point, Vec2};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::controls::{Input, MouseKey, Route, Wheel, schemes};
use crate::panel::{Panel, PanelAction, PanelInfo};
use reyvec_scene::game::Action;

/// Как часто обновлять счётчик кадров в заголовке окна.
const TITLE_INTERVAL: Duration = Duration::from_millis(500);

/// Настройки запуска окна.
#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Текст заголовка окна до сведений о видеокарте и FPS.
    pub title: String,
    /// Видеокарта и валидация.
    pub gpu: GpuOptions,
    /// Вертикальная синхронизация.
    pub vsync: bool,
    /// Перезагружать шейдеры после сохранения их файлов.
    pub hot_reload: bool,
    /// Включить замеры для Tracy: программа Tracy сможет подключиться к движку.
    pub tracy: bool,
    /// С какой сцены песочницы начать (клавиши 0–9 переключают).
    pub scene: DemoScene,
    /// Сглаживание сцены (клавиша M переключает). `None`: по видеокарте, 2× на встроенной
    /// и 4× на остальных (задача 4.0).
    pub msaa: Option<Msaa>,
    /// Края при MSAA «как в редакторах» (клавиша G переключает). Дороже обычных: см.
    /// `docs/decisions/0009-vector-layer.md`.
    pub perceptual_edges: bool,
    /// Как рисовать залитые векторные фигуры (клавиша F переключает).
    pub fill: reyvec_gpu::FillMode,
    /// Лупа векторной сцены (клавиша Z переключает).
    pub zoom: bool,
    /// Свои файлы для сцен «SVG», «Motif» и «из файла».
    pub files: reyvec_gpu::UserFiles,
    /// Файл сцены, за которым следить: после сохранения сцена «из файла» обновляется.
    pub scene_file: Option<std::path::PathBuf>,
    /// Файл сцены «3D», за которым следить (7.5): после сохранения сцена обновляется.
    pub space_file: Option<std::path::PathBuf>,
    /// Закрыть окно после стольких нарисованных кадров (для автопроверок).
    pub max_frames: Option<u64>,
    /// Закрыть окно через столько секунд (для автопроверок).
    pub max_seconds: Option<f64>,
}

/// Итог работы окна.
#[derive(Clone, Debug, Default)]
pub struct RunSummary {
    /// Сколько кадров нарисовано и показано.
    pub frames: u64,
    /// Название видеокарты.
    pub adapter: String,
    /// Средний FPS за всё время.
    pub average_fps: f64,
    /// Сколько в среднем видеокарта тратила на кадр и каждый проход (если умеет замерять).
    pub gpu: Option<GpuFrameTimings>,
}

/// Ошибки запуска окна.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Ошибка системы окон.
    #[error("окно: {0}")]
    Window(String),
    /// Ошибка видеокарты.
    #[error(transparent)]
    Gpu(#[from] GpuError),
}

/// Открывает окно и рисует кадры, пока окно не закроют (или не будет нарисовано `max_frames`).
pub fn run(config: AppConfig) -> Result<RunSummary, AppError> {
    if config.tracy {
        // Клиент Tracy живёт до конца процесса: остановить его безопасно нельзя, пока
        // хоть один поток пишет замеры. Он ждёт подключения только с этого компьютера.
        let _ = tracy_client::Client::start();
        tracing::info!(
            "Tracy: замеры доступны. Откройте Tracy 0.14.1 и нажмите Connect (адрес 127.0.0.1)"
        );
    }
    let event_loop = EventLoop::new().map_err(|e| AppError::Window(e.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        config,
        window: None,
        renderer: None,
        panel: None,
        last_snapshot: None,
        saving: Vec::new(),
        error: None,
        start: Instant::now(),
        frames: 0,
        stats: FrameStats::new(Instant::now()),
        gpu_total: GpuTimingAverage::default(),
        minimized: false,
        closing: false,
        closed_at: None,
        cursor: None,
        dragging: false,
        grabbing: false,
        scene_watcher: None,
        scene_file_label: None,
        space_watcher: None,
        space_file_label: None,
        modifiers: ModifiersState::empty(),
        gamepads: crate::gamepad::Gamepads::new(),
        game_ui: crate::game_ui::GameUi::new(),
        audio: reyvec_audio::Audio::new(true),
        fly: crate::fly::Fly::default(),
        last_frame: Instant::now(),
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| AppError::Window(e.to_string()))?;
    for handle in app.saving.drain(..) {
        let _ = handle.join();
    }
    if let Some(e) = app.error.take() {
        return Err(e);
    }
    let end = app.closed_at.unwrap_or_else(Instant::now);
    let seconds = end.duration_since(app.start).as_secs_f64();
    Ok(RunSummary {
        frames: app.frames,
        adapter: app.stats.adapter.clone(),
        average_fps: if seconds > 0.0 {
            app.frames as f64 / seconds
        } else {
            0.0
        },
        gpu: app.gpu_total.average(),
    })
}

struct App {
    config: AppConfig,
    // Рендерер держит своё Arc на окно, так что порядок полей здесь не опасен.
    renderer: Option<WindowRenderer>,
    panel: Option<Panel>,
    window: Option<Arc<Window>>,
    /// Имя файла последнего снимка кадра (для панели).
    last_snapshot: Option<String>,
    /// Потоки, которые пишут снимки: перед выходом ждём их, иначе файл оборвётся.
    saving: Vec<std::thread::JoinHandle<()>>,
    error: Option<AppError>,
    start: Instant,
    frames: u64,
    stats: FrameStats,
    /// Замеры видеокарты за всё время работы окна.
    gpu_total: GpuTimingAverage,
    minimized: bool,
    /// Окно уже попросили закрыть. На Windows цикл событий завершается не сразу (например,
    /// пока край окна держат мышкой), поэтому после этого кадры больше не рисуем.
    closing: bool,
    /// Когда окно попросили закрыть: до этого момента считаем средний FPS.
    closed_at: Option<Instant>,
    /// Где сейчас мышь в окне (пиксели).
    cursor: Option<PhysicalPosition<f64>>,
    /// Левая кнопка мыши зажата над сценой: движение мыши двигает камеру.
    dragging: bool,
    /// Q зажата в сцене из файла: мышь тащит тело (6.13).
    grabbing: bool,
    /// Слежка за файлом сцены (5.1) и его путь для панели.
    scene_watcher: Option<reyvec_scene::SceneWatcher>,
    scene_file_label: Option<String>,
    /// Слежка за файлом сцены «3D» (7.5) и его путь для панели.
    space_watcher: Option<reyvec_scene::SceneWatcher>,
    space_file_label: Option<String>,
    /// Зажатые Shift, Ctrl и другие (для Shift+U).
    modifiers: ModifiersState,
    /// Геймпады (6.3).
    gamepads: crate::gamepad::Gamepads,
    /// Игровой интерфейс и настройка клавиш (6.10б).
    game_ui: crate::game_ui::GameUi,
    /// Звук (6.7): звуки событий игры и музыка.
    audio: reyvec_audio::Audio,
    /// Полёт камерой в сцене «3D» (7.1) и время прошлого кадра для шага полёта.
    fly: crate::fly::Fly,
    last_frame: Instant,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: AppError) {
        // Сообщение выведет тот, кто вызвал run(), здесь только запоминаем ошибку.
        self.error = Some(error);
        self.close(event_loop);
    }

    /// Просит закрыть окно. Повторные вызовы ничего не делают.
    fn close(&mut self, event_loop: &ActiveEventLoop) {
        if !self.closing {
            self.closed_at = Some(Instant::now());
        }
        self.closing = true;
        event_loop.exit();
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), AppError> {
        let attributes = Window::default_attributes()
            .with_title(&self.config.title)
            .with_inner_size(LogicalSize::new(1280.0, 720.0))
            .with_min_inner_size(LogicalSize::new(160.0, 120.0));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| AppError::Window(e.to_string()))?,
        );
        let size = window.inner_size();
        let renderer = WindowRenderer::new(
            window.clone(),
            [size.width, size.height],
            &RendererOptions {
                gpu: self.config.gpu.clone(),
                vsync: self.config.vsync,
                hot_reload: self.config.hot_reload,
                files: self.config.files.clone(),
            },
        )?;
        self.stats.adapter = renderer.adapter().name.clone();
        self.stats.mode = renderer.present_mode_name();
        tracing::info!(
            "Окно открыто: {}×{}, {}. Esc закрывает окно",
            size.width,
            size.height,
            self.stats.mode
        );
        if renderer.shaders_hot_reload() {
            tracing::info!("Перезагрузка шейдеров включена");
        }
        tracing::info!("Панель счётчиков: F1 показывает и прячет");
        if let Some(path) = &self.config.scene_file {
            match reyvec_scene::SceneWatcher::new(path) {
                Ok(w) => {
                    self.scene_file_label = Some(w.path().display().to_string());
                    self.scene_watcher = Some(w);
                }
                Err(e) => {
                    tracing::warn!("Не удалось следить за файлом сцены {}: {e}", path.display())
                }
            }
        }
        if let Some(path) = &self.config.space_file {
            match reyvec_scene::SceneWatcher::new(path) {
                Ok(w) => {
                    self.space_file_label = Some(w.path().display().to_string());
                    self.space_watcher = Some(w);
                }
                Err(e) => {
                    tracing::warn!(
                        "Не удалось следить за файлом сцены 3D {}: {e}",
                        path.display()
                    )
                }
            }
        }
        self.panel = Some(Panel::new(&window));
        self.window = Some(window);
        let mut renderer = renderer;
        renderer.set_scene(self.config.scene);
        let default_msaa = renderer.msaa_default();
        let msaa = self.config.msaa.unwrap_or(default_msaa);
        if self.config.msaa.is_none() {
            tracing::info!(
                "Сглаживание по умолчанию для видеокарты «{}» ({}): {}. Клавиша M меняет, --msaa задаёт при запуске",
                renderer.adapter().name,
                renderer.adapter().kind,
                default_msaa.title()
            );
        }
        renderer.set_msaa(msaa);
        renderer.set_perceptual_edges(self.config.perceptual_edges);
        renderer.set_fill(self.config.fill);
        renderer.set_zoom(self.config.zoom);
        self.renderer = Some(renderer);
        self.start = Instant::now();
        self.stats.last_title = self.start;
        self.stats.frames_since_title = 0;
        Ok(())
    }

    /// Выполняет просьбу с панели или с клавиатуры.
    fn apply(&mut self, action: PanelAction) {
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        match action {
            PanelAction::Scene(scene) => {
                self.fly.release();
                r.set_scene(scene);
                tracing::info!("Сцена: {}", scene.title());
            }
            PanelAction::Msaa(msaa) => r.set_msaa(msaa),
            PanelAction::LoadPicture(background) => {
                tracing::info!(
                    "Картина: {}",
                    if background {
                        "в фоне"
                    } else {
                        "в кадре"
                    }
                );
                r.load_picture(background);
            }
            PanelAction::CompactScene(on) => {
                if r.compact_scene_supported() {
                    r.set_compact_scene(on);
                } else {
                    tracing::info!("Компактный кадр сцены эта видеокарта не умеет");
                }
            }
            PanelAction::PerceptualEdges(on) => {
                r.set_perceptual_edges(on);
                tracing::info!(
                    "Края при MSAA: {}",
                    if on {
                        "как в редакторах"
                    } else {
                        "физически точные"
                    }
                );
            }
            PanelAction::Fill(fill) => {
                r.set_fill(fill);
                tracing::info!("Заливка векторных фигур: {}", fill.label());
            }
            PanelAction::Zoom(on) => {
                r.set_zoom(on);
                tracing::info!("Лупа векторной сцены: {}", if on { "×8" } else { "выкл" });
            }
            PanelAction::LightField(on) => {
                r.set_light_field(on);
                tracing::info!(
                    "Свет: линии фона по {}",
                    if on {
                        "полю из картинки (jump flooding)"
                    } else {
                        "точному полю"
                    }
                );
            }
            PanelAction::Glow(on) => {
                r.set_glow(on);
                tracing::info!(
                    "Свечение и мягкие пересветы: {}",
                    if on { "вкл" } else { "выкл" }
                );
            }
            PanelAction::Cascades(on) => {
                r.set_cascades(on);
                tracing::info!(
                    "Свет: {}",
                    if on {
                        "каскадами лучей (опыт)"
                    } else {
                        "лучами к источникам"
                    }
                );
            }
            PanelAction::SpaceSmooth(on) => {
                r.set_space_smooth(on);
                tracing::info!("Сглаживание 3D: {}", if on { "вкл" } else { "выкл" });
            }
            PanelAction::Dither(on) => {
                r.set_dither(on);
                tracing::info!("Шум против полос: {}", if on { "вкл" } else { "выкл" });
            }
            PanelAction::Fountain(on) => {
                r.set_fountain(on);
                tracing::info!("Фонтан частиц: {}", if on { "вкл" } else { "выкл" });
            }
            PanelAction::Music(on) => {
                self.audio.set_music(on);
                tracing::info!("Музыка: {}", if on { "вкл" } else { "выкл" });
            }
            PanelAction::ResetCamera => {
                r.set_camera(Camera2D::default());
                r.set_space_camera(r.space_home_camera());
                r.set_follow(true);
                tracing::info!("Камера в начале");
            }
            PanelAction::Snapshot => {
                if !r.request_snapshot() {
                    tracing::warn!(
                        "Эта видеокарта не умеет копировать картинку окна: снимка не будет"
                    );
                }
            }
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.closing {
            return;
        }
        // Нажатое мышкой на панели в прошлом кадре.
        let actions = self
            .panel
            .as_mut()
            .map(Panel::take_actions)
            .unwrap_or_default();
        for action in actions {
            self.apply(action);
        }
        let (Some(window), Some(renderer)) = (&self.window, &mut self.renderer) else {
            return;
        };
        let _zone = reyvec_gpu::cpu_zone!("кадр");
        // Полёт камерой сцены «3D» (7.1): шаг по времени с прошлого кадра.
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        if renderer.scene() == DemoScene::Space {
            let mut camera = renderer.space_camera();
            camera.fly(&self.fly.take(), dt);
            renderer.set_space_camera(camera);
        }
        // Геймпад: в игру, только пока открыта сцена с героем.
        let playing = renderer.scene() == DemoScene::File && !self.game_ui.menu_open();
        renderer.set_game_paused(renderer.scene() == DemoScene::File && self.game_ui.menu_open());
        self.gamepads.poll(|action, down| {
            if playing {
                renderer.game_action(action, down);
            }
        });
        // Мышь для физики: каждый кадр, потому что камера может ехать за героем.
        if renderer.scene() == DemoScene::File {
            // Меню или консоль забирают клавиши: отпускание Q до нас не дойдёт.
            if self.game_ui.menu_open() || self.game_ui.console.is_open() {
                self.grabbing = false;
            }
            let at = self.cursor.map(|c| {
                let p = renderer
                    .camera()
                    .to_scene(to_screen(c, window.inner_size()).to_point());
                [p.x, p.y]
            });
            match at {
                Some(at) => renderer.physics_pointer(at, self.grabbing),
                None => self.grabbing = false,
            }
        }
        if let Some(watcher) = self.scene_watcher.as_mut() {
            match watcher.poll() {
                Some(Ok(text)) => {
                    tracing::info!("Файл сцены сохранён, читаю заново");
                    renderer.set_scene_file(&text);
                }
                Some(Err(e)) => tracing::warn!("Файл сцены: {e}"),
                None => {}
            }
        }
        if let Some(watcher) = self.space_watcher.as_mut() {
            match watcher.poll() {
                Some(Ok(text)) => {
                    tracing::info!("Файл сцены 3D сохранён, читаю заново");
                    renderer.set_space_file(&text);
                }
                Some(Err(e)) => tracing::warn!("Файл сцены 3D: {e}"),
                None => {}
            }
        }
        let size = window.inner_size();
        let time = self.start.elapsed().as_secs_f32();
        let msaa_options: Vec<Msaa> = [Msaa::Off, Msaa::X2, Msaa::X4, Msaa::X8]
            .into_iter()
            .filter(|m| renderer.msaa_supported(*m) == *m)
            .collect();
        // Звук и интерфейс событий игры этого кадра (6.7, 6.10б).
        self.audio.poll();
        let camera = renderer.camera();
        let aspect = f64::from(size.width) / f64::from(size.height.max(1));
        for event in renderer.take_game_events() {
            self.game_ui.on_event(&event);
            let Some((sound, decibels, at)) = crate::sound::sound_for(&event) else {
                continue;
            };
            let pan = crate::sound::pan_for(at[0], camera.center.x, camera.zoom, aspect);
            self.audio.play(sound, decibels, pan);
        }
        // Консоль разработчика (6.11): команды набранного в прошлом кадре — потоку
        // логики, его ответы — в журнал.
        run_console(&mut self.game_ui.console, renderer.scene_logic());
        // Точка прицела (6.12): курсор в координатах сцены, пока идёт игра.
        if let Some(logic) = renderer.scene_logic() {
            let aim = self.cursor.filter(|_| playing).map(|c| {
                let p = camera.to_scene(to_screen(c, size).to_point());
                [p.x, p.y]
            });
            logic.set_aim(aim);
        }
        let game_view = crate::game_ui::GameView {
            player: renderer.player_stats().map(|p| p.view),
            center: [camera.center.x, camera.center.y],
            zoom: camera.zoom,
            solids: if self.game_ui.console.colliders() {
                renderer.file_solids().to_vec()
            } else {
                Vec::new()
            },
        };
        let game_ui = &mut self.game_ui;
        let in_game = renderer.scene() == DemoScene::File;
        let ui = self.panel.as_mut().and_then(|panel| {
            let info = PanelInfo {
                adapter: &self.stats.adapter,
                mode: self.stats.mode,
                extent: renderer.extent(),
                fps: self.stats.fps,
                frame_ms: self.stats.ms,
                gpu: self.stats.gpu_avg.as_ref(),
                graph: renderer.last_graph(),
                validation: self.config.gpu.validation,
                validation_errors: reyvec_gpu::validation_error_count(),
                hot_reload: renderer.shaders_hot_reload(),
                shader_error: renderer.shader_error(),
                scene_file: self.scene_file_label.as_deref(),
                scene_error: renderer.scene_error(),
                logic: renderer.logic_stats(),
                player: renderer.player_stats(),
                scripts: renderer.script_info(),
                physics: renderer.physics_stats(),
                space: (renderer.scene() == DemoScene::Space)
                    .then(|| space_text(&renderer.space_camera(), self.fly.speed)),
                space_file: self.space_file_label.as_deref(),
                space_error: renderer.space_error(),
                space_objects: renderer.space_objects(),
                space_culling: renderer.space_culling(),
                space_in_view: renderer.space_in_view(),
                space_compute: renderer.space_compute(),
                space_smooth: renderer.space_smooth(),
                gamepad: self.gamepads.name(),
                follow: renderer.follow(),
                particles: renderer.particle_stats(),
                audio: Some(self.audio.stats()),
                loading: renderer.load_stats(),
                cull: renderer.cull_stats(),
                last_snapshot: self.last_snapshot.as_deref(),
                scene: renderer.scene().title(),
                msaa: renderer.msaa().title(),
                msaa_is_default: renderer.msaa() == renderer.msaa_default(),
                compact_scene: renderer.compact_scene(),
                perceptual_edges: renderer.perceptual_edges(),
                fill: renderer.fill().label(),
                zoom: renderer.zoom(),
                camera_zoom: renderer.camera().zoom,
                vector_cpu: renderer.vector_cpu(),
                scene_id: renderer.scene(),
                msaa_now: renderer.msaa(),
                msaa_options: &msaa_options,
                compact_supported: renderer.compact_scene_supported(),
                fill_mode: renderer.fill(),
                light_field: renderer.light_field(),
                glow: renderer.glow(),
                cascades: renderer.cascades(),
                dither: renderer.dither(),
            };
            panel.frame(window, &info, in_game.then_some((game_ui, &game_view)))
        });
        if self.game_ui.take_quit() {
            return self.close(event_loop);
        }
        match renderer.render([size.width, size.height], time, ui) {
            Ok(FrameStatus::Presented) => {
                self.frames += 1;
                if let Some(image) = renderer.take_snapshot()
                    && let Some((name, handle)) = save_snapshot(image)
                {
                    self.last_snapshot = Some(name);
                    self.saving.push(handle);
                }
                if let Some(client) = tracy_client::Client::running() {
                    client.frame_mark();
                }
                if let Some(t) = renderer.gpu_timings() {
                    self.stats.gpu.add(t);
                    self.gpu_total.add(t);
                    if let Some(panel) = self.panel.as_mut() {
                        panel.record_gpu(t.total_ms);
                    }
                }
                let now = Instant::now();
                let scene_error = match renderer.scene() {
                    DemoScene::File => renderer.scene_error(),
                    DemoScene::Space => renderer.space_error(),
                    _ => None,
                };
                let base = match (renderer.shader_error(), scene_error) {
                    (Some(err), _) => format!("ОШИБКА ШЕЙДЕРА: {err} | {}", self.config.title),
                    (None, Some(err)) => {
                        format!("ОШИБКА В ФАЙЛЕ СЦЕНЫ: {err} | {}", self.config.title)
                    }
                    (None, None) => self.config.title.clone(),
                };
                if let Some(title) = self.stats.frame(now, &base) {
                    window.set_title(&title);
                }
                if self.config.max_frames.is_some_and(|max| self.frames >= max) {
                    tracing::info!("Нарисовано {} кадров, закрываю окно", self.frames);
                    return self.close(event_loop);
                } else if self
                    .config
                    .max_seconds
                    .is_some_and(|max| self.start.elapsed().as_secs_f64() >= max)
                {
                    tracing::info!("Время вышло, закрываю окно");
                    return self.close(event_loop);
                }
            }
            Ok(FrameStatus::Skipped) => {}
            Err(e) => return self.fail(event_loop, e.into()),
        }
        // Сразу просим следующий кадр. На Windows, пока окно тянут мышкой, система крутит
        // свой цикл сообщений и about_to_wait не вызывается; без этого картинка замирает.
        if !self.minimized {
            window.request_redraw();
        }
    }
}

/// Во сколько раз приближает один щелчок колеса мыши.
const ZOOM_STEP: f64 = 1.2;

/// Точка окна (пиксели, y вниз) в координатах экрана сцены: y вверх от -1 до 1, x от
/// `-aspect` до `aspect`. Возвращается вектором от центра окна.
pub(crate) fn to_screen(p: PhysicalPosition<f64>, size: PhysicalSize<u32>) -> Vec2 {
    let h = f64::from(size.height.max(1));
    let w = f64::from(size.width);
    Vec2::new((2.0 * p.x - w) / h, (h - 2.0 * p.y) / h)
}

/// Строка камеры 3D для панели.
fn space_text(c: &reyvec_sdf::Camera3D, speed: f32) -> String {
    format!(
        "Камера 3D: {} ({:.1}; {:.1}; {:.1}) м, поворот {:.0}°, наклон {:.0}°, скорость {:.1} м/с",
        c.projection_label(),
        c.eye.x,
        c.eye.y,
        c.eye.z,
        c.yaw.to_degrees(),
        c.pitch.to_degrees(),
        speed
    )
}

/// Клавиша выбора сцены песочницы. Клавиши по месту на клавиатуре, поэтому раскладка
/// не важна.
fn scene_for_key(code: KeyCode) -> Option<DemoScene> {
    match code {
        KeyCode::Digit1 | KeyCode::Numpad1 => Some(DemoScene::Gpu),
        KeyCode::Digit2 | KeyCode::Numpad2 => Some(DemoScene::Vector),
        KeyCode::Digit3 | KeyCode::Numpad3 => Some(DemoScene::Morph),
        KeyCode::Digit4 | KeyCode::Numpad4 => Some(DemoScene::Layers),
        KeyCode::Digit5 | KeyCode::Numpad5 => Some(DemoScene::Masks),
        KeyCode::Digit6 | KeyCode::Numpad6 => Some(DemoScene::Svg),
        KeyCode::Digit7 | KeyCode::Numpad7 => Some(DemoScene::Motif),
        KeyCode::Digit8 | KeyCode::Numpad8 => Some(DemoScene::Stream),
        KeyCode::Digit9 | KeyCode::Numpad9 => Some(DemoScene::Text),
        KeyCode::Digit0 | KeyCode::Numpad0 => Some(DemoScene::Stress),
        // Сцены этапа 4 и дальше — буквами: цифры заняты сценами этапов 2–3.
        KeyCode::KeyL => Some(DemoScene::Light),
        KeyCode::KeyK => Some(DemoScene::Lamps),
        KeyCode::KeyJ => Some(DemoScene::Blobs),
        KeyCode::KeyH => Some(DemoScene::Lab),
        KeyCode::KeyN => Some(DemoScene::File),
        KeyCode::KeyI => Some(DemoScene::Space),
        _ => None,
    }
}

impl App {
    /// Событие окна в сцене «3D»: `true`, если его забрал полёт камерой.
    fn space_event(&mut self, event: &WindowEvent, panel_took: bool) -> bool {
        match event {
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat,
                        ..
                    },
                ..
            } => {
                let down = *state == ElementState::Pressed;
                if *code == crate::fly::TOGGLE_PROJECTION {
                    if down
                        && !*repeat
                        && let Some(r) = self.renderer.as_mut()
                    {
                        let mut camera = r.space_camera();
                        // Размер сохраняется у середины сцены (там шар и формы).
                        camera.toggle_projection(camera.eye.length());
                        r.set_space_camera(camera);
                        tracing::info!("Камера 3D: {}", camera.projection_label());
                    }
                    return true;
                }
                self.fly.key(*code, down)
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left | MouseButton::Right,
                ..
            } => {
                self.fly.looking = *state == ElementState::Pressed && !panel_took;
                true
            }
            WindowEvent::CursorMoved { position, .. } => {
                if !self.fly.looking {
                    return false;
                }
                if let Some(prev) = self.cursor {
                    self.fly.mouse(position.x - prev.x, position.y - prev.y);
                }
                self.cursor = Some(*position);
                true
            }
            WindowEvent::MouseWheel { delta, .. } if !panel_took => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                self.fly.wheel(steps);
                true
            }
            WindowEvent::Focused(false) | WindowEvent::CursorLeft { .. } => {
                self.fly.release();
                false
            }
            _ => false,
        }
    }

    /// Взрыв под мышью в сцене из файла (6.13).
    fn blast_at_cursor(&mut self) {
        let (Some(r), Some(w), Some(c)) = (self.renderer.as_mut(), &self.window, self.cursor)
        else {
            return;
        };
        let p = r.camera().to_scene(to_screen(c, w.inner_size()).to_point());
        r.physics_blast([p.x, p.y]);
    }

    /// Следующий файл сцены (`*.ron`) из папки текущего, по алфавиту и по кругу (6.13):
    /// за ним теперь и следим.
    fn next_scene_file(&mut self) {
        let Some(current) = self.scene_watcher.as_ref().map(|w| w.path().to_path_buf()) else {
            return;
        };
        let Some(next) = next_scene_path(&current) else {
            return;
        };
        let text = match std::fs::read_to_string(&next) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("Не прочитать файл сцены «{}»: {e}", next.display());
                return;
            }
        };
        tracing::info!("Следующая сцена: {}", next.display());
        match reyvec_scene::SceneWatcher::new(&next) {
            Ok(w) => {
                self.scene_file_label = Some(w.path().display().to_string());
                self.scene_watcher = Some(w);
            }
            Err(e) => tracing::warn!("Не удалось следить за файлом {}: {e}", next.display()),
        }
        if let Some(r) = self.renderer.as_mut() {
            r.switch_scene_file(&text);
            r.set_follow(true);
        }
    }
}

/// Файл сцены после `current` в его папке: `*.ron` по алфавиту, после последнего — первый.
fn next_scene_path(current: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = current.parent()?;
    let mut all: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ron") && p.is_file())
        .collect();
    all.sort();
    let name = current.file_name()?;
    let at = all.iter().position(|p| p.file_name() == Some(name));
    let next = at.map_or(0, |i| (i + 1) % all.len());
    all.get(next).cloned()
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if let Err(e) = self.create_window(event_loop) {
            self.fail(event_loop, e);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let mut panel_took = false;
        if let (Some(panel), Some(window)) = (self.panel.as_mut(), &self.window) {
            panel_took = panel.on_window_event(window, &event);
        }
        let in_game = self
            .renderer
            .as_ref()
            .is_some_and(|r| r.scene() == DemoScene::File);
        // Управление (6.12): клавиши, кнопки мыши и колесо в сцене с героем идут через
        // схемы (консоль, меню, игра, песочница) — нажатие получает верхняя схема, у
        // которой оно что-то значит. Без игры всё достаётся песочнице, как раньше.
        if in_game && let Some((input, down, repeat)) = input_of(&event) {
            // Меню ждёт клавишу или кнопку для назначения (6.10б): её забирает меню.
            if self.game_ui.capturing() {
                if down && !repeat {
                    self.game_ui.capture_input(input);
                }
                return;
            }
            // Щелчок по панели песочницы — панели, а не игре.
            if panel_took && down && input.is_mouse() && !self.game_ui.menu_open() {
                return;
            }
            let stack = schemes(
                true,
                self.game_ui.menu_open(),
                self.game_ui.console.is_open(),
            );
            match self.game_ui.controls().route(&stack, input) {
                Route::Sandbox => {}
                Route::Ui => return,
                Route::OpenConsole | Route::CloseConsole => {
                    if down && !repeat {
                        self.game_ui.console.toggle();
                        if self.game_ui.console.is_open()
                            && let Some(r) = self.renderer.as_mut()
                        {
                            r.release_game_input();
                        }
                    }
                    return;
                }
                Route::OpenMenu => {
                    if down && !repeat {
                        self.game_ui.escape();
                        self.dragging = false;
                        if let Some(r) = self.renderer.as_mut() {
                            r.release_game_input();
                        }
                    }
                    return;
                }
                Route::Menu(key) => {
                    if down {
                        self.game_ui.menu_key(key);
                    }
                    return;
                }
                Route::Game(action) => {
                    if repeat {
                        return;
                    }
                    let aimed = down
                        && input.is_mouse()
                        && action == Action::Attack
                        && self.game_ui.controls().attack_toward_cursor();
                    if let Some(r) = self.renderer.as_mut() {
                        if aimed && let Some(logic) = r.scene_logic() {
                            logic.press_aimed(action);
                        } else {
                            r.game_action(action, down);
                        }
                        // Колесо — щелчок: нажато и сразу отпущено.
                        if matches!(input, Input::Wheel(_)) {
                            r.game_action(action, false);
                        }
                    }
                    return;
                }
                Route::Camera => {
                    self.dragging = down && !panel_took;
                    return;
                }
            }
        }
        // Сцена «3D» (7.1): клавиши полёта, мышь и колесо — камере.
        if self
            .renderer
            .as_ref()
            .is_some_and(|r| r.scene() == DemoScene::Space)
            && self.space_event(&event, panel_took)
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => self.close(event_loop),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => self.close(event_loop),
            WindowEvent::Focused(false) => {
                // Отпускание клавиш, пока окно не в фокусе, оно не узнает.
                if let Some(r) = self.renderer.as_mut() {
                    r.release_game_input();
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::F1),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(panel) = self.panel.as_mut() {
                    panel.toggle();
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::F12),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                self.apply(PanelAction::Snapshot);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } if code == KeyCode::KeyN && in_game && self.scene_watcher.is_some() => {
                // Уже в сцене из файла: N берёт следующий файл сцены из той же папки.
                self.next_scene_file();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyY),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } if in_game => self.blast_at_cursor(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyQ),
                        state,
                        repeat: false,
                        ..
                    },
                ..
            } if in_game => {
                // Пока Q зажата, мышь тащит тело или частицу мягкого тела под собой.
                // Кнопки мыши в игре заняты (удар, рывок, камера, решение 0015).
                self.grabbing = state == ElementState::Pressed;
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } if scene_for_key(code).is_some() => {
                if let Some(scene) = scene_for_key(code) {
                    self.apply(PanelAction::Scene(scene));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyM),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(r) = self.renderer.as_ref() {
                    // Если видеокарта не умеет следующий вариант, пропускаем его.
                    let mut next = r.msaa().next();
                    for _ in 0..3 {
                        if r.msaa_supported(next) == next {
                            break;
                        }
                        next = next.next();
                    }
                    self.apply(PanelAction::Msaa(next));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyP),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(r) = self.renderer.as_ref() {
                    let on = !r.compact_scene();
                    self.apply(PanelAction::CompactScene(on));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyG),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(r) = self.renderer.as_ref() {
                    let on = !r.perceptual_edges();
                    self.apply(PanelAction::PerceptualEdges(on));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyF),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(r) = self.renderer.as_ref() {
                    let fill = r.fill().next();
                    self.apply(PanelAction::Fill(fill));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyB),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(r) = self.renderer.as_ref() {
                    let on = !r.glow();
                    self.apply(PanelAction::Glow(on));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyX),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                // X только в сцене «3D»: в сцене из файла это рывок персонажа.
                if let Some(r) = self.renderer.as_ref()
                    && r.scene() == DemoScene::Space
                {
                    let on = !r.space_smooth().0;
                    self.apply(PanelAction::SpaceSmooth(on));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyD),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(r) = self.renderer.as_ref() {
                    let on = !r.dither();
                    self.apply(PanelAction::Dither(on));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyC),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(r) = self.renderer.as_ref() {
                    let on = !r.cascades();
                    self.apply(PanelAction::Cascades(on));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyZ),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(r) = self.renderer.as_ref() {
                    let on = !r.zoom();
                    self.apply(PanelAction::Zoom(on));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyT),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(next) = self
                    .renderer
                    .as_ref()
                    .and_then(|r| r.scene().stress_variant())
                {
                    self.apply(PanelAction::Scene(next));
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyU),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } if self
                .renderer
                .as_ref()
                .is_some_and(|r| r.scene() == DemoScene::File) =>
            {
                self.apply(PanelAction::LoadPicture(!self.modifiers.shift_key()));
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyO),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                let on = !self.audio.stats().music;
                self.apply(PanelAction::Music(on));
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyV),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                if let Some(on) = self.renderer.as_ref().and_then(|r| r.particle_stats()) {
                    self.apply(PanelAction::Fountain(!on.fountain));
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyR | KeyCode::Home),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                self.apply(PanelAction::ResetCamera);
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let (true, Some(prev), Some(r), Some(w)) = (
                    self.dragging,
                    self.cursor,
                    self.renderer.as_mut(),
                    &self.window,
                ) {
                    let size = w.inner_size();
                    let delta = to_screen(position, size) - to_screen(prev, size);
                    let mut camera = r.camera();
                    camera.pan_screen(delta);
                    r.set_camera(camera);
                    // Камеру тащат мышью: за персонажем она больше не идёт (R вернёт).
                    if delta != Vec2::ZERO {
                        r.set_follow(false);
                    }
                }
                self.cursor = Some(position);
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.dragging = false;
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                // Панель забирает нажатия над собой: там мышь камеру не двигает.
                self.dragging = state == ElementState::Pressed && !panel_took;
            }
            WindowEvent::MouseWheel { delta, .. } if !panel_took => {
                // Один щелчок колеса: в 1,2 раза. Тачпад даёт пиксели: та же скорость
                // примерно на 50 пикселей.
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
            WindowEvent::Resized(size) => {
                self.minimized = size.width == 0 || size.height == 0;
                if let Some(r) = &mut self.renderer {
                    r.resize();
                }
                if let (false, Some(w)) = (self.minimized, &self.window) {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Свёрнутое или закрываемое окно не рисуем и не крутим цикл впустую: ждём событий.
        if self.minimized || self.closing {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        event_loop.set_control_flow(ControlFlow::Poll);
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Сначала Vulkan, потом панель и окно.
        self.renderer = None;
        self.panel = None;
        self.window = None;
    }
}

/// Счётчик кадров для заголовка окна.
struct FrameStats {
    adapter: String,
    mode: &'static str,
    last_title: Instant,
    frames_since_title: u32,
    /// Замеры видеокарты с прошлого обновления заголовка.
    gpu: GpuTimingAverage,
    /// Последние посчитанные значения (раз в полсекунды): для панели.
    fps: f64,
    ms: f64,
    gpu_avg: Option<GpuFrameTimings>,
}

impl FrameStats {
    fn new(now: Instant) -> Self {
        Self {
            adapter: String::new(),
            mode: "",
            last_title: now,
            frames_since_title: 0,
            gpu: GpuTimingAverage::default(),
            fps: 0.0,
            ms: 0.0,
            gpu_avg: None,
        }
    }

    /// Учитывает кадр. Раз в полсекунды возвращает новый заголовок окна.
    fn frame(&mut self, now: Instant, base: &str) -> Option<String> {
        self.frames_since_title += 1;
        let elapsed = now.duration_since(self.last_title);
        if elapsed < TITLE_INTERVAL {
            return None;
        }
        let fps = f64::from(self.frames_since_title) / elapsed.as_secs_f64();
        let ms = elapsed.as_secs_f64() * 1000.0 / f64::from(self.frames_since_title);
        self.last_title = now;
        self.frames_since_title = 0;
        self.gpu_avg = self.gpu.average();
        self.fps = fps;
        self.ms = ms;
        let gpu_ms = self.gpu_avg.as_ref().map(|t| t.total_ms);
        self.gpu.clear();
        Some(format_title(
            base,
            &self.adapter,
            self.mode,
            fps,
            ms,
            gpu_ms,
        ))
    }
}

fn format_title(
    base: &str,
    adapter: &str,
    mode: &str,
    fps: f64,
    ms: f64,
    gpu_ms: Option<f64>,
) -> String {
    let gpu = gpu_ms.map_or(String::new(), |g| format!(", видеокарта {g:.2} мс"));
    format!("{base} | {adapter} | {fps:.0} FPS, {ms:.2} мс{gpu} | {mode}")
}

/// Сохраняет снимок кадра в `snapshots` рядом с программой. PNG пишется в отдельном потоке,
/// чтобы не задерживать кадры. Возвращает имя файла.
fn save_snapshot(image: reyvec_gpu::FrameImage) -> Option<(String, std::thread::JoinHandle<()>)> {
    let dir = crate::snapshot::snapshot_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::error!("Снимок кадра: не создать папку {}: {e}", dir.display());
        return None;
    }
    let path = crate::snapshot::next_snapshot_path(&dir);
    // Занимаем имя сразу, чтобы следующий снимок не выбрал его, пока этот пишется.
    if let Err(e) = std::fs::File::create(&path) {
        tracing::error!("Снимок кадра: {}: {e}", path.display());
        return None;
    }
    let name = path.file_name()?.to_string_lossy().into_owned();
    let handle = std::thread::spawn(move || match crate::snapshot::save_png(&path, &image) {
        Ok(()) => tracing::info!(
            "Снимок кадра {}×{} сохранён: {}",
            image.width,
            image.height,
            path.display()
        ),
        Err(e) => tracing::error!("Снимок кадра не сохранён: {e}"),
    });
    Some((name, handle))
}

/// Источник ввода события окна: клавиша, кнопка мыши или щелчок колеса, нажато ли и
/// повтор ли это зажатой клавиши.
pub(crate) fn input_of(event: &WindowEvent) -> Option<(Input, bool, bool)> {
    match event {
        WindowEvent::KeyboardInput {
            event:
                KeyEvent {
                    physical_key: PhysicalKey::Code(code),
                    state,
                    repeat,
                    ..
                },
            ..
        } => Some((Input::Key(*code), *state == ElementState::Pressed, *repeat)),
        WindowEvent::MouseInput { state, button, .. } => MouseKey::from_winit(*button)
            .map(|m| (Input::Mouse(m), *state == ElementState::Pressed, false)),
        WindowEvent::MouseWheel { delta, .. } => {
            let y = match delta {
                MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
                MouseScrollDelta::PixelDelta(p) => p.y,
            };
            if y > 0.0 {
                Some((Input::Wheel(Wheel::Up), true, false))
            } else if y < 0.0 {
                Some((Input::Wheel(Wheel::Down), true, false))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Выполняет команды консоли разработчика и забирает ответы потока логики.
fn run_console(console: &mut crate::console::Console, logic: Option<&reyvec_scene::SceneLogic>) {
    use crate::console::Order;
    let orders = console.take_orders();
    let Some(logic) = logic else {
        if !orders.is_empty() {
            console.reply(Err("команды работают в сцене с героем (N)".into()));
        }
        return;
    };
    for order in orders {
        match order {
            Order::World(c) => logic.debug(c),
            Order::Pause(on) => {
                let on = on.unwrap_or(!logic.debug_paused());
                logic.set_debug_paused(on);
                console.say(if on {
                    "пауза: логика стоит («шаг» — по одному шагу)"
                } else {
                    "пауза снята"
                });
            }
            Order::Step(n) => {
                if !logic.debug_paused() {
                    logic.set_debug_paused(true);
                    console.say("пауза включена");
                }
                logic.step(n);
                console.say(&format!("шагов: {n} (по 1/60 с)"));
            }
            Order::Slow(k) => {
                let k = logic.set_time_scale(k);
                console.say(&format!("скорость времени ×{k:.2}"));
            }
        }
    }
    for reply in logic.take_replies() {
        console.reply(reply);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_shows_fps_and_frame_time() {
        let t = format_title(
            "ReyVec 0.1.0",
            "NVIDIA GeForce RTX 5080",
            "VSync",
            144.2,
            6.934,
            None,
        );
        assert_eq!(
            t,
            "ReyVec 0.1.0 | NVIDIA GeForce RTX 5080 | 144 FPS, 6.93 мс | VSync"
        );
        let t = format_title("R", "GPU", "VSync", 144.0, 6.94, Some(0.214));
        assert_eq!(t, "R | GPU | 144 FPS, 6.94 мс, видеокарта 0.21 мс | VSync");
    }

    #[test]
    fn stats_report_twice_a_second() {
        let start = Instant::now();
        let mut s = FrameStats::new(start);
        s.adapter = "GPU".into();
        assert!(s.frame(start + Duration::from_millis(100), "R").is_none());
        let title = s
            .frame(start + Duration::from_millis(500), "R")
            .expect("прошло полсекунды");
        assert!(title.contains("4 FPS"), "{title}");
        assert_eq!(s.frames_since_title, 0);
    }
}
