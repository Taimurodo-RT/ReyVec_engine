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
use reyvec_scene::{NodeDesc, SceneFile};
use winit::window::ResizeDirection;

use super::arrange;
use super::assets::{AssetFile, Assets};
use super::contour;
use super::doc::{Change, Edit};
use super::geom;
use super::layout::{START_WORKSPACE, Side, Tab, WORKSPACES, Workspace};
use super::model_params;
use super::outline::{self, Kind};
use super::overlay::{self, Overlay, ViewMode};
use super::parts;
use super::ron_edit;
use super::shape_edit;
use super::shapes::ShapeAngle;
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
    /// Играть без героя: физика и логика идут, клавиши герою не идут, камера свободна.
    Simulate,
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
    /// Открыть другую сцену из той же папки.
    OpenScene(std::path::PathBuf),
    /// Создать сцену «новая сцена.ron» рядом с открытой и открыть её.
    NewScene,
    /// Вернуть правки из автосохранения.
    RestoreAutosave,
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
    /// Предмет — в буфер обмена (Ctrl+C); с `cut` — и убрать (Ctrl+X).
    Copy {
        path: Vec<usize>,
        cut: bool,
    },
    /// Вставить из буфера за предметом `after` (или в конец сцены). `text` — текст из
    /// буфера системы, если там предмет (скопирован в блокноте или другим редактором).
    Paste {
        after: Option<Vec<usize>>,
        text: Option<String>,
    },
    /// Копия рядом (Ctrl+D).
    Duplicate(Vec<usize>),
    /// Убрать вместе с вложенными (Delete).
    Delete(Vec<usize>),
    /// Перенести в список `parent` на место `index` (перетаскивание в иерархии).
    Move {
        from: Vec<usize>,
        parent: Vec<usize>,
        index: usize,
    },
    /// Скрыть в редакторе или показать снова (глаз в иерархии; в файл не пишется).
    ToggleHidden(Vec<usize>),
    /// Закрыть, сохранив правки.
    SaveAndClose,
    /// Закрыть без сохранения.
    ForceClose,
    /// Во время игры: камера перестаёт следить за героем, сцену можно двигать (F8).
    Eject,
    /// На паузе: один шаг игры.
    Step,
    /// Скорость игры (замедление).
    SetTimeScale(f64),
    /// Файл из Ассетов бросили в сцену в точку `at`: новый предмет за `after`.
    DropAsset {
        file: std::path::PathBuf,
        at: [f64; 2],
        after: Option<Vec<usize>>,
    },
    /// Записать предмет заготовкой в `assets/prefabs` (её потом тащат из Ассетов в сцену).
    SavePrefab(Vec<usize>),
    /// Новые места нескольких предметов (выравнивание) — один шаг истории.
    MoveMany(Vec<(Vec<usize>, [f64; 2])>),
    /// Вложить предметы одного уровня в новый предмет «группа» (Ctrl+G).
    Group(Vec<Vec<usize>>),
    /// Достать вложенные и убрать сам предмет (Ctrl+Shift+G).
    Ungroup(Vec<usize>),
    /// Добавить часть `item` (текст для файла) в конец списка `field` предмета.
    AddPart {
        path: Vec<usize>,
        field: &'static str,
        item: String,
    },
    /// Убрать часть номер `index` из списка `field`.
    RemovePart {
        path: Vec<usize>,
        field: &'static str,
        index: usize,
    },
    /// Открыть модель формы на подставке (М.2, двойной щелчок в Ассетах).
    OpenShape(std::path::PathBuf),
    /// Закрыть подставку, вернуться к сцене.
    CloseShape,
    /// Камера подставки: повернуть, сдвинуть, приблизить, готовый вид, деталь крупно.
    ShapeCamera(ShapeCam),
    /// Спрятать деталь модели на подставке (`true`) или показать (в файл не пишется).
    ShapeHide(Vec<usize>, bool),
    /// Ручки модели на подставке (М.6, «Посмотреть»): имя → значение, в файл не пишется.
    ShapePreview(std::collections::BTreeMap<String, f32>),
    /// Разрешение 3D-кадра (во «Формах» — меню «Качество»).
    SpaceScale(reyvec_gpu::SpaceScale),
    /// Режим «треугольники»: формы рисуются сетками (быстрее на слабых видеокартах).
    SpaceTriangles(bool),
    /// Сглаживание 3D по картинке (`true`) или лучами по краям.
    PostAa(bool),
    /// Новый текст файла модели на подставке (М.3). `commit: false` — число ещё тянут
    /// мышью (в историю правка попадёт одним шагом, когда отпустят).
    ShapeText {
        text: String,
        commit: bool,
    },
}

/// Модель на подставке «Форм» (М.2).
/// Что сделать с камерой подставки «Форм».
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ShapeCam {
    /// Повернуть на столько точек мыши (в полёте — оглядеться).
    Orbit(Vec2),
    /// Сдвинуть вбок и вверх-вниз вслед за мышью.
    Pan(Vec2),
    /// Приблизить на столько щелчков колеса (минус — отдалить).
    Zoom(f32),
    /// Полёт клавишами: вправо, вверх, вперёд (−1…1) за столько секунд.
    Fly([f32; 3], f32),
    /// Готовый вид на модель.
    View(ShapeAngle),
    /// Показать крупно деталь по пути в дереве частей.
    Focus(Vec<usize>),
    /// Свободный полёт или ходить вокруг модели.
    Free(bool),
    /// Вращать модель на подставке или остановить.
    Spin(bool),
}

pub(crate) struct ShapeInfo<'a> {
    /// Имя модели (`склад/стеллаж`).
    pub name: &'a str,
    /// Модель вращается сама.
    pub spin: bool,
    /// Свободный полёт камеры.
    pub free: bool,
    /// Спрятанные детали.
    pub hidden: &'a [Vec<usize>],
    /// Разрешение 3D-кадра и режим «треугольники».
    pub scale: reyvec_gpu::SpaceScale,
    pub triangles: bool,
    /// Сглаживание 3D по картинке вместо краёв лучами.
    pub post_aa: bool,
    /// 3D рисуется только в окне просмотра (иначе — на всё окно, под панелями тоже).
    pub in_view: bool,
    /// Самые долгие проходы видеокарты за последнюю секунду, мс (и весь кадр).
    pub passes: &'a [(&'static str, f64)],
    /// Камера подставки в этом кадре (для выбора щелчком и стрелок).
    pub camera: reyvec_sdf::Camera3D,
    /// Сдвиг модели на подставке.
    pub shift: [f32; 3],
    /// Ошибка в файле модели.
    pub error: Option<&'a str>,
    /// Текст файла модели (из него — дерево частей и числа инспектора).
    pub text: &'a str,
    /// Ручки модели на подставке не как в файле (М.6).
    pub preview: &'a std::collections::BTreeMap<String, f32>,
    /// Цена модели (М.7): `None` — ещё считается.
    pub cost: Option<Result<super::cost::ShapeCost, String>>,
}

/// Ручки как в файле (для подставок в тестах и без «Посмотреть»).
#[cfg(test)]
static NO_PREVIEW: std::collections::BTreeMap<String, f32> = std::collections::BTreeMap::new();

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
    /// Скрытые в редакторе предметы.
    pub hidden: &'a HashSet<Vec<usize>>,
    /// В буфере обмена есть предмет.
    pub can_paste: bool,
    /// Сцены в папке открытой: «Файл → Открыть».
    pub scenes: &'a [std::path::PathBuf],
    /// Есть автосохранение, которое можно вернуть.
    pub can_restore: bool,
    /// Камера отцеплена от героя во время игры (F8).
    pub ejected: bool,
    /// Скорость игры.
    pub time_scale: f64,
    /// Идёт симуляция (без управления героем).
    pub simulating: bool,
    /// Открыта модель во «Формах»: окно сцены показывает её, а не сцену.
    pub shape: Option<ShapeInfo<'a>>,
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
    /// Закреплённые предметы: мышью в сцене их не выбрать и не сдвинуть.
    locked: HashSet<Vec<usize>>,
    /// Поиск в иерархии.
    search: String,
    /// Переименование в строке иерархии (F2): путь и текст.
    renaming: Option<(Vec<usize>, String)>,
    /// Предмет, который тянут в иерархии.
    dragging: Option<Vec<usize>>,
    /// Куда бросят предмет, который тянут в иерархии.
    drop_target: Option<(Vec<usize>, Drop)>,
    /// Режим просмотра, метки и закладки камеры.
    ov: Overlay,
    /// Спросить, сохранить ли правки перед закрытием.
    asking_close: bool,
    /// Ещё выбранные, кроме главного (Ctrl+щелчок в иерархии, Shift+щелчок в сцене).
    more: Vec<Vec<usize>>,
    /// Инспектор закреплён на этом предмете: выбор других его не меняет.
    pinned: Option<Vec<usize>>,
    /// Линейка: откуда и докуда (точки сцены).
    ruler: Option<([f64; 2], [f64; 2])>,
    /// Какие сообщения показывает консоль.
    log_filter: LogFilter,
    /// Выбранная часть модели во «Формах» (путь в дереве частей).
    shape_selected: Vec<usize>,
    /// Как новая деталь из палитры складывается с выбранной.
    shape_combine: shape_edit::Combine,
    /// Свёрнутые части в дереве модели.
    shape_closed: HashSet<Vec<usize>>,
    /// Закреплённые части модели: их не правят и не выбирают на подставке.
    shape_locked: HashSet<Vec<usize>>,
    /// Тянут стрелку детали на подставке (М.4).
    gizmo_drag: Option<GizmoDrag>,
    /// Что делают ручки детали: двигать, повернуть, размер.
    gizmo_mode: GizmoMode,
    /// Окно контура выдавливания и выточки (М.5).
    contour: ContourUi,
    /// Карточка «Ручки модели» (М.6).
    params: ParamsUi,
    /// Деталь под правой кнопкой: для неё меню на подставке.
    shape_menu: Option<Vec<usize>>,
}

/// Фильтр консоли.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogFilter {
    All,
    Warnings,
    Errors,
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
            locked: HashSet::new(),
            search: String::new(),
            renaming: None,
            dragging: None,
            drop_target: None,
            ov: Overlay::default(),
            asking_close: false,
            more: Vec::new(),
            pinned: None,
            ruler: None,
            log_filter: LogFilter::All,
            shape_selected: Vec::new(),
            shape_combine: shape_edit::Combine::Add,
            shape_closed: HashSet::new(),
            shape_locked: HashSet::new(),
            gizmo_drag: None,
            gizmo_mode: GizmoMode::Move,
            contour: ContourUi::default(),
            params: ParamsUi::default(),
            shape_menu: None,
        }
    }

    /// Папки проекта поменялись (записали заготовку): перечитать Ассеты.
    pub(crate) fn rescan_assets(&mut self, project: Option<&Path>) {
        self.assets.rescan(project);
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

    /// Выбрать предмет (после вставки или переноса).
    pub(crate) fn select(&mut self, path: Option<Vec<usize>>) {
        if let Some(p) = &path {
            for k in 1..p.len() {
                self.open.insert(p[..k].to_vec());
            }
        }
        self.selected = path;
        self.more.clear();
    }

    /// Все выбранные: главный первым.
    fn selection(&self) -> Vec<Vec<usize>> {
        self.selected.iter().chain(&self.more).cloned().collect()
    }

    /// Добавить к выбору или убрать из него (Ctrl/Shift+щелчок).
    fn toggle_select(&mut self, p: Vec<usize>) {
        if self.selected.as_ref() == Some(&p) {
            self.selected = self.more.pop();
        } else if let Some(i) = self.more.iter().position(|x| *x == p) {
            self.more.remove(i);
        } else {
            if let Some(s) = self.selected.take() {
                self.more.push(s);
            }
            self.selected = Some(p);
        }
    }

    /// Предметы добавили, убрали или переставили: старые номера больше не те.
    pub(crate) fn structure_changed(&mut self) {
        self.more.clear();
        self.pinned = None;
        self.locked.clear();
        self.open.clear();
        self.renaming = None;
        self.name_edit = None;
    }

    /// Окно просят закрыть, а правки не сохранены.
    pub(crate) fn ask_close(&mut self) {
        self.asking_close = true;
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
        if self.asking_close {
            self.close_dialog(&ctx, info);
        }
    }

    /// «Сохранить правки перед закрытием?»
    fn close_dialog(&mut self, ctx: &egui::Context, info: &Info) {
        let modal = egui::Modal::new(Id::unique("editor-close")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                RichText::new("Сохранить правки?")
                    .size(FONT_HEADING)
                    .color(Color32::WHITE),
            );
            ui.add_space(GAP_M);
            ui.label(
                RichText::new(format!(
                    "В сцене {} есть несохранённые правки. Без сохранения они пропадут.",
                    info.scene_name
                ))
                .color(TEXT_DIM),
            );
            ui.add_space(GAP_XL);
            ui.horizontal(|ui| {
                let save = egui::Button::new(RichText::new("Сохранить").color(BG)).fill(ACCENT);
                if ui.add_enabled(info.can_save, save).clicked() {
                    self.actions.push(Action::SaveAndClose);
                    self.asking_close = false;
                }
                if ui.button("Не сохранять").clicked() {
                    self.actions.push(Action::ForceClose);
                    self.asking_close = false;
                }
                if ui.button("Отмена").clicked() {
                    self.asking_close = false;
                }
            });
        });
        if modal.should_close() {
            self.asking_close = false;
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
        } else if esc && info.shape.is_some() && !ctx.egui_wants_keyboard_input() {
            self.actions.push(Action::CloseShape);
            return;
        }
        if info.play != PlayState::Edit {
            if ctx.input(|i| i.key_pressed(Key::F8)) {
                self.actions.push(Action::Eject);
            }
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
        // Ctrl+C, X, V приходят от окна событиями «копировать», «вырезать», «вставить».
        let (copy, cut, paste, dup, del, f2) = ctx.input_mut(|i| {
            let ev =
                |i: &egui::InputState, want: fn(&egui::Event) -> bool| i.events.iter().any(want);
            let paste_text = i.events.iter().find_map(|e| match e {
                egui::Event::Paste(t) if t.trim_start().starts_with('(') => Some(t.clone()),
                _ => None,
            });
            let copy = ev(i, |e| matches!(e, egui::Event::Copy));
            let cut = ev(i, |e| matches!(e, egui::Event::Cut));
            // В буфере Windows картинка, а не текст: окно шлёт «вставить картинку» —
            // вставляем предмет из своего буфера, как и при чужом тексте.
            let paste = ev(i, |e| {
                matches!(e, egui::Event::Paste(_) | egui::Event::PasteImage(_))
            });
            let mut cmd = |key| i.consume_key(Modifiers::COMMAND, key);
            let (c, x, v, d) = (cmd(Key::C), cmd(Key::X), cmd(Key::V), cmd(Key::D));
            (
                copy || c,
                cut || x,
                (paste || v).then_some(paste_text),
                d,
                i.consume_key(Modifiers::NONE, Key::Delete),
                i.consume_key(Modifiers::NONE, Key::F2),
            )
        });
        // Во «Формах» клавиши — для деталей модели, а не для предметов сцены.
        if let Some(shape) = &info.shape {
            let f = ctx.input(|i| i.modifiers.is_none() && i.key_pressed(Key::F));
            // W, E, R — двигать, повернуть, размер (в полёте эти буквы ведут камеру).
            if !shape.free {
                for (key, m) in [
                    (Key::W, GizmoMode::Move),
                    (Key::E, GizmoMode::Turn),
                    (Key::R, GizmoMode::Size),
                ] {
                    if ctx.input(|i| i.modifiers.is_none() && i.key_pressed(key)) {
                        self.gizmo_mode = m;
                    }
                }
            }
            for (on, cmd) in [
                (f, PartCmd::Focus),
                (dup, PartCmd::Duplicate),
                (del, PartCmd::Remove),
            ] {
                if on {
                    self.shape_command(shape.text, cmd);
                }
            }
            return;
        }
        if let Some(text) = paste {
            self.actions.push(Action::Paste {
                after: self.selected.clone(),
                text,
            });
        }
        if let Some(path) = self.selected.clone() {
            if copy || cut {
                self.actions.push(Action::Copy {
                    path: path.clone(),
                    cut,
                });
            }
            if dup {
                self.actions.push(Action::Duplicate(path.clone()));
            }
            if del {
                self.actions.push(Action::Delete(path.clone()));
            }
            if f2
                && let Some(file) = info.scene
                && let Some(n) = outline::node_at(file, &path)
            {
                self.renaming = Some((path, n.name.clone()));
            }
        }
        // Ctrl+Shift+G раньше Ctrl+G: иначе её съест более короткое сочетание.
        let (ungroup, group) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::G),
                i.consume_key(Modifiers::COMMAND, Key::G),
            )
        });
        if ungroup && let Some(p) = self.selected.clone() {
            self.actions.push(Action::Ungroup(p));
        } else if group && !self.more.is_empty() {
            self.actions.push(Action::Group(self.selection()));
        }
        let (q, w, e, r, t, f) = ctx.input(|i| {
            let k = |key| i.modifiers.is_none() && i.key_pressed(key);
            (
                k(Key::Q),
                k(Key::W),
                k(Key::E),
                k(Key::R),
                k(Key::T),
                k(Key::F),
            )
        });
        for (on, tool) in [
            (q, Tool::Select),
            (w, Tool::Move),
            (e, Tool::Rotate),
            (r, Tool::Scale),
            (t, Tool::Measure),
        ] {
            if on {
                self.vp.tool = tool;
            }
        }
        if f {
            self.focus_selected(info);
        }
        // Закладки камеры: Ctrl+цифра — запомнить, цифра — вернуться.
        const DIGITS: [Key; 9] = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        for (k, key) in DIGITS.into_iter().enumerate() {
            let (save, go) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::COMMAND, key),
                    i.consume_key(Modifiers::NONE, key),
                )
            });
            if save && let Some(v) = info.view {
                self.ov.bookmarks[k] = Some((v.center, v.zoom));
            } else if go && let Some((center, zoom)) = self.ov.bookmarks[k] {
                self.actions.push(Action::SetCamera { center, zoom });
            }
        }
        if esc && self.vp.drag.is_none() {
            self.selected = None;
        }
    }

    /// Дерево частей модели на подставке (М.3): строка на деталь, щелчок выбирает,
    /// двойной щелчок показывает деталь крупно, стрелка сворачивает, правая кнопка —
    /// меню действий.
    fn shape_tree(&mut self, ui: &mut Ui, info: &Info) {
        let Some(shape) = &info.shape else { return };
        let root = match shape_edit::parse(shape.text) {
            Ok(r) => r,
            Err(e) => {
                ui.label(RichText::new(format!("Не разобрать модель: {e}")).color(BAD));
                return;
            }
        };
        if root.at(&self.shape_selected).is_none() {
            self.shape_selected.clear();
        }
        let materials = shape_edit::materials(shape.text);
        ui.label(
            RichText::new(format!("Модель {}", shape.name))
                .color(MUTED)
                .size(FONT_SMALL),
        );
        ui.add_space(GAP_M);
        let mut out = TreeOut::default();
        let mut tree = ShapeTree {
            text: shape.text,
            root: &root,
            selected: &self.shape_selected,
            closed: &self.shape_closed,
            hidden: shape.hidden,
            locked: &self.shape_locked,
            materials: &materials,
            out: &mut out,
        };
        tree.rows(ui, &root, &mut Vec::new());
        if let Some(p) = out.toggle
            && !self.shape_closed.remove(&p)
        {
            self.shape_closed.insert(p);
        }
        if let Some(p) = out.lock
            && !self.shape_locked.remove(&p)
        {
            self.shape_locked.insert(p);
        }
        if let Some((p, hide)) = out.hide {
            self.actions.push(Action::ShapeHide(p, hide));
        }
        if let Some(p) = out.pick {
            self.shape_selected = p;
        }
        if let Some(cmd) = out.cmd {
            self.shape_command(shape.text, cmd);
        }
    }

    /// Действие над выбранной деталью модели (меню дерева и клавиши).
    fn shape_command(&mut self, text: &str, cmd: PartCmd) {
        let Ok(root) = shape_edit::parse(text) else {
            return;
        };
        let path = self.shape_selected.clone();
        if cmd == PartCmd::Focus {
            self.actions
                .push(Action::ShapeCamera(ShapeCam::Focus(path)));
            return;
        }
        let Some(part) = root.at(&path) else { return };
        if self.part_locked(&path) {
            tracing::info!("Деталь закреплена: сначала снимите замок в иерархии");
            return;
        }
        let sibling = |d: isize| {
            let (&last, up) = path.split_last()?;
            let i = last.checked_add_signed(d)?;
            let mut p = up.to_vec();
            p.push(i);
            root.at(&p).filter(|s| s.removable).map(|s| (s, p))
        };
        let (result, select) = match cmd {
            PartCmd::Focus => return,
            PartCmd::Duplicate => {
                let mut p = path.clone();
                if let Some(l) = p.last_mut() {
                    *l += 1;
                }
                (shape_edit::duplicate(text, part), p)
            }
            PartCmd::Up | PartCmd::Down => {
                let up = cmd == PartCmd::Up;
                let Some((other, p)) = sibling(if up { -1 } else { 1 }) else {
                    return;
                };
                let r = if up {
                    shape_edit::swap(text, other, part)
                } else {
                    shape_edit::swap(text, part, other)
                };
                (r, p)
            }
            PartCmd::Wrap(how) => (Ok(shape_edit::wrap_in(text, part, how)), path.clone()),
            PartCmd::Unwrap => (shape_edit::unwrap(text, part), path.clone()),
            PartCmd::Split => (shape_edit::split_repeat(text, part), path.clone()),
            PartCmd::Remove => {
                let mut p = path.clone();
                p.pop();
                (shape_edit::remove(text, part), p)
            }
        };
        match result {
            Ok(text) => {
                self.shape_selected = select;
                self.actions.push(Action::ShapeText { text, commit: true });
            }
            Err(e) => tracing::warn!("Модель не правится: {e}"),
        }
    }

    /// Где на экране кадр подставки: окно просмотра или всё окно.
    fn shape_screen(&self, ui: &Ui, shape: &ShapeInfo) -> Rect {
        match self.viewport {
            Some(r) if shape.in_view => r,
            _ => ui.ctx().content_rect(),
        }
    }

    /// Ручки выбранной детали на экране (М.4, М.4в): рамка детали и, по режиму, стрелки
    /// X, Y, Z (двигать), кольца вокруг осей (повернуть) или ручки размера. Возвращает,
    /// где ручки и какая под мышью.
    fn shape_gizmo(&mut self, ui: &mut Ui, shape: &ShapeInfo) -> Option<Gizmo> {
        let path = self.shape_selected.clone();
        let screen = self.shape_screen(ui, shape);
        let to_screen = |w: glam::Vec3| -> Option<Pos2> {
            let aspect = screen.width() / screen.height().max(1.0);
            let n = shape.camera.project(w, aspect)?;
            (n.z > 0.0 && n.z <= 1.0).then(|| {
                pos2(
                    screen.left() + (n.x * 0.5 + 0.5) * screen.width(),
                    screen.top() + (0.5 - n.y * 0.5) * screen.height(),
                )
            })
        };
        let shift = glam::Vec3::from(shape.shift);
        let painter = ui.painter_at(self.viewport?);
        // Рамка детали.
        let (lo, hi) = super::shapes::part_bounds(shape.text, &path)?;
        let (lo, hi) = (lo + shift, hi + shift);
        let corner = |k: usize| {
            glam::Vec3::new(
                if k & 1 == 0 { lo.x } else { hi.x },
                if k & 2 == 0 { lo.y } else { hi.y },
                if k & 4 == 0 { lo.z } else { hi.z },
            )
        };
        for (a, b) in [
            (0, 1),
            (2, 3),
            (4, 5),
            (6, 7),
            (0, 2),
            (1, 3),
            (4, 6),
            (5, 7),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            if let (Some(p), Some(q)) = (to_screen(corner(a)), to_screen(corner(b))) {
                painter.line_segment([p, q], Stroke::new(1.0, ACCENT.gamma_multiply(0.8)));
            }
        }
        if self.part_locked(&path) {
            return None;
        }
        let root = shape_edit::parse(shape.text).ok()?;
        let part = root.at(&path)?;
        let at = shape_edit::read_vec3(shape.text, part.moved, "at", [0.0; 3]);
        let turn = shape_edit::read_vec3(shape.text, part.moved, "turn", [0.0; 3]);
        let scale = shape_edit::read_num(shape.text, part.moved, "scale", 1.0);
        let frame = super::shapes::parent_frame(shape.text, &path)?;
        let at3 = glam::Vec3::new(at[0] as f32, at[1] as f32, at[2] as f32);
        let origin = shift + frame.transform_point3(at3);
        let extent = (hi - lo).max_element();
        let len = (extent * 0.55).max(0.02);
        let o = to_screen(origin)?;
        let hover = ui.ctx().pointer_hover_pos();
        let active = self.gizmo_drag.as_ref().map(|g| g.handle);
        let mode = self.gizmo_mode;
        let mut handles: Vec<GizmoHandle> = Vec::new();
        let colors = [BAD, GOOD, SELECT];
        let lit_color = |h: Handle, near: bool, color: Color32| {
            let lit = active == Some(h) || (active.is_none() && near);
            (lit, if lit { Color32::WHITE } else { color })
        };
        // Оси родителя (двигать и повернуть) или самой детали (размер коробки).
        let own = frame.matrix3
            * glam::Mat3A::from(reyvec_sdf::shape3d::rotation(turn.map(|v| v as f32)));
        let mut hot = None;
        for (i, color) in colors.into_iter().enumerate() {
            let axes = if mode == GizmoMode::Size {
                own
            } else {
                frame.matrix3
            };
            let v = glam::Vec3::from(axes.col(i));
            let per_local = v.length();
            if per_local < 1e-6 {
                continue;
            }
            let unit = v / per_local;
            match mode {
                GizmoMode::Move | GizmoMode::Size => {
                    let tip = origin + unit * len;
                    let Some(t) = to_screen(tip) else { continue };
                    let screen_len = (t - o).length();
                    if screen_len < 4.0 {
                        continue;
                    }
                    let h = if mode == GizmoMode::Move {
                        Handle::Move(i)
                    } else {
                        Handle::Size(i)
                    };
                    let near = hover.is_some_and(|p| dist_to_segment(p, o, t) < 7.0);
                    if near && hot.is_none() {
                        hot = Some(h);
                    }
                    // Точек экрана на единицу числа: у места — на единицу `at`, у размера
                    // — на всю длину ручки (протянули на её длину — деталь вдвое больше).
                    let px_per_unit = if mode == GizmoMode::Move {
                        screen_len / len * per_local
                    } else {
                        screen_len
                    };
                    handles.push(GizmoHandle {
                        handle: h,
                        dir: (t - o) / screen_len,
                        px_per_unit,
                    });
                    let (lit, c) = lit_color(h, near, color);
                    painter.line_segment([o, t], Stroke::new(if lit { 3.0 } else { 2.0 }, c));
                    let dir = (t - o) / screen_len;
                    if mode == GizmoMode::Move {
                        let side = vec2(-dir.y, dir.x);
                        painter.add(egui::Shape::convex_polygon(
                            vec![t + dir * 9.0, t + side * 4.0, t - side * 4.0],
                            c,
                            Stroke::NONE,
                        ));
                    } else {
                        painter.rect_filled(Rect::from_center_size(t, vec2(8.0, 8.0)), 1, c);
                    }
                }
                GizmoMode::Turn => {
                    // Кольцо в плоскости, поперёк оси.
                    let (a, b) = unit.any_orthonormal_pair();
                    let ring: Vec<Pos2> = (0..=48)
                        .filter_map(|k| {
                            let t = k as f32 / 48.0 * std::f32::consts::TAU;
                            to_screen(origin + (a * t.cos() + b * t.sin()) * len)
                        })
                        .collect();
                    if ring.len() < 2 {
                        continue;
                    }
                    let h = Handle::Turn(i);
                    let near = hover.is_some_and(|p| {
                        ring.windows(2)
                            .any(|w| dist_to_segment(p, w[0], w[1]) < 6.0)
                    });
                    if near && hot.is_none() {
                        hot = Some(h);
                    }
                    // Ось к нам — поворот на плюс идёт против часовой стрелки.
                    let toward = unit.dot(origin - shape.camera.eye) < 0.0;
                    handles.push(GizmoHandle {
                        handle: h,
                        dir: vec2(if toward { 1.0 } else { -1.0 }, 0.0),
                        px_per_unit: 1.0,
                    });
                    let (lit, c) = lit_color(h, near, color);
                    painter.add(egui::Shape::line(
                        ring,
                        Stroke::new(if lit { 3.0 } else { 2.0 }, c),
                    ));
                }
            }
        }
        // Общий размер детали — квадрат у её середины (в режиме «размер»).
        if mode == GizmoMode::Size {
            let r = Rect::from_center_size(o + vec2(14.0, -14.0), vec2(10.0, 10.0));
            let near = hover.is_some_and(|p| r.expand(3.0).contains(p));
            let h = Handle::Scale;
            if near && hot.is_none() {
                hot = Some(h);
            }
            handles.push(GizmoHandle {
                handle: h,
                dir: vec2(1.0, -1.0).normalized(),
                px_per_unit: 1.0,
            });
            let (_, c) = lit_color(h, near, TEXT);
            painter.rect_stroke(r, 1, Stroke::new(2.0, c), egui::StrokeKind::Middle);
        }
        painter.circle_filled(o, 3.0, TEXT);
        Some(Gizmo {
            path,
            start: GizmoStart { at, turn, scale },
            origin: o,
            handles,
            hot,
        })
    }

    /// Мышь на подставке (М.4): левый щелчок выбирает деталь (пусто — вся модель), левая
    /// по стрелке тянет деталь вдоль оси, правый щелчок по детали открывает её меню.
    fn shape_pointer(
        &mut self,
        ui: &mut Ui,
        response: &Response,
        shape: &ShapeInfo,
        gizmo: Option<&Gizmo>,
    ) {
        let screen = self.shape_screen(ui, shape);
        let ray = |pos: Pos2| {
            let aspect = screen.width() / screen.height().max(1.0);
            let x = (pos.x - screen.left()) / screen.width() * 2.0 - 1.0;
            let y = 1.0 - (pos.y - screen.top()) / screen.height() * 2.0;
            let (o, d) = shape.camera.ray([x, y], aspect);
            (o - glam::Vec3::from(shape.shift), d)
        };
        let far = shape.camera.far;
        let pick = |pos: Option<Pos2>| {
            let (o, d) = ray(pos?);
            super::shapes::pick(shape.text, shape.hidden, o, d, far)
        };
        // Ручки.
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(g) = gizmo
            && let Some(h) = g.hot
            && let Some(a) = g.handles.iter().find(|a| a.handle == h)
        {
            self.gizmo_drag = Some(GizmoDrag {
                handle: h,
                path: g.path.clone(),
                text: shape.text.to_owned(),
                start: g.start,
                dir: a.dir,
                px_per_unit: a.px_per_unit,
                origin: g.origin,
                pointer: response.interact_pointer_pos().unwrap_or(g.origin),
                moved: Vec2::ZERO,
            });
        }
        if let Some(g) = self.gizmo_drag.as_mut() {
            if response.dragged_by(egui::PointerButton::Primary) {
                g.moved += response.drag_delta();
                // Поворот идёт по углу курсора вокруг детали: курсор не переносится.
                if !matches!(g.handle, Handle::Turn(_)) {
                    endless_drag(ui.ctx());
                }
                let snap = ui.input(|i| i.modifiers.command);
                let edit = shape_edit::parse(&g.text)
                    .ok()
                    .and_then(|r| r.at(&g.path).cloned())
                    .map(|part| g.edit(&part, snap));
                if let Some(Ok(text)) = edit {
                    self.actions.push(Action::ShapeText {
                        text,
                        commit: false,
                    });
                }
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            } else {
                self.gizmo_drag = None;
                self.actions.push(Action::EndEdit);
            }
            return;
        }
        if gizmo.is_some_and(|g| g.hot.is_some()) {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        if response.clicked_by(egui::PointerButton::Primary) {
            match pick(response.interact_pointer_pos()) {
                Some(p) if !self.part_locked(&p) => self.shape_selected = p,
                Some(_) => {}
                None => self.shape_selected.clear(),
            }
        }
        if response.secondary_clicked() {
            self.shape_menu =
                pick(response.interact_pointer_pos()).filter(|p| !self.part_locked(p));
            if let Some(p) = &self.shape_menu {
                self.shape_selected = p.clone();
            }
        }
        let Some(path) = self.shape_menu.clone() else {
            return;
        };
        let Ok(root) = shape_edit::parse(shape.text) else {
            return;
        };
        let Some(part) = root.at(&path) else { return };
        let materials = shape_edit::materials(shape.text);
        let mut out = TreeOut::default();
        let mut tree = ShapeTree {
            text: shape.text,
            root: &root,
            selected: &self.shape_selected,
            closed: &self.shape_closed,
            hidden: shape.hidden,
            locked: &self.shape_locked,
            materials: &materials,
            out: &mut out,
        };
        response.context_menu(|ui| tree.menu(ui, part, &path));
        if let Some(cmd) = out.cmd {
            self.shape_command(shape.text, cmd);
        }
    }

    /// Закреплена ли деталь (она сама или часть, в которой она лежит).
    fn part_locked(&self, path: &[usize]) -> bool {
        (0..=path.len()).any(|n| self.shape_locked.contains(&path[..n]))
    }

    /// Числа выбранной части модели (М.3) по сворачиваемым карточкам: размеры, место,
    /// цвет, новая деталь. У чисел кнопки «−» и «+», тянуть их можно без края экрана.
    fn shape_inspector(&mut self, ui: &mut Ui, info: &Info) {
        let Some(shape) = &info.shape else { return };
        let text = shape.text;
        let Ok(root) = shape_edit::parse(text) else {
            ui.label(RichText::new("Модель не разобрать: правьте файл в блокноте").color(BAD));
            return;
        };
        let path = self.shape_selected.clone();
        let Some(part) = root.at(&path) else { return };
        let parent = path.split_last().and_then(|(_, up)| root.at(up));
        let materials = shape_edit::materials(text);
        if self.part_locked(&path) {
            ui.label(
                RichText::new("Деталь закреплена (замок в иерархии): числа не правятся")
                    .color(MUTED)
                    .size(FONT_SMALL),
            );
            ui.disable();
        }
        let actions = &mut self.actions;
        let mut end = false;
        let mut send = |r: Result<String, ron_edit::EditError>, commit: bool| match r {
            Ok(text) => actions.push(Action::ShapeText { text, commit }),
            Err(e) => tracing::warn!("Модель не правится: {e}"),
        };
        ui.label(
            RichText::new(shape_edit::label(&part.kind))
                .size(FONT_TITLE)
                .color(TEXT),
        );
        ui.add_space(GAP_M);
        // Свои числа части.
        let own = shape_edit::params(&part.kind);
        if !own.is_empty() {
            let title = if part.children.is_empty() {
                "Размеры"
            } else {
                "Настройки"
            };
            card(ui, title, |ui| {
                for &(field, name, param) in own {
                    match param {
                        shape_edit::Param::Num(d) => {
                            let v = shape_edit::read_num(text, part.open, field, d);
                            ui.horizontal(|ui| {
                                prop(ui, name);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    match step_field(ui, v, 0.005, 0.01, 0.0, None) {
                                        Some(FieldEdit::Value(x, commit)) => send(
                                            shape_edit::set(text, part, field, &ron_edit::num(x)),
                                            commit,
                                        ),
                                        Some(FieldEdit::End) => end = true,
                                        None => {}
                                    }
                                });
                            });
                        }
                        shape_edit::Param::Count(d) => {
                            let v = shape_edit::read_num(text, part.open, field, f64::from(d));
                            ui.horizontal(|ui| {
                                prop(ui, name);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if let Some(FieldEdit::Value(x, _)) =
                                        step_field(ui, v.round(), 0.1, 1.0, 1.0, None)
                                    {
                                        let n = x.round().clamp(1.0, 64.0) as u32;
                                        send(
                                            shape_edit::set(text, part, field, &n.to_string()),
                                            true,
                                        );
                                    }
                                });
                            });
                        }
                        shape_edit::Param::Axis => {
                            let now = shape_edit::read_word(text, part.open, field)
                                .unwrap_or_else(|| "X".into());
                            ui.horizontal(|ui| {
                                prop(ui, name);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    for a in ["Z", "Y", "X"] {
                                        if ui.selectable_label(now == a, a).clicked() && now != a {
                                            send(shape_edit::set(text, part, field, a), true);
                                        }
                                    }
                                });
                            });
                        }
                        shape_edit::Param::Vec3(d) => {
                            let v = shape_edit::read_vec3(text, part.open, field, d);
                            prop(ui, name);
                            match vec3_fields(ui, v, 0.005, 0.01) {
                                Some((Some((x, commit)), _)) => send(
                                    shape_edit::set(text, part, field, &shape_edit::triple(x)),
                                    commit,
                                ),
                                Some((None, true)) => end = true,
                                _ => {}
                            }
                        }
                    }
                }
            });
        }
        // Контур выдавливания и выточки (М.5).
        if matches!(part.kind.as_str(), "Extrude" | "Revolve") {
            let revolve = part.kind == "Revolve";
            let profile = shape_edit::read_string(text, part.open, "profile").unwrap_or_default();
            let title = if revolve {
                "Контур (вращается вокруг оси Y)"
            } else {
                "Контур (выдавливается по Z)"
            };
            let mut edit = None;
            let st = &mut self.contour;
            card(ui, title, |ui| {
                edit = contour_card(ui, st, &profile, revolve)
            });
            match edit {
                Some(ContourEdit::Path(p, commit)) => send(
                    shape_edit::set(text, part, "profile", &shape_edit::quoted(&p)),
                    commit,
                ),
                Some(ContourEdit::End) => end = true,
                None => {}
            }
        }
        // Место и цвет детали (её сдвиг и цвет вокруг).
        if part.kind != "Move" {
            card(ui, "Место", |ui| {
                for (field, name, speed, step) in [
                    ("at", "Положение, м", 0.005, 0.01),
                    ("turn", "Поворот, °", 0.5, 5.0),
                ] {
                    prop(ui, name);
                    let v = shape_edit::read_vec3(text, part.moved, field, [0.0; 3]);
                    match vec3_fields(ui, v, speed, step) {
                        Some((Some((x, commit)), _)) => send(
                            shape_edit::set_moved(text, part, field, &shape_edit::triple(x)),
                            commit,
                        ),
                        Some((None, true)) => end = true,
                        _ => {}
                    }
                }
            });
            if !materials.is_empty() {
                let now = part
                    .painted
                    .map(|o| shape_edit::read_num(text, Some(o), "material", 0.0).round() as usize);
                let name = |m: Option<usize>| {
                    m.map_or("как у предмета", |i| {
                        materials.get(i).map_or("?", String::as_str)
                    })
                };
                card(ui, "Цвет", |ui| {
                    ui.horizontal(|ui| {
                        prop(ui, "Материал");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            egui::ComboBox::from_id_salt("shape-paint")
                                .selected_text(name(now))
                                .show_ui(ui, |ui| {
                                    for m in
                                        std::iter::once(None).chain((0..materials.len()).map(Some))
                                    {
                                        if ui.selectable_label(now == m, name(m)).clicked()
                                            && now != m
                                        {
                                            send(shape_edit::set_paint(text, part, m), true);
                                        }
                                    }
                                });
                        });
                    });
                });
            }
        }
        if end {
            self.actions.push(Action::EndEdit);
        }
        // Новая деталь.
        let mut new = None;
        let combine = &mut self.shape_combine;
        card(ui, "Новая деталь", |ui| {
            ui.horizontal(|ui| {
                use shape_edit::Combine;
                ui.spacing_mut().item_spacing.x = GAP_XS;
                for c in [Combine::Add, Combine::Cut, Combine::Intersect] {
                    if combine_button(ui, c, *combine == c).clicked() {
                        *combine = c;
                    }
                }
                ui.add_space(GAP_M);
                let name = match *combine {
                    Combine::Add => "добавить",
                    Combine::Cut => "вырезать",
                    Combine::Intersect => "пересечь",
                };
                ui.label(RichText::new(name).color(MUTED).size(FONT_SMALL));
            });
            ui.horizontal_wrapped(|ui| {
                for kind in shape_edit::PALETTE {
                    if ui
                        .button(format!("+ {}", shape_edit::label(kind)))
                        .on_hover_text(
                            "Новая деталь там же, где выбранная; дальше двигайте числами",
                        )
                        .clicked()
                    {
                        new = Some(kind);
                    }
                }
            });
        });
        // Новая деталь там же, где выбранная (в своём списке части — в её середине).
        let at = if shape_edit::goes_inside(part, self.shape_combine) {
            [0.0; 3]
        } else {
            shape_edit::read_vec3(text, part.moved, "at", [0.0; 3])
        };
        if let Some(kind) = new {
            let item = shape_edit::new_part(kind, 0.2, at);
            match shape_edit::combine(text, part, parent, self.shape_combine, &item) {
                Ok(t) => {
                    // Новая деталь — последняя в списке, куда легла: выбрать её.
                    if let Ok(r) = shape_edit::parse(&t) {
                        self.shape_selected = newest(&root, &r, &path);
                    }
                    self.actions.push(Action::ShapeText {
                        text: t,
                        commit: true,
                    });
                }
                Err(e) => tracing::warn!("Деталь не добавилась: {e}"),
            }
        }
        if part.removable
            && ui
                .button(RichText::new("Убрать деталь").color(BAD))
                .on_hover_text("Убрать выбранную деталь из модели (Delete, Ctrl+Z вернёт)")
                .clicked()
        {
            self.shape_command(text, PartCmd::Remove);
        }
        self.params_card(ui, shape);
        self.cost_card(ui, shape);
    }

    /// Карточка «Цена модели» (М.7): сколько модель стоит лучу и какая у неё сетка для
    /// режима «Треугольники»; кнопка показать модель так, как её видит слабая видеокарта.
    fn cost_card(&mut self, ui: &mut Ui, shape: &ShapeInfo) {
        use super::cost::Weight;
        let mut triangles = None;
        card(ui, "Цена модели", |ui| {
            let line = |ui: &mut Ui, name: &str, value: String, hint: &str| {
                ui.horizontal(|ui| {
                    prop(ui, name);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(value).color(TEXT))
                            .on_hover_text(hint);
                    });
                });
            };
            match &shape.cost {
                None => {
                    ui.label(RichText::new("Считаю…").color(MUTED));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(BAD).size(FONT_SMALL));
                }
                Some(Ok(c)) => {
                    let color = match c.weight {
                        Weight::Light => GOOD,
                        Weight::Medium => ACCENT,
                        Weight::Heavy => BAD,
                    };
                    ui.label(
                        RichText::new(format!("● Для лучей модель {}", c.weight.title()))
                            .color(color),
                    )
                    .on_hover_text(
                        "Зелёная — до 8 примитивов на шаг луча, жёлтая — до 24, красная — больше \
                         (или программа почти во весь предел)",
                    );
                    line(
                        ui,
                        "Примитивов",
                        c.prims.to_string(),
                        "Частей в программе для видеокарты: каждая копия повтора и зеркала отдельно",
                    );
                    line(
                        ui,
                        "На шаг луча",
                        format!("{:.1}, до {}", c.mean, c.worst),
                        "Сколько примитивов видеокарта считает за один шаг луча рядом с моделью: \
                         далёкие группы деталей пропускаются целиком",
                    );
                    line(
                        ui,
                        "Слов программы",
                        format!("{} из {}", c.words, reyvec_sdf::shape3d::MAX_PROGRAM_WORDS),
                        "Длина программы формы; больше предела модель не соберётся",
                    );
                    match c.mesh {
                        Some(m) => {
                            line(
                                ui,
                                "Сетка",
                                format!("{} треуг.", m.triangles),
                                "Режим «Треугольники» (для слабых видеокарт) рисует модель этой сеткой",
                            );
                            line(
                                ui,
                                "Клетка сетки",
                                format!("{:.1} см", m.cell * 100.0),
                                "Детали тоньше клетки в сетке могут пропасть, острые кромки чуть скругляются",
                            );
                            line(
                                ui,
                                "Отход сетки",
                                format!("{:.1} мм", m.error * 1000.0),
                                "Насколько сетка отходит от настоящей формы в худшем месте",
                            );
                            line(
                                ui,
                                "Строится",
                                format!("{:.0} мс", m.ms),
                                "Сколько процессор строит сетку при чтении сцены (один раз на модель)",
                            );
                        }
                        None => {
                            ui.label(
                                RichText::new("Сетки нет: модель пустая или бесконечная")
                                    .color(MUTED)
                                    .size(FONT_SMALL),
                            );
                        }
                    }
                }
            }
            ui.add_space(GAP_S);
            let (text, hint) = if shape.triangles {
                (
                    "Снова лучами",
                    "Вернуть обычный вид: модель рисуется лучами",
                )
            } else {
                (
                    "Как на слабой видеокарте",
                    "Режим «Треугольники»: модель рисуется своей сеткой, как на встроенной видеокарте",
                )
            };
            if ui.button(text).on_hover_text(hint).clicked() {
                triangles = Some(!shape.triangles);
            }
        });
        if let Some(t) = triangles {
            self.actions.push(Action::SpaceTriangles(t));
        }
    }

    /// Карточка «Ручки модели» (М.6): ручки, которые сцена задаёт каждой копии модели,
    /// «Посмотреть» на подставке и обучение ручки.
    fn params_card(&mut self, ui: &mut Ui, shape: &ShapeInfo) {
        use std::collections::BTreeMap;
        let text = shape.text;
        let params = model_params::read(text);
        let st = &mut self.params;
        if st.teach.as_ref().is_some_and(|t| t.model != shape.name) {
            st.teach = None;
        }
        let mut write: Option<Vec<reyvec_sdf::shape_param::ModelParam>> = None;
        let mut set_text: Option<String> = None;
        let mut preview: Option<BTreeMap<String, f32>> = None;
        let mut end = false;
        let mut new_text = None;
        card(ui, "Ручки модели", |ui| {
            if let Some(t) = &st.teach {
                ui.label(
                    RichText::new(format!("Учим ручку «{}» = {}", t.name, t.target)).color(ACCENT),
                );
                ui.label(
                    RichText::new(
                        "Двигайте и растягивайте детали, пока модель не станет такой, какой \
                         должна быть при этом значении. Детали не добавляйте и не убирайте.",
                    )
                    .color(MUTED)
                    .size(FONT_SMALL),
                );
                ui.horizontal(|ui| {
                    if ui
                        .button("Запомнить")
                        .on_hover_text("Изменённые числа связываются с ручкой, модель в файле возвращается к прежнему значению")
                        .clicked()
                    {
                        match model_params::learn(&t.before, text, &t.name, t.target) {
                            Ok(l) => {
                                st.note = Some((format!("Ручка «{}»: связано чисел {}", t.name, l.links), false));
                                new_text = Some(l.text);
                            }
                            Err(e) => st.note = Some((e, true)),
                        }
                    }
                    if ui.button("Отмена").on_hover_text("Вернуть модель, как была до обучения").clicked() {
                        set_text = Some(t.before.clone());
                        st.note = None;
                    }
                });
            } else {
                if params.is_empty() {
                    ui.label(
                        RichText::new(
                            "Ручка — число, которое сцена задаёт каждой копии модели по-своему: \
                             длина конвейера, высота стеллажа.",
                        )
                        .color(MUTED)
                        .size(FONT_SMALL),
                    );
                }
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    ui.horizontal(|ui| {
                        let mut buf = match &st.naming {
                            Some((j, b)) if *j == i => b.clone(),
                            _ => p.name.clone(),
                        };
                        let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(140.0));
                        if r.has_focus() {
                            st.naming = Some((i, buf.clone()));
                        }
                        if r.lost_focus() {
                            st.naming = None;
                            let name = buf.trim();
                            if !name.is_empty()
                                && name != p.name
                                && params.iter().all(|q| q.name != name)
                            {
                                let mut ps = params.clone();
                                ps[i].name = name.to_owned();
                                write = Some(ps);
                            }
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .button(RichText::new("✕").color(BAD))
                                .on_hover_text("Убрать ручку и её связи")
                                .clicked()
                            {
                                let mut ps = params.clone();
                                ps.remove(i);
                                write = Some(ps);
                            }
                        });
                    });
                    let mut q = p.clone();
                    let mut touched = false;
                    ui.horizontal(|ui| {
                        prop(ui, "В файле");
                        ui.add_enabled_ui(p.links.is_empty(), |ui| {
                            let r = ui.add(egui::DragValue::new(&mut q.value).speed(0.01).range(q.min..=q.max));
                            let r = if p.links.is_empty() {
                                r
                            } else {
                                r.on_disabled_hover_text("Значение, под которое нарисована модель. С обученной ручкой не меняется: сначала «Забыть»")
                            };
                            touched |= r.changed();
                            end |= r.drag_stopped();
                        });
                        ui.label("от");
                        let r = ui.add(egui::DragValue::new(&mut q.min).speed(0.01).range(f32::MIN..=q.value));
                        touched |= r.changed();
                        end |= r.drag_stopped();
                        ui.label("до");
                        let r = ui.add(egui::DragValue::new(&mut q.max).speed(0.01).range(q.value..=f32::MAX));
                        touched |= r.changed();
                        end |= r.drag_stopped();
                    });
                    if touched {
                        let mut ps = params.clone();
                        ps[i] = q;
                        write = Some(ps);
                    }
                    ui.horizontal(|ui| {
                        prop(ui, "Посмотреть");
                        ui.spacing_mut().slider_width = 150.0;
                        let mut v = shape.preview.get(&p.name).copied().unwrap_or(p.value);
                        let r = ui
                            .add(egui::Slider::new(&mut v, p.min..=p.max))
                            .on_hover_text(
                                "Модель на подставке при другом значении (в файл не пишется)",
                            );
                        if r.changed() {
                            let mut m = shape.preview.clone();
                            if v == p.value {
                                m.remove(&p.name);
                            } else {
                                m.insert(p.name.clone(), v);
                            }
                            preview = Some(m);
                        }
                        if shape.preview.contains_key(&p.name)
                            && ui.small_button("↺").on_hover_text("Как в файле").clicked()
                        {
                            let mut m = shape.preview.clone();
                            m.remove(&p.name);
                            preview = Some(m);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Связано чисел: {}", p.links.len()))
                                .color(MUTED)
                                .size(FONT_SMALL),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !p.links.is_empty()
                                && ui.button("Забыть").on_hover_text("Убрать связи ручки с деталями").clicked()
                            {
                                let mut ps = params.clone();
                                ps[i].links.clear();
                                write = Some(ps);
                            }
                            if ui
                                .button("Научить")
                                .on_hover_text("Поставить ручку на значение из «Посмотреть» (или на край) и показать мышью, как меняется модель")
                                .clicked()
                            {
                                let seen = shape.preview.get(&p.name).copied().filter(|v| *v != p.value);
                                let target = seen.unwrap_or(if p.max > p.value { p.max } else { p.min });
                                let values = BTreeMap::from([(p.name.clone(), target)]);
                                match model_params::applied(text, &values) {
                                    Ok(t) if target != p.value => {
                                        st.teach = Some(Teach {
                                            model: shape.name.to_owned(),
                                            name: p.name.clone(),
                                            before: text.to_owned(),
                                            target,
                                        });
                                        st.note = None;
                                        set_text = Some(t);
                                        preview = Some(BTreeMap::new());
                                    }
                                    Ok(_) => st.note = Some(("у ручки нет другого значения: раздвиньте «от» и «до»".into(), true)),
                                    Err(e) => st.note = Some((e, true)),
                                }
                            }
                        });
                    });
                }
                if !shape.preview.is_empty() {
                    ui.label(
                        RichText::new(
                            "Ручки не как в файле: стрелки деталей спрятаны, пока не нажмёте «↺»",
                        )
                        .color(MUTED)
                        .size(FONT_SMALL),
                    );
                }
                ui.add_space(GAP_S);
                if ui
                    .button("+ Ручка")
                    .on_hover_text("Новая ручка модели")
                    .clicked()
                {
                    let mut ps = params.clone();
                    ps.push(model_params::new_param(&params));
                    write = Some(ps);
                }
            }
            if let Some((note, bad)) = &st.note {
                ui.label(
                    RichText::new(note)
                        .color(if *bad { BAD } else { MUTED })
                        .size(FONT_SMALL),
                );
            }
        });
        if let Some(t) = new_text {
            st.teach = None;
            set_text = Some(t);
        }
        if set_text.is_some()
            && st
                .teach
                .as_ref()
                .is_some_and(|t| Some(&t.before) == set_text.as_ref())
        {
            st.teach = None;
        }
        if let Some(ps) = write {
            match model_params::write(text, &ps) {
                Ok(t) => {
                    let committed = !ui.ctx().input(|i| i.pointer.primary_down());
                    self.actions.push(Action::ShapeText {
                        text: t,
                        commit: committed,
                    });
                    // Убранные и переименованные ручки уходят из «Посмотреть».
                    let m: BTreeMap<String, f32> = shape
                        .preview
                        .iter()
                        .filter(|(n, _)| ps.iter().any(|p| &p.name == *n))
                        .map(|(n, v)| (n.clone(), *v))
                        .collect();
                    if m.len() != shape.preview.len() {
                        preview = Some(m);
                    }
                }
                Err(e) => tracing::warn!("Ручки не записались: {e}"),
            }
        }
        if let Some(t) = set_text {
            self.actions.push(Action::ShapeText {
                text: t,
                commit: true,
            });
        }
        if end {
            self.actions.push(Action::EndEdit);
        }
        if let Some(m) = preview {
            self.actions.push(Action::ShapePreview(m));
        }
    }

    /// Окно сцены во «Формах» (М.2): движок рисует модель на подставке; сверху полоса с
    /// именем модели и кнопками, мышь поворачивает камеру.
    fn shape_view(&mut self, ui: &mut Ui, shape: &ShapeInfo) {
        let full = ui.max_rect();
        let bar = Rect::from_min_size(full.min, vec2(full.width(), TOOLBAR_HEIGHT));
        let rect = Rect::from_min_max(pos2(full.left(), bar.bottom()), full.max);
        self.viewport = Some(rect);
        ui.painter().rect_filled(bar, 0, PANEL);
        ui.painter()
            .hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(LINE, BORDER));
        let actions = &mut self.actions;
        let selected = &self.shape_selected;
        let cam = |c| Action::ShapeCamera(c);
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(bar.shrink2(vec2(GAP_L, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.label(
                    RichText::new(format!("Форма: {}", shape.name))
                        .size(FONT_CHIP)
                        .color(TEXT),
                );
                ui.add_space(GAP_XL);
                ui.spacing_mut().item_spacing.x = GAP_XS;
                for a in ShapeAngle::ALL {
                    if angle_button(ui, a).clicked() {
                        actions.push(cam(ShapeCam::View(a)));
                    }
                }
                ui.add_space(GAP_L);
                if ui
                    .button(RichText::new("Деталь крупно").size(FONT_CHIP))
                    .on_hover_text(
                        "Камера встаёт к выбранной детали и ходит вокруг неё (F). \
                         Двойной щелчок по детали в иерархии — то же самое",
                    )
                    .clicked()
                {
                    actions.push(cam(ShapeCam::Focus(selected.clone())));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = GAP_M;
                    if ui
                        .button(RichText::new("Закрыть").size(FONT_CHIP))
                        .on_hover_text("Вернуться к сцене (Esc)")
                        .clicked()
                    {
                        actions.push(Action::CloseShape);
                    }
                    let mut spin = shape.spin;
                    if ui
                        .add_enabled(
                            !shape.free,
                            egui::Checkbox::new(
                                &mut spin,
                                RichText::new("Вращать").size(FONT_CHIP),
                            ),
                        )
                        .on_hover_text("Модель медленно поворачивается на подставке")
                        .changed()
                    {
                        actions.push(cam(ShapeCam::Spin(spin)));
                    }
                    quality_menu(ui, shape, actions);
                    ui.separator();
                    ui.spacing_mut().item_spacing.x = GAP_XS;
                    for (free, name, hint) in [
                        (
                            true,
                            "Полёт",
                            "Свободная камера: правая кнопка — оглядеться, W A S D — лететь, \
                             Q и E — вниз и вверх, Shift — быстрее, колесо — вперёд-назад",
                        ),
                        (
                            false,
                            "Вокруг",
                            "Камера ходит вокруг модели или показанной детали: правая кнопка — \
                             повернуть, средняя — сдвинуть, колесо — ближе",
                        ),
                    ] {
                        if ui
                            .selectable_label(
                                shape.free == free,
                                RichText::new(name).size(FONT_CHIP),
                            )
                            .on_hover_text(hint)
                            .clicked()
                            && shape.free != free
                        {
                            actions.push(cam(ShapeCam::Free(free)));
                        }
                    }
                });
            },
        );
        let response = ui.interact(
            rect,
            ui.scope_id().with("shape-view"),
            Sense::click_and_drag(),
        );
        self.viewport_hovered = response.hovered() || response.dragged();
        // Режимы ручек детали — столбик значков у левого края окна.
        let tools = Rect::from_min_size(
            rect.left_top() + vec2(12.0, 72.0),
            vec2(36.0, 3.0 * 28.0 + 8.0),
        );
        ui.painter()
            .rect_filled(tools, RADIUS_S, PANEL.gamma_multiply(0.9));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(tools.shrink(4.0))
                .layout(Layout::top_down(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for m in GizmoMode::ALL {
                    if gizmo_button(ui, m, self.gizmo_mode == m)
                        .on_hover_text(m.hint())
                        .clicked()
                    {
                        self.gizmo_mode = m;
                    }
                }
            },
        );
        // Левая кнопка — выбор и ручки детали (М.4), правая — повернуть, средняя —
        // сдвинуть.
        // Ручки модели не как в файле (М.6, «Посмотреть»): на подставке не то, что в
        // тексте, поэтому стрелки деталей спрятаны.
        let gizmo = if shape.preview.is_empty() {
            self.shape_gizmo(ui, shape)
        } else {
            None
        };
        self.shape_pointer(ui, &response, shape, gizmo.as_ref());
        let d = response.drag_delta();
        let camera_drag = response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle);
        if camera_drag && d != Vec2::ZERO {
            let pan = response.dragged_by(egui::PointerButton::Middle);
            self.actions.push(cam(if pan {
                ShapeCam::Pan(d)
            } else {
                ShapeCam::Orbit(d)
            }));
            endless_drag(ui.ctx());
            ui.ctx().set_cursor_icon(if pan {
                CursorIcon::Move
            } else {
                CursorIcon::Grabbing
            });
        }
        if response.hovered() || response.dragged() {
            let steps = ui.input(wheel_steps);
            if steps != 0.0 {
                self.actions.push(cam(ShapeCam::Zoom(steps)));
            }
            if shape.free && !ui.ctx().egui_wants_keyboard_input() {
                let (dir, dt) = ui.input(|i| {
                    let k = |a: egui::Key, b: egui::Key| {
                        f32::from(u8::from(i.key_down(a))) - f32::from(u8::from(i.key_down(b)))
                    };
                    use egui::Key::*;
                    let fast = if i.modifiers.shift { 3.0 } else { 1.0 };
                    (
                        [k(D, A) * fast, k(E, Q) * fast, k(W, S) * fast],
                        i.stable_dt.min(0.1),
                    )
                });
                if dir != [0.0; 3] {
                    self.actions.push(cam(ShapeCam::Fly(dir, dt)));
                }
            }
        }
        let painter = ui.painter_at(rect);
        let hint = if shape.free {
            "Полёт: правая кнопка — оглядеться, W A S D — лететь, Q E — вниз и вверх, \
             Shift — быстрее. Esc — назад к сцене"
        } else {
            "Левая — выбрать деталь и тянуть её ручки (W двигать, E повернуть, R размер), \
             правая — повернуть камеру (щелчок — меню детали), средняя — сдвинуть, колесо — \
             ближе, F — крупно. Esc — назад к сцене"
        };
        chip(&painter, rect.left_top() + vec2(12.0, 12.0), hint, MUTED);
        if let Some(e) = shape.error {
            chip(
                &painter,
                rect.left_top() + vec2(12.0, 40.0),
                &format!("Ошибка в модели: {e}"),
                BAD,
            );
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
                // Другую сцену — только когда правки этой сохранены и идёт правка.
                let free = !info.dirty && info.play == PlayState::Edit && info.can_save;
                let why = "Сначала сохраните правки (Ctrl+S) и остановите игру";
                if ui
                    .add_enabled(free, egui::Button::new("Новая сцена"))
                    .on_disabled_hover_text(why)
                    .clicked()
                {
                    actions.push(Action::NewScene);
                    ui.close();
                }
                ui.add_enabled_ui(free, |ui| {
                    ui.menu_button("Открыть сцену", |ui| {
                        ui.set_min_width(200.0);
                        if info.scenes.is_empty() {
                            ui.label(RichText::new("Других сцен нет").color(MUTED));
                        }
                        for p in info.scenes {
                            let name = p
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let current = name == info.scene_name;
                            if ui.selectable_label(current, name).clicked() && !current {
                                actions.push(Action::OpenScene(p.clone()));
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_disabled_hover_text(why);
                });
                if ui
                    .add_enabled(
                        info.can_restore,
                        egui::Button::new("Восстановить автосохранение"),
                    )
                    .on_hover_text(
                        "Вернуть правки, которые редактор сам записал, но в файл они не попали",
                    )
                    .clicked()
                {
                    actions.push(Action::RestoreAutosave);
                    ui.close();
                }
                ui.separator();
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
                if icon_button(ui, Icon::Play, "Симуляция", false)
                    .on_hover_text(
                        "Физика и логика идут, а герой стоит: смотреть, как падают и катятся \
                         предметы. Камера свободна.",
                    )
                    .clicked()
                {
                    self.actions.push(Action::Simulate);
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
                Tab::Assets => {
                    if let Some(path) = self.assets.draw(ui) {
                        self.actions.push(Action::OpenShape(path));
                    }
                    return;
                }
                Tab::Profiler => return profiler(ui, info, &self.frames),
                // Фильтр и «Очистить» над прокруткой: всегда на виду.
                Tab::Console => console_bar(ui, &mut self.log_filter, &mut self.log),
                _ => {}
            }
            egui::ScrollArea::vertical()
                .id_salt(("area", self.workspace, side.index()))
                .auto_shrink(false)
                .stick_to_bottom(tab == Tab::Console)
                .show(ui, |ui| match tab {
                    Tab::Hierarchy if info.shape.is_some() => self.shape_tree(ui, info),
                    Tab::Inspector if info.shape.is_some() => self.shape_inspector(ui, info),
                    Tab::Hierarchy => self.hierarchy(ui, info),
                    Tab::Inspector => self.inspector(ui, info),
                    Tab::Console => console(ui, &self.log, self.log_filter),
                    Tab::Assets | Tab::Profiler => {}
                    Tab::Soon { title, text } => soon(ui, title, text),
                });
        });
    }

    fn hierarchy(&mut self, ui: &mut Ui, info: &Info) {
        let Some(file) = info.scene else {
            ui.label(
                RichText::new(info.scene_name.to_uppercase())
                    .small()
                    .color(MUTED),
            );
            ui.label(RichText::new("Сцена не прочитана").color(BAD));
            return;
        };
        let editing = info.play == PlayState::Edit;
        Frame::NONE
            .fill(BG)
            .corner_radius(RADIUS_S)
            .inner_margin(PAD_FIELD)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    paint_search(ui);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Поиск")
                            .frame(Frame::NONE)
                            .desired_width(ui.available_width() - 24.0),
                    );
                    if !self.search.is_empty()
                        && ui
                            .small_button("×")
                            .on_hover_text("Сбросить поиск")
                            .clicked()
                    {
                        self.search.clear();
                    }
                });
            });
        ui.add_space(GAP_S);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(info.scene_name.to_uppercase())
                    .small()
                    .color(MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if editing {
                    self.catalog_menu(ui, info);
                }
                ui.label(
                    RichText::new(format!("предметов: {}", outline::count(file)))
                        .small()
                        .color(MUTED),
                );
            });
        });
        ui.spacing_mut().item_spacing.y = 1.0;
        let query = self.search.trim().to_owned();
        let rows = if query.is_empty() {
            outline::rows(file, &self.open)
        } else {
            outline::rows_matching(file, &query)
        };
        if rows.is_empty() && !query.is_empty() {
            ui.label(RichText::new("Ничего не нашлось").color(MUTED));
        }
        let pointer = ui.ctx().pointer_interact_pos();
        let mut drop: Option<(Rect, Drop)> = None;
        for row in &rows {
            let selected =
                self.selected.as_ref() == Some(&row.path) || self.more.contains(&row.path);
            let hidden = covered(&row.path, info.hidden);
            let sense = if editing {
                Sense::click_and_drag()
            } else {
                Sense::click()
            };
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), sense);
            let painter = ui.painter().clone();
            if selected {
                painter.rect(
                    rect,
                    RADIUS_S,
                    SELECT_BG,
                    Stroke::new(LINE, SELECT),
                    egui::StrokeKind::Inside,
                );
            } else if response.hovered() {
                painter.rect_filled(rect, RADIUS_S, HOVER);
            }
            let indent = rect.left() + GAP_M + row.depth as f32 * INDENT;
            let arrow = Rect::from_min_size(pos2(indent, rect.top()), vec2(14.0, rect.height()));
            if row.has_children {
                let open = !query.is_empty() || self.open.contains(&row.path);
                let icon = if open {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                };
                paint_icon(&painter, arrow.center(), icon, MUTED);
            }
            let dot =
                Rect::from_center_size(pos2(arrow.right() + 8.0, rect.center().y), vec2(8.0, 8.0));
            let round = if row.kind == Kind::Light { 4 } else { 2 };
            let dot_color = if hidden {
                row.kind.color().gamma_multiply(0.35)
            } else {
                row.kind.color()
            };
            painter.rect_filled(dot, round, dot_color);
            // Глаз и замок справа: видны под мышью или когда включены.
            let eye = Rect::from_center_size(
                pos2(rect.right() - 14.0, rect.center().y),
                vec2(20.0, 20.0),
            );
            let lock = eye.translate(vec2(-22.0, 0.0));
            let text_x = dot.right() + 8.0;
            let renaming = matches!(&self.renaming, Some((p, _)) if *p == row.path);
            if renaming {
                let r = Rect::from_min_max(
                    pos2(text_x - 4.0, rect.top() + 2.0),
                    pos2(lock.left() - 4.0, rect.bottom() - 2.0),
                );
                if let Some((_, text)) = self.renaming.as_mut() {
                    let edit = ui.put(
                        r,
                        egui::TextEdit::singleline(text)
                            .font(FontId::proportional(FONT_BODY))
                            .margin(Margin::symmetric(4, 1)),
                    );
                    if !edit.lost_focus() {
                        edit.request_focus();
                    }
                    if edit.lost_focus() {
                        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        if let Some((path, text)) = self.renaming.take()
                            && !esc
                            && outline::node_at(file, &path).is_some_and(|n| n.name != text)
                        {
                            self.actions.push(Action::Edit {
                                edit: Edit {
                                    path,
                                    change: Change::Name(text),
                                },
                                commit: true,
                            });
                        }
                    }
                }
            } else {
                let color = if hidden {
                    MUTED
                } else if selected {
                    Color32::WHITE
                } else {
                    TEXT_DIM
                };
                painter.text(
                    pos2(text_x, rect.center().y),
                    Align2::LEFT_CENTER,
                    &row.name,
                    FontId::proportional(FONT_BODY),
                    color,
                );
            }
            let own_hidden = info.hidden.contains(&row.path);
            let own_locked = self.locked.contains(&row.path);
            let hover_eye = response.hovered() && pointer.is_some_and(|p| eye.contains(p));
            let hover_lock = response.hovered() && pointer.is_some_and(|p| lock.contains(p));
            if editing && (response.hovered() || own_hidden || own_locked) {
                let c = |on: bool, hot: bool| {
                    if hot {
                        Color32::WHITE
                    } else if on {
                        ACCENT
                    } else {
                        MUTED
                    }
                };
                paint_eye(
                    &painter,
                    eye.center(),
                    !own_hidden,
                    c(own_hidden, hover_eye),
                );
                paint_lock(
                    &painter,
                    lock.center(),
                    own_locked,
                    c(own_locked, hover_lock),
                );
            }
            if response.clicked() {
                let p = response.interact_pointer_pos();
                let on = |r: Rect| p.is_some_and(|p| r.contains(p));
                if editing && on(eye) {
                    self.actions.push(Action::ToggleHidden(row.path.clone()));
                } else if editing && on(lock) {
                    if !self.locked.remove(&row.path) {
                        self.locked.insert(row.path.clone());
                    }
                } else if row.has_children && on(arrow.expand2(vec2(4.0, 0.0))) {
                    if !self.open.remove(&row.path) {
                        self.open.insert(row.path.clone());
                    }
                } else if ui.input(|i| i.modifiers.command) {
                    self.toggle_select(row.path.clone());
                } else {
                    self.select(Some(row.path.clone()));
                }
            }
            if response.double_clicked() && row.has_children && !self.open.remove(&row.path) {
                self.open.insert(row.path.clone());
            }
            if response.drag_started() && !renaming {
                self.dragging = Some(row.path.clone());
            }
            if let (Some(from), Some(p)) = (&self.dragging, pointer)
                && rect.contains(p)
                && !row.path.starts_with(from)
            {
                let y = (p.y - rect.top()) / rect.height();
                let open_parent = row.has_children && self.open.contains(&row.path);
                let d = if y < 0.25 {
                    Drop::Before
                } else if y > 0.75 && !open_parent {
                    Drop::After
                } else {
                    Drop::Into
                };
                drop = Some((rect, d));
                self.drop_target = Some((row.path.clone(), d));
            }
            if editing {
                let path = row.path.clone();
                response.context_menu(|ui| {
                    self.selected = Some(path.clone());
                    self.node_menu(ui, &path, file, info);
                });
            }
        }
        if self.dragging.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            match drop {
                Some((r, d)) => {
                    let p = ui.painter();
                    let s = Stroke::new(LINE_FOCUS, ACCENT);
                    match d {
                        Drop::Before => {
                            p.hline(r.x_range(), r.top(), s);
                        }
                        Drop::After => {
                            p.hline(r.x_range(), r.bottom(), s);
                        }
                        Drop::Into => {
                            p.rect_stroke(r, RADIUS_S, s, egui::StrokeKind::Inside);
                        }
                    }
                }
                None => self.drop_target = None,
            }
            if !ui.input(|i| i.pointer.any_down()) {
                if let (Some(from), Some((to, d))) = (self.dragging.take(), self.drop_target.take())
                    && let Some((parent, index)) = drop_place(&to, d)
                {
                    self.actions.push(Action::Move {
                        from,
                        parent,
                        index,
                    });
                }
                self.dragging = None;
            }
        }
    }

    /// Кнопка «+»: новый предмет из каталога там, куда смотрит камера, за выбранным.
    fn catalog_menu(&mut self, ui: &mut Ui, info: &Info) {
        let at = match (info.view, self.viewport) {
            (Some(v), Some(r)) => v.to_scene(r.center()),
            (Some(v), None) => v.center,
            _ => [0.0, 0.0],
        };
        let at = at.map(|c| (c * 1000.0).round() / 1000.0);
        ui.menu_button(RichText::new("+").strong().color(ACCENT), |ui| {
            ui.set_min_width(260.0);
            ui.label(RichText::new("Новый предмет").small().color(MUTED));
            for (name, hint, template) in parts::CATALOG {
                if ui.button(*name).on_hover_text(*hint).clicked() {
                    self.actions.push(Action::Paste {
                        after: top_of(self.selected.as_deref()),
                        text: Some(parts::catalog_item(template, at)),
                    });
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("Добавить предмет");
    }

    /// Пункты меню предмета (правая кнопка в иерархии или в сцене).
    fn node_menu(&mut self, ui: &mut Ui, path: &[usize], file: &SceneFile, info: &Info) {
        ui.set_min_width(230.0);
        let item = |ui: &mut Ui, text: &str, keys: &str| {
            ui.add(egui::Button::new(text).shortcut_text(keys))
                .clicked()
        };
        let p = path.to_vec();
        if item(ui, "Переименовать", "F2") {
            if let Some(n) = outline::node_at(file, path) {
                self.renaming = Some((p.clone(), n.name.clone()));
            }
            ui.close();
        }
        if item(ui, "Дублировать", "Ctrl+D") {
            self.actions.push(Action::Duplicate(p.clone()));
            ui.close();
        }
        if ui
            .button("Сохранить как заготовку")
            .on_hover_text("Файл в assets/prefabs: его можно перетащить из Ассетов в любую сцену")
            .clicked()
        {
            self.actions.push(Action::SavePrefab(p.clone()));
            ui.close();
        }
        ui.separator();
        if item(ui, "Копировать", "Ctrl+C") {
            self.actions.push(Action::Copy {
                path: p.clone(),
                cut: false,
            });
            ui.close();
        }
        if item(ui, "Вырезать", "Ctrl+X") {
            self.actions.push(Action::Copy {
                path: p.clone(),
                cut: true,
            });
            ui.close();
        }
        let paste = egui::Button::new("Вставить после").shortcut_text("Ctrl+V");
        if ui.add_enabled(info.can_paste, paste).clicked() {
            self.actions.push(Action::Paste {
                after: Some(p.clone()),
                text: None,
            });
            ui.close();
        }
        ui.separator();
        if item(ui, "Показать в сцене", "F") {
            self.selected = Some(p.clone());
            self.focus_selected(info);
            ui.close();
        }
        let hidden = info.hidden.contains(path);
        let text = if hidden {
            "Показать в редакторе"
        } else {
            "Скрыть в редакторе"
        };
        if item(ui, text, "") {
            self.actions.push(Action::ToggleHidden(p.clone()));
            ui.close();
        }
        let locked = self.locked.contains(path);
        if item(
            ui,
            if locked {
                "Открепить"
            } else {
                "Закрепить"
            },
            "",
        ) {
            if !self.locked.remove(path) {
                self.locked.insert(p.clone());
            }
            ui.close();
        }
        ui.separator();
        let many = !self.more.is_empty();
        if ui
            .add_enabled(
                many,
                egui::Button::new("Сгруппировать выбранные").shortcut_text("Ctrl+G"),
            )
            .on_disabled_hover_text(
                "Выберите несколько: Ctrl+щелчок в иерархии или Shift+щелчок в сцене",
            )
            .clicked()
        {
            self.actions
                .push(Action::Group(arrange::roots(&self.selection())));
            ui.close();
        }
        let has_children = outline::node_at(file, path).is_some_and(|n| !n.children.is_empty());
        if ui
            .add_enabled(
                has_children,
                egui::Button::new("Разгруппировать").shortcut_text("Ctrl+Shift+G"),
            )
            .clicked()
        {
            self.actions.push(Action::Ungroup(p.clone()));
            ui.close();
        }
        ui.separator();
        if item(ui, "Удалить", "Delete") {
            self.actions.push(Action::Delete(p));
            ui.close();
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
        if self
            .pinned
            .as_ref()
            .is_some_and(|p| outline::node_at(file, p).is_none())
        {
            self.pinned = None;
        }
        let shown = self.pinned.clone().or_else(|| self.selected.clone());
        let node = shown
            .as_ref()
            .and_then(|p| outline::node_at(file, p).map(|n| (p, n)));
        if let Some((path, _)) = node {
            // Замок: инспектор остаётся на этом предмете, даже если выбрать другой.
            ui.horizontal(|ui| {
                let pinned = self.pinned.is_some();
                let note = if pinned {
                    "Инспектор закреплён"
                } else if !self.more.is_empty() {
                    "Выбрано несколько: здесь главный"
                } else {
                    ""
                };
                ui.label(RichText::new(note).small().color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
                    let color = if resp.hovered() {
                        Color32::WHITE
                    } else if pinned {
                        ACCENT
                    } else {
                        MUTED
                    };
                    paint_lock(ui.painter(), r.center(), pinned, color);
                    let hint = if pinned {
                        "Открепить инспектор: снова показывать выбранный"
                    } else {
                        "Закрепить инспектор на этом предмете"
                    };
                    if resp.on_hover_text(hint).clicked() {
                        self.pinned = if pinned { None } else { Some(path.clone()) };
                    }
                });
            });
        }
        match node {
            Some((path, node)) => node_inspector(
                ui,
                path,
                node,
                editing,
                &mut self.name_edit,
                &mut self.actions,
            ),
            None => parts::scene_settings(ui, file, info.scene_name, editing, &mut self.actions),
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
        if let Some(shape) = &info.shape {
            return self.shape_view(ui, shape);
        }
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
        let actions = &mut self.actions;
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
                    let ov = &mut self.ov;
                    let flat = |t: &str| {
                        egui::Button::new(RichText::new(t).size(FONT_CHIP).color(TEXT_DIM))
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE)
                    };
                    if editing {
                        if ui
                            .add_enabled(info.scene.is_some(), flat("Фокус"))
                            .on_hover_text("Камера на выбранный предмет или на всю сцену (F)")
                            .clicked()
                        {
                            focus = true;
                        }
                        ui.menu_button(RichText::new("Камера").size(FONT_CHIP), |ui| {
                            camera_menu(ui, ov, info, actions);
                        });
                    } else {
                        play_controls(ui, info, actions);
                    }
                    ui.menu_button(RichText::new("Метки").size(FONT_CHIP), |ui| {
                        for (name, on) in ov.show.items() {
                            ui.checkbox(on, name);
                        }
                    })
                    .response
                    .on_hover_text("Что показывать поверх сцены");
                    let mode = RichText::new(format!("Вид: {}", ov.mode.label())).size(FONT_CHIP);
                    ui.menu_button(mode, |ui| {
                        for m in ViewMode::ALL {
                            if ui
                                .selectable_label(ov.mode == m, m.label())
                                .on_hover_text(m.hint())
                                .clicked()
                            {
                                ov.mode = m;
                                ui.close();
                            }
                        }
                        ui.separator();
                        ui.label(
                            RichText::new(
                                "Режимы «без света», «только свет» и «поле расстояний SDF» \
                                 появятся вместе с отладочными проходами движка.",
                            )
                            .small()
                            .color(MUTED),
                        );
                    });
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
        // Рисунок тащат из Ассетов: рамка вокруг сцены, отпустили — новый предмет.
        if editing && response.dnd_hover_payload::<AssetFile>().is_some() {
            painter.rect_stroke(
                rect.shrink(1.0),
                0,
                Stroke::new(LINE_FOCUS, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        if editing
            && let Some(f) = response.dnd_release_payload::<AssetFile>()
            && let (Some(view), Some(m)) = (info.view, response.hover_pos())
        {
            let at = view.to_scene(m).map(|c| (c * 1000.0).round() / 1000.0);
            self.actions.push(Action::DropAsset {
                file: f.0.clone(),
                at,
                after: top_of(self.selected.as_deref()),
            });
        }
        let mut busy = false;
        if let (Some(file), Some(view)) = (info.scene, info.view) {
            let all = geom::place_all(file);
            self.ov.draw(&overlay::Ctx {
                painter: &painter,
                view,
                rect,
                file,
                placed: &all,
                hidden: info.hidden,
            });
            if self.ov.show.stats {
                let on_screen = all
                    .iter()
                    .filter(|p| {
                        let r = Rect::from_two_pos(view.to_ui(p.own.min), view.to_ui(p.own.max));
                        r.intersects(rect)
                    })
                    .count();
                let selected = self
                    .selected
                    .as_ref()
                    .and_then(|p| outline::node_at(file, p))
                    .map_or_else(|| "—".to_owned(), outline::display_name);
                overlay::stats(
                    &painter,
                    rect,
                    &[
                        format!("{:.0} к/с · кадр {:.1} мс", info.fps, info.frame_ms),
                        format!("предметов {} · на экране {on_screen}", all.len()),
                        format!("выбран: {selected}"),
                        format!(
                            "камера ({:.2}, {:.2}) · {:.0}%",
                            view.center[0],
                            view.center[1],
                            view.zoom * 100.0
                        ),
                    ],
                );
            }
        }
        if editing && let (Some(file), Some(view)) = (info.scene, info.view) {
            let all = geom::place_all(file);
            // Скрытые и закреплённые мышью в сцене не выбираются.
            let placed: Vec<geom::Placed> = all
                .iter()
                .filter(|p| !covered(&p.path, info.hidden) && !covered(&p.path, &self.locked))
                .cloned()
                .collect();
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
                .and_then(|path| Some((geom::find(&all, path)?, outline::node_at(file, path)?)));
            if let Some((p, node)) = sel {
                self.vp.selection(&painter, &view, p);
                viewport::label(&painter, &view, p, file);
                let locked = covered(&p.path, &self.locked) || covered(&p.path, info.hidden);
                busy = !locked
                    && ui
                        .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            ui.set_clip_rect(rect);
                            self.vp.gizmo(ui, &view, p, node, &mut self.actions)
                        })
                        .inner;
            } else if self.vp.drag.take().is_some() {
                // Предмет пропал, пока его тянули.
                self.actions.push(Action::CancelEdit);
            }
            // Остальные выбранные — рамкой; над сценой полоса выравнивания.
            for p in self.more.iter().filter_map(|path| geom::find(&all, path)) {
                self.vp.selection(&painter, &view, p);
            }
            if !self.more.is_empty() {
                let pick = egui::Area::new(ui.scope_id().with("align-bar"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(pos2(rect.center().x, rect.top() + GAP_M))
                    .pivot(Align2::CENTER_TOP)
                    .show(ui.ctx(), |ui| arrange::bar(ui, self.more.len() + 1))
                    .inner;
                let chosen = arrange::roots(&self.selection());
                match pick {
                    Some(arrange::BarPick::Align(how)) => {
                        let items: Vec<arrange::Item> = chosen
                            .iter()
                            .filter_map(|path| {
                                let p = geom::find(&all, path)?;
                                let n = outline::node_at(file, path)?;
                                Some(arrange::Item {
                                    path: path.clone(),
                                    bounds: p.all,
                                    parent: p.parent,
                                    at: [n.at.0, n.at.1],
                                })
                            })
                            .collect();
                        let moves = arrange::arrange(&items, how);
                        if !moves.is_empty() {
                            self.actions.push(Action::MoveMany(moves));
                        }
                    }
                    Some(arrange::BarPick::Group) => self.actions.push(Action::Group(chosen)),
                    None => {}
                }
            }
            // Линейка: тянут мышью — длина и угол; остаётся, пока выбрана линейка.
            if self.vp.tool == viewport::Tool::Measure {
                busy = true;
                if let Some(m) = response.interact_pointer_pos() {
                    let at = view.to_scene(m);
                    if response.drag_started_by(egui::PointerButton::Primary) {
                        let from = ui.input(|i| i.pointer.press_origin()).unwrap_or(m);
                        self.ruler = Some((view.to_scene(from), at));
                    } else if response.dragged_by(egui::PointerButton::Primary)
                        && let Some((_, end)) = self.ruler.as_mut()
                    {
                        *end = at;
                    }
                }
                if let Some((a, b)) = self.ruler {
                    arrange::draw_ruler(&painter, &view, a, b);
                }
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            }
            if !busy
                && let Some(m) = response.interact_pointer_pos()
                && (response.clicked() || response.double_clicked())
            {
                let deep = response.double_clicked();
                let picked = geom::pick(&placed, view.to_scene(m), deep);
                if ui.input(|i| i.modifiers.shift) {
                    if let Some(p) = picked {
                        self.toggle_select(p);
                    }
                } else {
                    // Родители выбранного раскрываются в иерархии.
                    self.select(picked);
                }
            }
            // Правая кнопка без перетаскивания: выбрать предмет под мышью и показать меню.
            if response.secondary_clicked()
                && let Some(m) = response.interact_pointer_pos()
                && let Some(path) = geom::pick(&placed, view.to_scene(m), false)
            {
                self.selected = Some(path);
            }
            if let Some(path) = self.selected.clone() {
                response.context_menu(|ui| self.node_menu(ui, &path, file, info));
            }
        }
        if (editing || info.ejected) && !busy && self.vp.drag.is_none() {
            let pan = response.dragged_by(egui::PointerButton::Primary)
                || response.dragged_by(egui::PointerButton::Middle)
                || response.dragged_by(egui::PointerButton::Secondary);
            let d = response.drag_delta();
            if pan && d != Vec2::ZERO {
                self.actions.push(Action::Pan(d));
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
        }
        if self.ov.show.map
            && (editing || info.ejected)
            && let (Some(file), Some(view)) = (info.scene, info.view)
        {
            let all = geom::place_all(file);
            if let Some(center) = overlay::minimap(ui, rect, view, &all, self.selected.as_deref()) {
                // Середина окна сцены — не середина всего окна (панели по бокам).
                let mid = view.to_scene(rect.center());
                self.actions.push(Action::SetCamera {
                    center: [
                        center[0] + view.center[0] - mid[0],
                        center[1] + view.center[1] - mid[1],
                    ],
                    zoom: view.zoom,
                });
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
                if info.simulating {
                    "Симуляция · герой не управляется · Esc — вернуться к правке"
                } else {
                    "Игра идёт · Esc или «Стоп» — вернуться к правке"
                },
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

/// Меню «Камера»: закладки Ctrl+1…9.
fn camera_menu(ui: &mut Ui, ov: &mut Overlay, info: &Info, actions: &mut Vec<Action>) {
    ui.set_min_width(240.0);
    ui.label(
        RichText::new("Ctrl+цифра — запомнить, цифра — вернуться")
            .small()
            .color(MUTED),
    );
    for k in 0..ov.bookmarks.len() {
        ui.horizontal(|ui| {
            let text = match ov.bookmarks[k] {
                Some((c, z)) => format!("{} · ({:.2}, {:.2}) {:.0}%", k + 1, c[0], c[1], z * 100.0),
                None => format!("{} · пусто", k + 1),
            };
            let go = ui.add_enabled(ov.bookmarks[k].is_some(), egui::Button::new(text));
            if go.clicked()
                && let Some((center, zoom)) = ov.bookmarks[k]
            {
                actions.push(Action::SetCamera { center, zoom });
                ui.close();
            }
            if ui
                .add_enabled(info.view.is_some(), egui::Button::new("запомнить").small())
                .clicked()
                && let Some(v) = info.view
            {
                ov.bookmarks[k] = Some((v.center, v.zoom));
            }
        });
    }
    ui.separator();
    if ui.button("Камера в начало сцены").clicked() {
        actions.push(Action::ResetCamera);
        ui.close();
    }
}

/// Кнопки игры над сценой: скорость, шаг кадра на паузе, отцепить камеру.
fn play_controls(ui: &mut Ui, info: &Info, actions: &mut Vec<Action>) {
    let eject = if info.ejected {
        "Камера за героем"
    } else {
        "Отцепить камеру"
    };
    if ui
        .button(RichText::new(eject).size(FONT_CHIP))
        .on_hover_text("F8: камера перестаёт следить за героем, сцену можно двигать мышью")
        .clicked()
    {
        actions.push(Action::Eject);
    }
    let step = egui::Button::new(RichText::new("Шаг кадра").size(FONT_CHIP));
    if ui
        .add_enabled(info.play == PlayState::Paused, step)
        .on_hover_text("На паузе: игра делает один шаг (1/60 секунды)")
        .clicked()
    {
        actions.push(Action::Step);
    }
    let speed = RichText::new(format!("Скорость {}×", fmt_scale(info.time_scale))).size(FONT_CHIP);
    ui.menu_button(speed, |ui| {
        for k in [0.1, 0.25, 0.5, 1.0, 2.0] {
            if ui
                .selectable_label(
                    (info.time_scale - k).abs() < 1e-6,
                    format!("{}×", fmt_scale(k)),
                )
                .clicked()
            {
                actions.push(Action::SetTimeScale(k));
                ui.close();
            }
        }
    });
}

fn fmt_scale(k: f64) -> String {
    let t = format!("{k}");
    t.replace('.', ",")
}

/// Предмет или один из его родителей есть в наборе (скрытых, закреплённых).
fn covered(path: &[usize], set: &HashSet<Vec<usize>>) -> bool {
    (1..=path.len()).any(|k| set.contains(&path[..k]))
}

/// Верхний предмет над `path`: новые предметы встают за ним, на верхний уровень, где
/// место `at` — это место в сцене (у вложенного оно было бы от родителя).
fn top_of(path: Option<&[usize]>) -> Option<Vec<usize>> {
    path.and_then(|p| p.first()).map(|&i| vec![i])
}

/// Куда бросить предмет относительно строки иерархии.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drop {
    Before,
    After,
    /// Внутрь, последним вложенным.
    Into,
}

/// Список и место в нём для броска на строку `to`.
fn drop_place(to: &[usize], d: Drop) -> Option<(Vec<usize>, usize)> {
    let (last, parent) = to.split_last()?;
    Some(match d {
        Drop::Before => (parent.to_vec(), *last),
        Drop::After => (parent.to_vec(), last + 1),
        Drop::Into => (to.to_vec(), usize::MAX),
    })
}

/// Лупа поиска.
fn paint_search(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    let p = ui.painter();
    let c = r.center() + vec2(-1.5, -1.5);
    p.circle_stroke(c, 4.5, Stroke::new(1.3, MUTED));
    p.line_segment(
        [c + vec2(3.2, 3.2), c + vec2(6.5, 6.5)],
        Stroke::new(1.5, MUTED),
    );
}

/// Глаз: открытый — предмет виден, перечёркнутый — скрыт в редакторе.
fn paint_eye(p: &egui::Painter, c: Pos2, open: bool, color: Color32) {
    let s = Stroke::new(1.3, color);
    let arc = |sign: f32| {
        (0..=12).map(move |i| {
            let t = std::f32::consts::PI * i as f32 / 12.0;
            c + vec2(-7.0 * sign * t.cos(), -4.0 * sign * t.sin())
        })
    };
    let pts: Vec<Pos2> = arc(1.0).chain(arc(-1.0)).collect();
    p.add(egui::Shape::closed_line(pts, s));
    p.circle_filled(c, 1.8, color);
    if !open {
        p.line_segment([c + vec2(-7.0, 5.0), c + vec2(7.0, -5.0)], s);
    }
}

/// Замок: закрытый — предмет закреплён, открытый — дужка приподнята.
fn paint_lock(p: &egui::Painter, c: Pos2, closed: bool, color: Color32) {
    let s = Stroke::new(1.3, color);
    let body = Rect::from_center_size(c + vec2(0.0, 2.5), vec2(10.0, 7.0));
    p.rect_stroke(body, 1, s, egui::StrokeKind::Middle);
    let lift = if closed { 0.0 } else { 2.5 };
    let top = body.top() - 4.5 - lift;
    let (l, r) = (body.left() + 2.0, body.right() - 2.0);
    p.line_segment([pos2(l, body.top()), pos2(l, top)], s);
    p.line_segment([pos2(l, top), pos2(r, top)], s);
    p.line_segment([pos2(r, top), pos2(r, body.top() - lift)], s);
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
                // Подпись, значение, шаг мыши, единица, правка, значение «как было».
                let rows: [(Row, f64); 4] = [
                    (("Поворот", node.angle, 0.5, "°", Change::Angle), 0.0),
                    (("Размер", node.scale, 0.005, "", Change::Scale), 1.0),
                    (("Глубина", node.depth, 0.01, "", Change::Depth), 0.0),
                    (
                        (
                            "Прозрачность",
                            f64::from(node.opacity),
                            0.005,
                            "",
                            Change::Opacity,
                        ),
                        1.0,
                    ),
                ];
                for ((name, v, speed, suffix, make), default) in rows {
                    prop(ui, name);
                    if let Some(f) = num_field(ui, v, speed, None, suffix, editing) {
                        actions.push(f.action(|v| edit(make(v), true)));
                    }
                    // Значение поменяли: стрелка-«сброс» возвращает обычное.
                    if editing && (v - default).abs() > 1e-9 {
                        if reset_button(ui).clicked() {
                            actions.push(edit(make(default), true));
                        }
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
    });
    parts::parts_card(ui, path, node, editing, actions);
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

/// Карточка с заголовком; щелчок по заголовку сворачивает её (запоминается по названию).
pub(crate) fn card(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
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

/// Маленькая круговая стрелка «сбросить к обычному значению».
fn reset_button(ui: &mut Ui) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    let color = if resp.hovered() {
        Color32::WHITE
    } else {
        MUTED
    };
    let c = r.center();
    let s = Stroke::new(1.3, color);
    let pts: Vec<Pos2> = (0..=16)
        .map(|k| {
            let t = 0.6 + k as f32 * 0.32;
            c + vec2(t.cos(), t.sin()) * 5.0
        })
        .collect();
    let tip = pts[0];
    ui.painter().add(egui::Shape::line(pts, s));
    ui.painter().line_segment([tip, tip + vec2(3.0, -1.0)], s);
    ui.painter().line_segment([tip, tip + vec2(-0.5, -3.2)], s);
    resp.on_hover_text("Вернуть обычное значение")
}

pub(crate) fn prop(ui: &mut Ui, name: &str) {
    ui.label(RichText::new(name).color(TEXT_DIM));
}

/// Строка инспектора: подпись, значение, шаг мыши, единица, какая это правка.
type Row = (&'static str, f64, f64, &'static str, fn(f64) -> Change);

/// Что сделали с числовым полем в этом кадре.
pub(crate) enum FieldEdit {
    /// Новое значение; `commit: false` — его ещё тянут мышью.
    Value(f64, bool),
    /// Перетаскивание кончилось на прошлом значении.
    End,
}

impl FieldEdit {
    /// Действие для цикла окна; `make` строит правку с готовым значением.
    pub(crate) fn action(self, make: impl FnOnce(f64) -> Action) -> Action {
        match self {
            FieldEdit::Value(v, commit) => match make(v) {
                Action::Edit { edit, .. } => Action::Edit { edit, commit },
                other => other,
            },
            FieldEdit::End => Action::EndEdit,
        }
    }
}

/// Сколько щелчков колеса мыши в этом кадре (плюс — от себя, приблизить).
fn wheel_steps(i: &egui::InputState) -> f32 {
    i.events
        .iter()
        .map(|e| match e {
            egui::Event::MouseWheel { unit, delta, .. } => match unit {
                egui::MouseWheelUnit::Line => delta.y,
                egui::MouseWheelUnit::Point => delta.y / 50.0,
                egui::MouseWheelUnit::Page => delta.y * 4.0,
            },
            _ => 0.0,
        })
        .sum()
}

fn endless_id() -> Id {
    Id::unique("editor-endless-drag")
}

/// В этом кадре мышью тянут число или камеру: окно не даёт курсору упереться в край
/// экрана (перепрыгивает на другую сторону), и тянуть можно сколько угодно.
pub(crate) fn endless_drag(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(endless_id(), true));
}

/// Тянули ли в прошлом кадре что-то без края экрана (и забыть об этом).
pub(crate) fn take_endless_drag(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(endless_id()))
        .unwrap_or(false)
}

/// Кнопка готового вида: кубик, у которого выделена грань, на которую смотрит камера
/// (у невидимых граней — пунктир); название при наведении.
/// Кнопка режима ручек значком: крест стрелок — двигать, дуга со стрелкой — повернуть,
/// квадрат со стрелкой из угла — размер.
fn gizmo_button(ui: &mut Ui, mode: GizmoMode, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 24.0), Sense::click());
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, RADIUS_S, ACCENT.gamma_multiply(0.25));
    } else if resp.hovered() {
        painter.rect_filled(rect, RADIUS_S, HOVER);
    }
    let c = rect.center();
    let col = if on || resp.hovered() { TEXT } else { MUTED };
    let stroke = Stroke::new(1.5, col);
    let head = |tip: Pos2, dir: Vec2| {
        let side = vec2(-dir.y, dir.x);
        egui::Shape::convex_polygon(
            vec![
                tip,
                tip - dir * 4.0 + side * 3.0,
                tip - dir * 4.0 - side * 3.0,
            ],
            col,
            Stroke::NONE,
        )
    };
    match mode {
        GizmoMode::Move => {
            for d in [
                vec2(1.0, 0.0),
                vec2(-1.0, 0.0),
                vec2(0.0, 1.0),
                vec2(0.0, -1.0),
            ] {
                painter.line_segment([c, c + d * 6.0], stroke);
                painter.add(head(c + d * 9.0, d));
            }
        }
        GizmoMode::Turn => {
            let arc: Vec<Pos2> = (0..=20)
                .map(|k| {
                    let t = 0.6 + k as f32 / 20.0 * 4.6;
                    c + vec2(t.cos(), -t.sin()) * 7.0
                })
                .collect();
            let end = arc[arc.len() - 1];
            let before = arc[arc.len() - 2];
            painter.add(egui::Shape::line(arc, stroke));
            painter.add(head(
                end + (end - before).normalized() * 3.0,
                (end - before).normalized(),
            ));
        }
        GizmoMode::Size => {
            let r = Rect::from_center_size(c + vec2(-2.0, 2.0), vec2(10.0, 10.0));
            painter.rect_stroke(r, 1, stroke, egui::StrokeKind::Middle);
            let from = r.right_top();
            let d = vec2(1.0, -1.0).normalized();
            painter.line_segment([from, from + d * 4.0], stroke);
            painter.add(head(from + d * 8.0, d));
        }
    }
    resp
}

fn angle_button(ui: &mut Ui, angle: ShapeAngle) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 24.0), Sense::click());
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, RADIUS_S, HOVER);
    }
    let c = rect.center() + vec2(0.0, 0.5);
    let k = 5.2;
    let at = |x: f32, y: f32, z: f32| c + vec2((x - z) * 0.866 * k, ((x + z) * 0.5 - y) * k);
    let face = |axis: usize, sign: f32| -> Vec<Pos2> {
        let quad = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        quad.iter()
            .map(|&(a, b)| match axis {
                0 => at(sign, a, b),
                1 => at(a, sign, b),
                _ => at(a, b, sign),
            })
            .collect()
    };
    let line = if resp.hovered() { TEXT } else { MUTED };
    let hot = if resp.hovered() {
        Color32::WHITE
    } else {
        ACCENT
    };
    let fill = hot.gamma_multiply(0.45);
    let lit = match angle {
        ShapeAngle::Front => vec![(2, 1.0)],
        ShapeAngle::Back => vec![(2, -1.0)],
        ShapeAngle::Left => vec![(0, -1.0)],
        ShapeAngle::Right => vec![(0, 1.0)],
        ShapeAngle::Top => vec![(1, 1.0)],
        ShapeAngle::Overview => vec![(0, 1.0), (1, 1.0), (2, 1.0)],
    };
    for &(axis, sign) in &lit {
        if sign < 0.0 {
            let pts = face(axis, sign);
            painter.add(egui::Shape::convex_polygon(pts.clone(), fill, Stroke::NONE));
            let mut ring = pts;
            ring.push(ring[0]);
            painter.extend(egui::Shape::dashed_line(
                &ring,
                Stroke::new(1.0, hot),
                2.0,
                1.5,
            ));
        }
    }
    for (axis, sign) in [(0, 1.0), (1, 1.0), (2, 1.0)] {
        let on = lit.contains(&(axis, sign));
        painter.add(egui::Shape::convex_polygon(
            face(axis, sign),
            if on { fill } else { Color32::TRANSPARENT },
            Stroke::new(1.0, if on { hot } else { line }),
        ));
    }
    resp.on_hover_text(angle.label())
}

/// Меню «Качество» подставки: время кадра на видеокарте и самые долгие проходы,
/// разрешение 3D-кадра, режим «треугольники».
fn quality_menu(ui: &mut Ui, shape: &ShapeInfo, actions: &mut Vec<Action>) {
    let total = shape
        .passes
        .first()
        .map_or(String::new(), |(_, ms)| format!(" · {ms:.1} мс"));
    ui.menu_button(
        RichText::new(format!("Качество{total}")).size(FONT_CHIP),
        |ui| {
            ui.set_min_width(260.0);
            ui.label(RichText::new("Видеокарта на кадр, среднее за секунду").color(TEXT_DIM));
            if shape.passes.is_empty() {
                ui.label(RichText::new("замеров нет").color(MUTED));
            }
            for (i, (name, ms)) in shape.passes.iter().enumerate() {
                ui.horizontal(|ui| {
                    let name = if i == 0 { "весь кадр" } else { name };
                    ui.label(RichText::new(name).color(if i == 0 { TEXT } else { TEXT_DIM }));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(format!("{ms:.2} мс")).monospace().color(TEXT));
                    });
                });
            }
            ui.separator();
            ui.label(RichText::new("Разрешение 3D").color(TEXT_DIM));
            ui.horizontal(|ui| {
                for s in reyvec_gpu::SpaceScale::ALL {
                    if ui.selectable_label(shape.scale == s, s.title()).clicked() {
                        actions.push(Action::SpaceScale(s));
                    }
                }
            });
            let mut tri = shape.triangles;
            if ui
                .checkbox(&mut tri, "Треугольники")
                .on_hover_text(
                    "Формы рисуются сетками, тени — картами: быстро на слабых видеокартах, \
                     но края чуть грубее",
                )
                .changed()
            {
                actions.push(Action::SpaceTriangles(tri));
            }
            let mut aa = shape.post_aa;
            if ui
                .checkbox(&mut aa, "Сглаживание по картинке")
                .on_hover_text(
                    "Края сглаживаются по готовой картинке, а не лучами: кадр заметно \
                     дешевле, тонкие края чуть мягче",
                )
                .changed()
            {
                actions.push(Action::PostAa(aa));
            }
        },
    )
    .response
    .on_hover_text("Сколько стоит кадр и как его удешевить");
}

/// Кнопка режима новой детали значком: два круга — объединение, круг с вырезом —
/// вырезание, общая часть кругов — пересечение. Название и пояснение при наведении.
fn combine_button(ui: &mut Ui, how: shape_edit::Combine, on: bool) -> Response {
    use shape_edit::Combine;
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 24.0), Sense::click());
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, RADIUS_S, SELECT_BG);
    } else if resp.hovered() {
        painter.rect_filled(rect, RADIUS_S, HOVER);
    }
    let hot = if on {
        SELECT
    } else if resp.hovered() {
        TEXT
    } else {
        MUTED
    };
    let fill = hot.gamma_multiply(0.55);
    let c = rect.center();
    let (a, b, r) = (c - vec2(3.2, 0.0), c + vec2(3.2, 0.0), 6.0);
    let arc = |o: Pos2, from: f32, to: f32| -> Vec<Pos2> {
        (0..=12)
            .map(|k| {
                let t = from + (to - from) * k as f32 / 12.0;
                o + vec2(t.cos(), t.sin()) * r
            })
            .collect()
    };
    let stroke = Stroke::new(1.0, hot);
    let half = (3.2f32 / r).acos();
    match how {
        Combine::Add => {
            painter.circle(a, r, fill, Stroke::NONE);
            painter.circle(b, r, fill, Stroke::NONE);
            painter.circle_stroke(a, r, stroke);
            painter.circle_stroke(b, r, stroke);
        }
        Combine::Cut => {
            painter.circle(a, r, fill, stroke);
            let bg = if on {
                SELECT_BG
            } else if resp.hovered() {
                HOVER
            } else {
                CARD
            };
            painter.circle_filled(b, r, bg);
            let mut ring = arc(b, 0.0, std::f32::consts::TAU);
            ring.push(ring[0]);
            painter.extend(egui::Shape::dashed_line(&ring, stroke, 2.0, 1.5));
        }
        Combine::Intersect => {
            let mut lens = arc(a, -half, half);
            lens.extend(arc(
                b,
                std::f32::consts::PI - half,
                std::f32::consts::PI + half,
            ));
            painter.add(egui::Shape::convex_polygon(lens, fill, Stroke::NONE));
            painter.circle_stroke(a, r, Stroke::new(1.0, hot.gamma_multiply(0.6)));
            painter.circle_stroke(b, r, Stroke::new(1.0, hot.gamma_multiply(0.6)));
        }
    }
    let hint = match how {
        Combine::Add => "Добавить: новая деталь прибавляется к модели",
        Combine::Cut => "Вырезать: новая деталь вырезается из выбранной части",
        Combine::Intersect => "Пересечь: от выбранной части остаётся общее с новой деталью",
    };
    resp.on_hover_text(hint)
}

/// Число с кнопками «−» и «+» по бокам: тянуть мышью (без края экрана), ввести с
/// клавиатуры или шагнуть на `step`. Не меньше `min`.
fn step_field(
    ui: &mut Ui,
    v: f64,
    speed: f64,
    step: f64,
    min: f64,
    mark: Option<Color32>,
) -> Option<FieldEdit> {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        let small = |ui: &mut Ui, text: &str, hint: String| {
            ui.add(
                egui::Button::new(RichText::new(text).size(FONT_SMALL).color(TEXT_DIM))
                    .min_size(vec2(15.0, 20.0))
                    .corner_radius(RADIUS_S),
            )
            .on_hover_text(hint)
        };
        let s = ron_edit::num(step);
        // В строке справа налево виджеты встают в обратном порядке.
        let flip = ui.layout().prefer_right_to_left();
        for k in 0..3 {
            match (k, flip) {
                (0, false) | (2, true) => {
                    if small(ui, "−", format!("Меньше на {s}")).clicked() {
                        out = Some(FieldEdit::Value((v - step).max(min), true));
                    }
                }
                (1, _) => {
                    if let Some(e) = num_field(ui, v, speed, mark, "", true) {
                        out = Some(match e {
                            FieldEdit::Value(x, c) => FieldEdit::Value(x.max(min), c),
                            end => end,
                        });
                    }
                }
                _ => {
                    if small(ui, "+", format!("Больше на {s}")).clicked() {
                        out = Some(FieldEdit::Value((v + step).max(min), true));
                    }
                }
            }
        }
    });
    out
}

/// Число, которое тянут мышью или вводят с клавиатуры; `mark` — цветная полоска слева.
pub(crate) fn num_field(
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
    if r.dragged() {
        endless_drag(ui.ctx());
    }
    if r.changed() && x != v {
        Some(FieldEdit::Value(x, !r.dragged()))
    } else if r.drag_stopped() {
        Some(FieldEdit::End)
    } else {
        None
    }
}

/// Полоса над журналом: что показывать и «Очистить».
fn console_bar(ui: &mut Ui, filter: &mut LogFilter, log: &mut VecDeque<(Color32, String)>) {
    let lines = log.len();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP_XS;
        for (f, name) in [
            (LogFilter::All, "Все"),
            (LogFilter::Warnings, "Предупреждения"),
            (LogFilter::Errors, "Ошибки"),
        ] {
            if ui
                .selectable_label(*filter == f, RichText::new(name).size(FONT_SMALL))
                .clicked()
            {
                *filter = f;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let clear = ui
                .add_enabled(
                    lines > 0,
                    egui::Button::new(RichText::new("Очистить").size(FONT_SMALL)),
                )
                .clicked();
            if clear {
                log.clear();
            }
            ui.label(
                RichText::new(format!("строк: {lines}"))
                    .size(FONT_SMALL)
                    .color(MUTED),
            );
        });
    });
    ui.add_space(GAP_S);
}

/// Подходит ли строка журнала под фильтр (важность видна по цвету строки).
fn log_shown(filter: LogFilter, color: Color32) -> bool {
    use reyvec_app::for_editor::{GOLD, RED};
    match filter {
        LogFilter::All => true,
        LogFilter::Warnings => color == GOLD || color == RED,
        LogFilter::Errors => color == RED,
    }
}

fn console(ui: &mut Ui, log: &VecDeque<(Color32, String)>, filter: LogFilter) {
    let mut any = false;
    ui.spacing_mut().item_spacing.y = 2.0;
    for (color, line) in log.iter().filter(|(c, _)| log_shown(filter, *c)) {
        any = true;
        ui.label(RichText::new(line).monospace().color(*color));
    }
    if !any {
        let text = match filter {
            LogFilter::All => "Сообщений пока нет",
            LogFilter::Warnings => "Предупреждений нет",
            LogFilter::Errors => "Ошибок нет",
        };
        ui.label(RichText::new(text).color(MUTED));
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

/// Карточка «Ручки модели» (М.6).
#[derive(Default)]
struct ParamsUi {
    /// Идёт обучение ручки.
    teach: Option<Teach>,
    /// Что вышло у «Запомнить» или «Научить» (текст, это ошибка).
    note: Option<(String, bool)>,
    /// Имя ручки в поле правки (номер ручки, текст), пока поле не отпустили.
    naming: Option<(usize, String)>,
}

/// Обучение ручки: модель, ручка, текст до обучения и значение ручки.
struct Teach {
    model: String,
    name: String,
    before: String,
    target: f32,
}

/// Окно контура в инспекторе (М.5): что тянут и куда смотрит.
#[derive(Default)]
struct ContourUi {
    drag: Option<ContourDrag>,
    /// Точка, для которой открыто меню.
    menu: Option<contour::Grip>,
    /// Как контур лежал в окне в последнем кадре.
    view: Option<ContourView>,
    /// Над чем нажали кнопку (тянуть начинают чуть позже, курсор уже ушёл).
    pressed: Option<contour::Grip>,
}

/// Точку контура тянут: всё считается от контура и вида в начале.
struct ContourDrag {
    grip: contour::Grip,
    contours: Vec<contour::Contour>,
    start: [f64; 2],
    view: ContourView,
    moved: Vec2,
}

/// Как контур лежит в окне: середина и точек экрана на метр.
#[derive(Clone, Copy)]
struct ContourView {
    rect: Rect,
    center: [f64; 2],
    k: f64,
}

impl ContourView {
    fn fit(rect: Rect, lo: [f64; 2], hi: [f64; 2]) -> Self {
        let size = [(hi[0] - lo[0]).max(0.05), (hi[1] - lo[1]).max(0.05)];
        let k = (f64::from(rect.width()) / size[0]).min(f64::from(rect.height()) / size[1]) * 0.8;
        Self {
            rect,
            center: [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5],
            k,
        }
    }

    fn to_screen(self, p: [f64; 2]) -> Pos2 {
        let c = self.rect.center();
        pos2(
            c.x + ((p[0] - self.center[0]) * self.k) as f32,
            c.y - ((p[1] - self.center[1]) * self.k) as f32,
        )
    }

    fn to_model(self, p: Pos2) -> [f64; 2] {
        let c = self.rect.center();
        [
            self.center[0] + f64::from(p.x - c.x) / self.k,
            self.center[1] - f64::from(p.y - c.y) / self.k,
        ]
    }
}

/// Что сделали с контуром за кадр.
#[derive(Debug, PartialEq)]
enum ContourEdit {
    /// Новый контур (`false` — точку ещё тянут).
    Path(String, bool),
    /// Отпустили точку.
    End,
}

/// Шаг сетки: 1, 2 или 5 на степень десяти, не меньше `min` метров.
fn grid_step(min: f64) -> f64 {
    let p = 10f64.powf(min.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * p)
        .find(|s| *s >= min)
        .unwrap_or(10.0 * p)
}

/// Окно контура: сетка в метрах, контур, точки (квадраты) и ручки изгибов (круги).
/// Точки тянут левой кнопкой (Ctrl — по сантиметру), двойной щелчок по линии ставит
/// новую точку, правый щелчок по точке — меню: убрать, изгиб или прямой отрезок.
/// `revolve` — контур выточки: ось вращения — линия X = 0, левее неё контура нет.
fn contour_card(ui: &mut Ui, st: &mut ContourUi, path: &str, revolve: bool) -> Option<ContourEdit> {
    let Some(contours) = contour::parse(path) else {
        ui.label(
            RichText::new("Контур не разобрать: правьте его в блокноте")
                .color(BAD)
                .size(FONT_SMALL),
        );
        return None;
    };
    let width = ui.available_width();
    let (rect, resp) =
        ui.allocate_exact_size(vec2(width, width.min(320.0)), Sense::click_and_drag());
    let view = match &st.drag {
        Some(d) => ContourView { rect, ..d.view },
        None => {
            let (mut lo, mut hi) = contour::bounds(&contours)?;
            // Начало координат (и ось выточки) всегда в окне.
            for i in 0..2 {
                lo[i] = lo[i].min(0.0);
                hi[i] = hi[i].max(0.0);
            }
            ContourView::fit(rect, lo, hi)
        }
    };
    st.view = Some(view);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, RADIUS_S, BG);
    // Сетка.
    let step = grid_step(24.0 / view.k);
    let (lo, hi) = (
        view.to_model(rect.left_bottom()),
        view.to_model(rect.right_top()),
    );
    let grid = Stroke::new(1.0, BORDER);
    let mut x = (lo[0] / step).floor() * step;
    while x <= hi[0] {
        let sx = view.to_screen([x, 0.0]).x;
        painter.vline(sx, rect.y_range(), grid);
        x += step;
    }
    let mut y = (lo[1] / step).floor() * step;
    while y <= hi[1] {
        let sy = view.to_screen([0.0, y]).y;
        painter.hline(rect.x_range(), sy, grid);
        y += step;
    }
    let o = view.to_screen([0.0, 0.0]);
    painter.hline(
        rect.x_range(),
        o.y,
        Stroke::new(1.0, MUTED.gamma_multiply(0.6)),
    );
    if revolve {
        // Левее оси контура не бывает: там тень, ось — пунктир.
        let left = Rect::from_min_max(rect.min, pos2(o.x.max(rect.left()), rect.bottom()));
        painter.rect_filled(left, 0, BAD.gamma_multiply(0.06));
        painter.extend(egui::Shape::dashed_line(
            &[pos2(o.x, rect.top()), pos2(o.x, rect.bottom())],
            Stroke::new(1.5, ACCENT),
            6.0,
            4.0,
        ));
        painter.text(
            pos2(o.x + 4.0, rect.top() + 4.0),
            egui::Align2::LEFT_TOP,
            "ось вращения",
            egui::FontId::proportional(FONT_SMALL),
            ACCENT,
        );
    } else {
        painter.vline(
            o.x,
            rect.y_range(),
            Stroke::new(1.0, MUTED.gamma_multiply(0.6)),
        );
    }
    painter.text(
        rect.right_bottom() - vec2(4.0, 4.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("клетка {} см", ron_edit::num(step * 100.0)),
        egui::FontId::proportional(FONT_SMALL),
        MUTED,
    );
    // Контур и линии ручек.
    for c in &contours {
        for (ni, n) in c.nodes.iter().enumerate() {
            let Some(seg) = contour::segment(c, ni) else {
                continue;
            };
            let steps = if n.seg == contour::Seg::Line { 1 } else { 24 };
            let pts: Vec<Pos2> = contour::sample(seg, steps)
                .into_iter()
                .map(|p| view.to_screen(p))
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(2.0, TEXT)));
            let prev = if ni > 0 {
                c.nodes[ni - 1].p
            } else {
                c.nodes[c.nodes.len() - 1].p
            };
            let thin = Stroke::new(1.0, MUTED);
            match n.seg {
                contour::Seg::Line => {}
                contour::Seg::Quad(q) => {
                    painter.line_segment([view.to_screen(prev), view.to_screen(q)], thin);
                    painter.line_segment([view.to_screen(q), view.to_screen(n.p)], thin);
                }
                contour::Seg::Cubic(a, b) => {
                    painter.line_segment([view.to_screen(prev), view.to_screen(a)], thin);
                    painter.line_segment([view.to_screen(b), view.to_screen(n.p)], thin);
                }
            }
        }
    }
    // Точки и ручки.
    let grips = contour::grips(&contours);
    let hover = resp.hover_pos().and_then(|h| {
        grips
            .iter()
            .map(|(g, p)| (*g, view.to_screen(*p).distance(h)))
            .filter(|(_, d)| *d < 9.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(g, _)| g)
    });
    let active = st.drag.as_ref().map(|d| d.grip).or(hover);
    for (g, p) in &grips {
        let s = view.to_screen(*p);
        let lit = active == Some(*g);
        if g.kind == 0 {
            let c = if lit { Color32::WHITE } else { ACCENT };
            painter.rect_filled(Rect::from_center_size(s, vec2(8.0, 8.0)), 1, c);
        } else {
            let c = if lit { Color32::WHITE } else { SELECT };
            painter.circle_filled(s, 4.0, c);
        }
    }
    if hover.is_some() || st.drag.is_some() {
        ui.ctx().set_cursor_icon(if st.drag.is_some() {
            CursorIcon::Grabbing
        } else {
            CursorIcon::Grab
        });
    }
    let mut out = None;
    if resp.hovered() && ui.input(|i| i.pointer.primary_pressed()) {
        st.pressed = hover;
    }
    // Тянут точку или ручку.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(g) = st.pressed.take()
        && let Some(start) = contour::grip_at(&contours, g)
    {
        st.drag = Some(ContourDrag {
            grip: g,
            contours: contours.clone(),
            start,
            view,
            moved: Vec2::ZERO,
        });
    }
    if let Some(d) = st.drag.as_mut() {
        if resp.dragged_by(egui::PointerButton::Primary) {
            d.moved += resp.drag_delta();
            let mut p = [
                d.start[0] + f64::from(d.moved.x) / d.view.k,
                d.start[1] - f64::from(d.moved.y) / d.view.k,
            ];
            if ui.input(|i| i.modifiers.command) {
                p = p.map(|v| (v * 100.0).round() / 100.0);
            }
            if revolve && d.grip.kind == 0 {
                p[0] = p[0].max(0.0);
            }
            let mut c = d.contours.clone();
            contour::set_grip(&mut c, d.grip, p);
            out = Some(ContourEdit::Path(contour::to_path(&c), false));
        } else {
            st.drag = None;
            out = Some(ContourEdit::End);
        }
    }
    // Двойной щелчок по линии — новая точка.
    if resp.double_clicked()
        && hover.is_none()
        && let Some(h) = resp.interact_pointer_pos()
        && let Some((ci, ni, t, dist)) = contour::nearest_segment(&contours, view.to_model(h))
        && dist * view.k < 10.0
    {
        let mut c = contours.clone();
        if contour::insert(&mut c, ci, ni, t).is_some() {
            out = Some(ContourEdit::Path(contour::to_path(&c), true));
        }
    }
    // Правый щелчок по точке — её меню.
    if resp.secondary_clicked() {
        st.menu = hover.filter(|g| g.kind == 0);
    }
    if let Some(g) = st.menu {
        let seg = contours
            .get(g.contour)
            .and_then(|c| c.nodes.get(g.node))
            .map(|n| n.seg);
        let has_seg = contours
            .get(g.contour)
            .and_then(|c| contour::segment(c, g.node))
            .is_some();
        let mut pick = None;
        resp.context_menu(|ui| {
            if ui.button("Убрать точку").clicked() {
                pick = Some(0);
                ui.close();
            }
            let curve = seg.is_some_and(|s| s != contour::Seg::Line);
            if has_seg {
                let name = if curve {
                    "Прямой отрезок к точке"
                } else {
                    "Изгиб к точке"
                };
                if ui.button(name).clicked() {
                    pick = Some(if curve { 2 } else { 1 });
                    ui.close();
                }
            }
        });
        if let Some(what) = pick {
            let mut c = contours.clone();
            let changed = match what {
                0 => contour::remove(&mut c, g.contour, g.node),
                1 => {
                    contour::make_curve(&mut c, g.contour, g.node);
                    true
                }
                _ => {
                    contour::make_line(&mut c, g.contour, g.node);
                    true
                }
            };
            if changed {
                out = Some(ContourEdit::Path(contour::to_path(&c), true));
            }
            st.menu = None;
        }
    }
    ui.label(
        RichText::new(
            "Тяните точки (Ctrl — по сантиметру); двойной щелчок по линии — новая точка; \
             правый щелчок по точке — убрать или изогнуть",
        )
        .color(MUTED)
        .size(FONT_SMALL),
    );
    out
}

/// Что делают ручки выбранной детали на подставке.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GizmoMode {
    /// Стрелки X, Y, Z двигают деталь.
    #[default]
    Move,
    /// Кольца поворачивают деталь вокруг осей её родителя.
    Turn,
    /// Ручки меняют размер: у коробки — по каждой её оси, квадрат — всю деталь.
    Size,
}

impl GizmoMode {
    pub(crate) const ALL: [GizmoMode; 3] = [GizmoMode::Move, GizmoMode::Turn, GizmoMode::Size];

    fn hint(self) -> &'static str {
        match self {
            GizmoMode::Move => "Двигать деталь стрелками X, Y, Z (W). Ctrl — по сантиметру",
            GizmoMode::Turn => "Поворачивать деталь кольцами вокруг осей (E). Ctrl — по 15°",
            GizmoMode::Size => {
                "Менять размер (R): ручки на осях растягивают деталь по одной оси, квадрат \
                 — всю сразу. Ctrl — ровные шаги"
            }
        }
    }
}

/// Ручка детали.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    /// Стрелка вдоль оси родителя.
    Move(usize),
    /// Кольцо вокруг оси родителя.
    Turn(usize),
    /// Растянуть деталь по её оси.
    Size(usize),
    /// Масштаб всей детали.
    Scale,
}

/// Числа детали в начале правки ручкой.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GizmoStart {
    at: [f64; 3],
    turn: [f64; 3],
    scale: f64,
}

/// Ручки выбранной детали на экране в этом кадре.
struct Gizmo {
    path: Vec<usize>,
    start: GizmoStart,
    /// Середина детали на экране.
    origin: Pos2,
    handles: Vec<GizmoHandle>,
    /// Ручка под мышью.
    hot: Option<Handle>,
}

struct GizmoHandle {
    handle: Handle,
    /// Куда тянуть на экране (у колец — знак поворота в `x`).
    dir: Vec2,
    /// Точек экрана на единицу числа вдоль `dir`.
    px_per_unit: f32,
}

/// Деталь тянут за ручку: всё считается от текста и чисел в начале, чтобы запись в
/// файл с задержкой не теряла движение.
struct GizmoDrag {
    handle: Handle,
    path: Vec<usize>,
    text: String,
    start: GizmoStart,
    dir: Vec2,
    px_per_unit: f32,
    origin: Pos2,
    /// Где нажали.
    pointer: Pos2,
    /// Сколько протянули с нажатия.
    moved: Vec2,
}

impl GizmoDrag {
    /// Текст модели после того, как ручку протянули на `moved` (`snap` — ровные шаги).
    fn edit(&self, part: &shape_edit::Part, snap: bool) -> Result<String, ron_edit::EditError> {
        let s = self.start;
        let along = self.moved.dot(self.dir) / self.px_per_unit;
        let round = |v: f64, step: f64| if snap { (v / step).round() * step } else { v };
        match self.handle {
            Handle::Move(i) => {
                let mut at = s.at;
                at[i] = round(at[i] + f64::from(along), 0.01);
                shape_edit::set_moved(&self.text, part, "at", &shape_edit::triple(at))
            }
            Handle::Turn(i) => {
                let a = self.pointer - self.origin;
                let b = a + self.moved;
                // Экран смотрит вниз по y: угол по часовой — минус.
                let delta = -(b.y.atan2(b.x) - a.y.atan2(a.x)) * self.dir.x;
                let delta = round(f64::from(delta).to_degrees(), 15.0);
                let turn = turned(s.turn, i, delta);
                shape_edit::set_moved(&self.text, part, "turn", &shape_edit::triple(turn))
            }
            Handle::Size(i) => {
                // Конец ручки идёт за курсором, деталь растягивается по оси в обе стороны.
                let mut f = [1.0; 3];
                f[i] = round(1.0 + f64::from(along), 0.05).max(0.05);
                shape_edit::stretch(&self.text, part, f)
            }
            Handle::Scale => {
                let k = f64::from(self.moved.dot(self.dir) / 120.0).exp();
                let scale = round(s.scale * k, 0.05).max(0.01);
                shape_edit::set_moved(&self.text, part, "scale", &ron_edit::num(scale))
            }
        }
    }
}

/// Поворот детали `turn` (градусы, как в файле), повёрнутый ещё на `degrees` вокруг оси
/// `axis` её родителя.
fn turned(turn: [f64; 3], axis: usize, degrees: f64) -> [f64; 3] {
    let old = reyvec_sdf::shape3d::rotation(turn.map(|v| v as f32));
    let mut e = glam::Vec3::ZERO;
    e[axis] = 1.0;
    let new = glam::Mat3::from_axis_angle(e, (degrees as f32).to_radians()) * old;
    // rotation() — Rz · Ry · Rx.
    let (z, y, x) = glam::Quat::from_mat3(&new).to_euler(glam::EulerRot::ZYX);
    let clean = |r: f32| {
        let d = (f64::from(r.to_degrees()) * 1000.0).round() / 1000.0;
        if d == 0.0 { 0.0 } else { d }
    };
    [clean(x), clean(y), clean(z)]
}

/// Расстояние от точки до отрезка, точки экрана.
fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Действие над деталью модели из меню дерева или с клавиатуры./// Действие над деталью модели из меню дерева или с клавиатуры.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PartCmd {
    Focus,
    Duplicate,
    Up,
    Down,
    Wrap(shape_edit::Wrap),
    Unwrap,
    Split,
    Remove,
}

/// Что сделали в дереве модели за кадр.
#[derive(Default)]
struct TreeOut {
    pick: Option<Vec<usize>>,
    toggle: Option<Vec<usize>>,
    cmd: Option<PartCmd>,
    hide: Option<(Vec<usize>, bool)>,
    lock: Option<Vec<usize>>,
}

/// Дерево частей модели (М.3): что нужно строкам.
struct ShapeTree<'a> {
    text: &'a str,
    root: &'a shape_edit::Part,
    selected: &'a [usize],
    closed: &'a HashSet<Vec<usize>>,
    hidden: &'a [Vec<usize>],
    locked: &'a HashSet<Vec<usize>>,
    materials: &'a [String],
    out: &'a mut TreeOut,
}

impl ShapeTree<'_> {
    fn rows(&mut self, ui: &mut Ui, part: &shape_edit::Part, path: &mut Vec<usize>) {
        let mut title = shape_edit::label(&part.kind).to_owned();
        if let Some(o) = part.painted {
            let m = shape_edit::read_num(self.text, Some(o), "material", 0.0).round() as usize;
            if let Some(name) = self.materials.get(m) {
                title = format!("{title} · {name}");
            }
        }
        let closed = self.closed.contains(path.as_slice());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_XS;
            ui.add_space(path.len() as f32 * 14.0);
            let (arrow, resp) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::click());
            if !part.children.is_empty() {
                paint_icon(
                    ui.painter(),
                    arrow.center(),
                    if closed {
                        Icon::ChevronRight
                    } else {
                        Icon::ChevronDown
                    },
                    if resp.hovered() { TEXT } else { MUTED },
                );
                if resp
                    .on_hover_text(if closed {
                        "Развернуть"
                    } else {
                        "Свернуть"
                    })
                    .clicked()
                {
                    self.out.toggle = Some(path.clone());
                }
            }
            let row = ui.selectable_label(path.as_slice() == self.selected, title);
            if row.clicked() {
                self.out.pick = Some(path.clone());
            }
            if row.double_clicked() {
                self.out.pick = Some(path.clone());
                self.out.cmd = Some(PartCmd::Focus);
            }
            if row.secondary_clicked() {
                self.out.pick = Some(path.clone());
            }
            row.context_menu(|ui| self.menu(ui, part, path));
            // Глаз и замок справа: видны всегда, включённые — ярче.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let hidden = self.hidden.iter().any(|p| p == path);
                let locked = self.locked.contains(path.as_slice());
                let icon = |ui: &mut Ui, on: bool, can: bool| {
                    let (r, resp) = ui.allocate_exact_size(
                        vec2(16.0, 18.0),
                        if can { Sense::click() } else { Sense::hover() },
                    );
                    let color = if !can {
                        Color32::TRANSPARENT
                    } else if resp.hovered() {
                        Color32::WHITE
                    } else if on {
                        ACCENT
                    } else {
                        MUTED.gamma_multiply(0.6)
                    };
                    (r, resp, color)
                };
                let (r, resp, color) = icon(ui, locked, true);
                paint_lock(ui.painter(), r.center(), locked, color);
                if resp
                    .on_hover_text(if locked {
                        "Закреплена: числа не правятся. Щёлкните, чтобы открепить"
                    } else {
                        "Закрепить: числа детали не будут правиться"
                    })
                    .clicked()
                {
                    self.out.lock = Some(path.clone());
                }
                let can_hide = part.removable;
                let (r, resp, color) = icon(ui, hidden, can_hide);
                paint_eye(ui.painter(), r.center(), !hidden, color);
                if can_hide
                    && resp
                        .on_hover_text(if hidden {
                            "Спрятана на подставке (в файле есть). Щёлкните, чтобы показать"
                        } else {
                            "Спрятать на подставке (в файле останется)"
                        })
                        .clicked()
                {
                    self.out.hide = Some((path.clone(), !hidden));
                }
            });
        });
        if closed {
            return;
        }
        for (i, c) in part.children.iter().enumerate() {
            path.push(i);
            self.rows(ui, c, path);
            path.pop();
        }
    }

    /// Меню детали по правой кнопке.
    fn menu(&mut self, ui: &mut Ui, part: &shape_edit::Part, path: &[usize]) {
        let sibling = |d: isize| {
            let (&last, up) = path.split_last()?;
            let mut p = up.to_vec();
            p.push(last.checked_add_signed(d)?);
            self.root.at(&p).map(|s| s.removable)
        };
        let mut pick = None;
        let mut item = |ui: &mut Ui, on: bool, name: &str, keys: &str, cmd: PartCmd| {
            let b = egui::Button::new(name).shortcut_text(keys);
            if ui.add_enabled(on, b).clicked() {
                pick = Some(cmd);
                ui.close();
            }
        };
        item(ui, true, "Показать крупно", "F", PartCmd::Focus);
        ui.separator();
        item(
            ui,
            part.removable,
            "Повторить рядом",
            "Ctrl+D",
            PartCmd::Duplicate,
        );
        let up = part.removable && sibling(-1) == Some(true);
        let down = part.removable && sibling(1) == Some(true);
        item(ui, up, "Выше в списке", "", PartCmd::Up);
        item(ui, down, "Ниже в списке", "", PartCmd::Down);
        ui.menu_button("Обернуть", |ui| {
            let step = super::shapes::part_bounds(self.text, path)
                .map_or(0.3, |(lo, hi)| f64::from((hi.x - lo.x) * 1.25).max(0.05));
            let step = (step * 100.0).round() / 100.0;
            item(
                ui,
                true,
                "В объединение",
                "",
                PartCmd::Wrap(shape_edit::Wrap::Union),
            );
            item(
                ui,
                true,
                "В зеркало (две половины)",
                "",
                PartCmd::Wrap(shape_edit::Wrap::Mirror),
            );
            item(
                ui,
                true,
                "В повтор (ряд копий)",
                "",
                PartCmd::Wrap(shape_edit::Wrap::Repeat(step)),
            );
        });
        item(
            ui,
            shape_edit::can_unwrap(part),
            "Снять обёртку",
            "",
            PartCmd::Unwrap,
        );
        item(
            ui,
            shape_edit::can_split(part),
            "Разделить повтор",
            "",
            PartCmd::Split,
        );
        ui.separator();
        item(ui, part.removable, "Удалить", "Delete", PartCmd::Remove);
        if let Some(cmd) = pick {
            self.out.pick = Some(path.to_vec());
            self.out.cmd = Some(cmd);
        }
    }
}

/// Новые три числа и отпустили ли их.
type Vec3Edit = Option<([f64; 3], bool)>;

/// Три числа в строку (x, y, z) с кнопками шага `step`. Возвращает новое значение (и
/// отпустили ли его) или конец перетаскивания.
fn vec3_fields(ui: &mut Ui, v: [f64; 3], speed: f64, step: f64) -> Option<(Vec3Edit, bool)> {
    let marks = [BAD, GOOD, SELECT];
    let mut out = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP_S;
        for i in 0..3 {
            match step_field(ui, v[i], speed, step, f64::NEG_INFINITY, Some(marks[i])) {
                Some(FieldEdit::Value(x, commit)) => {
                    let mut n = v;
                    n[i] = x;
                    out = Some((Some((n, commit)), false));
                }
                Some(FieldEdit::End) => out = out.or(Some((None, true))),
                None => {}
            }
        }
    });
    out
}

/// Путь новой детали: первая часть нового дерева, которой не было в старом (по порядку
/// обхода), иначе прежний путь.
fn newest(old: &shape_edit::Part, new: &shape_edit::Part, fallback: &[usize]) -> Vec<usize> {
    fn walk(p: &shape_edit::Part, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, usize)>) {
        out.push((path.clone(), p.children.len()));
        for (i, c) in p.children.iter().enumerate() {
            path.push(i);
            walk(c, path, out);
            path.pop();
        }
    }
    let (mut a, mut b) = (Vec::new(), Vec::new());
    walk(old, &mut Vec::new(), &mut a);
    walk(new, &mut Vec::new(), &mut b);
    // Деталь легла последней в чей-то список: у этой части детей стало больше.
    for (path, n) in &b {
        let before = a.iter().find(|(p, _)| p == path).map(|x| x.1);
        if before.is_some_and(|m| *n > m) && *n > 0 {
            let mut p = path.clone();
            p.push(n - 1);
            return p;
        }
    }
    fallback.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    static NO_HIDDEN: std::sync::LazyLock<HashSet<Vec<usize>>> =
        std::sync::LazyLock::new(HashSet::new);

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
            hidden: &NO_HIDDEN,
            can_paste: false,
            scenes: &[],
            can_restore: false,
            ejected: false,
            time_scale: 1.0,
            simulating: false,
            shape: None,
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
        ctx.set_fonts(reyvec_app::for_editor::fonts());
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

    #[test]
    fn shape_view_closes_on_esc_and_shows_error() {
        let ctx = new_ctx();
        let mut ui = EditorUi::new(None);
        let mut shape_info = info(None, PlayState::Edit);
        shape_info.shape = Some(ShapeInfo {
            name: "склад/стеллаж",
            spin: true,
            free: false,
            hidden: &[],
            scale: reyvec_gpu::SpaceScale::Full,
            triangles: false,
            post_aa: false,
            in_view: true,
            passes: &[],
            camera: reyvec_sdf::Camera3D::default(),
            shift: [0.0; 3],
            error: Some("нет такого файла"),
            text: "(format: \"reyvec-shape\", version: 1, shape: Union(shapes: [Sphere(radius: 0.3), Box(size: (0.2, 0.2, 0.2))]))",
            preview: &NO_PREVIEW,
            cost: None,
        });
        frame(&ctx, &mut ui, &shape_info, Vec::new());
        assert!(ui.take_actions().is_empty(), "без нажатий нет действий");
        assert!(ui.viewport().is_some(), "окно сцены занято подставкой");
        let esc = key(egui::Key::Escape, egui::Modifiers::NONE);
        frame(&ctx, &mut ui, &shape_info, vec![esc]);
        assert_eq!(ui.take_actions(), vec![Action::CloseShape]);
    }

    #[test]
    fn shape_view_wheel_keys_and_part_commands() {
        let text = "(format: \"reyvec-shape\", version: 1, shape: Union(shapes: [Sphere(radius: 0.3), Box(size: (0.2, 0.2, 0.2))]))";
        let ctx = new_ctx();
        let mut ui = EditorUi::new(None);
        let mut shape_info = info(None, PlayState::Edit);
        shape_info.shape = Some(ShapeInfo {
            name: "шар",
            spin: true,
            free: false,
            hidden: &[],
            scale: reyvec_gpu::SpaceScale::Full,
            triangles: false,
            post_aa: false,
            in_view: true,
            passes: &[],
            camera: reyvec_sdf::Camera3D::default(),
            shift: [0.0; 3],
            error: None,
            text,
            preview: &NO_PREVIEW,
            cost: None,
        });
        frame(&ctx, &mut ui, &shape_info, Vec::new());
        let vp = ui.viewport().expect("окно подставки");
        let at = vp.center();
        // Колесо над подставкой приближает камеру (раньше его забирал интерфейс).
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut ui, &shape_info, pointer(at, None));
        ui.take_actions();
        frame(
            &ctx,
            &mut ui,
            &shape_info,
            vec![egui::Event::PointerMoved(at), wheel],
        );
        assert!(
            ui.take_actions()
                .contains(&Action::ShapeCamera(ShapeCam::Zoom(1.0)))
        );
        // F — выбранная деталь крупно; Ctrl+D — копия рядом, она и выбрана.
        ui.shape_selected = vec![1];
        let f = key(egui::Key::F, egui::Modifiers::NONE);
        frame(&ctx, &mut ui, &shape_info, vec![f]);
        assert!(
            ui.take_actions()
                .contains(&Action::ShapeCamera(ShapeCam::Focus(vec![1])))
        );
        ui.shape_command(text, PartCmd::Duplicate);
        assert_eq!(ui.shape_selected, vec![2]);
        let Some(Action::ShapeText { text: t, commit }) = ui.take_actions().pop() else {
            panic!("нет правки");
        };
        assert!(commit && t.matches("Box(").count() == 2, "{t}");
        ui.shape_selected = vec![1];
        ui.shape_command(text, PartCmd::Up);
        assert_eq!(ui.shape_selected, vec![0]);
        ui.take_actions();
        ui.shape_selected = vec![0];
        ui.shape_command(text, PartCmd::Up);
        assert!(ui.take_actions().is_empty(), "первую выше не поднять");
    }

    #[test]
    fn click_on_the_stand_selects_a_part_and_right_click_opens_its_menu() {
        let text = "(format: \"reyvec-shape\", version: 1, shape: Union(shapes: [Sphere(radius: 0.3), Move(at: (30.0, 0.0, 0.0), shape: Box(size: (0.2, 0.2, 0.2)))]))";
        let ctx = new_ctx();
        let mut ui = EditorUi::new(None);
        let mut shape_info = info(None, PlayState::Edit);
        let mut camera =
            reyvec_sdf::Camera3D::looking_at(glam::Vec3::new(0.0, 0.0, 3.0), glam::Vec3::ZERO);
        camera.far = 100.0;
        shape_info.shape = Some(ShapeInfo {
            name: "шар",
            spin: false,
            free: false,
            hidden: &[],
            scale: reyvec_gpu::SpaceScale::Full,
            triangles: false,
            post_aa: false,
            in_view: true,
            passes: &[],
            camera,
            shift: [0.0; 3],
            error: None,
            text,
            preview: &NO_PREVIEW,
            cost: None,
        });
        frame(&ctx, &mut ui, &shape_info, Vec::new());
        // Середина экрана — шар (камера смотрит на него).
        let mid = ui.viewport().unwrap().center();
        assert!(ui.viewport().unwrap().contains(mid));
        ui.shape_selected = vec![1];
        frame(&ctx, &mut ui, &shape_info, pointer(mid, Some(true)));
        frame(&ctx, &mut ui, &shape_info, pointer(mid, Some(false)));
        assert_eq!(ui.shape_selected, vec![0]);
        // Щелчок мимо модели — выбрана вся модель.
        let empty = ui.viewport().unwrap().left_bottom() + vec2(20.0, -20.0);
        frame(&ctx, &mut ui, &shape_info, pointer(empty, Some(true)));
        frame(&ctx, &mut ui, &shape_info, pointer(empty, Some(false)));
        assert!(ui.shape_selected.is_empty());
        // Закреплённый шар щелчком не выбирается.
        ui.shape_locked.insert(vec![0]);
        frame(&ctx, &mut ui, &shape_info, pointer(mid, Some(true)));
        frame(&ctx, &mut ui, &shape_info, pointer(mid, Some(false)));
        assert!(ui.shape_selected.is_empty());
        ui.shape_locked.clear();
        // Правый щелчок по шару — выбран и для него меню.
        let right = |pressed| egui::Event::PointerButton {
            pos: mid,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &ctx,
            &mut ui,
            &shape_info,
            vec![egui::Event::PointerMoved(mid), right(true)],
        );
        frame(&ctx, &mut ui, &shape_info, vec![right(false)]);
        assert_eq!(ui.shape_menu, Some(vec![0]));
        assert_eq!(ui.shape_selected, vec![0]);
        ui.shape_menu = None;
        ui.take_actions();
        // Стрелка X: тянем вправо — шар уезжает по X (обёрнут в сдвиг), отпустили — конец
        // правки.
        let screen = ui.viewport().expect("окно подставки");
        let p = camera
            .project(
                glam::Vec3::new(0.12, 0.0, 0.0),
                screen.width() / screen.height(),
            )
            .unwrap();
        let on_arrow = pos2(
            screen.left() + (p.x * 0.5 + 0.5) * screen.width(),
            screen.top() + (0.5 - p.y * 0.5) * screen.height(),
        );
        frame(&ctx, &mut ui, &shape_info, pointer(on_arrow, None));
        frame(&ctx, &mut ui, &shape_info, pointer(on_arrow, Some(true)));
        for k in 1..=4 {
            let to = on_arrow + vec2(15.0 * k as f32, 0.0);
            frame(&ctx, &mut ui, &shape_info, pointer(to, None));
        }
        let actions = ui.take_actions();
        let Some(Action::ShapeText {
            text: moved,
            commit,
        }) = actions
            .iter()
            .rev()
            .find(|a| matches!(a, Action::ShapeText { .. }))
            .cloned()
        else {
            panic!("стрелка не тянется: {actions:?}");
        };
        assert!(!commit);
        let x: f64 = moved
            .split("Move(at: (")
            .nth(1)
            .and_then(|r| r.split(',').next())
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| panic!("{moved}"));
        assert!(x > 0.05, "{x}");
        frame(
            &ctx,
            &mut ui,
            &shape_info,
            pointer(on_arrow + vec2(60.0, 0.0), Some(false)),
        );
        frame(&ctx, &mut ui, &shape_info, Vec::new());
        assert!(ui.take_actions().contains(&Action::EndEdit));
    }

    #[test]
    fn turn_and_size_handles_edit_the_part() {
        // Поворот вокруг оси родителя складывается с прежним поворотом.
        let near = |a: [f64; 3], b: [f64; 3]| {
            let (ra, rb) = (
                reyvec_sdf::shape3d::rotation(a.map(|v| v as f32)),
                reyvec_sdf::shape3d::rotation(b.map(|v| v as f32)),
            );
            ra.abs_diff_eq(rb, 1e-4)
        };
        assert_eq!(turned([0.0; 3], 1, 90.0), [0.0, 90.0, 0.0]);
        let t = turned([30.0, 0.0, 0.0], 2, 45.0);
        let want = glam::Mat3::from_rotation_z(45f32.to_radians())
            * glam::Mat3::from_rotation_x(30f32.to_radians());
        assert!(
            reyvec_sdf::shape3d::rotation(t.map(|v| v as f32)).abs_diff_eq(want, 1e-4),
            "{t:?}"
        );
        assert!(near(turned(t, 2, -45.0), [30.0, 0.0, 0.0]));

        let text = "(format: \"reyvec-shape\", version: 1, shape: Union(shapes: [\n    \
                    Box(size: (0.4, 0.2, 0.2)),\n]))\n";
        let root = shape_edit::parse(text).unwrap();
        let part = root.at(&[0]).unwrap().clone();
        let start = GizmoStart {
            at: [0.0; 3],
            turn: [0.0; 3],
            scale: 1.0,
        };
        let drag = |handle, dir: Vec2, moved: Vec2| GizmoDrag {
            handle,
            path: vec![0],
            text: text.to_owned(),
            start,
            dir,
            px_per_unit: 100.0,
            origin: pos2(100.0, 100.0),
            pointer: pos2(150.0, 100.0),
            moved,
        };
        // Ручка размера по X: протянули на 5 % её длины — коробка на 5 % шире.
        let t = drag(Handle::Size(0), vec2(1.0, 0.0), vec2(5.0, 0.0))
            .edit(&part, false)
            .unwrap();
        assert!(t.contains("Box(size: (0.42, 0.2, 0.2))"), "{t}");
        // Кольцо Y: курсор ушёл с «трёх часов» на «двенадцать» — плюс 90° (ось к нам).
        let t = drag(Handle::Turn(1), vec2(1.0, 0.0), vec2(-50.0, -50.0))
            .edit(&part, true)
            .unwrap();
        assert!(t.contains("Move(turn: (0.0, 90.0, 0.0), shape: Box"), "{t}");
        // Квадрат: тянем вправо-вверх — деталь крупнее.
        let t = drag(
            Handle::Scale,
            vec2(1.0, -1.0).normalized(),
            vec2(60.0, -60.0),
        )
        .edit(&part, true)
        .unwrap();
        assert!(t.contains("Move(scale: 2.05, shape: Box"), "{t}");
    }

    #[test]
    fn contour_points_are_dragged_and_added_with_the_mouse() {
        let ctx = egui::Context::default();
        let mut st = ContourUi::default();
        let mut time = 0.0;
        let mut run = |st: &mut ContourUi, path: &str, events: Vec<egui::Event>| {
            time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 500.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut out = None;
            let mut full = ctx.run_ui(input, |ui| out = contour_card(ui, st, path, true));
            full.textures_delta.clear();
            out
        };
        let path = "M 0 0 L 0.4 0 L 0.2 0.5 L 0 0.5 Z";
        assert_eq!(run(&mut st, path, Vec::new()), None);
        let view = st.view.expect("окно контура");
        // Тянем правую нижнюю точку вправо на 10 см.
        let from = view.to_screen([0.4, 0.0]);
        let to = view.to_screen([0.5, 0.0]);
        run(&mut st, path, pointer(from, None));
        run(&mut st, path, pointer(from, Some(true)));
        let mut last = None;
        for k in 1..=4 {
            let at = from + (to - from) * (k as f32 / 4.0);
            if let Some(e) = run(&mut st, path, pointer(at, None)) {
                last = Some(e);
            }
        }
        let Some(ContourEdit::Path(moved, false)) = last else {
            panic!("точка не тянется: {last:?}");
        };
        let c = contour::parse(&moved).unwrap();
        assert!((c[0].nodes[1].p[0] - 0.5).abs() < 0.01, "{moved}");
        assert_eq!(
            run(&mut st, path, pointer(to, Some(false))),
            Some(ContourEdit::End)
        );
        assert_eq!(run(&mut st, path, Vec::new()), None);
        // Точку выточки левее оси не утащить.
        let left = view.to_screen([-0.3, 0.0]);
        run(&mut st, path, pointer(from, Some(true)));
        let mut last = None;
        for k in 1..=4 {
            let at = from + (left - from) * (k as f32 / 4.0);
            if let Some(e) = run(&mut st, path, pointer(at, None)) {
                last = Some(e);
            }
        }
        run(&mut st, path, pointer(left, Some(false)));
        run(&mut st, path, Vec::new());
        let Some(ContourEdit::Path(moved, _)) = last else {
            panic!("{last:?}");
        };
        assert_eq!(contour::parse(&moved).unwrap()[0].nodes[1].p[0], 0.0);
        // Двойной щелчок по низу — новая точка.
        let mid = view.to_screen([0.2, 0.0]);
        let mut added = None;
        for pressed in [true, false, true, false] {
            if let Some(e) = run(&mut st, path, pointer(mid, Some(pressed))) {
                added = Some(e);
            }
        }
        let Some(ContourEdit::Path(added, true)) = added else {
            panic!("точка не добавилась: {added:?}");
        };
        assert_eq!(contour::parse(&added).unwrap()[0].nodes.len(), 5, "{added}");
    }

    #[test]
    fn new_extrude_and_revolve_parts_read_and_stretch() {
        for kind in ["Extrude", "Revolve"] {
            let item = shape_edit::new_part(kind, 0.2, [0.0; 3]);
            let text = format!("(shape: Union(shapes: [{item}]))");
            let root = shape_edit::parse(&text).unwrap();
            let part = root.at(&[0]).unwrap();
            let profile = shape_edit::read_string(&text, part.open, "profile").unwrap();
            assert!(
                reyvec_sdf::profile2d::Profile::new(&profile).is_ok(),
                "{profile}"
            );
            let t = shape_edit::stretch(&text, part, [2.0, 1.0, 1.0]).unwrap();
            let r = shape_edit::parse(&t).unwrap();
            let p = shape_edit::read_string(&t, r.at(&[0]).unwrap().open, "profile").unwrap();
            let (lo, hi) = contour::bounds(&contour::parse(&p).unwrap()).unwrap();
            let (lo0, hi0) = contour::bounds(&contour::parse(&profile).unwrap()).unwrap();
            assert!(
                ((hi[0] - lo[0]) - 2.0 * (hi0[0] - lo0[0])).abs() < 1e-9,
                "{t}"
            );
            let file = format!("(format: \"reyvec-shape\", version: 1, {}", &t[1..]);
            if let Err(e) = reyvec_sdf::shape_model::ModelFile::parse(&file) {
                panic!("{e}\n{file}");
            }
        }
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
    fn parts_card_for_every_node() {
        let ctx = new_ctx();
        let mut ui = EditorUi::new(None);
        let file = SceneFile::parse(reyvec_scene::EXAMPLE).unwrap();
        let info = info(Some(&file), PlayState::Edit);
        // Каждый предмет примера: карточка «Части» рисуется, сама ничего не правит.
        for i in 0..file.nodes.len() {
            ui.selected = Some(vec![i]);
            frame(&ctx, &mut ui, &info, Vec::new());
        }
        ui.selected = None;
        frame(&ctx, &mut ui, &info, Vec::new());
        assert!(ui.take_actions().is_empty());
        assert_eq!(top_of(Some(&[2, 0, 1])), Some(vec![2]));
        assert_eq!(top_of(None), None);
    }

    #[test]
    fn several_selected_group_and_tools() {
        use egui::{Key, Modifiers};
        let ctx = new_ctx();
        let mut ui = EditorUi::new(None);
        let file = SceneFile::parse(reyvec_scene::EXAMPLE).unwrap();
        let info = info(Some(&file), PlayState::Edit);
        ui.select(Some(vec![1]));
        ui.toggle_select(vec![3]);
        ui.toggle_select(vec![2]);
        assert_eq!(ui.selection(), [vec![2], vec![1], vec![3]]);
        ui.toggle_select(vec![1]);
        assert_eq!(ui.selection(), [vec![2], vec![3]]);
        // С несколькими выбранными над сценой полоса выравнивания; Ctrl+G — группа.
        frame(&ctx, &mut ui, &info, Vec::new());
        frame(&ctx, &mut ui, &info, vec![key(Key::G, Modifiers::COMMAND)]);
        frame(
            &ctx,
            &mut ui,
            &info,
            vec![key(Key::G, Modifiers::COMMAND | Modifiers::SHIFT)],
        );
        assert_eq!(
            ui.take_actions(),
            [
                Action::Group(vec![vec![2], vec![3]]),
                Action::Ungroup(vec![2])
            ]
        );
        // T — линейка; закреплённый инспектор не меняется вместе с выбором.
        frame(&ctx, &mut ui, &info, vec![key(Key::T, Modifiers::NONE)]);
        assert_eq!(ui.vp.tool, Tool::Measure);
        ui.ruler = Some(([0.0, 0.0], [0.3, 0.4]));
        ui.pinned = Some(vec![0]);
        ui.select(Some(vec![1]));
        frame(&ctx, &mut ui, &info, Vec::new());
        assert_eq!(ui.pinned, Some(vec![0]));
        ui.structure_changed();
        assert!(ui.pinned.is_none() && ui.more.is_empty());
        assert!(log_shown(LogFilter::Errors, reyvec_app::for_editor::RED));
        assert!(!log_shown(LogFilter::Warnings, TEXT_DIM));
    }

    #[test]
    fn clipboard_keys_and_hierarchy() {
        use egui::{Key, Modifiers};
        let ctx = new_ctx();
        let mut ui = EditorUi::new(None);
        let file = SceneFile::parse(reyvec_scene::EXAMPLE).unwrap();
        let mut info = info(Some(&file), PlayState::Edit);
        info.can_paste = true;
        // Без выбранного — только вставка в конец сцены.
        frame(&ctx, &mut ui, &info, vec![egui::Event::Copy]);
        assert!(ui.take_actions().is_empty());
        ui.selected = Some(vec![1]);
        frame(&ctx, &mut ui, &info, vec![egui::Event::Copy]);
        frame(&ctx, &mut ui, &info, vec![egui::Event::Cut]);
        let pasted = "(name: \"ящик\")".to_owned();
        frame(
            &ctx,
            &mut ui,
            &info,
            vec![egui::Event::Paste(pasted.clone())],
        );
        frame(&ctx, &mut ui, &info, vec![key(Key::D, Modifiers::COMMAND)]);
        frame(
            &ctx,
            &mut ui,
            &info,
            vec![key(Key::Delete, Modifiers::NONE)],
        );
        assert_eq!(
            ui.take_actions(),
            [
                Action::Copy {
                    path: vec![1],
                    cut: false
                },
                Action::Copy {
                    path: vec![1],
                    cut: true
                },
                Action::Paste {
                    after: Some(vec![1]),
                    text: Some(pasted)
                },
                Action::Duplicate(vec![1]),
                Action::Delete(vec![1]),
            ]
        );
        // F2 — переименование в строке иерархии; поиск, скрытые и окно «сохранить?» рисуются.
        frame(&ctx, &mut ui, &info, vec![key(Key::F2, Modifiers::NONE)]);
        assert_eq!(ui.renaming.as_ref().map(|r| r.0.clone()), Some(vec![1]));
        ui.renaming = None;
        ui.search = "нога".into();
        let hidden: HashSet<Vec<usize>> = [vec![0]].into();
        info.hidden = &hidden;
        info.dirty = true;
        ui.ask_close();
        frame(&ctx, &mut ui, &info, Vec::new());
        assert!(ui.asking_close);
        assert_eq!(drop_place(&[2, 1], Drop::Before), Some((vec![2], 1)));
        assert_eq!(drop_place(&[2, 1], Drop::After), Some((vec![2], 2)));
        assert_eq!(drop_place(&[2], Drop::Into), Some((vec![2], usize::MAX)));
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
    /// `cargo test -p reyvec-editor editor_preview -- --ignored`.
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
        // Один предмет скрыт, другой закреплён: в иерархии видны глаз и замок.
        let hidden: HashSet<Vec<usize>> = [vec![4]].into();
        ui.locked.insert(vec![5]);
        let (w, h) = (1600usize, 900usize);
        let mut textures: HashMap<egui::TextureId, (usize, usize, Vec<Color32>)> = HashMap::new();
        // Время только растёт: egui считает по нему анимации и частоту кадров.
        let mut clock = 0.0f64;
        // Мир, Мир во время игры, Ассеты, Сюжет, Логика и Мир с раскрытым меню «ReyVec → Вид».
        // Стеллаж склада для «Форм» (М.3): выбрана коробка ящика.
        let rack =
            std::fs::read_to_string(root.join("scenes/3d/models/склад/стеллаж.shape.ron")).unwrap();
        // Цена стеллажа (М.7) для карточки «Цена модели».
        let rack_cost = crate::editor::cost::measure(
            &reyvec_sdf::shape_model::ModelFile::parse(&rack)
                .unwrap()
                .shape,
        );
        for (ws, play, menu, shape) in [
            (START_WORKSPACE, PlayState::Edit, false, false),
            (START_WORKSPACE, PlayState::Playing, false, false),
            (1, PlayState::Edit, false, false),
            (0, PlayState::Edit, false, false),
            (3, PlayState::Edit, false, false),
            (5, PlayState::Edit, false, false),
            // Пауза в «Мире» снимается с профайлером внизу: широкая панель, много колонок.
            (START_WORKSPACE, PlayState::Paused, false, false),
            (START_WORKSPACE, PlayState::Edit, true, false),
            (START_WORKSPACE, PlayState::Edit, false, true),
        ] {
            ui.shape_selected = vec![3, 0];
            // «Формы» — с ручками размера по осям у выбранной части (М.4г).
            ui.gizmo_mode = GizmoMode::Size;
            if play == PlayState::Paused {
                ui.tabs[ws][Side::Bottom.index()] = 2;
            }
            // Пауза — заодно режим «Физика» со сводкой, игра — «Контуры» с метками.
            ui.ov.mode = match play {
                PlayState::Edit => ViewMode::Lit,
                PlayState::Paused => ViewMode::Physics,
                PlayState::Playing => ViewMode::Outlines,
            };
            ui.ov.show.stats = play != PlayState::Edit;
            ui.ov.show.lights = play == PlayState::Playing;
            ui.ov.show.bones = play == PlayState::Playing;
            ui.workspace = ws;
            // С меню — другой выбранный: ящик, у него в инспекторе видны части.
            let pick = if menu {
                file.nodes
                    .iter()
                    .position(|n| n.body.is_some())
                    .unwrap_or(hero)
            } else {
                hero
            };
            ui.selected = Some(vec![pick]);
            // В первой картинке выбраны ещё две монеты: видна полоса выравнивания.
            ui.more = if ws == START_WORKSPACE && play == PlayState::Edit && !menu {
                file.nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.name.starts_with("монета"))
                    .take(2)
                    .map(|(i, _)| vec![i])
                    .collect()
            } else {
                Vec::new()
            };
            if ws == 5 {
                // Сборка: профайлер справа; и он же внизу «Мира» в отдельной картинке ниже.
                ui.frames = (0..FRAME_HISTORY)
                    .map(|i| 3.0 + (i as f32 * 0.21).sin() + if i % 37 == 0 { 3.5 } else { 0.0 })
                    .collect();
            }
            let mut info = info(Some(&file), play);
            info.view = Some(hero_view(&file, hero));
            info.dirty = true;
            info.hidden = &hidden;
            if shape {
                info.shape = Some(ShapeInfo {
                    name: "склад/стеллаж",
                    spin: true,
                    free: false,
                    hidden: &[],
                    scale: reyvec_gpu::SpaceScale::Full,
                    triangles: false,
                    post_aa: false,
                    in_view: true,
                    passes: &[],
                    camera: reyvec_sdf::Camera3D::looking_at(
                        glam::Vec3::new(1.6, 1.4, 2.4),
                        glam::Vec3::new(0.0, 0.8, 0.0),
                    ),
                    shift: [0.0; 3],
                    error: None,
                    text: &rack,
                    preview: &NO_PREVIEW,
                    cost: Some(rack_cost.clone()),
                });
            }
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
            let image = reyvec_app::snapshot::FrameImage {
                width: w as u32,
                height: h as u32,
                rgba,
            };
            let menu = if menu { "-меню" } else { "" };
            let name = if shape {
                "Формы-стеллаж.png".to_owned()
            } else {
                format!("{}-{:?}{menu}.png", WORKSPACES[ws].name, play)
            };
            reyvec_app::snapshot::save_png(&out_dir.join(name), &image).unwrap();
        }
    }
}
