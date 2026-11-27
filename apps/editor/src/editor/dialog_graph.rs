//! Схема диалога из карточек (Сю.3): холст с приближением и сдвигом, карточки по цветам
//! групп (как на образце Романа: красные — то, что видит игрок, зелёные — ход разговора,
//! синяя — переменные, фиолетовая — события), стрелки между выходами и входами.
//!
//! Мышь: колесо — приближение, перетаскивание пустого места — сдвиг, заголовок карточки —
//! перенос, кружок выхода справа — новая стрелка (отпустить на карточке — связать, на пустом
//! месте — убрать стрелку). Del — удалить выбранную карточку, Ctrl+Z / Ctrl+Y — отмена и
//! возврат правок схемы. Тексты правятся прямо в карточках.

use std::collections::HashMap;

use egui::epaint::CubicBezierShape;
use egui::{
    Align, Color32, CornerRadius, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Pos2, Rect,
    RichText, Sense, Stroke, TextEdit, Ui, UiBuilder, Vec2, vec2,
};
use reyvec_story::format::{Change, Choice, Value};
use reyvec_story::{Dialog, LineSettings, Node, View};

use super::theme::*;

/// Ширина карточки реплики и остальных.
/// События, которые выполняет сам сюжет: имя, аргументы, что делает (меню «▾» у события).
const EVENTS: &[(&str, &str, &str)] = &[
    ("quest.start", "задание", "взять задание"),
    ("quest.add", "задание, цель, сколько", "продвинуть цель"),
    ("quest.stage", "задание, стадия", "перейти на стадию"),
    ("quest.complete", "задание", "задание выполнено"),
    ("quest.fail", "задание", "задание провалено"),
    (
        "memory.add",
        "персонаж, факт, текст, важность",
        "персонаж запомнил",
    ),
    ("memory.forget", "персонаж, факт", "персонаж забыл"),
    (
        "relation.add",
        "персонаж, к кому, сколько",
        "отношение лучше или хуже",
    ),
    (
        "relation.set",
        "персонаж, к кому, число",
        "отношение — ровно",
    ),
];

const LINE_W: f32 = 300.0;
const NODE_W: f32 = 230.0;
const HEADER_H: f32 = 30.0;
const PORT_R: f32 = 6.0;
/// Сколько шагов отмены помнить на диалог.
const UNDO_LIMIT: usize = 100;

/// Группы карточек и их цвета.
const RED: Color32 = Color32::from_rgb(201, 74, 74);
const GREEN: Color32 = Color32::from_rgb(63, 166, 107);
const BLUE: Color32 = Color32::from_rgb(76, 123, 217);
const PURPLE: Color32 = Color32::from_rgb(155, 89, 208);
const GRAY: Color32 = Color32::from_rgb(90, 97, 112);

/// Персонаж для списка «кто говорит».
pub(crate) struct Speaker {
    pub id: String,
    pub name: String,
    pub color: Color32,
}

/// Что известно холсту о сюжете вокруг диалога.
pub(crate) struct Around<'a> {
    pub speakers: &'a [Speaker],
    pub dialogs: &'a [String],
}

/// Выход карточки.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Port {
    Next,
    Choice(usize),
    Yes,
    No,
    Rand(usize),
}

/// Вид карточки для полосы «добавить».
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Line,
    Generate,
    Run,
    Wait,
    Check,
    Random,
    Chance,
    Jump,
    Set,
    Event,
    End,
}

pub(crate) const KINDS: [Kind; 11] = [
    Kind::Line,
    Kind::Generate,
    Kind::Run,
    Kind::Wait,
    Kind::Check,
    Kind::Random,
    Kind::Chance,
    Kind::Jump,
    Kind::Set,
    Kind::Event,
    Kind::End,
];

impl Kind {
    fn of(n: &Node) -> Kind {
        match n {
            Node::Line { .. } => Kind::Line,
            Node::Generate { .. } => Kind::Generate,
            Node::Run { .. } => Kind::Run,
            Node::Wait { .. } => Kind::Wait,
            Node::Check { .. } => Kind::Check,
            Node::Random { .. } => Kind::Random,
            Node::Chance { .. } => Kind::Chance,
            Node::Jump { .. } => Kind::Jump,
            Node::Set { .. } => Kind::Set,
            Node::Event { .. } => Kind::Event,
            Node::End { .. } => Kind::End,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Kind::Line => "Реплика",
            Kind::Generate => "Сгенерировать",
            Kind::Run => "Выполнить",
            Kind::Wait => "Пауза",
            Kind::Check => "Развилка",
            Kind::Random => "Наугад",
            Kind::Chance => "Процент",
            Kind::Jump => "Перейти",
            Kind::Set => "Переменная",
            Kind::Event => "Событие",
            Kind::End => "Конец",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Kind::Line => "Персонаж говорит; у реплики могут быть варианты ответа игрока",
            Kind::Generate => {
                "Реплику пишет модель по задаче и памяти персонажа; без модели — запасной текст"
            }
            Kind::Run => "Позвать функцию скрипта Luau",
            Kind::Wait => "Подождать секунды или нажатия",
            Kind::Check => "Развилка по условию: да или нет",
            Kind::Random => "Пойти по одной из стрелок наугад",
            Kind::Chance => "С заданной вероятностью — «да», иначе «нет»",
            Kind::Jump => "Перейти в другой диалог",
            Kind::Set => "Записать или прибавить переменную мира",
            Kind::Event => "Сказать игре: начать задание, выдать предмет…",
            Kind::End => "Конец разговора",
        }
    }

    pub(crate) fn color(self) -> Color32 {
        match self {
            Kind::Line | Kind::Generate | Kind::Run | Kind::Wait => RED,
            Kind::Check | Kind::Random | Kind::Chance | Kind::Jump => GREEN,
            Kind::Set => BLUE,
            Kind::Event => PURPLE,
            Kind::End => GRAY,
        }
    }

    fn base_id(self) -> &'static str {
        match self {
            Kind::Line => "replika",
            Kind::Generate => "zhivaya",
            Kind::Run => "vypolnit",
            Kind::Wait => "pauza",
            Kind::Check => "razvilka",
            Kind::Random => "naugad",
            Kind::Chance => "procent",
            Kind::Jump => "perehod",
            Kind::Set => "peremennaya",
            Kind::Event => "sobytie",
            Kind::End => "konec",
        }
    }

    /// Новая карточка этого вида.
    pub(crate) fn make(self, id: String, at: (f32, f32), who: &str) -> Node {
        match self {
            Kind::Line => Node::Line {
                id,
                at,
                who: who.to_string(),
                text: String::new(),
                settings: LineSettings::default(),
                choices: Vec::new(),
                next: String::new(),
                key: String::new(),
            },
            Kind::Generate => Node::Generate {
                id,
                at,
                who: who.to_string(),
                task: String::new(),
                fallback: String::new(),
                words: 30,
                allowed: Vec::new(),
                settings: LineSettings::default(),
                next: String::new(),
                key: String::new(),
            },
            Kind::Run => Node::Run {
                id,
                at,
                func: String::new(),
                args: Vec::new(),
                next: String::new(),
            },
            Kind::Wait => Node::Wait {
                id,
                at,
                seconds: Some(1.0),
                next: String::new(),
            },
            Kind::Check => Node::Check {
                id,
                at,
                when: String::new(),
                yes: String::new(),
                no: String::new(),
            },
            Kind::Random => Node::Random {
                id,
                at,
                next: vec![String::new(), String::new()],
            },
            Kind::Chance => Node::Chance {
                id,
                at,
                percent: 50.0,
                yes: String::new(),
                no: String::new(),
            },
            Kind::Jump => Node::Jump {
                id,
                at,
                dialog: String::new(),
                node: String::new(),
            },
            Kind::Set => Node::Set {
                id,
                at,
                changes: vec![Change {
                    var: String::new(),
                    to: Some(Value::Bool(true)),
                    add: None,
                }],
                next: String::new(),
            },
            Kind::Event => Node::Event {
                id,
                at,
                name: String::new(),
                args: Vec::new(),
                next: String::new(),
            },
            Kind::End => Node::End { id, at },
        }
    }
}

/// Состояние холста: вид каждого диалога, места карточек и портов с прошлого кадра, отмена.
#[derive(Default)]
pub(crate) struct DialogGraph {
    /// Видимая часть схемы для каждого диалога (координаты схемы).
    views: HashMap<String, Rect>,
    /// Прямоугольники карточек (координаты схемы), прошлый кадр.
    rects: HashMap<String, Rect>,
    /// Входы карточек, прошлый кадр.
    inputs: HashMap<String, Pos2>,
    /// Выходы карточек, прошлый кадр.
    ports: HashMap<(String, Port), Pos2>,
    /// Тянем новую стрелку: из какой карточки, из какого выхода, откуда.
    wire: Option<(String, Port, Pos2)>,
    pub(crate) selected: Option<String>,
    /// Карточка, на которой сейчас проигрыватель (обводится оранжевым).
    pub(crate) playing: Option<String>,
    /// К какой карточке проигрывателя уже сдвигали вид.
    shown_playing: Option<String>,
    undo: HashMap<String, Vec<Dialog>>,
    redo: HashMap<String, Vec<Dialog>>,
    /// Снимок до начала переноса карточки.
    moving: Option<Dialog>,
}

impl DialogGraph {
    fn remember(&mut self, d: &Dialog) {
        let u = self.undo.entry(d.id.clone()).or_default();
        u.push(d.clone());
        if u.len() > UNDO_LIMIT {
            u.remove(0);
        }
        self.redo.remove(&d.id);
    }

    /// Ctrl+Z: вернуть схему на шаг назад. `true` — что-то вернули.
    pub(crate) fn undo(&mut self, d: &mut Dialog) -> bool {
        let Some(prev) = self.undo.get_mut(&d.id).and_then(Vec::pop) else {
            return false;
        };
        self.redo
            .entry(d.id.clone())
            .or_default()
            .push(std::mem::replace(d, prev));
        true
    }

    /// Ctrl+Y: повторить отменённое.
    pub(crate) fn redo(&mut self, d: &mut Dialog) -> bool {
        let Some(next) = self.redo.get_mut(&d.id).and_then(Vec::pop) else {
            return false;
        };
        self.undo
            .entry(d.id.clone())
            .or_default()
            .push(std::mem::replace(d, next));
        true
    }

    /// Новая карточка в середине видимой части. Возвращает её имя.
    pub(crate) fn add(&mut self, d: &mut Dialog, kind: Kind, who: &str) -> String {
        self.remember(d);
        let id = free_id(d, kind.base_id());
        let c = self.views.get(&d.id).map_or(Pos2::ZERO, |r| r.center());
        let n = d.nodes.len() as f32;
        // Чуть сдвигаем каждую новую, чтобы они не ложились одна на другую.
        let at = (
            c.x - 120.0 + (n % 5.0) * 16.0,
            c.y - 60.0 + (n % 5.0) * 16.0,
        );
        d.nodes.push(kind.make(id.clone(), at, who));
        if d.nodes.len() == 1 {
            d.start = id.clone();
        }
        self.selected = Some(id.clone());
        id
    }

    /// «Разложить»: столбцы по шагам от начала, в столбце — сверху вниз. Нужна для диалогов,
    /// где места карточек не заданы (написаны руками или моделью).
    pub(crate) fn arrange(&mut self, d: &mut Dialog) {
        self.remember(d);
        let place = arrange(d, &self.rects);
        for n in &mut d.nodes {
            if let Some(&(x, y)) = place.get(n.id()) {
                *at_mut(n) = (x, y);
            }
        }
        self.views.remove(&d.id);
    }

    /// Выбрать карточку и сдвинуть вид так, чтобы она была посередине.
    pub(crate) fn focus(&mut self, dialog: &str, node: &str) {
        self.selected = Some(node.to_string());
        if let (Some(r), Some(v)) = (self.rects.get(node), self.views.get_mut(dialog)) {
            *v = v.translate(r.center() - v.center());
        }
    }

    /// Удалить карточку; стрелки, что вели в неё, становятся пустыми.
    pub(crate) fn remove(&mut self, d: &mut Dialog, id: &str) {
        self.remember(d);
        d.nodes.retain(|n| n.id() != id);
        for n in &mut d.nodes {
            for p in ports(n) {
                if target(n, p) == id {
                    set_target(n, p, String::new());
                }
            }
        }
        if d.start == id {
            d.start.clear();
        }
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
        }
    }

    /// Рисует холст на всё место `ui`. `true` — диалог поменялся.
    pub(crate) fn draw(&mut self, ui: &mut Ui, d: &mut Dialog, around: &Around) -> bool {
        let mut changed = false;
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if !typing {
            let (z, y, del) = ui.ctx().input_mut(|i| {
                (
                    i.consume_key(Modifiers::COMMAND, Key::Z),
                    i.consume_key(Modifiers::COMMAND, Key::Y)
                        || i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z),
                    i.consume_key(Modifiers::NONE, Key::Delete),
                )
            });
            if z {
                changed |= self.undo(d);
            }
            if y {
                changed |= self.redo(d);
            }
            if del && let Some(id) = self.selected.clone() {
                self.remove(d, &id);
                changed = true;
            }
        }
        let outer = ui.available_rect_before_wrap();
        ui.painter().rect_filled(outer, 0, BG);
        let mut view = self
            .views
            .get(&d.id)
            .copied()
            .unwrap_or_else(|| fit(d, outer.size()));
        // Проигрыватель перешёл на другую карточку — показать её, если она за краем.
        // Места карточек берутся с прошлого кадра: пока их нет, ждём.
        if self.playing != self.shown_playing
            && (self.playing.is_none() || self.rects.len() == d.nodes.len())
        {
            self.shown_playing = self.playing.clone();
            if let Some(r) = self.playing.as_ref().and_then(|p| self.rects.get(p))
                && !view.contains_rect(*r)
            {
                view = view.translate(r.center() - view.center());
            }
        }
        let start = d.start_node().unwrap_or("").to_string();
        let mut drop_wire = None;
        let scene = egui::Scene::new()
            .zoom_range(0.25..=1.6)
            .scroll_zooms(true)
            .drag_pan_buttons(
                egui::containers::DragPanButtons::PRIMARY
                    | egui::containers::DragPanButtons::MIDDLE,
            );
        let resp = scene.show(ui, &mut view, |ui| {
            dots(ui);
            // Стрелки под карточками — по местам с прошлого кадра.
            for n in &d.nodes {
                for p in ports(n) {
                    let t = target(n, p);
                    let Some(&from) = self.ports.get(&(n.id().to_string(), p)) else {
                        continue;
                    };
                    if t.is_empty() {
                        continue;
                    }
                    match self.inputs.get(t) {
                        Some(&to) => wire(ui, from, to, Kind::of(n).color().gamma_multiply(0.9)),
                        None => {
                            // Стрелка в несуществующую карточку — красный обрывок.
                            wire(ui, from, from + vec2(40.0, 0.0), BAD);
                        }
                    }
                }
            }
            let mut new_rects = HashMap::new();
            let mut new_inputs = HashMap::new();
            let mut new_ports: Vec<(String, Port, Pos2, Color32)> = Vec::new();
            let mut remove = None;
            let mut make_start = None;
            let ids: Vec<String> = d.nodes.iter().map(|n| n.id().to_string()).collect();
            for i in 0..d.nodes.len() {
                let id = d.nodes[i].id().to_string();
                let on = self.selected.as_deref() == Some(id.as_str());
                let ring = if self.playing.as_deref() == Some(id.as_str()) {
                    Some(Stroke::new(3.0, ACCENT))
                } else if on {
                    Some(Stroke::new(2.0, SELECT))
                } else {
                    None
                };
                let out = node_card(ui, &mut d.nodes[i], around, &ids, ring, start == id);
                changed |= out.changed;
                if out.header.drag_started() {
                    self.moving = Some(d.clone());
                    self.selected = Some(id.clone());
                }
                let n = &mut d.nodes[i];
                if out.header.dragged() {
                    let delta = out.header.drag_delta();
                    let at = at_mut(n);
                    at.0 += delta.x;
                    at.1 += delta.y;
                }
                if out.header.drag_stopped()
                    && let Some(before) = self.moving.take()
                {
                    self.remember(&before);
                    changed = true;
                }
                if out.header.clicked() {
                    self.selected = Some(id.clone());
                }
                out.header.context_menu(|ui| {
                    if ui.button("Сделать началом диалога").clicked() {
                        make_start = Some(id.clone());
                    }
                    if ui.button("Удалить карточку").clicked() {
                        remove = Some(id.clone());
                    }
                });
                if out.close {
                    remove = Some(id.clone());
                }
                new_rects.insert(id.clone(), out.rect);
                new_inputs.insert(id.clone(), out.input);
                for (p, pos) in out.ports {
                    new_ports.push((id.clone(), p, pos, Kind::of(n).color()));
                }
            }
            // Кружки выходов поверх карточек: за них тянут новую стрелку.
            for (id, p, pos, color) in &new_ports {
                let r = ui.interact(
                    Rect::from_center_size(*pos, Vec2::splat(PORT_R * 3.0)),
                    Id::unique(("port", id, *p)),
                    Sense::drag(),
                );
                let hot = r.hovered() || r.dragged();
                ui.painter().circle(
                    *pos,
                    if hot { PORT_R + 1.5 } else { PORT_R },
                    *color,
                    Stroke::new(2.0, BG),
                );
                if r.drag_started() {
                    self.wire = Some((id.clone(), *p, *pos));
                }
                if r.drag_stopped() {
                    drop_wire = Some(ui.ctx().pointer_latest_pos().map(|g| {
                        ui.ctx()
                            .layer_transform_from_global(ui.layer_id())
                            .map_or(g, |t| t * g)
                    }));
                }
            }
            for (id, pos) in &new_inputs {
                ui.painter()
                    .circle(*pos, PORT_R - 1.0, BG, Stroke::new(2.0, TEXT_DIM));
                let _ = id;
            }
            // Стрелка, которую тянут сейчас.
            if let Some((_, _, from)) = &self.wire
                && let Some(g) = ui.ctx().pointer_latest_pos()
            {
                let to = ui
                    .ctx()
                    .layer_transform_from_global(ui.layer_id())
                    .map_or(g, |t| t * g);
                wire(ui, *from, to, ACCENT);
            }
            self.rects = new_rects;
            self.inputs = new_inputs;
            self.ports = new_ports
                .into_iter()
                .map(|(id, p, pos, _)| ((id, p), pos))
                .collect();
            if let Some(id) = remove {
                self.remove(d, &id);
                changed = true;
            }
            if let Some(id) = make_start {
                self.remember(d);
                d.start = id;
                changed = true;
            }
        });
        if resp.response.clicked() {
            self.selected = None;
        }
        if let Some(at) = drop_wire
            && let Some((from, port, _)) = self.wire.take()
        {
            let to = at.and_then(|p| {
                self.rects
                    .iter()
                    .find(|(_, r)| r.contains(p))
                    .map(|(id, _)| id.clone())
            });
            if to.as_deref() != Some(from.as_str()) || to.is_none() {
                self.remember(d);
                if let Some(n) = d.nodes.iter_mut().find(|n| n.id() == from) {
                    set_target(n, port, to.unwrap_or_default());
                    changed = true;
                }
            }
        }
        if self.inputs.len() != d.nodes.len() {
            // Первый кадр: стрелкам ещё некуда вести — нарисовать ещё раз.
            ui.ctx().request_repaint();
        }
        self.views.insert(d.id.clone(), view);
        changed
    }
}

/// Что вернула карточка.
struct CardOut {
    rect: Rect,
    input: Pos2,
    ports: Vec<(Port, Pos2)>,
    header: egui::Response,
    close: bool,
    changed: bool,
}

fn node_card(
    ui: &mut Ui,
    n: &mut Node,
    around: &Around,
    ids: &[String],
    ring: Option<Stroke>,
    start: bool,
) -> CardOut {
    let kind = Kind::of(n);
    let w = if matches!(kind, Kind::Line | Kind::Generate) {
        LINE_W
    } else {
        NODE_W
    };
    let at = Pos2::new(at_mut(n).0, at_mut(n).1);
    let id = n.id().to_string();
    let mut ports = Vec::new();
    let mut changed = false;
    let mut close = false;
    let max = Rect::from_min_size(at, vec2(w, 4000.0));
    let inner = ui.scope_builder(
        UiBuilder::new()
            .max_rect(max)
            .id_salt(("node", &id))
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            let stroke = ring.unwrap_or(Stroke::new(LINE, BORDER_STRONG));
            Frame::NONE
                .fill(CARD)
                .stroke(stroke)
                .corner_radius(RADIUS_L)
                .show(ui, |ui| {
                    ui.set_width(w);
                    ui.spacing_mut().item_spacing = vec2(GAP_M, GAP_M);
                    let (hrect, header) =
                        ui.allocate_exact_size(vec2(w, HEADER_H), Sense::click_and_drag());
                    ui.painter().rect_filled(
                        hrect,
                        CornerRadius {
                            nw: RADIUS_L,
                            ne: RADIUS_L,
                            sw: 0,
                            se: 0,
                        },
                        kind.color(),
                    );
                    ui.painter().text(
                        Pos2::new(hrect.min.x + 12.0, hrect.center().y),
                        egui::Align2::LEFT_CENTER,
                        kind.title(),
                        FontId::proportional(FONT_TITLE),
                        Color32::WHITE,
                    );
                    let tag = if start {
                        format!("▶ {id}")
                    } else {
                        id.clone()
                    };
                    ui.painter().text(
                        Pos2::new(hrect.max.x - 34.0, hrect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        tag,
                        FontId::monospace(FONT_SMALL),
                        Color32::from_white_alpha(190),
                    );
                    let xr = Rect::from_center_size(
                        Pos2::new(hrect.max.x - 16.0, hrect.center().y),
                        vec2(20.0, 20.0),
                    );
                    let x = ui.interact(xr, ui.scope_id().with("x"), Sense::click());
                    let xs = Stroke::new(
                        1.4,
                        if x.hovered() {
                            Color32::WHITE
                        } else {
                            Color32::from_white_alpha(170)
                        },
                    );
                    ui.painter().line_segment(
                        [xr.center() + vec2(-4.0, -4.0), xr.center() + vec2(4.0, 4.0)],
                        xs,
                    );
                    ui.painter().line_segment(
                        [xr.center() + vec2(-4.0, 4.0), xr.center() + vec2(4.0, -4.0)],
                        xs,
                    );
                    close = x.clicked();
                    if header.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    Frame::NONE
                        .inner_margin(Margin {
                            left: 10,
                            right: 14,
                            top: 2,
                            bottom: 10,
                        })
                        .show(ui, |ui| {
                            ui.set_width(w - 24.0);
                            changed |= card_body(ui, n, around, ids, &mut ports);
                        });
                    header
                })
                .inner
        },
    );
    let rect = inner.response.rect;
    // Выходы, которые карточка не поставила сама, — внизу справа.
    let right = rect.max.x;
    let ports = ports
        .into_iter()
        .map(|(p, y): (Port, f32)| (p, Pos2::new(right, y)))
        .collect::<Vec<_>>();
    CardOut {
        rect,
        input: Pos2::new(rect.min.x, rect.min.y + HEADER_H / 2.0),
        ports,
        header: inner.inner,
        close,
        changed,
    }
}

/// Кто говорит: список персонажей диалога.
fn who_combo(ui: &mut Ui, who: &mut String, around: &Around) -> bool {
    let mut changed = false;
    let shown = around
        .speakers
        .iter()
        .find(|s| s.id == *who)
        .map_or_else(|| (who.clone(), BAD), |s| (s.name.clone(), s.color));
    egui::ComboBox::from_id_salt("who")
        .width(150.0)
        .selected_text(RichText::new(shown.0).color(shown.1))
        .show_ui(ui, |ui| {
            for s in around.speakers {
                if ui
                    .selectable_label(*who == s.id, RichText::new(&s.name).color(s.color))
                    .clicked()
                {
                    *who = s.id.clone();
                    changed = true;
                }
            }
        });
    changed
}

/// Тело карточки. В `ports` — выходы и высота их строки.
fn card_body(
    ui: &mut Ui,
    n: &mut Node,
    around: &Around,
    ids: &[String],
    ports: &mut Vec<(Port, f32)>,
) -> bool {
    let mut changed = false;
    let row_y = |r: &egui::Response| r.rect.center().y;
    match n {
        Node::Line {
            who,
            text,
            settings,
            choices,
            next,
            ..
        } => {
            ui.horizontal(|ui| {
                changed |= who_combo(ui, who, around);
                let names = |v: Option<View>| match v {
                    None => "вид —",
                    Some(View::Window) => "окно",
                    Some(View::Bubble) => "над головой",
                    Some(View::Voiceover) => "за кадром",
                };
                egui::ComboBox::from_id_salt("view")
                    .width(90.0)
                    .selected_text(names(settings.view))
                    .show_ui(ui, |ui| {
                        for v in [
                            None,
                            Some(View::Window),
                            Some(View::Bubble),
                            Some(View::Voiceover),
                        ] {
                            changed |= ui
                                .selectable_value(&mut settings.view, v, names(v))
                                .changed();
                        }
                    });
            });
            changed |= ui
                .add(
                    TextEdit::multiline(text)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("Что говорит персонаж")
                        .margin(PAD_FIELD),
                )
                .changed();
            let mut drop = None;
            for (i, c) in choices.iter_mut().enumerate() {
                let r = ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("×").color(MUTED)).frame(false))
                        .on_hover_text("Убрать вариант")
                        .clicked()
                    {
                        drop = Some(i);
                    }
                    changed |= choice_fields(ui, c, i);
                });
                ports.push((Port::Choice(i), row_y(&r.response)));
            }
            if let Some(i) = drop {
                choices.remove(i);
                changed = true;
            }
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("+ вариант ответа").color(GOOD))
                            .frame(false),
                    )
                    .clicked()
                {
                    choices.push(Choice {
                        text: String::new(),
                        when: String::new(),
                        next: String::new(),
                        key: String::new(),
                    });
                    changed = true;
                }
                let mut cam = settings.camera == Some(true);
                if ui
                    .checkbox(&mut cam, RichText::new("камера").color(TEXT_DIM))
                    .on_hover_text("Камера плавно едет к говорящему")
                    .changed()
                {
                    settings.camera = cam.then_some(true);
                    changed = true;
                }
            });
            if choices.is_empty() {
                let r = out_row(ui, "дальше");
                ports.push((Port::Next, row_y(&r)));
            } else if !next.is_empty() {
                // Были варианты и «дальше» сразу: «дальше» не действует, убираем.
                next.clear();
                changed = true;
            }
        }
        Node::Generate {
            who,
            task,
            fallback,
            words,
            allowed,
            ..
        } => {
            ui.horizontal(|ui| {
                changed |= who_combo(ui, who, around);
                ui.label(RichText::new("слов ≤").color(TEXT_DIM));
                changed |= ui
                    .add(egui::DragValue::new(words).range(1..=200).speed(1.0))
                    .changed();
            });
            changed |= ui
                .add(
                    TextEdit::multiline(task)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("Задача для модели: о чём сказать")
                        .margin(PAD_FIELD),
                )
                .changed();
            changed |= ui
                .add(
                    TextEdit::multiline(fallback)
                        .desired_rows(1)
                        .desired_width(f32::INFINITY)
                        .hint_text("Запасной текст: звучит, если модели нет")
                        .margin(PAD_FIELD),
                )
                .changed();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("модель может:").color(TEXT_DIM));
                for (e, _, about) in EVENTS {
                    let mut on = allowed.iter().any(|a| a == e);
                    if ui
                        .checkbox(&mut on, RichText::new(*e).size(FONT_SMALL))
                        .on_hover_text(*about)
                        .changed()
                    {
                        if on {
                            allowed.push((*e).to_string());
                        } else {
                            allowed.retain(|a| a != e);
                        }
                        changed = true;
                    }
                }
            });
            let r = out_row(ui, "дальше");
            ports.push((Port::Next, row_y(&r)));
        }
        Node::Check { when, .. } => {
            changed |= cond_field(ui, when, "kuznec.znakom and hero.money >= 10");
            let r = out_row(ui, "да");
            ports.push((Port::Yes, row_y(&r)));
            let r = out_row(ui, "нет");
            ports.push((Port::No, row_y(&r)));
        }
        Node::Chance { percent, .. } => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("вероятность, %").color(TEXT_DIM));
                changed |= ui
                    .add(egui::DragValue::new(percent).range(0.0..=100.0).speed(1.0))
                    .changed();
            });
            let r = out_row(ui, "да");
            ports.push((Port::Yes, row_y(&r)));
            let r = out_row(ui, "нет");
            ports.push((Port::No, row_y(&r)));
        }
        Node::Random { next, .. } => {
            for i in 0..next.len() {
                let r = out_row(ui, &format!("вариант {}", i + 1));
                ports.push((Port::Rand(i), row_y(&r)));
            }
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(RichText::new("+ стрелка").color(GOOD)).frame(false))
                    .clicked()
                {
                    next.push(String::new());
                    changed = true;
                }
                if next.len() > 1
                    && ui
                        .add(
                            egui::Button::new(RichText::new("− стрелка").color(MUTED)).frame(false),
                        )
                        .clicked()
                {
                    next.pop();
                    changed = true;
                }
            });
        }
        Node::Set { changes, .. } => {
            let mut drop = None;
            for (i, c) in changes.iter_mut().enumerate() {
                let (ch, del) = change_row(ui, c, ("var", i), 110.0);
                changed |= ch;
                if del {
                    drop = Some(i);
                }
            }
            if let Some(i) = drop {
                changes.remove(i);
                changed = true;
            }
            if ui
                .add(egui::Button::new(RichText::new("+ переменная").color(GOOD)).frame(false))
                .clicked()
            {
                changes.push(Change {
                    var: String::new(),
                    to: Some(Value::Bool(true)),
                    add: None,
                });
                changed = true;
            }
            let r = out_row(ui, "дальше");
            ports.push((Port::Next, row_y(&r)));
        }
        Node::Event { name, args, .. } => {
            ui.horizontal(|ui| {
                // Готовые события сюжета: задания (Сю.1), память и отношения (Сю.9).
                let pick = ui.add(egui::Button::new("▾").frame(false));
                egui::Popup::menu(&pick).show(|ui| {
                    ui.set_min_width(300.0);
                    for (e, a, about) in EVENTS {
                        if ui
                            .button(format!("{e}  ({a})"))
                            .on_hover_text(*about)
                            .clicked()
                        {
                            *name = (*e).to_string();
                            changed = true;
                        }
                    }
                });
                changed |= ui
                    .add(
                        TextEdit::singleline(name)
                            .hint_text("quest.start")
                            .font(FontId::monospace(FONT_MONO))
                            .desired_width(f32::INFINITY)
                            .margin(PAD_FIELD),
                    )
                    .changed();
            });
            changed |= args_field(ui, args);
            let r = out_row(ui, "дальше");
            ports.push((Port::Next, row_y(&r)));
        }
        Node::Run { func, args, .. } => {
            changed |= ui
                .add(
                    TextEdit::singleline(func)
                        .hint_text("имя функции Luau")
                        .font(FontId::monospace(FONT_MONO))
                        .desired_width(f32::INFINITY)
                        .margin(PAD_FIELD),
                )
                .changed();
            changed |= args_field(ui, args);
            let r = out_row(ui, "дальше");
            ports.push((Port::Next, row_y(&r)));
        }
        Node::Wait { seconds, .. } => {
            ui.horizontal(|ui| {
                let mut press = seconds.is_none();
                if ui.checkbox(&mut press, "до нажатия").changed() {
                    *seconds = if press { None } else { Some(1.0) };
                    changed = true;
                }
                if let Some(s) = seconds.as_mut() {
                    changed |= ui
                        .add(
                            egui::DragValue::new(s)
                                .range(0.0..=60.0)
                                .speed(0.05)
                                .suffix(" с"),
                        )
                        .changed();
                }
            });
            let r = out_row(ui, "дальше");
            ports.push((Port::Next, row_y(&r)));
        }
        Node::Jump { dialog, node, .. } => {
            egui::ComboBox::from_id_salt("jump")
                .width(ui.available_width())
                .selected_text(if dialog.is_empty() {
                    "выберите диалог"
                } else {
                    dialog.as_str()
                })
                .show_ui(ui, |ui| {
                    for d in around.dialogs {
                        if ui.selectable_label(d == dialog, d).clicked() {
                            *dialog = d.clone();
                            node.clear();
                            changed = true;
                        }
                    }
                });
        }
        Node::End { .. } => {
            ui.label(RichText::new("разговор окончен").color(MUTED));
        }
    }
    let _ = ids;
    changed
}

fn choice_fields(ui: &mut Ui, c: &mut Choice, i: usize) -> bool {
    let mut changed = false;
    ui.vertical(|ui| {
        changed |= ui
            .add(
                TextEdit::singleline(&mut c.text)
                    .id_salt(("choice", i))
                    .hint_text(format!("Ответ {}", i + 1))
                    .desired_width(f32::INFINITY)
                    .margin(PAD_FIELD),
            )
            .changed();
        ui.horizontal(|ui| {
            ui.label(RichText::new("если").color(MUTED).size(FONT_SMALL));
            changed |= ui
                .add(
                    TextEdit::singleline(&mut c.when)
                        .id_salt(("when", i))
                        .hint_text("всегда")
                        .font(FontId::monospace(FONT_SMALL))
                        .desired_width(f32::INFINITY)
                        .frame(Frame::NONE),
                )
                .changed();
        });
    });
    changed
}

fn cond_field(ui: &mut Ui, when: &mut String, hint: &str) -> bool {
    ui.horizontal(|ui| {
        ui.label(RichText::new("если").color(MUTED));
        ui.add(
            TextEdit::singleline(when)
                .hint_text(hint)
                .font(FontId::monospace(FONT_MONO))
                .desired_width(f32::INFINITY)
                .margin(PAD_FIELD),
        )
        .changed()
    })
    .inner
}

fn args_field(ui: &mut Ui, args: &mut Vec<Value>) -> bool {
    let mut text = args
        .iter()
        .map(|v| match v {
            Value::Text(t) => t.clone(),
            other => other.show(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let r = ui.add(
        TextEdit::singleline(&mut text)
            .hint_text("аргументы через запятую")
            .font(FontId::monospace(FONT_MONO))
            .desired_width(f32::INFINITY)
            .margin(PAD_FIELD),
    );
    if r.changed() {
        *args = text
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(parse_value)
            .collect();
        return true;
    }
    false
}

/// Текст поля аргумента → значение: число, да/нет или текст.
pub(crate) fn parse_value(s: &str) -> Value {
    match s {
        "true" | "да" => Value::Bool(true),
        "false" | "нет" => Value::Bool(false),
        _ => s
            .parse::<i64>()
            .map(Value::Int)
            .or_else(|_| s.parse::<f64>().map(Value::Float))
            .unwrap_or_else(|_| Value::Text(s.to_string())),
    }
}

/// Строка «переменная = значение» или «переменная += число» с крестиком. Возвращает
/// (поменялась, убрать). Та же строка — в наградах задания.
pub(crate) fn change_row(
    ui: &mut Ui,
    c: &mut Change,
    salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    width: f32,
) -> (bool, bool) {
    let mut changed = false;
    let mut drop = false;
    ui.horizontal(|ui| {
        changed |= ui
            .add(
                TextEdit::singleline(&mut c.var)
                    .id_salt(("change-var", salt))
                    .hint_text("переменная")
                    .font(FontId::monospace(FONT_MONO))
                    .desired_width(width)
                    .margin(PAD_FIELD),
            )
            .changed();
        let mut add = c.add.is_some();
        egui::ComboBox::from_id_salt(("change-op", salt))
            .width(40.0)
            .selected_text(if add { "+=" } else { "=" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut add, false, "= записать");
                ui.selectable_value(&mut add, true, "+= прибавить");
            });
        if add != c.add.is_some() {
            if add {
                c.add = Some(1.0);
                c.to = None;
            } else {
                c.add = None;
                c.to = Some(Value::Bool(true));
            }
            changed = true;
        }
        if let Some(a) = c.add.as_mut() {
            changed |= ui.add(egui::DragValue::new(a).speed(0.1)).changed();
        } else if let Some(v) = c.to.as_mut() {
            ui.push_id(("change-val", salt), |ui| {
                changed |= value_field(ui, v, 0);
            });
        }
        if ui
            .add(egui::Button::new(RichText::new("×").color(MUTED)).frame(false))
            .on_hover_text("Убрать")
            .clicked()
        {
            drop = true;
        }
    });
    (changed, drop)
}

fn value_field(ui: &mut Ui, v: &mut Value, i: usize) -> bool {
    match v {
        Value::Bool(b) => ui.checkbox(b, if *b { "да" } else { "нет" }).changed(),
        Value::Int(n) => ui.add(egui::DragValue::new(n)).changed(),
        Value::Float(f) => ui.add(egui::DragValue::new(f).speed(0.1)).changed(),
        Value::Text(t) => ui
            .add(
                TextEdit::singleline(t)
                    .id_salt(("val", i))
                    .desired_width(70.0)
                    .margin(PAD_FIELD),
            )
            .changed(),
    }
}

/// Строка выхода: подпись справа, кружок — на краю карточки.
fn out_row(ui: &mut Ui, label: &str) -> egui::Response {
    let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        ui.label(RichText::new(label).color(TEXT_DIM).size(FONT_SMALL));
    })
    .response
}

fn wire(ui: &Ui, from: Pos2, to: Pos2, color: Color32) {
    let dx = ((to.x - from.x).abs() * 0.5).clamp(40.0, 220.0);
    ui.painter().add(CubicBezierShape::from_points_stroke(
        [from, from + vec2(dx, 0.0), to - vec2(dx, 0.0), to],
        false,
        Color32::TRANSPARENT,
        Stroke::new(2.0, color),
    ));
}

/// Точечная сетка фона.
fn dots(ui: &Ui) {
    let clip = ui.clip_rect();
    let step = 32.0;
    let x0 = (clip.min.x / step).floor() as i32;
    let y0 = (clip.min.y / step).floor() as i32;
    let x1 = (clip.max.x / step).ceil() as i32;
    let y1 = (clip.max.y / step).ceil() as i32;
    if (x1 - x0) * (y1 - y0) > 40_000 {
        return;
    }
    for x in x0..x1 {
        for y in y0..y1 {
            ui.painter().circle_filled(
                Pos2::new(x as f32 * step, y as f32 * step),
                1.2,
                BORDER_STRONG,
            );
        }
    }
}

/// Вид, в который помещается вся схема.
/// Первый вид схемы: всё, если влезает с приближением не мельче 0,9; иначе левый край
/// схемы (начало разговора) при 0,9, чтобы текст на карточках читался.
fn fit(d: &Dialog, screen: Vec2) -> Rect {
    let mut r = Rect::NOTHING;
    for n in &d.nodes {
        let (x, y) = node_at(n);
        r = r.union(Rect::from_min_size(Pos2::new(x, y), vec2(LINE_W, 220.0)));
    }
    if !r.is_positive() {
        return Rect::from_center_size(Pos2::ZERO, screen);
    }
    let r = r.expand(60.0);
    let zoom = (screen.x / r.width()).min(screen.y / r.height());
    let zoom = zoom.clamp(0.9, 1.0);
    let size = screen / zoom;
    if size.x >= r.width() {
        Rect::from_center_size(r.center(), size)
    } else {
        Rect::from_min_size(Pos2::new(r.min.x, r.center().y - size.y / 2.0), size)
    }
}

fn free_id(d: &Dialog, base: &str) -> String {
    (1..)
        .map(|i| format!("{base}_{i}"))
        .find(|id| d.node(id).is_none())
        .unwrap_or_default()
}

fn node_at(n: &Node) -> (f32, f32) {
    match n {
        Node::Line { at, .. }
        | Node::Generate { at, .. }
        | Node::Check { at, .. }
        | Node::Set { at, .. }
        | Node::Event { at, .. }
        | Node::Run { at, .. }
        | Node::Jump { at, .. }
        | Node::Random { at, .. }
        | Node::Chance { at, .. }
        | Node::Wait { at, .. }
        | Node::End { at, .. } => *at,
    }
}

fn at_mut(n: &mut Node) -> &mut (f32, f32) {
    match n {
        Node::Line { at, .. }
        | Node::Generate { at, .. }
        | Node::Check { at, .. }
        | Node::Set { at, .. }
        | Node::Event { at, .. }
        | Node::Run { at, .. }
        | Node::Jump { at, .. }
        | Node::Random { at, .. }
        | Node::Chance { at, .. }
        | Node::Wait { at, .. }
        | Node::End { at, .. } => at,
    }
}

/// Выходы карточки по порядку.
fn ports(n: &Node) -> Vec<Port> {
    match n {
        Node::Line { choices, .. } if !choices.is_empty() => {
            (0..choices.len()).map(Port::Choice).collect()
        }
        Node::Line { .. }
        | Node::Generate { .. }
        | Node::Set { .. }
        | Node::Event { .. }
        | Node::Run { .. }
        | Node::Wait { .. } => vec![Port::Next],
        Node::Check { .. } | Node::Chance { .. } => vec![Port::Yes, Port::No],
        Node::Random { next, .. } => (0..next.len()).map(Port::Rand).collect(),
        Node::Jump { .. } | Node::End { .. } => Vec::new(),
    }
}

fn target(n: &Node, p: Port) -> &str {
    match (n, p) {
        (Node::Line { choices, .. }, Port::Choice(i)) => choices.get(i).map_or("", |c| &c.next),
        (
            Node::Line { next, .. }
            | Node::Generate { next, .. }
            | Node::Set { next, .. }
            | Node::Event { next, .. }
            | Node::Run { next, .. }
            | Node::Wait { next, .. },
            Port::Next,
        ) => next,
        (Node::Check { yes, .. } | Node::Chance { yes, .. }, Port::Yes) => yes,
        (Node::Check { no, .. } | Node::Chance { no, .. }, Port::No) => no,
        (Node::Random { next, .. }, Port::Rand(i)) => next.get(i).map_or("", String::as_str),
        _ => "",
    }
}

fn set_target(n: &mut Node, p: Port, to: String) {
    match (n, p) {
        (Node::Line { choices, .. }, Port::Choice(i)) => {
            if let Some(c) = choices.get_mut(i) {
                c.next = to;
            }
        }
        (
            Node::Line { next, .. }
            | Node::Generate { next, .. }
            | Node::Set { next, .. }
            | Node::Event { next, .. }
            | Node::Run { next, .. }
            | Node::Wait { next, .. },
            Port::Next,
        ) => *next = to,
        (Node::Check { yes, .. } | Node::Chance { yes, .. }, Port::Yes) => *yes = to,
        (Node::Check { no, .. } | Node::Chance { no, .. }, Port::No) => *no = to,
        (Node::Random { next, .. }, Port::Rand(i)) => {
            if let Some(t) = next.get_mut(i) {
                *t = to;
            }
        }
        _ => {}
    }
}

/// Места карточек для «Разложить»: столбец = число шагов от начала (недостижимые — в
/// последнем столбце), высота — по прошлому кадру или по оценке.
fn arrange(d: &Dialog, rects: &HashMap<String, Rect>) -> HashMap<String, (f32, f32)> {
    const COLUMN: f32 = LINE_W + 90.0;
    const ROW_GAP: f32 = 40.0;
    let index: HashMap<&str, usize> = d
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id(), i))
        .collect();
    let mut depth: Vec<Option<usize>> = vec![None; d.nodes.len()];
    let mut queue = std::collections::VecDeque::new();
    if let Some(&s) = d.start_node().and_then(|s| index.get(s)) {
        depth[s] = Some(0);
        queue.push_back(s);
    }
    while let Some(i) = queue.pop_front() {
        let n = &d.nodes[i];
        for p in ports(n) {
            if let Some(&j) = index.get(target(n, p))
                && depth[j].is_none()
            {
                depth[j] = Some(depth[i].unwrap_or(0) + 1);
                queue.push_back(j);
            }
        }
    }
    let last = depth.iter().flatten().max().map_or(0, |m| m + 1);
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); last + 1];
    for (i, dep) in depth.iter().enumerate() {
        columns[dep.unwrap_or(last)].push(i);
    }
    let height = |n: &Node| {
        rects.get(n.id()).map_or_else(
            || match n {
                Node::Line { choices, .. } => 170.0 + 56.0 * choices.len() as f32,
                _ => 110.0,
            },
            |r| r.height(),
        )
    };
    let mut out = HashMap::new();
    for (c, col) in columns.iter().enumerate() {
        let total: f32 = col
            .iter()
            .map(|&i| height(&d.nodes[i]) + ROW_GAP)
            .sum::<f32>()
            - ROW_GAP;
        let mut y = -total / 2.0;
        for &i in col {
            let n = &d.nodes[i];
            out.insert(n.id().to_string(), (c as f32 * COLUMN, y.round()));
            y += height(n) + ROW_GAP;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_story::Story;

    fn privet() -> Dialog {
        let (s, _) = Story::from_texts(reyvec_story::EXAMPLE);
        s.dialogs["kuznec.privet"].clone()
    }

    #[test]
    fn arrange_puts_start_first_and_keeps_cards_apart() {
        let (s, _) = Story::from_texts(reyvec_story::EXAMPLE);
        for d in s.dialogs.values() {
            let place = arrange(d, &HashMap::new());
            assert_eq!(place.len(), d.nodes.len());
            assert_eq!(place[d.start_node().unwrap()].0, 0.0);
            for a in &d.nodes {
                for b in &d.nodes {
                    let (pa, pb) = (place[a.id()], place[b.id()]);
                    if a.id() != b.id() && pa.0 == pb.0 {
                        assert!((pa.1 - pb.1).abs() >= 110.0, "{} и {}", a.id(), b.id());
                    }
                }
            }
            let mut v: Vec<_> = place.iter().collect();
            v.sort_by(|a, b| a.0.cmp(b.0));
            println!("{}: {v:?}", d.id);
        }
    }

    #[test]
    fn ports_and_targets_cover_all_arrows() {
        let d = privet();
        for n in &d.nodes {
            let mut by_ports: Vec<&str> = ports(n).into_iter().map(|p| target(n, p)).collect();
            let mut by_node = n.targets();
            by_ports.sort();
            by_node.sort();
            assert_eq!(by_ports, by_node, "{}", n.id());
        }
    }

    #[test]
    fn add_remove_undo_redo() {
        let mut g = DialogGraph::default();
        let mut d = privet();
        let before = format!("{:?}", d.nodes);
        let id = g.add(&mut d, Kind::Check, "kuznec");
        assert_eq!(id, "razvilka_1");
        assert_eq!(d.nodes.len(), 13);
        // Удаление «vopros» обрывает стрелки, что в неё вели.
        g.remove(&mut d, "vopros");
        assert!(d.node("vopros").is_none());
        let Some(Node::Line { next, .. }) = d.node("chuzhak") else {
            panic!()
        };
        assert!(next.is_empty());
        assert!(g.undo(&mut d));
        assert!(d.node("vopros").is_some());
        assert!(g.undo(&mut d));
        assert_eq!(format!("{:?}", d.nodes), before);
        assert!(!g.undo(&mut d));
        assert!(g.redo(&mut d));
        assert_eq!(d.nodes.len(), 13);
    }

    #[test]
    fn new_nodes_pass_parse_after_write() {
        let mut d = privet();
        let mut g = DialogGraph::default();
        for k in KINDS {
            g.add(&mut d, k, "kuznec");
        }
        let text = reyvec_story::format::write(&d);
        let (s, problems) = Story::from_texts(&[("dialogs/d.ron", text.as_str())]);
        assert!(
            s.dialogs.contains_key("kuznec.privet"),
            "{problems:?}\n{text}"
        );
        assert_eq!(s.dialogs["kuznec.privet"].nodes.len(), d.nodes.len());
    }

    #[test]
    fn values_from_text() {
        assert_eq!(parse_value("mech"), Value::Text("mech".into()));
        assert_eq!(parse_value("3"), Value::Int(3));
        assert_eq!(parse_value("2.5"), Value::Float(2.5));
        assert_eq!(parse_value("да"), Value::Bool(true));
    }
}
