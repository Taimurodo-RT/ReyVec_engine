//! Окно и цикл кадра на winit.

use std::sync::Arc;
use std::time::{Duration, Instant};

use reyvec_gpu::{
    DemoScene, FrameStatus, GpuError, GpuFrameTimings, GpuOptions, GpuTimingAverage, Msaa,
    RendererOptions, WindowRenderer,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::panel::{Panel, PanelInfo};

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
    /// С какой сцены песочницы начать (клавиши 1–6 переключают).
    pub scene: DemoScene,
    /// Сглаживание сцены (клавиша M переключает).
    pub msaa: Msaa,
    /// Края при MSAA «как в редакторах» (клавиша G переключает). Дороже обычных: см.
    /// `docs/decisions/0009-vector-layer.md`.
    pub perceptual_edges: bool,
    /// Как рисовать залитые векторные фигуры (клавиша F переключает).
    pub fill: reyvec_gpu::FillMode,
    /// Лупа векторной сцены (клавиша Z переключает).
    pub zoom: bool,
    /// Свой SVG для сцены «SVG» (содержимое файла).
    pub svg: Option<std::sync::Arc<[u8]>>,
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
                svg: self.config.svg.clone(),
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
        self.panel = Some(Panel::new(&window));
        self.window = Some(window);
        let mut renderer = renderer;
        renderer.set_scene(self.config.scene);
        renderer.set_msaa(self.config.msaa);
        renderer.set_perceptual_edges(self.config.perceptual_edges);
        renderer.set_fill(self.config.fill);
        renderer.set_zoom(self.config.zoom);
        self.renderer = Some(renderer);
        self.start = Instant::now();
        self.stats.last_title = self.start;
        self.stats.frames_since_title = 0;
        Ok(())
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.closing {
            return;
        }
        let (Some(window), Some(renderer)) = (&self.window, &mut self.renderer) else {
            return;
        };
        let _zone = reyvec_gpu::cpu_zone!("кадр");
        let size = window.inner_size();
        let time = self.start.elapsed().as_secs_f32();
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
                last_snapshot: self.last_snapshot.as_deref(),
                scene: renderer.scene().title(),
                msaa: renderer.msaa().title(),
                perceptual_edges: renderer.perceptual_edges(),
                fill: renderer.fill().label(),
                zoom: renderer.zoom(),
            };
            panel.frame(window, &info)
        });
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
                let base = match renderer.shader_error() {
                    Some(err) => format!("ОШИБКА ШЕЙДЕРА: {err} | {}", self.config.title),
                    None => self.config.title.clone(),
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
        _ => None,
    }
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
        if let (Some(panel), Some(window)) = (self.panel.as_mut(), &self.window) {
            panel.on_window_event(window, &event);
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
                if let Some(r) = self.renderer.as_mut()
                    && !r.request_snapshot()
                {
                    tracing::warn!(
                        "Эта видеокарта не умеет копировать картинку окна: снимка не будет"
                    );
                }
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
                if let (Some(r), Some(scene)) = (self.renderer.as_mut(), scene_for_key(code)) {
                    r.set_scene(scene);
                    tracing::info!("Сцена: {}", scene.title());
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
                if let Some(r) = self.renderer.as_mut() {
                    // Если видеокарта не умеет следующий вариант, пропускаем его.
                    let current = r.msaa();
                    let mut next = current.next();
                    for _ in 0..2 {
                        if r.msaa_supported(next) == next {
                            break;
                        }
                        next = next.next();
                    }
                    r.set_msaa(next);
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
                if let Some(r) = self.renderer.as_mut() {
                    let on = !r.perceptual_edges();
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
                if let Some(r) = self.renderer.as_mut() {
                    let fill = r.fill().next();
                    r.set_fill(fill);
                    tracing::info!("Заливка векторных фигур: {}", fill.label());
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
                if let Some(r) = self.renderer.as_mut() {
                    let on = !r.zoom();
                    r.set_zoom(on);
                    tracing::info!("Лупа векторной сцены: {}", if on { "×8" } else { "выкл" });
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
