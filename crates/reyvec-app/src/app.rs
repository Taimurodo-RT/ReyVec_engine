//! Окно и цикл кадра на winit.

use std::sync::Arc;
use std::time::{Duration, Instant};

use reyvec_gpu::{FrameStatus, GpuError, GpuOptions, RendererOptions, WindowRenderer};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

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
    /// Закрыть окно после стольких нарисованных кадров (для автопроверок).
    pub max_frames: Option<u64>,
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
    let event_loop = EventLoop::new().map_err(|e| AppError::Window(e.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        config,
        window: None,
        renderer: None,
        error: None,
        start: Instant::now(),
        frames: 0,
        stats: FrameStats::new(Instant::now()),
        minimized: false,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| AppError::Window(e.to_string()))?;
    if let Some(e) = app.error.take() {
        return Err(e);
    }
    let seconds = app.start.elapsed().as_secs_f64();
    Ok(RunSummary {
        frames: app.frames,
        adapter: app.stats.adapter.clone(),
        average_fps: if seconds > 0.0 {
            app.frames as f64 / seconds
        } else {
            0.0
        },
    })
}

struct App {
    config: AppConfig,
    // Рендерер держит своё Arc на окно, так что порядок полей здесь не опасен.
    renderer: Option<WindowRenderer>,
    window: Option<Arc<Window>>,
    error: Option<AppError>,
    start: Instant,
    frames: u64,
    stats: FrameStats,
    minimized: bool,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: AppError) {
        // Сообщение выведет тот, кто вызвал run(), здесь только запоминаем ошибку.
        self.error = Some(error);
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
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.start = Instant::now();
        self.stats.last_title = self.start;
        self.stats.frames_since_title = 0;
        Ok(())
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer)) = (&self.window, &mut self.renderer) else {
            return;
        };
        let size = window.inner_size();
        let time = self.start.elapsed().as_secs_f32();
        match renderer.render([size.width, size.height], time) {
            Ok(FrameStatus::Presented) => {
                self.frames += 1;
                let now = Instant::now();
                if let Some(title) = self.stats.frame(now, &self.config.title) {
                    window.set_title(&title);
                }
                if self.config.max_frames.is_some_and(|max| self.frames >= max) {
                    tracing::info!("Нарисовано {} кадров, закрываю окно", self.frames);
                    event_loop.exit();
                }
            }
            Ok(FrameStatus::Skipped) => {}
            Err(e) => self.fail(event_loop, e.into()),
        }
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
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.minimized = size.width == 0 || size.height == 0;
                if let Some(r) = &mut self.renderer {
                    r.resize();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Свёрнутое окно не рисуем и не крутим цикл впустую: ждём событий.
        if self.minimized {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        event_loop.set_control_flow(ControlFlow::Poll);
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Сначала Vulkan, потом окно.
        self.renderer = None;
        self.window = None;
    }
}

/// Счётчик кадров для заголовка окна.
struct FrameStats {
    adapter: String,
    mode: &'static str,
    last_title: Instant,
    frames_since_title: u32,
}

impl FrameStats {
    fn new(now: Instant) -> Self {
        Self {
            adapter: String::new(),
            mode: "",
            last_title: now,
            frames_since_title: 0,
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
        Some(format_title(base, &self.adapter, self.mode, fps, ms))
    }
}

fn format_title(base: &str, adapter: &str, mode: &str, fps: f64, ms: f64) -> String {
    format!("{base} | {adapter} | {fps:.0} FPS, {ms:.2} мс | {mode}")
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
        );
        assert_eq!(
            t,
            "ReyVec 0.1.0 | NVIDIA GeForce RTX 5080 | 144 FPS, 6.93 мс | VSync"
        );
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
