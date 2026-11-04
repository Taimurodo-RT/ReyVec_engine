//! Интерфейс редактора на egui: верхняя полоса со своими кнопками окна, вкладки рабочих
//! пространств, панели вокруг живой сцены и строка состояния.
//!
//! Здесь только раскладка и рисование. Всё, что надо сделать с окном, движком или сценой,
//! интерфейс не делает сам, а складывает в список [`Action`]: его выполняет цикл окна.
//! Поэтому интерфейс раскладывается и проверяется тестами без окна и видеокарты.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Id, Layout, Margin, Pos2,
    Rect, Response, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2,
};
use reyvec_scene::{Background, NodeDesc, SceneFile};
use winit::window::ResizeDirection;

use super::layout::{START_WORKSPACE, Side, Tab, WORKSPACES, Workspace};
use super::outline::{self, Kind};
use super::theme::*;

/// Сколько строк журнала держать в консоли редактора.
const LOG_LINES: usize = 500;
/// Ширина края окна без рамки, за который его тянут, точек.
const EDGE: f32 = 5.0;

/// Идёт ли игра в окне сцены.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayState {
    /// Правка: игра стоит, камеру двигает мышь.
    Edit,
    /// Игра идёт: клавиши управляют героем.
    Playing,
    /// Игра на паузе.
    Paused,
}

/// Что попросили сделать в интерфейсе.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Action {
    Play,
    Pause,
    Resume,
    Stop,
    Minimize,
    ToggleMaximize,
    Close,
    /// Тащить окно за верхнюю полосу.
    DragWindow,
    /// Тянуть окно за край.
    Resize(ResizeDirection),
    /// Сдвинуть камеру сцены на столько точек интерфейса.
    Pan(Vec2),
    /// Камеру в начало сцены.
    ResetCamera,
    /// Прочитать файл сцены с диска заново.
    ReloadScene,
}

/// Что интерфейсу показать в этом кадре.
pub(crate) struct Info<'a> {
    pub play: PlayState,
    /// Окно развёрнуто на весь экран.
    pub maximized: bool,
    /// Окно без рамки системы: свои кнопки и края.
    pub frameless: bool,
    pub fps: f64,
    pub frame_ms: f64,
    /// Время видеокарты на кадр, мс (если она умеет замерять).
    pub gpu_ms: Option<f64>,
    pub adapter: &'a str,
    /// Прочитанный файл сцены (для иерархии и инспектора).
    pub scene: Option<&'a SceneFile>,
    /// Имя файла сцены.
    pub scene_name: &'a str,
    /// Ошибка в файле сцены или шейдере.
    pub error: Option<&'a str>,
}

/// Состояние интерфейса между кадрами.
pub(crate) struct EditorUi {
    workspace: usize,
    /// Открытая вкладка каждой области каждого пространства.
    tabs: Vec<[usize; 3]>,
    selected: Option<Vec<usize>>,
    open: HashSet<Vec<usize>>,
    log: VecDeque<(Color32, String)>,
    assets: Assets,
    /// Где сцена в прошлом кадре (точки интерфейса).
    viewport: Option<Rect>,
    viewport_hovered: bool,
    actions: Vec<Action>,
    /// Что показывать в нижней строке (меню «Вид → Нижняя строка»).
    status: [bool; STATUS_ITEMS.len()],
    /// Ширина вкладок пространств в прошлом кадре: по ней они ставятся в середину полосы.
    tabs_width: f32,
}

/// Элементы нижней строки, их можно выключить в меню «Вид». Порядок — как на экране.
const STATUS_ITEMS: [&str; 6] = [
    "Сцена и её состояние",
    "Видеокарта",
    "Число предметов",
    "Кадров в секунду",
    "Время кадра",
    "Подсказка клавиш",
];
const ST_SCENE: usize = 0;
const ST_GPU: usize = 1;
const ST_COUNT: usize = 2;
const ST_FPS: usize = 3;
const ST_FRAME: usize = 4;
const ST_HINT: usize = 5;

impl EditorUi {
    /// `project`: папка проекта (в ней `assets` и `scenes`), если нашлась.
    pub(crate) fn new(project: Option<&Path>) -> Self {
        Self {
            workspace: START_WORKSPACE,
            tabs: vec![[0; 3]; WORKSPACES.len()],
            selected: None,
            open: HashSet::new(),
            log: VecDeque::new(),
            assets: Assets::scan(project),
            viewport: None,
            viewport_hovered: false,
            actions: Vec::new(),
            status: [true; STATUS_ITEMS.len()],
            tabs_width: 0.0,
        }
    }

    /// Что попросили сделать с прошлого вызова.
    pub(crate) fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    /// Прямоугольник сцены в прошлом кадре, точки интерфейса.
    pub(crate) fn viewport(&self) -> Option<Rect> {
        self.viewport
    }

    /// Мышь над сценой (а не над панелью).
    pub(crate) fn viewport_hovered(&self) -> bool {
        self.viewport_hovered
    }

    /// Новые строки журнала программы.
    pub(crate) fn push_log(&mut self, lines: impl IntoIterator<Item = (Color32, String)>) {
        for line in lines {
            if self.log.len() == LOG_LINES {
                self.log.pop_front();
            }
            self.log.push_back(line);
        }
    }

    /// Файл сцены перечитан: выбранный предмет мог пропасть.
    pub(crate) fn scene_changed(&mut self, file: Option<&SceneFile>) {
        if let (Some(path), Some(file)) = (&self.selected, file)
            && outline::node_at(file, path).is_none()
        {
            self.selected = None;
        }
    }

    /// Раскладывает и рисует весь интерфейс.
    pub(crate) fn draw(&mut self, ui: &mut Ui, info: &Info) {
        let ctx = ui.ctx().clone();
        self.hotkeys(&ctx, info);
        egui::Panel::top("editor-title")
            .exact_size(TITLE_HEIGHT)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(BAR).inner_margin(PAD_BAR))
            .show(ui, |ui| self.title_bar(ui, info));
        egui::Panel::bottom("editor-status")
            .exact_size(STATUS_HEIGHT)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(BAR)
                    .inner_margin(Margin::symmetric(GAP_XL as i8, 0)),
            )
            .show(ui, |ui| status_bar(ui, info, &self.status));
        let ws = &WORKSPACES[self.workspace];
        let panel_frame = Frame::NONE.fill(PANEL);
        // Одни и те же панели у всех пространств: размер, который потянули мышью,
        // сохраняется при переходе между вкладками.
        egui::Panel::left("editor-left")
            .resizable(true)
            .default_size(LEFT_WIDTH)
            .size_range(SIDE_RANGE)
            .frame(panel_frame)
            .show(ui, |ui| self.area(ui, ws, Side::Left, info));
        egui::Panel::right("editor-right")
            .resizable(true)
            .default_size(RIGHT_WIDTH)
            .size_range(SIDE_RANGE)
            .frame(panel_frame)
            .show(ui, |ui| self.area(ui, ws, Side::Right, info));
        egui::Panel::bottom("editor-bottom")
            .resizable(true)
            .default_size(BOTTOM_HEIGHT)
            .size_range(BOTTOM_RANGE)
            .frame(panel_frame)
            .show(ui, |ui| self.area(ui, ws, Side::Bottom, info));
        egui::CentralPanel::no_frame().show(ui, |ui| self.scene_view(ui, info));
        if info.frameless && !info.maximized {
            self.window_edges(&ctx);
        }
    }

    fn hotkeys(&mut self, ctx: &egui::Context, info: &Info) {
        let (f5, esc) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::F5),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if f5 {
            self.actions.push(match info.play {
                PlayState::Edit => Action::Play,
                PlayState::Playing | PlayState::Paused => Action::Stop,
            });
        } else if esc && info.play != PlayState::Edit {
            self.actions.push(Action::Stop);
        }
    }

    /// Край окна под мышью: курсор «тянуть» и начало растягивания.
    fn window_edges(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let Some(p) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let Some(dir) = edge_at(screen, p, EDGE) else {
            return;
        };
        ctx.set_cursor_icon(resize_cursor(dir));
        if ctx.input(|i| i.pointer.primary_pressed()) {
            self.actions.push(Action::Resize(dir));
        }
    }

    fn title_bar(&mut self, ui: &mut Ui, info: &Info) {
        // Пустое место полосы: тащить окно и разворачивать двойным щелчком. Кнопки и
        // вкладки ниже лежат поверх и забирают мышь себе.
        let bar = ui.max_rect();
        let full = ui.clip_rect();
        ui.painter().hline(
            full.x_range(),
            full.bottom() - 0.5,
            Stroke::new(LINE, BORDER),
        );
        let drag = ui.interact(
            bar,
            ui.scope_id().with("drag-window"),
            Sense::click_and_drag(),
        );
        if info.frameless {
            if drag.drag_started_by(egui::PointerButton::Primary) {
                self.actions.push(Action::DragWindow);
            }
            if drag.double_clicked() {
                self.actions.push(Action::ToggleMaximize);
            }
        }
        let mut left = 0.0;
        let mut right = bar.right();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_S;
            logo(ui);
            ui.scope(|ui| self.menus(ui));
            left = ui.min_rect().right();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if info.frameless {
                    self.window_buttons(ui, info);
                    ui.add_space(GAP_XL);
                }
                ui.spacing_mut().item_spacing.x = GAP_M;
                self.play_buttons(ui, info.play);
                right = ui.min_rect().left();
            });
        });
        // Вкладки пространств посередине окна; если места мало — сразу за названием.
        let w = self.tabs_width;
        let x = (full.center().x - w / 2.0).clamp(left + GAP_XL, (right - GAP_XL - w).max(left));
        let rect = Rect::from_min_size(pos2(x, bar.top()), vec2(w.max(1.0), bar.height()));
        let r = ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| self.workspace_tabs(ui),
        );
        if (r.inner - self.tabs_width).abs() > 0.5 {
            // Первый кадр или новый шрифт: переложить кадр, чтобы вкладки не прыгали.
            ui.ctx().request_discard("ширина вкладок пространств");
        }
        self.tabs_width = r.inner;
    }

    /// Меню под названием движка: «Файл», «Вид», «Справка» и их пункты.
    fn menus(&mut self, ui: &mut Ui) {
        // Название — как подпись, без рамки; рамка только под мышью.
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
        w.inactive.bg_stroke = Stroke::NONE;
        let mut pick = None;
        let actions = &mut self.actions;
        let current = self.workspace;
        let status = &mut self.status;
        let brand = RichText::new("ReyVec").color(TEXT).size(FONT_BRAND);
        ui.menu_button(brand, |ui| {
            ui.set_min_width(180.0);
            ui.menu_button("Файл", |ui| {
                if ui.button("Перечитать сцену с диска").clicked() {
                    actions.push(Action::ReloadScene);
                    ui.close();
                }
                ui.separator();
                if ui.button("Выход").clicked() {
                    actions.push(Action::Close);
                    ui.close();
                }
            });
            ui.menu_button("Вид", |ui| {
                for (i, w) in WORKSPACES.iter().enumerate() {
                    if ui
                        .selectable_label(i == current, w.name)
                        .on_hover_text(w.hint)
                        .clicked()
                    {
                        pick = Some(i);
                        ui.close();
                    }
                }
                ui.separator();
                ui.menu_button("Нижняя строка", |ui| {
                    for (on, name) in status.iter_mut().zip(STATUS_ITEMS) {
                        ui.checkbox(on, name);
                    }
                });
                if ui.button("Камера в начало сцены").clicked() {
                    actions.push(Action::ResetCamera);
                    ui.close();
                }
            });
            ui.menu_button("Справка", |ui| {
                ui.label("F5 — играть или остановить");
                ui.label("Esc во время игры — остановить");
                ui.label("Колесо над сценой — приблизить");
                ui.label("Перетаскивание сцены — сдвинуть камеру");
                ui.label("Двойной щелчок по верхней полосе — окно меньше или больше");
            });
        });
        if let Some(i) = pick {
            self.workspace = i;
        }
    }

    /// Вкладки пространств; возвращает их ширину.
    fn workspace_tabs(&mut self, ui: &mut Ui) -> f32 {
        Frame::NONE
            .fill(BG)
            .stroke(Stroke::new(LINE, BORDER))
            .corner_radius(RADIUS_L)
            .inner_margin(Margin::same(GAP_XS as i8 + 1))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = GAP_XS;
                for (i, w) in WORKSPACES.iter().enumerate() {
                    let on = i == self.workspace;
                    let text =
                        RichText::new(w.name).color(if on { Color32::WHITE } else { TEXT_DIM });
                    let button = egui::Button::new(text)
                        .fill(if on { HOVER } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(RADIUS_L - 3);
                    if ui.add(button).on_hover_text(w.hint).clicked() {
                        self.workspace = i;
                    }
                }
            })
            .response
            .rect
            .width()
    }

    fn play_buttons(&mut self, ui: &mut Ui, play: PlayState) {
        // Справа налево: последняя кнопка в коде — левая на экране.
        match play {
            PlayState::Edit => {
                if icon_button(ui, Icon::Play, "Играть", true).clicked() {
                    self.actions.push(Action::Play);
                }
            }
            PlayState::Playing => {
                if icon_button(ui, Icon::Stop, "Стоп", false).clicked() {
                    self.actions.push(Action::Stop);
                }
                if icon_button(ui, Icon::Pause, "Пауза", false).clicked() {
                    self.actions.push(Action::Pause);
                }
            }
            PlayState::Paused => {
                if icon_button(ui, Icon::Stop, "Стоп", false).clicked() {
                    self.actions.push(Action::Stop);
                }
                if icon_button(ui, Icon::Play, "Дальше", true).clicked() {
                    self.actions.push(Action::Resume);
                }
            }
        }
    }

    fn window_buttons(&mut self, ui: &mut Ui, info: &Info) {
        let size = vec2(WINDOW_BUTTON_WIDTH, TITLE_HEIGHT);
        for (icon, action, hover) in [
            (Icon::Close, Action::Close, CLOSE_HOVER),
            (
                if info.maximized {
                    Icon::Restore
                } else {
                    Icon::Maximize
                },
                Action::ToggleMaximize,
                HOVER,
            ),
            (Icon::Minimize, Action::Minimize, HOVER),
        ] {
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            let label = match action {
                Action::Close => "Закрыть",
                Action::Minimize => "Свернуть",
                _ if info.maximized => "Сделать окно меньше",
                _ => "Развернуть",
            };
            let response = response.on_hover_text(label);
            if response.hovered() {
                ui.painter().rect_filled(rect, 0, hover);
            }
            let color = if response.hovered() {
                Color32::WHITE
            } else {
                TEXT_DIM
            };
            paint_icon(ui.painter(), rect.center(), icon, color);
            if response.clicked() {
                self.actions.push(action);
            }
        }
    }

    /// Одна область: строка вкладок и открытая вкладка.
    fn area(&mut self, ui: &mut Ui, ws: &Workspace, side: Side, info: &Info) {
        let tabs = ws.tabs(side);
        let active = &mut self.tabs[self.workspace][side.index()];
        if *active >= tabs.len() {
            *active = 0;
        }
        tab_strip(ui, tabs, active);
        let tab = tabs[*active];
        Frame::NONE.inner_margin(PAD_PANEL).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("area", self.workspace, side.index()))
                .auto_shrink(false)
                .stick_to_bottom(tab == Tab::Console)
                .show(ui, |ui| match tab {
                    Tab::Hierarchy => self.hierarchy(ui, info),
                    Tab::Inspector => self.inspector(ui, info),
                    Tab::Assets => self.assets.draw(ui),
                    Tab::Console => console(ui, &self.log),
                    Tab::Profiler => profiler(ui, info),
                    Tab::Soon { title, text } => soon(ui, title, text),
                });
        });
    }

    fn hierarchy(&mut self, ui: &mut Ui, info: &Info) {
        ui.label(
            RichText::new(info.scene_name.to_uppercase())
                .small()
                .color(MUTED),
        );
        let Some(file) = info.scene else {
            ui.label(RichText::new("Сцена не прочитана").color(BAD));
            return;
        };
        ui.spacing_mut().item_spacing.y = 1.0;
        for row in outline::rows(file, &self.open) {
            let selected = self.selected.as_ref() == Some(&row.path);
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            let painter = ui.painter();
            if selected {
                painter.rect(
                    rect,
                    6,
                    SELECT_BG,
                    Stroke::new(LINE, SELECT),
                    egui::StrokeKind::Inside,
                );
            } else if response.hovered() {
                painter.rect_filled(rect, 6, HOVER);
            }
            let indent = rect.left() + GAP_M + row.depth as f32 * INDENT;
            let arrow = Rect::from_min_size(pos2(indent, rect.top()), vec2(14.0, rect.height()));
            if row.has_children {
                let open = self.open.contains(&row.path);
                paint_icon(
                    painter,
                    arrow.center(),
                    if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    MUTED,
                );
            }
            let dot =
                Rect::from_center_size(pos2(arrow.right() + 8.0, rect.center().y), vec2(8.0, 8.0));
            let round = if row.kind == Kind::Light { 4 } else { 2 };
            painter.rect_filled(dot, round, row.kind.color());
            painter.text(
                pos2(dot.right() + 8.0, rect.center().y),
                Align2::LEFT_CENTER,
                &row.name,
                FontId::proportional(FONT_BODY),
                if selected { Color32::WHITE } else { TEXT_DIM },
            );
            if response.clicked() {
                let on_arrow = row.has_children
                    && response
                        .interact_pointer_pos()
                        .is_some_and(|p| arrow.expand2(vec2(4.0, 0.0)).contains(p));
                if on_arrow {
                    if !self.open.remove(&row.path) {
                        self.open.insert(row.path.clone());
                    }
                } else {
                    self.selected = Some(row.path.clone());
                }
            }
            if response.double_clicked() && row.has_children && !self.open.remove(&row.path) {
                self.open.insert(row.path.clone());
            }
        }
    }

    fn inspector(&mut self, ui: &mut Ui, info: &Info) {
        let Some(file) = info.scene else {
            ui.label(RichText::new("Сцена не прочитана").color(BAD));
            if let Some(e) = info.error {
                ui.label(RichText::new(e).color(TEXT_DIM));
            }
            return;
        };
        let node = self
            .selected
            .as_ref()
            .and_then(|p| outline::node_at(file, p).map(|n| (p, n)));
        match node {
            Some((path, node)) => node_inspector(ui, path, node),
            None => scene_inspector(ui, file, info.scene_name),
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Править свойства мышью можно будет в следующих шагах. Пока сцену меняют в \
                 файле .ron: после сохранения редактор покажет новую версию.",
            )
            .small()
            .color(MUTED),
        );
    }

    fn scene_view(&mut self, ui: &mut Ui, info: &Info) {
        let rect = ui.max_rect();
        self.viewport = Some(rect);
        let response = ui.interact(
            rect,
            ui.scope_id().with("scene-view"),
            Sense::click_and_drag(),
        );
        self.viewport_hovered = response.hovered() || response.dragged();
        if info.play == PlayState::Edit
            && (response.dragged_by(egui::PointerButton::Primary)
                || response.dragged_by(egui::PointerButton::Middle))
        {
            let d = response.drag_delta();
            if d != Vec2::ZERO {
                self.actions.push(Action::Pan(d));
            }
        }
        let painter = ui.painter_at(rect);
        if info.play == PlayState::Playing {
            // Рамка цвета акцента: сразу видно, что идёт игра, а не правка.
            painter.rect_stroke(
                rect.shrink(1.0),
                0,
                Stroke::new(LINE_FOCUS, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        let (text, color) = match info.play {
            PlayState::Edit => (
                "Правка · колесо — масштаб, перетаскивание — сдвиг",
                TEXT_DIM,
            ),
            PlayState::Playing => ("Игра идёт · Esc или «Стоп» — вернуться к правке", ACCENT),
            PlayState::Paused => ("Пауза", ACCENT),
        };
        chip(&painter, rect.left_top() + vec2(12.0, 12.0), text, color);
        if let Some(e) = info.error {
            chip(
                &painter,
                pos2(rect.left() + 12.0, rect.bottom() - 40.0),
                &format!("Ошибка: {e}"),
                BAD,
            );
        }
    }
}

/// Строка вкладок наверху области.
fn tab_strip(ui: &mut Ui, tabs: &[Tab], active: &mut usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), TAB_HEIGHT), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, BORDER),
    );
    let mut x = rect.left() + 6.0;
    for (i, tab) in tabs.iter().enumerate() {
        let font = FontId::proportional(FONT_BODY);
        let width = ui
            .painter()
            .layout_no_wrap(tab.label().to_owned(), font.clone(), TEXT)
            .size()
            .x
            + 20.0;
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(width, TAB_HEIGHT));
        let response = ui.interact(r, ui.scope_id().with(("tab", i)), Sense::click());
        let on = i == *active;
        if response.clicked() {
            *active = i;
        }
        if response.hovered() && !on {
            ui.painter()
                .rect_filled(r.shrink2(vec2(0.0, 4.0)), 6, HOVER);
        }
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            tab.label(),
            font,
            if on { Color32::WHITE } else { MUTED },
        );
        if on {
            let line = Rect::from_min_max(
                pos2(r.left() + 8.0, r.bottom() - 2.0),
                pos2(r.right() - 8.0, r.bottom()),
            );
            ui.painter().rect_filled(line, 1, ACCENT);
        }
        x += width + 2.0;
    }
}

fn node_inspector(ui: &mut Ui, path: &[usize], node: &NodeDesc) {
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        ui.painter().rect_filled(dot, 3, Kind::of(node).color());
        ui.label(
            RichText::new(outline::display_name(node))
                .heading()
                .color(Color32::WHITE),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let id: Vec<String> = path.iter().map(ToString::to_string).collect();
            ui.label(
                RichText::new(format!("#{}", id.join(".")))
                    .monospace()
                    .color(MUTED),
            );
        });
    });
    let parts = outline::parts(node);
    if !parts.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for p in parts {
                Frame::NONE
                    .fill(RAISED)
                    .corner_radius(10)
                    .inner_margin(Margin::symmetric(8, 2))
                    .show(ui, |ui| ui.label(RichText::new(p).small().color(TEXT_DIM)));
            }
        });
    }
    ui.add_space(4.0);
    card(ui, "Положение", |ui| {
        egui::Grid::new("node-place")
            .num_columns(3)
            .spacing(vec2(6.0, 6.0))
            .show(ui, |ui| {
                prop(ui, "Место");
                value(ui, &format!("{:.2}", node.at.0), Some(BAD));
                value(ui, &format!("{:.2}", node.at.1), Some(GOOD));
                ui.end_row();
                prop(ui, "Поворот");
                value(ui, &format!("{:.1}°", node.angle), None);
                ui.label("");
                ui.end_row();
                prop(ui, "Размер");
                value(ui, &format!("{:.2}", node.scale), None);
                ui.label("");
                ui.end_row();
                prop(ui, "Глубина");
                value(ui, &format!("{:.2}", node.depth), None);
                ui.label("");
                ui.end_row();
                prop(ui, "Прозрачность");
                value(ui, &format!("{:.2}", node.opacity), None);
                ui.label("");
                ui.end_row();
            });
    });
    let mut motion = Vec::new();
    if node.spin != 0.0 {
        motion.push(format!("вращение {:.0}°/с", node.spin));
    }
    if let Some(s) = &node.swing {
        motion.push(format!("качание {:.0}° за {:.1} с", s.angle, s.period));
    }
    if let Some(b) = &node.bob {
        motion.push(format!(
            "покачивание на ({:.2}, {:.2}) за {:.1} с",
            b.by.0, b.by.1, b.period
        ));
    }
    if !motion.is_empty() {
        card(ui, "Движение", |ui| {
            for m in motion {
                ui.label(RichText::new(m).color(TEXT_DIM));
            }
        });
    }
}

fn scene_inspector(ui: &mut Ui, file: &SceneFile, name: &str) {
    ui.label(
        RichText::new("Свойства сцены")
            .heading()
            .color(Color32::WHITE),
    );
    ui.label(RichText::new(name).small().color(MUTED));
    ui.add_space(4.0);
    card(ui, "Сцена", |ui| {
        egui::Grid::new("scene-props")
            .num_columns(2)
            .spacing(vec2(10.0, 6.0))
            .show(ui, |ui| {
                prop(ui, "Фон");
                ui.label(match file.background {
                    Background::Plain => "светлый пол",
                    Background::Lab => "лаборатория",
                });
                ui.end_row();
                prop(ui, "Общий свет");
                ui.horizontal(|ui| {
                    let [r, g, b, _] = file.ambient.0.to_srgb_array();
                    let c = Color32::from_rgb(
                        (r * 255.0).round() as u8,
                        (g * 255.0).round() as u8,
                        (b * 255.0).round() as u8,
                    );
                    let (sw, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    ui.painter().rect(
                        sw,
                        4,
                        c,
                        Stroke::new(1.0, BORDER_STRONG),
                        egui::StrokeKind::Inside,
                    );
                    ui.label(
                        RichText::new(format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()))
                            .monospace(),
                    );
                });
                ui.end_row();
                prop(ui, "Предметов");
                ui.label(outline::count(file).to_string());
                ui.end_row();
                prop(ui, "Слоёв");
                ui.label(file.layers.len().to_string());
                ui.end_row();
            });
    });
    ui.label(
        RichText::new("Выберите предмет в иерархии слева, чтобы увидеть его свойства.")
            .color(TEXT_DIM),
    );
}

/// Карточка с заголовком; щелчок по заголовку сворачивает её (запоминается по названию).
fn card(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    Frame::NONE
        .fill(CARD)
        .stroke(Stroke::new(LINE, BORDER_STRONG))
        .corner_radius(RADIUS_L)
        .inner_margin(PAD_CARD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let id = Id::unique(("editor-card", title));
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                true,
            );
            let (rect, header) = ui.allocate_exact_size(
                vec2(ui.available_width(), FONT_TITLE + GAP_S),
                Sense::click(),
            );
            if header.clicked() {
                state.toggle(ui);
            }
            let open = state.openness(ui.ctx());
            let color = if header.hovered() {
                Color32::WHITE
            } else {
                TEXT
            };
            let arrow = Rect::from_center_size(
                pos2(rect.right() - GAP_M, rect.center().y),
                vec2(GAP_L, GAP_L),
            );
            let painter = ui.painter();
            paint_icon(
                painter,
                arrow.center(),
                if open > 0.5 {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                MUTED,
            );
            painter.text(
                pos2(rect.left(), rect.center().y),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional(FONT_TITLE),
                color,
            );
            state.show_body_unindented(ui, |ui| {
                ui.add_space(GAP_XS);
                add(ui);
            });
        });
    ui.add_space(GAP_M);
}

fn prop(ui: &mut Ui, name: &str) {
    ui.label(RichText::new(name).color(TEXT_DIM));
}

/// Значение в тёмной рамке; `mark` — цветная полоска слева (оси x и y).
fn value(ui: &mut Ui, text: &str, mark: Option<Color32>) {
    let r = Frame::NONE
        .fill(BG)
        .corner_radius(RADIUS_S)
        .inner_margin(PAD_FIELD)
        .show(ui, |ui| {
            ui.set_min_width(56.0);
            ui.label(RichText::new(text).monospace().color(TEXT));
        });
    if let Some(c) = mark {
        let rect = r.response.rect;
        let bar = Rect::from_min_max(rect.left_top(), pos2(rect.left() + 2.0, rect.bottom()));
        ui.painter().rect_filled(
            bar,
            CornerRadius {
                nw: 6,
                sw: 6,
                ne: 0,
                se: 0,
            },
            c,
        );
    }
}

fn console(ui: &mut Ui, log: &VecDeque<(Color32, String)>) {
    if log.is_empty() {
        ui.label(RichText::new("Сообщений пока нет").color(MUTED));
    }
    ui.spacing_mut().item_spacing.y = 2.0;
    for (color, line) in log {
        ui.label(RichText::new(line).monospace().color(*color));
    }
}

fn profiler(ui: &mut Ui, info: &Info) {
    egui::Grid::new("profiler")
        .num_columns(2)
        .spacing(vec2(GAP_XXL, GAP_M))
        .show(ui, |ui| {
            prop(ui, "Кадров в секунду");
            ui.label(RichText::new(format!("{:.0}", info.fps)).monospace());
            ui.end_row();
            prop(ui, "Кадр целиком");
            ui.label(RichText::new(format!("{:.2} мс", info.frame_ms)).monospace());
            ui.end_row();
            prop(ui, "Видеокарта на кадр");
            ui.label(
                RichText::new(
                    info.gpu_ms
                        .map_or_else(|| "нет замеров".into(), |ms| format!("{ms:.2} мс")),
                )
                .monospace(),
            );
            ui.end_row();
            prop(ui, "Видеокарта");
            ui.label(info.adapter);
            ui.end_row();
        });
    if let Some(ms) = info.gpu_ms {
        ui.add_space(6.0);
        ui.label(
            RichText::new("Бюджет кадра 4 мс (RTX 5080, 1080p)")
                .small()
                .color(MUTED),
        );
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width().min(360.0), 6.0), Sense::hover());
        ui.painter().rect_filled(rect, 3, BORDER_STRONG);
        let part = (ms / 4.0).clamp(0.0, 1.0) as f32;
        let fill = Rect::from_min_size(rect.min, vec2(rect.width() * part, rect.height()));
        ui.painter()
            .rect_filled(fill, 3, if ms <= 4.0 { ACCENT } else { BAD });
    }
}

fn soon(ui: &mut Ui, title: &str, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).heading().color(Color32::WHITE));
    ui.label(RichText::new(text).color(TEXT_DIM));
    ui.add_space(6.0);
    ui.label(
        RichText::new("Этот инструмент появится в следующих шагах редактора.")
            .small()
            .color(MUTED),
    );
}

/// Нижняя строка: короткие сведения слева и справа; лишнее выключается в меню «Вид».
fn status_bar(ui: &mut Ui, info: &Info, on: &[bool; STATUS_ITEMS.len()]) {
    let small = |text: String, color| RichText::new(text).small().color(color);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = GAP_XXL;
        if on[ST_SCENE] {
            let (dot, text) = match info.error {
                Some(_) => (BAD, format!("Ошибка в сцене {}", info.scene_name)),
                None => (GOOD, format!("Сцена {}", info.scene_name)),
            };
            ui.label(small(format!("● {text}"), dot));
        }
        if on[ST_GPU] {
            ui.label(small(info.adapter.to_owned(), MUTED));
        }
        if on[ST_COUNT]
            && let Some(file) = info.scene
        {
            ui.label(small(format!("Предметов: {}", outline::count(file)), MUTED));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if on[ST_HINT] {
                ui.label(small("F5 — играть или стоп".into(), MUTED));
            }
            if on[ST_FRAME] {
                let gpu = info
                    .gpu_ms
                    .map_or_else(String::new, |ms| format!(" · видеокарта {ms:.1} мс"));
                let text = format!("кадр {:.1} мс{gpu}", info.frame_ms);
                ui.label(
                    RichText::new(text)
                        .monospace()
                        .size(FONT_SMALL)
                        .color(MUTED),
                );
            }
            if on[ST_FPS] {
                let text = format!("{:.0} к/с", info.fps);
                ui.label(
                    RichText::new(text)
                        .monospace()
                        .size(FONT_SMALL)
                        .color(TEXT_DIM),
                );
            }
        });
    });
}

/// Логотип: треугольник с точкой света.
fn logo(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
    let p = |x: f32, y: f32| {
        pos2(
            rect.left() + x / 24.0 * rect.width(),
            rect.top() + y / 24.0 * rect.height(),
        )
    };
    ui.painter().add(egui::Shape::line(
        vec![p(4.0, 19.0), p(12.0, 4.0), p(20.0, 19.0)],
        Stroke::new(2.2, ACCENT),
    ));
    ui.painter().circle_filled(p(12.0, 14.0), 2.6, ACCENT);
}

/// Подпись на тёмной плашке поверх сцены.
fn chip(painter: &egui::Painter, at: Pos2, text: &str, color: Color32) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(FONT_CHIP), color);
    let rect = Rect::from_min_size(at, galley.size() + vec2(16.0, 8.0));
    painter.rect(
        rect,
        6,
        Color32::from_rgba_unmultiplied(21, 24, 30, 220),
        Stroke::new(1.0, BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + vec2(8.0, 4.0), galley, color);
}

/// Значки, нарисованные линиями (в шрифте их может не быть).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Icon {
    Play,
    Pause,
    Stop,
    Minimize,
    Maximize,
    Restore,
    Close,
    ChevronRight,
    ChevronDown,
}

fn paint_icon(painter: &egui::Painter, c: Pos2, icon: Icon, color: Color32) {
    let s = Stroke::new(1.3, color);
    match icon {
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    c + vec2(-4.0, -5.0),
                    c + vec2(5.0, 0.0),
                    c + vec2(-4.0, 5.0),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Pause => {
            painter.rect_filled(
                Rect::from_center_size(c + vec2(-2.5, 0.0), vec2(3.0, 10.0)),
                1,
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(c + vec2(2.5, 0.0), vec2(3.0, 10.0)),
                1,
                color,
            );
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(c, vec2(9.0, 9.0)), 2, color);
        }
        Icon::Minimize => {
            painter.hline((c.x - 5.0)..=(c.x + 5.0), c.y, s);
        }
        Icon::Maximize => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(10.0, 10.0)),
                1,
                s,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Restore => {
            painter.rect_stroke(
                Rect::from_center_size(c + vec2(-1.5, 1.5), vec2(8.0, 8.0)),
                1,
                s,
                egui::StrokeKind::Inside,
            );
            painter.add(egui::Shape::line(
                vec![
                    c + vec2(-3.0, -2.5),
                    c + vec2(-3.0, -5.0),
                    c + vec2(5.0, -5.0),
                    c + vec2(5.0, 3.0),
                    c + vec2(2.5, 3.0),
                ],
                s,
            ));
        }
        Icon::Close => {
            painter.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], s);
            painter.line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], s);
        }
        Icon::ChevronRight => {
            painter.add(egui::Shape::line(
                vec![
                    c + vec2(-2.0, -4.0),
                    c + vec2(2.0, 0.0),
                    c + vec2(-2.0, 4.0),
                ],
                s,
            ));
        }
        Icon::ChevronDown => {
            painter.add(egui::Shape::line(
                vec![
                    c + vec2(-4.0, -2.0),
                    c + vec2(0.0, 2.0),
                    c + vec2(4.0, -2.0),
                ],
                s,
            ));
        }
    }
}

/// Кнопка со значком и подписью; `primary` — залитая цветом акцента.
fn icon_button(ui: &mut Ui, icon: Icon, text: &str, primary: bool) -> Response {
    let font = FontId::proportional(FONT_BODY);
    let (fg, fill) = if primary {
        (ON_ACCENT, ACCENT)
    } else {
        (TEXT, RAISED)
    };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, fg);
    let size = vec2(galley.size().x + 40.0, BUTTON_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if response.hovered() {
        if primary {
            Color32::from_rgb(255, 194, 122)
        } else {
            HOVER
        }
    } else {
        fill
    };
    let stroke = if primary {
        Stroke::NONE
    } else {
        Stroke::new(LINE, BORDER_STRONG)
    };
    ui.painter()
        .rect(rect, 8, fill, stroke, egui::StrokeKind::Inside);
    paint_icon(
        ui.painter(),
        pos2(rect.left() + 17.0, rect.center().y),
        icon,
        fg,
    );
    ui.painter().galley(
        pos2(rect.left() + 28.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
    response
}

/// Край окна под точкой `p`: углы важнее сторон.
pub(crate) fn edge_at(screen: Rect, p: Pos2, edge: f32) -> Option<ResizeDirection> {
    if !screen.expand(1.0).contains(p) {
        return None;
    }
    let left = p.x - screen.left() < edge;
    let right = screen.right() - p.x < edge;
    let top = p.y - screen.top() < edge;
    let bottom = screen.bottom() - p.y < edge;
    use ResizeDirection as D;
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => D::NorthWest,
        (_, true, true, _) => D::NorthEast,
        (true, _, _, true) => D::SouthWest,
        (_, true, _, true) => D::SouthEast,
        (true, ..) => D::West,
        (_, true, ..) => D::East,
        (_, _, true, _) => D::North,
        (_, _, _, true) => D::South,
        _ => return None,
    })
}

fn resize_cursor(dir: ResizeDirection) -> CursorIcon {
    use ResizeDirection as D;
    match dir {
        D::East | D::West => CursorIcon::ResizeHorizontal,
        D::North | D::South => CursorIcon::ResizeVertical,
        D::NorthWest | D::SouthEast => CursorIcon::ResizeNwSe,
        D::NorthEast | D::SouthWest => CursorIcon::ResizeNeSw,
    }
}

/// Файлы проекта для вкладки «Ассеты»: папки `scenes` и `assets/*`.
struct Assets {
    folders: Vec<(String, Vec<String>)>,
    open: usize,
}

impl Assets {
    fn scan(project: Option<&Path>) -> Self {
        let mut folders = Vec::new();
        if let Some(root) = project {
            let mut dirs: Vec<PathBuf> = vec![root.join("scenes")];
            if let Ok(rd) = std::fs::read_dir(root.join("assets")) {
                let mut sub: Vec<PathBuf> = rd
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.is_dir())
                    .collect();
                sub.sort();
                dirs.extend(sub);
            }
            for dir in dirs {
                let Ok(rd) = std::fs::read_dir(&dir) else {
                    continue;
                };
                let mut files: Vec<String> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if e.path().is_dir() {
                            format!("{name}/")
                        } else {
                            name
                        }
                    })
                    .collect();
                files.sort();
                let name = dir
                    .strip_prefix(root)
                    .unwrap_or(&dir)
                    .to_string_lossy()
                    .replace('\\', "/");
                folders.push((name, files));
            }
        }
        Self { folders, open: 0 }
    }

    fn draw(&mut self, ui: &mut Ui) {
        if self.folders.is_empty() {
            ui.label(RichText::new("Папка проекта не найдена").color(MUTED));
            return;
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(150.0);
                for (i, (name, _)) in self.folders.iter().enumerate() {
                    if ui.selectable_label(i == self.open, name.as_str()).clicked() {
                        self.open = i;
                    }
                }
            });
            ui.separator();
            ui.vertical(|ui| {
                let files = &self.folders[self.open.min(self.folders.len() - 1)].1;
                if files.is_empty() {
                    ui.label(RichText::new("Пусто").color(MUTED));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                    for f in files {
                        asset_tile(ui, f);
                    }
                });
            });
        });
    }
}

fn asset_tile(ui: &mut Ui, name: &str) {
    let (rect, response) = ui.allocate_exact_size(vec2(96.0, 92.0), Sense::click());
    let thumb = Rect::from_min_size(rect.min, vec2(rect.width(), 64.0));
    let fill = if response.hovered() { HOVER } else { CARD };
    ui.painter().rect(
        thumb,
        8,
        fill,
        Stroke::new(1.0, BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
    let ext = if name.ends_with('/') {
        "папка".to_owned()
    } else {
        name.rsplit_once('.')
            .map_or_else(String::new, |(_, e)| format!(".{e}"))
    };
    ui.painter().text(
        thumb.center(),
        Align2::CENTER_CENTER,
        ext,
        FontId::monospace(FONT_SMALL),
        MUTED,
    );
    let short: String = if name.chars().count() > 14 {
        name.chars().take(13).chain(['…']).collect()
    } else {
        name.to_owned()
    };
    ui.painter().text(
        pos2(rect.left() + 2.0, thumb.bottom() + 8.0),
        Align2::LEFT_TOP,
        short,
        FontId::proportional(FONT_SMALL),
        TEXT_DIM,
    );
    response.on_hover_text(name);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info<'a>(scene: Option<&'a SceneFile>, play: PlayState) -> Info<'a> {
        Info {
            play,
            maximized: false,
            frameless: true,
            fps: 144.0,
            frame_ms: 6.9,
            gpu_ms: Some(1.2),
            adapter: "Тестовая видеокарта",
            scene,
            scene_name: "first-room.ron",
            error: None,
        }
    }

    fn frame(ctx: &egui::Context, ui_state: &mut EditorUi, info: &Info, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 900.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| ui_state.draw(ui, info));
        out.textures_delta.clear();
    }

    fn new_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::panel::fonts());
        apply(&ctx);
        ctx
    }

    /// Все пространства раскладываются без окна, сцена остаётся посередине.
    #[test]
    fn every_workspace_lays_out() {
        let file = SceneFile::parse(reyvec_scene::EXAMPLE).unwrap();
        let ctx = new_ctx();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut ui = EditorUi::new(Some(&root));
        assert!(!ui.assets.folders.is_empty(), "папки проекта нашлись");
        for ws in 0..WORKSPACES.len() {
            ui.workspace = ws;
            for play in [PlayState::Edit, PlayState::Playing, PlayState::Paused] {
                frame(&ctx, &mut ui, &info(Some(&file), play), Vec::new());
                frame(&ctx, &mut ui, &info(None, play), Vec::new());
            }
            let view = ui.viewport().expect("есть место для сцены");
            assert!(view.width() > 600.0 && view.height() > 300.0, "{view:?}");
            assert!(view.top() >= TITLE_HEIGHT);
        }
        assert!(ui.take_actions().is_empty(), "без нажатий нет действий");
    }

    #[test]
    fn selection_shows_node_and_survives_reload() {
        let file = SceneFile::parse(reyvec_scene::EXAMPLE).unwrap();
        let ctx = new_ctx();
        let mut ui = EditorUi::new(None);
        let hero = file.nodes.iter().position(|n| n.player.is_some()).unwrap();
        ui.selected = Some(vec![hero]);
        frame(
            &ctx,
            &mut ui,
            &info(Some(&file), PlayState::Edit),
            Vec::new(),
        );
        ui.scene_changed(Some(&file));
        assert_eq!(ui.selected, Some(vec![hero]));
        let empty = SceneFile::parse("(format: \"reyvec-scene\", version: 1)").unwrap();
        ui.scene_changed(Some(&empty));
        assert_eq!(ui.selected, None, "пропавший предмет больше не выбран");
    }

    #[test]
    fn f5_plays_and_stops() {
        let ctx = new_ctx();
        let mut ui = EditorUi::new(None);
        let f5 = || egui::Event::Key {
            key: egui::Key::F5,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut ui, &info(None, PlayState::Edit), vec![f5()]);
        assert_eq!(ui.take_actions(), vec![Action::Play]);
        frame(&ctx, &mut ui, &info(None, PlayState::Playing), vec![f5()]);
        assert_eq!(ui.take_actions(), vec![Action::Stop]);
    }

    #[test]
    fn window_edges() {
        let s = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0));
        use ResizeDirection as D;
        assert_eq!(edge_at(s, pos2(500.0, 300.0), EDGE), None);
        assert_eq!(edge_at(s, pos2(1.0, 300.0), EDGE), Some(D::West));
        assert_eq!(edge_at(s, pos2(999.0, 300.0), EDGE), Some(D::East));
        assert_eq!(edge_at(s, pos2(500.0, 1.0), EDGE), Some(D::North));
        assert_eq!(edge_at(s, pos2(500.0, 599.0), EDGE), Some(D::South));
        assert_eq!(edge_at(s, pos2(1.0, 1.0), EDGE), Some(D::NorthWest));
        assert_eq!(edge_at(s, pos2(999.0, 599.0), EDGE), Some(D::SouthEast));
        assert_eq!(edge_at(s, pos2(-50.0, 300.0), EDGE), None);
    }
    /// Картинка интерфейса без видеокарты: треугольники egui раскрашиваются на процессоре
    /// (примерно, без точной гаммы) и сохраняются в `target/editor-preview/*.png`. Нужна,
    /// чтобы посмотреть раскладку там, где нет видеокарты. Запуск:
    /// `cargo test -p reyvec-app --lib editor_preview -- --ignored`.
    #[test]
    #[ignore = "пишет картинки для просмотра глазами"]
    fn editor_preview() {
        use std::collections::HashMap;
        let file = SceneFile::parse(reyvec_scene::EXAMPLE).unwrap();
        let ctx = new_ctx();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let out_dir = root.join("target/editor-preview");
        std::fs::create_dir_all(&out_dir).unwrap();
        let mut ui = EditorUi::new(Some(&root));
        ui.push_log([
            (TEXT_DIM, "Редактор открыт: 1600×900".to_owned()),
            (ACCENT, "Файл сцены сохранён, читаю заново".to_owned()),
        ]);
        let hero = file.nodes.iter().position(|n| n.player.is_some()).unwrap();
        ui.selected = Some(vec![hero]);
        ui.open.insert(vec![hero]);
        let (w, h) = (1600usize, 900usize);
        let mut textures: HashMap<egui::TextureId, (usize, usize, Vec<Color32>)> = HashMap::new();
        // Мир, Мир во время игры, Ассеты, Сюжет, Логика и Мир с раскрытым меню «ReyVec → Вид».
        for (ws, play, menu) in [
            (START_WORKSPACE, PlayState::Edit, false),
            (START_WORKSPACE, PlayState::Playing, false),
            (1, PlayState::Edit, false),
            (0, PlayState::Edit, false),
            (3, PlayState::Edit, false),
            (START_WORKSPACE, PlayState::Edit, true),
        ] {
            ui.workspace = ws;
            let info = info(Some(&file), play);
            let mut out = None;
            for frame in 0..12u32 {
                let mut events = Vec::new();
                if menu {
                    let at = |x, y| egui::Event::PointerMoved(pos2(x, y));
                    let click = |pressed| egui::Event::PointerButton {
                        pos: pos2(64.0, TITLE_HEIGHT / 2.0),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    };
                    match frame {
                        0 => events.extend([at(64.0, TITLE_HEIGHT / 2.0), click(true)]),
                        1 => events.push(click(false)),
                        2.. => events.push(at(80.0, TITLE_HEIGHT + 44.0)),
                    }
                }
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w as f32, h as f32))),
                    events,
                    time: Some(f64::from(frame) * 0.1),
                    ..Default::default()
                };
                let mut o = ctx.run_ui(input, |u| ui.draw(u, &info));
                for (id, deltas) in &o.textures_delta.set {
                    for delta in deltas {
                        let egui::ImageData::Color(img) = &delta.image;
                        let [iw, ih] = img.size;
                        match delta.pos {
                            None => {
                                textures.insert(*id, (iw, ih, img.pixels.clone()));
                            }
                            Some([x0, y0]) => {
                                let t = textures.get_mut(id).unwrap();
                                for y in 0..ih {
                                    for x in 0..iw {
                                        t.2[(y0 + y) * t.0 + x0 + x] = img.pixels[y * iw + x];
                                    }
                                }
                            }
                        }
                    }
                }
                o.textures_delta.clear();
                out = Some(o);
            }
            let out = out.unwrap();
            let prims = ctx.tessellate(out.shapes, 1.0);
            let mut px = vec![[0.0f32; 3]; w * h];
            // Сцена движка под панелями: тёмно-синяя заливка.
            for p in px.iter_mut() {
                *p = [0.05, 0.07, 0.12];
            }
            for cp in prims {
                let egui::epaint::Primitive::Mesh(mesh) = cp.primitive else {
                    continue;
                };
                let Some((tw, th, tex)) = textures.get(&mesh.texture_id) else {
                    continue;
                };
                let clip = cp
                    .clip_rect
                    .intersect(Rect::from_min_size(Pos2::ZERO, vec2(w as f32, h as f32)));
                for tri in mesh.indices.chunks_exact(3) {
                    let v = [
                        &mesh.vertices[tri[0] as usize],
                        &mesh.vertices[tri[1] as usize],
                        &mesh.vertices[tri[2] as usize],
                    ];
                    let (a, b, c) = (v[0].pos, v[1].pos, v[2].pos);
                    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                    if area.abs() < 1e-6 {
                        continue;
                    }
                    let minx = a.x.min(b.x).min(c.x).max(clip.left()).floor() as i32;
                    let maxx = a.x.max(b.x).max(c.x).min(clip.right()).ceil() as i32;
                    let miny = a.y.min(b.y).min(c.y).max(clip.top()).floor() as i32;
                    let maxy = a.y.max(b.y).max(c.y).min(clip.bottom()).ceil() as i32;
                    for y in miny.max(0)..maxy.min(h as i32) {
                        for x in minx.max(0)..maxx.min(w as i32) {
                            let p = pos2(x as f32 + 0.5, y as f32 + 0.5);
                            let w0 = ((b.x - p.x) * (c.y - p.y) - (b.y - p.y) * (c.x - p.x)) / area;
                            let w1 = ((c.x - p.x) * (a.y - p.y) - (c.y - p.y) * (a.x - p.x)) / area;
                            let w2 = 1.0 - w0 - w1;
                            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                                continue;
                            }
                            let uv = v[0].uv.to_vec2() * w0
                                + v[1].uv.to_vec2() * w1
                                + v[2].uv.to_vec2() * w2;
                            let tx = ((uv.x * *tw as f32) as usize).min(tw - 1);
                            let ty = ((uv.y * *th as f32) as usize).min(th - 1);
                            let t = tex[ty * tw + tx];
                            let mut src = [0.0f32; 4];
                            for (k, s) in src.iter_mut().enumerate() {
                                let col = f32::from(v[0].color[k]) * w0
                                    + f32::from(v[1].color[k]) * w1
                                    + f32::from(v[2].color[k]) * w2;
                                *s = col / 255.0 * f32::from(t[k]) / 255.0;
                            }
                            let d = &mut px[y as usize * w + x as usize];
                            for k in 0..3 {
                                d[k] = src[k] + d[k] * (1.0 - src[3]);
                            }
                        }
                    }
                }
            }
            let rgba: Vec<u8> = px
                .iter()
                .flat_map(|p| [p[0], p[1], p[2], 1.0].map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8))
                .collect();
            let image = crate::snapshot::FrameImage {
                width: w as u32,
                height: h as u32,
                rgba,
            };
            let menu = if menu { "-меню" } else { "" };
            let name = format!("{}-{:?}{menu}.png", WORKSPACES[ws].name, play);
            crate::snapshot::save_png(&out_dir.join(name), &image).unwrap();
        }
    }
}
