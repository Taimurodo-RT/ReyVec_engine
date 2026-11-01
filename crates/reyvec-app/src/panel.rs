//! Панель со счётчиками поверх кадра (egui). F1 показывает и прячет её.

use std::collections::VecDeque;
use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, RichText};
use reyvec_gpu::{GpuFrameTimings, GraphReport, UiFrame};
use winit::event::WindowEvent;
use winit::window::Window;

/// Бюджет видеокарты на кадр, мс (`docs/decisions/0003-performance-targets.md`).
const GPU_BUDGET_MS: f64 = 4.0;
/// Сколько последних кадров видно на графике.
const HISTORY: usize = 240;

/// Что показывает панель. Собирается окном каждый кадр.
pub(crate) struct PanelInfo<'a> {
    pub adapter: &'a str,
    pub mode: &'a str,
    pub extent: [u32; 2],
    /// Средние за полсекунды.
    pub fps: f64,
    pub frame_ms: f64,
    pub gpu: Option<&'a GpuFrameTimings>,
    pub graph: Option<&'a GraphReport>,
    pub validation: bool,
    pub validation_errors: u32,
    pub hot_reload: bool,
    pub shader_error: Option<String>,
    /// Имя файла последнего снимка кадра.
    pub last_snapshot: Option<&'a str>,
    /// Название сцены песочницы.
    pub scene: &'a str,
    /// Сглаживание сцены словами.
    pub msaa: &'a str,
    /// Края при MSAA «как в редакторах», а не физически точные.
    pub perceptual_edges: bool,
    /// Как рисуются заливки векторных фигур.
    pub fill: &'a str,
    /// Лупа векторной сцены включена.
    pub zoom: bool,
}

pub(crate) struct Panel {
    ctx: egui::Context,
    state: egui_winit::State,
    visible: bool,
    /// Время видеокарты последних кадров, мс.
    gpu_history: VecDeque<f32>,
}

impl Panel {
    pub fn new(window: &Window) -> Self {
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts());
        ctx.set_theme(egui::Theme::Dark);
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            Some(winit::window::Theme::Dark),
            Some(4096),
        );
        Self {
            ctx,
            state,
            visible: true,
            gpu_history: VecDeque::with_capacity(HISTORY),
        }
    }

    /// Показывает или прячет панель.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Отдаёт событие окна egui (мышь, клавиатура, масштаб). `true`: событие для панели.
    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// Добавляет время видеокарты кадра на график.
    pub fn record_gpu(&mut self, ms: f64) {
        if self.gpu_history.len() == HISTORY {
            self.gpu_history.pop_front();
        }
        self.gpu_history.push_back(ms as f32);
    }

    /// Раскладывает панель и превращает её в треугольники. `None`, если панель спрятана.
    pub fn frame(&mut self, window: &Window, info: &PanelInfo) -> Option<UiFrame> {
        if !self.visible {
            return None;
        }
        let _zone = reyvec_gpu::cpu_zone!("панель");
        let input = self.state.take_egui_input(window);
        let history = &self.gpu_history;
        let out = self.ctx.run_ui(input, |ui| {
            egui::Window::new("Счётчики")
                .title_bar(false)
                .resizable(false)
                .default_pos([12.0, 12.0])
                .show(ui.ctx(), |ui| contents(ui, info, history));
        });
        self.state
            .handle_platform_output(window, out.platform_output);
        let primitives = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        Some(UiFrame {
            primitives,
            textures_delta: out.textures_delta,
            pixels_per_point: out.pixels_per_point,
        })
    }
}

fn contents(ui: &mut egui::Ui, info: &PanelInfo, history: &VecDeque<f32>) {
    let mono = |text: String| RichText::new(text).family(FontFamily::Monospace);
    ui.label(RichText::new("Счётчики").strong());
    ui.label(format!("{}, {}", info.adapter, info.mode));
    ui.label(mono(format!(
        "Окно {}×{}   {:.0} FPS, {:.2} мс",
        info.extent[0], info.extent[1], info.fps, info.frame_ms
    )));
    ui.separator();

    match info.gpu {
        Some(gpu) => {
            let color = budget_color(gpu.total_ms);
            ui.label(
                mono(format!(
                    "Видеокарта {:.3} мс (бюджет {GPU_BUDGET_MS:.0} мс)",
                    gpu.total_ms
                ))
                .color(color),
            );
            for p in &gpu.passes {
                ui.label(mono(format!("  {:<8} {:.3} мс", p.name, p.ms)));
            }
            gpu_chart(ui, history);
        }
        None => {
            ui.label("Видеокарта не умеет замерять время проходов");
        }
    }
    ui.separator();

    if let Some(graph) = info.graph {
        ui.label(format!("Граф: {}", graph.passes.join(" → ")));
        ui.label(mono(format!(
            "Барьеров {}, временных картинок {}",
            graph.barriers, graph.transient_images
        )));
    }
    if info.validation {
        let errors = info.validation_errors;
        let text = mono(format!("Ошибок валидации Vulkan: {errors}"));
        ui.label(if errors == 0 {
            text
        } else {
            text.color(Color32::from_rgb(255, 90, 80))
        });
    } else {
        ui.label("Проверка Vulkan выключена (--validation)");
    }
    match &info.shader_error {
        Some(err) => {
            ui.label(
                RichText::new(format!("Ошибка шейдера: {err}"))
                    .color(Color32::from_rgb(255, 90, 80)),
            );
        }
        None if info.hot_reload => {
            ui.label("Шейдеры: перезагрузка на лету включена");
        }
        None => {}
    }
    if let Some(name) = info.last_snapshot {
        ui.label(mono(format!("Снимок: {name}")));
    }
    ui.label(mono(format!(
        "Сцена: {}, сглаживание: {}",
        info.scene, info.msaa
    )));
    ui.label(mono(format!(
        "Края: {}",
        if info.perceptual_edges {
            "как в редакторах"
        } else {
            "физически точные"
        }
    )));
    ui.label(mono(format!(
        "Заливка: {}{}",
        info.fill,
        if info.zoom { ", лупа ×8" } else { "" }
    )));
    ui.label(
        RichText::new(
            "1–7 сцена, M сглаживание, G края, F заливка, Z лупа, F1 прячет панель, F12 снимок",
        )
        .weak(),
    );
}

/// Зелёный, пока видеокарта укладывается в бюджет с запасом, жёлтый у края, красный сверх.
fn budget_color(ms: f64) -> Color32 {
    if ms <= GPU_BUDGET_MS * 0.75 {
        Color32::from_rgb(110, 210, 120)
    } else if ms <= GPU_BUDGET_MS {
        Color32::from_rgb(240, 200, 80)
    } else {
        Color32::from_rgb(255, 90, 80)
    }
}

/// Столбики времени видеокарты по кадрам и линия бюджета.
fn gpu_chart(ui: &mut egui::Ui, history: &VecDeque<f32>) {
    let size = egui::vec2(260.0, 56.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Color32::from_black_alpha(90));
    let max = history
        .iter()
        .copied()
        .fold(GPU_BUDGET_MS as f32 * 1.25, f32::max);
    let y_of = |ms: f32| rect.bottom() - (ms / max).min(1.0) * rect.height();
    let bar = rect.width() / HISTORY as f32;
    let start = HISTORY - history.len();
    for (i, &ms) in history.iter().enumerate() {
        let x = rect.left() + (start + i) as f32 * bar;
        let r =
            egui::Rect::from_min_max(egui::pos2(x, y_of(ms)), egui::pos2(x + bar, rect.bottom()));
        painter.rect_filled(r, 0.0, budget_color(f64::from(ms)));
    }
    let budget_y = y_of(GPU_BUDGET_MS as f32);
    painter.line_segment(
        [
            egui::pos2(rect.left(), budget_y),
            egui::pos2(rect.right(), budget_y),
        ],
        egui::Stroke::new(1.0, Color32::from_white_alpha(140)),
    );
    painter.text(
        egui::pos2(rect.right() - 4.0, budget_y - 2.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{GPU_BUDGET_MS:.0} мс"),
        egui::FontId::monospace(11.0),
        Color32::from_white_alpha(170),
    );
}

/// Шрифты с кириллицей (лицензии в `assets/fonts/README.md`).
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.otf"
        ))),
    );
    fonts.font_data.insert(
        "jetbrains-mono".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/JetBrainsMono-Regular.ttf"
        ))),
    );
    fonts.families.insert(
        FontFamily::Proportional,
        vec!["inter".into(), "jetbrains-mono".into()],
    );
    fonts.families.insert(
        FontFamily::Monospace,
        vec!["jetbrains-mono".into(), "inter".into()],
    );
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_colors() {
        assert_eq!(budget_color(1.0), Color32::from_rgb(110, 210, 120));
        assert_eq!(budget_color(3.5), Color32::from_rgb(240, 200, 80));
        assert_eq!(budget_color(5.0), Color32::from_rgb(255, 90, 80));
    }

    /// Панель раскладывается без окна: шрифты читаются, кириллица и цифры есть в атласе.
    #[test]
    fn panel_lays_out_with_cyrillic_fonts() {
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts());
        let timings = GpuFrameTimings {
            total_ms: 0.25,
            passes: vec![reyvec_gpu::PassTiming {
                name: "сцена",
                ms: 0.25,
            }],
        };
        let info = PanelInfo {
            adapter: "Видеокарта",
            mode: "VSync",
            extent: [1280, 720],
            fps: 144.0,
            frame_ms: 6.94,
            gpu: Some(&timings),
            graph: None,
            validation: true,
            validation_errors: 0,
            hot_reload: true,
            shader_error: None,
            last_snapshot: Some("кадр-001.png"),
            scene: "векторы",
            msaa: "MSAA 4×",
            perceptual_edges: true,
            fill: "у каждой фигуры свой способ",
            zoom: false,
        };
        let history: VecDeque<f32> = [0.2, 0.3, 5.0].into_iter().collect();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        // Первый проход egui только меряет окна и ничего не рисует, поэтому кадров два.
        let mut atlas = false;
        let mut prims = Vec::new();
        for _ in 0..2 {
            let mut out = ctx.run_ui(input.clone(), |ui| {
                egui::Window::new("Счётчики")
                    .title_bar(false)
                    .show(ui.ctx(), |ui| contents(ui, &info, &history));
            });
            atlas |= !out.textures_delta.set.is_empty();
            out.textures_delta.clear();
            prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        }
        assert!(!prims.is_empty(), "панель нарисована");
        assert!(atlas, "атлас шрифта загружается");
    }
}
