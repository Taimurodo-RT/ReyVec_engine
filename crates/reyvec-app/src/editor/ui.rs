//! Интерфейс редактора на egui: верхняя полоса со своими кнопками окна, вкладки рабочих
//! пространств, панели вокруг живой сцены и строка состояния.
//!
//! Здесь только раскладка и рисование. Всё, что надо сделать с окном, движком или сценой,
//! интерфейс не делает сам, а складывает в список [`Action`]: его выполняет цикл окна.
//! Поэтому интерфейс раскладывается и проверяется тестами без окна и видеокарты.

use std::collections::{HashSet, VecDeque};
use std::path::Path;

use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Id, Layout, Margin, Pos2,
    Rect, Response, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2,
};
use reyvec_scene::{Background, NodeDesc, SceneFile};
use winit::window::ResizeDirection;

use super::assets::Assets;
use super::doc::{Change, Edit};
use super::geom;
use super::layout::{START_WORKSPACE, Side, Tab, WORKSPACES, Workspace};
use super::outline::{self, Kind};
use super::theme::*;
use super::viewport::{self, TOOLBAR_HEIGHT, Tool, View, Viewport};

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
#[derive(Clone, Debug, PartialEq)]
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
    /// Правка предмета. `commit: false` — её ещё тянут мышью (видна сразу, в историю
    /// попадёт одним шагом, когда отпустят).
    Edit {
        edit: Edit,
        commit: bool,
    },
    /// Перетаскивание бросили (Esc): вернуть как было.
    CancelEdit,
    /// Перетаскивание кончилось: то, что уже видно, — один шаг истории.
    EndEdit,
    /// Записать сцену в файл (Ctrl+S).
    Save,
    Undo,
    Redo,
    /// Поставить камеру (кнопка F — показать выбранный предмет).
    SetCamera {
        center: [f64; 2],
        zoom: f64,
    },
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
    /// Камера сцены в точках интерфейса (нет — окно ещё не открыто).
    pub view: Option<View>,
    /// Есть несохранённые правки.
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Сцену есть куда сохранить (это файл, а не встроенный пример).
    pub can_save: bool,
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
    /// Время последних кадров, мс: график в профайлере.
    frames: VecDeque<f32>,
    /// Ширина вкладок пространств в прошлом кадре: по ней они ставятся в середину полосы.
    tabs_width: f32,
    /// Инструменты и ручки окна сцены.
    vp: Viewport,
    /// Имя предмета, которое сейчас печатают в инспекторе.
    name_edit: Option<(Vec<usize>, String)>,
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
/// Сколько последних кадров на графике профайлера.
const FRAME_HISTORY: usize = 240;
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
            frames: VecDeque::new(),
            vp: Viewport::default(),
            name_edit: None,
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
        if self.frames.len() == FRAME_HISTORY {
            self.frames.pop_front();
        }
        self.frames.push_back(info.frame_ms as f32);
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
        use egui::{Key, Modifiers};
        let (f5, esc) = ctx.input(|i| (i.key_pressed(Key::F5), i.key_pressed(Key::Escape)));
        if f5 {
            self.actions.push(match info.play {
                PlayState::Edit => Action::Play,
                PlayState::Playing | PlayState::Paused => Action::Stop,
            });
        } else if esc && info.play != PlayState::Edit {
            self.actions.push(Action::Stop);
        }
        if info.play != PlayState::Edit {
            return;
        }
        let save = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S));
        if save {
            self.actions.push(Action::Save);
        }
        // Пока печатают в поле, буквы и Ctrl+Z принадлежат полю.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (redo, undo) = ctx.input_mut(|i| {
            let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y);
            (redo, i.consume_key(Modifiers::COMMAND, Key::Z))
        });
        if redo {
            self.actions.push(Action::Redo);
        } else if undo {
            self.actions.push(Action::Undo);
        }
        let (q, w, e, r, f) = ctx.input(|i| {
            let k = |key| i.modifiers.is_none() && i.key_pressed(key);
            (k(Key::Q), k(Key::W), k(Key::E), k(Key::R), k(Key::F))
        });
        for (on, tool) in [
            (q, Tool::Select),
            (w, Tool::Move),
            (e, Tool::Rotate),
            (r, Tool::Scale),
        ] {
            if on {
                self.vp.tool = tool;
            }
        }
        if f {
            self.focus_selected(info);
        }
        if esc && self.vp.drag.is_none() {
            self.selected = None;
        }
    }

    /// Камера на выбранный предмет (или на всю сцену, если ничего не выбрано).
    fn focus_selected(&mut self, info: &Info) {
        let (Some(file), Some(view), Some(rect)) = (info.scene, info.view, self.viewport) else {
            return;
        };
        let placed = geom::place_all(file);
        let bounds = match &self.selected {
            Some(path) => geom::find(&placed, path).map(|p| p.all),
            None => placed
                .iter()
                .filter(|p| p.path.len() == 1)
                .map(|p| p.all)
                .reduce(geom::Bounds::union),
        };
        if let Some(b) = bounds {
            let (center, zoom) = view.focus(&b, rect);
            self.actions.push(Action::SetCamera { center, zoom });
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
            ui.scope(|ui| self.menus(ui, info));
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
    fn menus(&mut self, ui: &mut Ui, info: &Info) {
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
                let save = egui::Button::new("Сохранить сцену").shortcut_text("Ctrl+S");
                if ui.add_enabled(info.can_save, save).clicked() {
                    actions.push(Action::Save);
                    ui.close();
                }
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
            ui.menu_button("Правка", |ui| {
                let undo = egui::Button::new("Отменить").shortcut_text("Ctrl+Z");
                if ui.add_enabled(info.can_undo, undo).clicked() {
                    actions.push(Action::Undo);
                    ui.close();
                }
                let redo = egui::Button::new("Повторить").shortcut_text("Ctrl+Y");
                if ui.add_enabled(info.can_redo, redo).clicked() {
                    actions.push(Action::Redo);
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
                ui.label("Щелчок по предмету — выбрать, двойной — выбрать вложенный");
                ui.label("Q, W, E, R — выбор, сдвиг, поворот, размер");
                ui.label("F — показать выбранный предмет");
                ui.label("Ctrl+S — сохранить, Ctrl+Z — отменить, Ctrl+Y — повторить");
                ui.label("Колесо над сценой — приблизить");
                ui.label(
                    "Перетаскивание пустого места, средней или правой кнопкой — сдвинуть камеру",
                );
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
            // Ассеты прокручивают дерево и плитки сами, профайлер раскладывается в колонки
            // по месту и прокрутки не имеет.
            match tab {
                Tab::Assets => return self.assets.draw(ui),
                Tab::Profiler => return profiler(ui, info, &self.frames),
                _ => {}
            }
            egui::ScrollArea::vertical()
                .id_salt(("area", self.workspace, side.index()))
                .auto_shrink(false)
                .stick_to_bottom(tab == Tab::Console)
                .show(ui, |ui| match tab {
                    Tab::Hierarchy => self.hierarchy(ui, info),
                    Tab::Inspector => self.inspector(ui, info),
                    Tab::Console => console(ui, &self.log),
                    Tab::Assets | Tab::Profiler => {}
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
        let editing = info.play == PlayState::Edit;
        let node = self
            .selected
            .as_ref()
            .and_then(|p| outline::node_at(file, p).map(|n| (p, n)));
        match node {
            Some((path, node)) => node_inspector(
                ui,
                path,
                node,
                editing,
                &mut self.name_edit,
                &mut self.actions,
            ),
            None => scene_inspector(ui, file, info.scene_name),
        }
        if !editing {
            ui.add_space(GAP_L);
            ui.label(
                RichText::new("Во время игры свойства не меняются: «Стоп» вернёт к правке.")
                    .small()
                    .color(MUTED),
            );
        }
    }

    fn scene_view(&mut self, ui: &mut Ui, info: &Info) {
        let full = ui.max_rect();
        let editing = info.play == PlayState::Edit;
        let bar = Rect::from_min_size(full.min, vec2(full.width(), TOOLBAR_HEIGHT));
        let rect = Rect::from_min_max(pos2(full.left(), bar.bottom()), full.max);
        self.viewport = Some(rect);

        // Полоса инструментов над сценой, как в Unreal 5.
        ui.painter().rect_filled(bar, 0, PANEL);
        ui.painter()
            .hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(LINE, BORDER));
        let mut focus = false;
        let vp = &mut self.vp;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(bar.shrink2(vec2(GAP_L, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                vp.toolbar(ui, editing);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = GAP_M;
                    if let Some(v) = info.view {
                        ui.label(
                            RichText::new(format!("{:.0}%", v.zoom * 100.0))
                                .monospace()
                                .size(FONT_SMALL)
                                .color(MUTED),
                        )
                        .on_hover_text("Приближение камеры: колесо мыши над сценой");
                    }
                    let show = egui::Button::new(
                        RichText::new("Показать").size(FONT_CHIP).color(TEXT_DIM),
                    )
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE);
                    if ui
                        .add_enabled(editing && info.scene.is_some(), show)
                        .on_hover_text("Камера на выбранный предмет или на всю сцену (F)")
                        .clicked()
                    {
                        focus = true;
                    }
                });
            },
        );
        if focus {
            self.focus_selected(info);
        }

        let response = ui.interact(
            rect,
            ui.scope_id().with("scene-view"),
            Sense::click_and_drag(),
        );
        self.viewport_hovered = response.hovered() || response.dragged();
        let painter = ui.painter_at(rect);
        let mut busy = false;
        if editing && let (Some(file), Some(view)) = (info.scene, info.view) {
            let placed = geom::place_all(file);
            self.vp.grid(&painter, &view, rect);
            let pointer = response.hover_pos();
            // Предмет под мышью подсвечивается тонкой рамкой.
            if self.vp.drag.is_none()
                && let Some(m) = pointer
                && let Some(path) = geom::pick(&placed, view.to_scene(m), false)
                && Some(&path) != self.selected.as_ref()
                && let Some(p) = geom::find(&placed, &path)
            {
                let r = Rect::from_two_pos(view.to_ui(p.all.min), view.to_ui(p.all.max));
                painter.rect_stroke(
                    r,
                    0,
                    Stroke::new(LINE, SELECT.gamma_multiply(0.5)),
                    egui::StrokeKind::Middle,
                );
            }
            let sel = self
                .selected
                .as_ref()
                .and_then(|path| Some((geom::find(&placed, path)?, outline::node_at(file, path)?)));
            if let Some((p, node)) = sel {
                self.vp.selection(&painter, &view, p);
                viewport::label(&painter, &view, p, file);
                busy = ui
                    .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        ui.set_clip_rect(rect);
                        self.vp.gizmo(ui, &view, p, node, &mut self.actions)
                    })
                    .inner;
            } else if self.vp.drag.take().is_some() {
                // Предмет пропал, пока его тянули.
                self.actions.push(Action::CancelEdit);
            }
            if !busy
                && let Some(m) = response.interact_pointer_pos()
                && (response.clicked() || response.double_clicked())
            {
                let deep = response.double_clicked();
                self.selected = geom::pick(&placed, view.to_scene(m), deep);
                if let Some(path) = &self.selected {
                    // Родители выбранного раскрываются в иерархии.
                    for k in 1..path.len() {
                        self.open.insert(path[..k].to_vec());
                    }
                }
            }
        }
        if editing && !busy && self.vp.drag.is_none() {
            let pan = response.dragged_by(egui::PointerButton::Primary)
                || response.dragged_by(egui::PointerButton::Middle)
                || response.dragged_by(egui::PointerButton::Secondary);
            let d = response.drag_delta();
            if pan && d != Vec2::ZERO {
                self.actions.push(Action::Pan(d));
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
        }
        if info.play == PlayState::Playing {
            // Рамка цвета акцента: сразу видно, что идёт игра, а не правка.
            painter.rect_stroke(
                rect.shrink(1.0),
                0,
                Stroke::new(LINE_FOCUS, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        match info.play {
            PlayState::Edit => {}
            PlayState::Playing => chip(
                &painter,
                rect.left_top() + vec2(12.0, 12.0),
                "Игра идёт · Esc или «Стоп» — вернуться к правке",
                ACCENT,
            ),
            PlayState::Paused => chip(
                &painter,
                rect.left_top() + vec2(12.0, 12.0),
                "Пауза",
                ACCENT,
            ),
        }
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

fn node_inspector(
    ui: &mut Ui,
    path: &[usize],
    node: &NodeDesc,
    editing: bool,
    name_edit: &mut Option<(Vec<usize>, String)>,
    actions: &mut Vec<Action>,
) {
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
    let edit = |change: Change, commit: bool| Action::Edit {
        edit: Edit {
            path: path.to_vec(),
            change,
        },
        commit,
    };
    card(ui, "Предмет", |ui| {
        ui.horizontal(|ui| {
            prop(ui, "Имя");
            let buf = match name_edit {
                Some((p, text)) if p.as_slice() == path => text,
                _ => &mut name_edit.insert((path.to_vec(), node.name.clone())).1,
            };
            let r = ui.add_enabled(
                editing,
                egui::TextEdit::singleline(buf)
                    .hint_text("без имени")
                    .margin(PAD_FIELD)
                    .desired_width(f32::INFINITY),
            );
            if r.lost_focus() {
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if !esc && *buf != node.name {
                    actions.push(edit(Change::Name(buf.clone()), true));
                } else {
                    buf.clone_from(&node.name);
                }
            } else if !r.has_focus() && *buf != node.name {
                // Имя поменяли не здесь (отмена, файл): показать новое.
                buf.clone_from(&node.name);
            }
        });
    });
    card(ui, "Положение", |ui| {
        egui::Grid::new("node-place")
            .num_columns(3)
            .spacing(vec2(6.0, 6.0))
            .show(ui, |ui| {
                let at = [node.at.0, node.at.1];
                prop(ui, "Место");
                let x = num_field(ui, at[0], 0.005, Some(BAD), "", editing);
                let y = num_field(ui, at[1], 0.005, Some(GOOD), "", editing);
                for (k, f) in [x, y].into_iter().enumerate() {
                    if let Some(f) = f {
                        actions.push(f.action(|v| {
                            let mut a = at;
                            a[k] = v;
                            edit(Change::At(a), true)
                        }));
                    }
                }
                ui.end_row();
                let rows: [Row; 4] = [
                    ("Поворот", node.angle, 0.5, "°", Change::Angle),
                    ("Размер", node.scale, 0.005, "", Change::Scale),
                    ("Глубина", node.depth, 0.01, "", Change::Depth),
                    (
                        "Прозрачность",
                        f64::from(node.opacity),
                        0.005,
                        "",
                        Change::Opacity,
                    ),
                ];
                for (name, v, speed, suffix, make) in rows {
                    prop(ui, name);
                    if let Some(f) = num_field(ui, v, speed, None, suffix, editing) {
                        actions.push(f.action(|v| edit(make(v), true)));
                    }
                    ui.label("");
                    ui.end_row();
                }
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

/// Строка инспектора: подпись, значение, шаг мыши, единица, какая это правка.
type Row = (&'static str, f64, f64, &'static str, fn(f64) -> Change);

/// Что сделали с числовым полем в этом кадре.
enum FieldEdit {
    /// Новое значение; `commit: false` — его ещё тянут мышью.
    Value(f64, bool),
    /// Перетаскивание кончилось на прошлом значении.
    End,
}

impl FieldEdit {
    /// Действие для цикла окна; `make` строит правку с готовым значением.
    fn action(self, make: impl FnOnce(f64) -> Action) -> Action {
        match self {
            FieldEdit::Value(v, commit) => match make(v) {
                Action::Edit { edit, .. } => Action::Edit { edit, commit },
                other => other,
            },
            FieldEdit::End => Action::EndEdit,
        }
    }
}

/// Число, которое тянут мышью или вводят с клавиатуры; `mark` — цветная полоска слева.
fn num_field(
    ui: &mut Ui,
    v: f64,
    speed: f64,
    mark: Option<Color32>,
    suffix: &str,
    enabled: bool,
) -> Option<FieldEdit> {
    let mut x = v;
    let r = ui.add_enabled(
        enabled,
        egui::DragValue::new(&mut x)
            .speed(speed)
            .min_decimals(2)
            .max_decimals(3)
            .suffix(suffix)
            .update_while_editing(false),
    );
    if let Some(c) = mark {
        let rect = r.rect;
        let bar = Rect::from_min_max(rect.left_top(), pos2(rect.left() + 2.0, rect.bottom()));
        ui.painter().rect_filled(
            bar,
            CornerRadius {
                nw: 4,
                sw: 4,
                ne: 0,
                se: 0,
            },
            c,
        );
    }
    if r.changed() && x != v {
        Some(FieldEdit::Value(x, !r.dragged()))
    } else if r.drag_stopped() {
        Some(FieldEdit::End)
    } else {
        None
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

/// Профайлер: плитки в несколько колонок по ширине панели, без прокрутки.
fn profiler(ui: &mut Ui, info: &Info, frames: &VecDeque<f32>) {
    const BUDGET_MS: f64 = 4.0;
    const MIN_TILE: f32 = 150.0;
    let worst = frames.iter().copied().fold(0.0f32, f32::max);
    let avg = frames.iter().sum::<f32>() / frames.len().max(1) as f32;
    let items = info.scene.map_or(0, outline::count);
    type Tile<'a> = (&'a str, String, String, Option<f64>);
    let tiles: [Tile; 5] = [
        (
            "Кадров в секунду",
            format!("{:.0}", info.fps),
            format!("худший кадр {worst:.1} мс"),
            None,
        ),
        (
            "Кадр целиком",
            format!("{:.2} мс", info.frame_ms),
            format!("в среднем {avg:.1} мс"),
            None,
        ),
        (
            "Видеокарта на кадр",
            info.gpu_ms
                .map_or_else(|| "—".into(), |ms| format!("{ms:.2} мс")),
            format!("бюджет {BUDGET_MS:.0} мс (RTX 5080, 1080p)"),
            info.gpu_ms.map(|ms| ms / BUDGET_MS),
        ),
        ("Видеокарта", info.adapter.to_owned(), String::new(), None),
        (
            "Предметов в сцене",
            format!("{items}"),
            "со вложенными".into(),
            None,
        ),
    ];
    // Плитки и последней — график времени кадра; колонок столько, сколько влезает.
    let count = tiles.len() + 1;
    let width = ui.available_width();
    let cols = ((width + GAP_L) / (MIN_TILE + GAP_L))
        .floor()
        .clamp(1.0, count as f32) as usize;
    let tile_w = (width - GAP_L * (cols - 1) as f32) / cols as f32;
    let rows = count.div_ceil(cols);
    let tile_h =
        ((ui.available_height() - GAP_L * (rows - 1) as f32) / rows as f32).clamp(52.0, 72.0);
    let top = ui.cursor().min;
    for n in 0..count {
        let (r, c) = (n / cols, n % cols);
        let at = top + vec2(c as f32 * (tile_w + GAP_L), r as f32 * (tile_h + GAP_L));
        let rect = Rect::from_min_size(at, vec2(tile_w, tile_h));
        match tiles.get(n) {
            Some((title, big, small, part)) => metric_tile(ui, rect, title, big, small, *part),
            None => frame_graph(ui, rect, frames, BUDGET_MS as f32),
        }
    }
    ui.allocate_rect(
        Rect::from_min_size(top, vec2(width, rows as f32 * (tile_h + GAP_L))),
        Sense::hover(),
    );
}

/// Плитка профайлера: подпись, крупное число, пояснение и полоска бюджета.
fn metric_tile(ui: &Ui, rect: Rect, title: &str, big: &str, small: &str, part: Option<f64>) {
    let p = ui.painter();
    p.rect(
        rect,
        RADIUS_L,
        CARD,
        Stroke::new(LINE, BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(vec2(f32::from(PAD_CARD.left), GAP_L));
    let clip = p.with_clip_rect(rect.shrink(1.0));
    clip.text(
        inner.left_top(),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(FONT_SMALL),
        MUTED,
    );
    // Числа моноширинным шрифтом, название видеокарты обычным.
    let font = if big.starts_with(|c: char| c.is_ascii_digit() || c == '—') {
        FontId::monospace(FONT_HEADING)
    } else {
        FontId::proportional(FONT_TITLE)
    };
    clip.text(
        pos2(inner.left(), inner.top() + FONT_SMALL + GAP_S),
        Align2::LEFT_TOP,
        big,
        font,
        TEXT,
    );
    if let Some(part) = part {
        let bar = Rect::from_min_size(
            pos2(inner.left(), inner.bottom() - GAP_S),
            vec2(inner.width(), GAP_S),
        );
        clip.rect_filled(bar, 2, BORDER_STRONG);
        let fill = Rect::from_min_size(
            bar.min,
            vec2(bar.width() * part.clamp(0.0, 1.0) as f32, bar.height()),
        );
        clip.rect_filled(fill, 2, if part <= 1.0 { ACCENT } else { BAD });
    } else if !small.is_empty() {
        clip.text(
            pos2(inner.left(), inner.bottom()),
            Align2::LEFT_BOTTOM,
            small,
            FontId::proportional(FONT_SMALL),
            MUTED,
        );
    }
    // Полный текст под мышью: длинное название видеокарты может не влезть.
    ui.interact(rect, ui.scope_id().with(title), Sense::hover())
        .on_hover_text(format!("{title}: {big}\n{small}"));
}

/// График времени последних кадров с линией бюджета.
fn frame_graph(ui: &Ui, rect: Rect, frames: &VecDeque<f32>, budget: f32) {
    let p = ui.painter();
    p.rect(
        rect,
        RADIUS_L,
        CARD,
        Stroke::new(LINE, BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(vec2(f32::from(PAD_CARD.left), GAP_L));
    p.text(
        inner.left_top(),
        Align2::LEFT_TOP,
        "Время кадра",
        FontId::proportional(FONT_SMALL),
        MUTED,
    );
    let plot = Rect::from_min_max(
        pos2(inner.left(), inner.top() + FONT_SMALL + GAP_M),
        inner.max,
    );
    if frames.len() < 2 || plot.height() < GAP_L {
        return;
    }
    let top = frames.iter().copied().fold(budget * 2.0, f32::max);
    let y = |ms: f32| plot.bottom() - (ms / top).clamp(0.0, 1.0) * plot.height();
    let step = plot.width() / (FRAME_HISTORY - 1) as f32;
    let x0 = plot.right() - (frames.len() - 1) as f32 * step;
    let line: Vec<Pos2> = frames
        .iter()
        .enumerate()
        .map(|(i, &ms)| pos2(x0 + i as f32 * step, y(ms)))
        .collect();
    p.hline(plot.x_range(), y(budget), Stroke::new(LINE, BORDER_STRONG));
    p.text(
        inner.right_top(),
        Align2::RIGHT_TOP,
        format!("{budget:.0} мс"),
        FontId::proportional(FONT_SMALL),
        MUTED,
    );
    p.line(line, Stroke::new(1.5, ACCENT));
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
            if info.dirty {
                ui.label(small("не сохранено · Ctrl+S".into(), ACCENT));
            }
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
            view: Some(View {
                center: [0.0, 0.0],
                zoom: 1.0,
                size: [1600.0, 900.0],
            }),
            dirty: false,
            can_undo: false,
            can_redo: false,
            can_save: true,
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

    fn pointer(pos: Pos2, pressed: Option<bool>) -> Vec<egui::Event> {
        let mut e = vec![egui::Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            e.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        e
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Камера, при которой герой посередине окна сцены.
    fn hero_view(file: &SceneFile, hero: usize) -> View {
        let placed = geom::place_all(file);
        let p = geom::find(&placed, &[hero]).unwrap();
        let c = p.xform.apply([0.0, 0.0]);
        // Середина места под сцену (между панелями) правее и выше середины окна.
        View {
            center: [c[0] - 0.05, c[1] + 0.1],
            zoom: 1.0,
            size: [1600.0, 900.0],
        }
    }

    #[test]
    fn click_selects_and_handle_moves() {
        let file = SceneFile::parse(reyvec_scene::EXAMPLE).unwrap();
        let hero = file.nodes.iter().position(|n| n.player.is_some()).unwrap();
        let ctx = new_ctx();
        let mut ui = EditorUi::new(None);
        let mut info = info(Some(&file), PlayState::Edit);
        let view = hero_view(&file, hero);
        info.view = Some(view);
        let placed = geom::place_all(&file);
        let origin = view.to_ui(
            geom::find(&placed, &[hero])
                .unwrap()
                .xform
                .apply([0.0, 0.0]),
        );
        frame(&ctx, &mut ui, &info, Vec::new());
        let vp = ui.viewport().unwrap();
        assert!(vp.contains(origin), "герой в окне сцены: {origin:?} {vp:?}");

        // Щелчок по пустому месту снимает выбор, по герою — выбирает его.
        ui.selected = Some(vec![0]);
        let empty = vp.left_top() + vec2(4.0, 4.0);
        frame(&ctx, &mut ui, &info, pointer(empty, Some(true)));
        frame(&ctx, &mut ui, &info, pointer(empty, Some(false)));
        let picked = geom::pick(&placed, view.to_scene(empty), false);
        assert_eq!(ui.selected, picked);
        frame(&ctx, &mut ui, &info, pointer(origin, Some(true)));
        frame(&ctx, &mut ui, &info, pointer(origin, Some(false)));
        let at_hero = geom::pick(&placed, view.to_scene(origin), false);
        assert!(at_hero.is_some());
        assert_eq!(ui.selected, at_hero);
        ui.take_actions();

        // Стрелка x: тянем на 100 точек вправо.
        ui.selected = Some(vec![hero]);
        let grab = origin + vec2(50.0, 0.0);
        frame(&ctx, &mut ui, &info, pointer(grab, None));
        frame(&ctx, &mut ui, &info, pointer(grab, Some(true)));
        for k in 1..=5 {
            let m = grab + vec2(20.0 * k as f32, 3.0);
            frame(&ctx, &mut ui, &info, pointer(m, None));
        }
        frame(
            &ctx,
            &mut ui,
            &info,
            pointer(grab + vec2(100.0, 3.0), Some(false)),
        );
        let actions = ui.take_actions();
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::Edit { commit: false, .. })),
            "{actions:?}"
        );
        let Some(Action::Edit { edit, commit: true }) = actions.last() else {
            panic!("последней идёт готовая правка: {actions:?}")
        };
        assert_eq!(edit.path, vec![hero]);
        let Change::At(at) = edit.change else {
            panic!("{edit:?}")
        };
        let node = &file.nodes[hero];
        // 100 точек при 450 точках на единицу — 0.22, с привязкой к сетке 0.05.
        let moved = at[0] - node.at.0;
        assert!((moved - 0.2).abs() < 0.051, "{moved}");
        assert!((at[1] - node.at.1).abs() < 1e-9, "y не сдвинулся");
    }

    #[test]
    fn edit_hotkeys() {
        use egui::{Key, Modifiers};
        let ctx = new_ctx();
        let mut ui = EditorUi::new(None);
        let file = SceneFile::parse(reyvec_scene::EXAMPLE).unwrap();
        let info = info(Some(&file), PlayState::Edit);
        frame(&ctx, &mut ui, &info, vec![key(Key::S, Modifiers::COMMAND)]);
        frame(&ctx, &mut ui, &info, vec![key(Key::Z, Modifiers::COMMAND)]);
        frame(
            &ctx,
            &mut ui,
            &info,
            vec![key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT)],
        );
        frame(&ctx, &mut ui, &info, vec![key(Key::E, Modifiers::NONE)]);
        assert_eq!(ui.vp.tool, Tool::Rotate);
        frame(&ctx, &mut ui, &info, vec![key(Key::F, Modifiers::NONE)]);
        let actions = ui.take_actions();
        assert_eq!(actions[..3], [Action::Save, Action::Undo, Action::Redo]);
        assert!(
            matches!(actions[3], Action::SetCamera { .. }),
            "{actions:?}"
        );
        // Во время игры клавиши правки не работают.
        let playing = super::tests::info(Some(&file), PlayState::Playing);
        frame(
            &ctx,
            &mut ui,
            &playing,
            vec![key(Key::Z, Modifiers::COMMAND)],
        );
        assert!(ui.take_actions().is_empty());
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
        // Время только растёт: egui считает по нему анимации и частоту кадров.
        let mut clock = 0.0f64;
        // Мир, Мир во время игры, Ассеты, Сюжет, Логика и Мир с раскрытым меню «ReyVec → Вид».
        for (ws, play, menu) in [
            (START_WORKSPACE, PlayState::Edit, false),
            (START_WORKSPACE, PlayState::Playing, false),
            (1, PlayState::Edit, false),
            (0, PlayState::Edit, false),
            (3, PlayState::Edit, false),
            (5, PlayState::Edit, false),
            // Пауза в «Мире» снимается с профайлером внизу: широкая панель, много колонок.
            (START_WORKSPACE, PlayState::Paused, false),
            (START_WORKSPACE, PlayState::Edit, true),
        ] {
            if play == PlayState::Paused {
                ui.tabs[ws][Side::Bottom.index()] = 2;
            }
            ui.workspace = ws;
            if ws == 5 {
                // Сборка: профайлер справа; и он же внизу «Мира» в отдельной картинке ниже.
                ui.frames = (0..FRAME_HISTORY)
                    .map(|i| 3.0 + (i as f32 * 0.21).sin() + if i % 37 == 0 { 3.5 } else { 0.0 })
                    .collect();
            }
            let mut info = info(Some(&file), play);
            info.view = Some(hero_view(&file, hero));
            info.dirty = true;
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
                    time: Some({
                        clock += 0.1;
                        clock
                    }),
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
