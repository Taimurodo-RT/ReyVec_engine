//! Окно сцены в режиме правки: полоса инструментов над сценой (как в Unreal 5), выделение
//! щелчком, ручки сдвига, поворота и размера с привязкой к сетке, сетка поверх сцены.
//!
//! Всё в точках интерфейса: камера движка переводится в них через [`View`]. Сами правки
//! интерфейс не делает: он складывает [`Action::Edit`], их выполняет цикл окна.

use egui::{Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use reyvec_scene::SceneFile;

use super::doc::{Change, Edit};
use super::geom::{self, Placed, Xform};
use super::outline;
use super::theme::*;
use super::ui::Action;

/// Высота полосы инструментов над сценой.
pub(crate) const TOOLBAR_HEIGHT: f32 = 38.0;
/// Длина стрелок ручки сдвига и радиус кольца поворота, точек.
const ARROW: f32 = 84.0;
const RING: f32 = 70.0;
/// Насколько близко к ручке надо попасть мышью, точек.
const GRAB: f32 = 7.0;

/// Камера сцены в точках интерфейса: экран движка (y вверх, высота кадра 2) ↔ окно.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct View {
    /// Точка сцены в середине окна и приближение (как у `Camera2D`).
    pub center: [f64; 2],
    pub zoom: f64,
    /// Размер всего окна в точках интерфейса.
    pub size: [f64; 2],
}

impl View {
    pub fn to_ui(self, p: [f64; 2]) -> Pos2 {
        let [w, h] = self.size;
        let sx = (p[0] - self.center[0]) * self.zoom;
        let sy = (p[1] - self.center[1]) * self.zoom;
        pos2(((sx * h + w) / 2.0) as f32, ((h - sy * h) / 2.0) as f32)
    }

    pub fn to_scene(self, q: Pos2) -> [f64; 2] {
        let [w, h] = self.size;
        let sx = (2.0 * f64::from(q.x) - w) / h;
        let sy = (h - 2.0 * f64::from(q.y)) / h;
        [
            self.center[0] + sx / self.zoom,
            self.center[1] + sy / self.zoom,
        ]
    }

    /// Сколько точек интерфейса в одной единице сцены.
    pub fn points_per_unit(&self) -> f64 {
        self.zoom * self.size[1] / 2.0
    }

    /// Камера, при которой прямоугольник сцены `b` занимает примерно две трети окна
    /// `viewport` и стоит посередине него (кнопка F).
    pub fn focus(&self, b: &geom::Bounds, viewport: Rect) -> ([f64; 2], f64) {
        let [w, h] = self.size;
        let s = b.size();
        let fit_h = 2.0 * f64::from(viewport.height()) / h;
        let fit_w = 2.0 * f64::from(viewport.width()) / h;
        let zoom = (0.66 * (fit_h / s[1].max(0.05)).min(fit_w / s[0].max(0.05))).clamp(0.05, 200.0);
        // Середина окна сцены в координатах экрана движка.
        let c = viewport.center();
        let sx = (2.0 * f64::from(c.x) - w) / h;
        let sy = (h - 2.0 * f64::from(c.y)) / h;
        let m = b.center();
        ([m[0] - sx / zoom, m[1] - sy / zoom], zoom)
    }
}

/// Инструмент: что делает левая кнопка с выделенным предметом.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    Select,
    Move,
    Rotate,
    Scale,
    /// Линейка: протянуть мышью — длина и угол (T).
    Measure,
}

/// Привязки: шаг сетки (единиц сцены), поворота (градусов), размера.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Snap {
    pub grid_on: bool,
    pub grid: f64,
    pub angle_on: bool,
    pub angle: f64,
    pub scale_on: bool,
    pub scale: f64,
}

impl Default for Snap {
    fn default() -> Self {
        Self {
            grid_on: true,
            grid: 0.05,
            angle_on: true,
            angle: 15.0,
            scale_on: false,
            scale: 0.25,
        }
    }
}

const GRID_STEPS: [f64; 6] = [0.01, 0.025, 0.05, 0.1, 0.25, 0.5];
const ANGLE_STEPS: [f64; 5] = [1.0, 5.0, 10.0, 15.0, 45.0];
const SCALE_STEPS: [f64; 4] = [0.05, 0.1, 0.25, 0.5];

fn snap_to(x: f64, step: f64, on: bool) -> f64 {
    if on && step > 0.0 {
        (x / step).round() * step
    } else {
        x
    }
}

/// Какую ручку тянут.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Handle {
    /// Сдвиг вдоль оси (единичный вектор в координатах сцены) или свободно (`None`).
    Move(Option<[f64; 2]>),
    Rotate,
    Scale,
}

/// Начатое перетаскивание ручки.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Drag {
    path: Vec<usize>,
    handle: Handle,
    /// Мышь в начале, в координатах сцены.
    from: [f64; 2],
    /// Место предмета в сцене в начале и его родитель.
    origin: [f64; 2],
    parent: Xform,
    at: [f64; 2],
    angle: f64,
    scale: f64,
    /// Последнее отправленное значение (чтобы не слать одно и то же каждый кадр).
    last: Option<Change>,
}

/// Состояние окна сцены между кадрами.
#[derive(Debug)]
pub(crate) struct Viewport {
    pub tool: Tool,
    /// Оси ручек: свои оси предмета (`true`) или оси сцены.
    pub local: bool,
    pub snap: Snap,
    pub show_grid: bool,
    pub drag: Option<Drag>,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            tool: Tool::Move,
            local: false,
            snap: Snap::default(),
            show_grid: true,
            drag: None,
        }
    }
}

/// Кнопка-значок полосы инструментов.
fn tool_button(
    ui: &mut Ui,
    on: bool,
    label: &str,
    hint: &str,
    paint: impl FnOnce(&egui::Painter, Pos2, Color32),
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 26.0), Sense::click());
    let fill = if on {
        SELECT_BG
    } else if resp.hovered() {
        HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, RADIUS_S, fill);
    let color = if on { SELECT } else { TEXT_DIM };
    paint(ui.painter(), rect.center(), color);
    let resp = resp.on_hover_text(format!("{label}\n{hint}"));
    resp.clicked()
}

/// Кнопка с подписью и выпадающим списком шагов привязки.
fn snap_button(
    ui: &mut Ui,
    id: &str,
    on: &mut bool,
    step: &mut f64,
    steps: &[f64],
    unit: &str,
    hint: &str,
) {
    let text = format!("{}{unit}", fmt_step(*step));
    let color = if *on { SELECT } else { MUTED };
    let resp = ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .color(if *on { TEXT } else { MUTED })
                .size(FONT_CHIP),
        )
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(
            LINE,
            if *on { SELECT_BG } else { Color32::TRANSPARENT },
        ))
        .corner_radius(RADIUS_S),
    );
    let _ = color;
    if resp.clicked() {
        *on = !*on;
    }
    let resp = resp.on_hover_text(format!(
        "{hint}\nЩелчок — включить или выключить, правая кнопка — шаг"
    ));
    egui::Popup::context_menu(&resp)
        .id(egui::Id::unique(("snap", id)))
        .show(|ui| {
            for s in steps {
                if ui
                    .selectable_label(*step == *s, format!("{}{unit}", fmt_step(*s)))
                    .clicked()
                {
                    *step = *s;
                    *on = true;
                    ui.close();
                }
            }
        });
}

fn fmt_step(x: f64) -> String {
    let t = super::ron_edit::num(x);
    t.trim_end_matches(".0").replace('.', ",")
}

impl Viewport {
    /// Полоса инструментов над сценой.
    pub fn toolbar(&mut self, ui: &mut Ui, editing: bool) {
        ui.add_enabled_ui(editing, |ui| {
            ui.spacing_mut().item_spacing.x = GAP_XS;
            for (tool, label, key) in [
                (Tool::Select, "Выбор", "Q"),
                (Tool::Move, "Сдвиг", "W"),
                (Tool::Rotate, "Поворот", "E"),
                (Tool::Scale, "Размер", "R"),
                (Tool::Measure, "Линейка", "T"),
            ] {
                let hint = format!("клавиша {key}");
                if tool_button(ui, self.tool == tool, label, &hint, |p, c, col| {
                    paint_tool(p, c, tool, col)
                }) {
                    self.tool = tool;
                }
            }
            ui.add_space(GAP_M);
            let axes = if self.local {
                "Оси предмета"
            } else {
                "Оси сцены"
            };
            if ui
                .add(
                    egui::Button::new(egui::RichText::new(axes).size(FONT_CHIP).color(TEXT_DIM))
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE),
                )
                .on_hover_text(
                    "Куда смотрят стрелки сдвига: по осям сцены или повёрнуты вместе с предметом",
                )
                .clicked()
            {
                self.local = !self.local;
            }
            ui.add(egui::Separator::default().vertical().spacing(GAP_L));
            ui.label(egui::RichText::new("сетка").size(FONT_SMALL).color(MUTED));
            let s = &mut self.snap;
            snap_button(
                ui,
                "grid",
                &mut s.grid_on,
                &mut s.grid,
                &GRID_STEPS,
                "",
                "Привязка места к сетке, шаг в единицах сцены",
            );
            ui.label(egui::RichText::new("угол").size(FONT_SMALL).color(MUTED));
            snap_button(
                ui,
                "angle",
                &mut s.angle_on,
                &mut s.angle,
                &ANGLE_STEPS,
                "°",
                "Привязка поворота",
            );
            ui.label(egui::RichText::new("размер").size(FONT_SMALL).color(MUTED));
            snap_button(
                ui,
                "scale",
                &mut s.scale_on,
                &mut s.scale,
                &SCALE_STEPS,
                "",
                "Привязка размера",
            );
            ui.add(egui::Separator::default().vertical().spacing(GAP_L));
            let grid_text = egui::RichText::new("Сетка")
                .size(FONT_CHIP)
                .color(if self.show_grid { TEXT } else { MUTED });
            if ui
                .add(
                    egui::Button::new(grid_text)
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE),
                )
                .on_hover_text("Показывать сетку поверх сцены")
                .clicked()
            {
                self.show_grid = !self.show_grid;
            }
        });
    }

    /// Сетка поверх сцены: линии каждые `snap.grid` (или крупнее, если мелкие слились).
    pub fn grid(&self, painter: &egui::Painter, view: &View, rect: Rect) {
        if !self.show_grid {
            return;
        }
        let ppu = view.points_per_unit();
        let mut step = self.snap.grid.max(0.001);
        while step * ppu < 10.0 {
            step *= 2.0;
        }
        let a = view.to_scene(rect.left_bottom());
        let b = view.to_scene(rect.right_top());
        let thin = Stroke::new(LINE, Color32::from_white_alpha(10));
        let thick = Stroke::new(LINE, Color32::from_white_alpha(22));
        let mut x = (a[0] / step).floor() * step;
        let mut n = 0;
        while x <= b[0] && n < 2000 {
            let px = view.to_ui([x, 0.0]).x;
            let major = (x / (step * 5.0)).round() * step * 5.0 - x;
            painter.vline(
                px,
                rect.y_range(),
                if major.abs() < step * 0.01 {
                    thick
                } else {
                    thin
                },
            );
            x += step;
            n += 1;
        }
        let mut y = (a[1] / step).floor() * step;
        while y <= b[1] && n < 4000 {
            let py = view.to_ui([0.0, y]).y;
            let major = (y / (step * 5.0)).round() * step * 5.0 - y;
            painter.hline(
                rect.x_range(),
                py,
                if major.abs() < step * 0.01 {
                    thick
                } else {
                    thin
                },
            );
            y += step;
            n += 1;
        }
        // Оси сцены.
        let o = view.to_ui([0.0, 0.0]);
        painter.vline(
            o.x,
            rect.y_range(),
            Stroke::new(LINE, Color32::from_rgba_unmultiplied(107, 227, 138, 60)),
        );
        painter.hline(
            rect.x_range(),
            o.y,
            Stroke::new(LINE, Color32::from_rgba_unmultiplied(255, 107, 107, 60)),
        );
    }

    /// Рамка выделенного предмета: его собственные границы и (пунктиром) со вложенными.
    pub fn selection(&self, painter: &egui::Painter, view: &View, p: &Placed) {
        let r = |b: &geom::Bounds| Rect::from_two_pos(view.to_ui(b.min), view.to_ui(b.max));
        if p.all != p.own {
            let all = r(&p.all);
            dashed_rect(painter, all, Stroke::new(LINE, SELECT.gamma_multiply(0.6)));
        }
        painter.rect_stroke(
            r(&p.own),
            0,
            Stroke::new(1.5, SELECT),
            egui::StrokeKind::Middle,
        );
    }

    /// Ручки выделенного предмета и их перетаскивание. Возвращает `true`, если мышь занята
    /// ручкой (тогда щелчок по сцене не выделяет другой предмет).
    pub fn gizmo(
        &mut self,
        ui: &mut Ui,
        view: &View,
        p: &Placed,
        node: &reyvec_scene::NodeDesc,
        actions: &mut Vec<Action>,
    ) -> bool {
        if matches!(self.tool, Tool::Select | Tool::Measure) {
            return false;
        }
        let origin = p.xform.apply([0.0, 0.0]);
        let o = view.to_ui(origin);
        let theta = if self.local { p.xform.angle() } else { 0.0 }.to_radians();
        let (s, c) = theta.sin_cos();
        let ax = [c, s];
        let ay = [-s, c];
        let dir = |a: [f64; 2]| vec2(a[0] as f32, -(a[1] as f32));
        let pointer = ui.ctx().pointer_hover_pos();
        let painter = ui.painter().clone();

        // Что под мышью сейчас (подсветка) — если ничего не тянут.
        let hover = if self.drag.is_some() {
            None
        } else {
            pointer.and_then(|m| self.hit(m, o, dir(ax), dir(ay)))
        };
        let active = self.drag.as_ref().map(|d| d.handle);
        let lit = |h: Handle| {
            active.map_or(hover.is_some() && hover == h_kind(h), |a| {
                h_kind(a) == h_kind(h)
            })
        };

        match self.tool {
            Tool::Move => {
                for (axis, color, kind) in [
                    (ax, BAD, Handle::Move(Some(ax))),
                    (ay, GOOD, Handle::Move(Some(ay))),
                ] {
                    let col = if lit(kind) { Color32::WHITE } else { color };
                    let end = o + dir(axis) * ARROW;
                    painter.line_segment([o, end], Stroke::new(3.0, col));
                    let n = dir(axis).rot90() * 7.0;
                    painter.add(egui::Shape::convex_polygon(
                        vec![end + dir(axis) * 14.0, end + n, end - n],
                        col,
                        Stroke::NONE,
                    ));
                }
                let sq = Rect::from_min_size(o + vec2(0.0, -24.0), vec2(24.0, 24.0));
                let free = lit(Handle::Move(None));
                painter.rect(
                    sq,
                    2,
                    ACCENT.gamma_multiply(if free { 0.6 } else { 0.3 }),
                    Stroke::new(1.5, ACCENT),
                    egui::StrokeKind::Middle,
                );
            }
            Tool::Rotate => {
                let col = if lit(Handle::Rotate) {
                    Color32::WHITE
                } else {
                    SELECT
                };
                painter.circle_stroke(o, RING, Stroke::new(3.0, col));
                if let Some(d) = &self.drag
                    && let Some(Change::Angle(angle)) = &d.last
                {
                    let delta = angle - d.angle;
                    painter.text(
                        o + vec2(0.0, -RING - 14.0),
                        Align2::CENTER_BOTTOM,
                        format!("{delta:+.1}°"),
                        FontId::monospace(FONT_SMALL),
                        TEXT,
                    );
                }
            }
            Tool::Scale => {
                let end = o + vec2(1.0, -1.0).normalized() * ARROW;
                let col = if lit(Handle::Scale) {
                    Color32::WHITE
                } else {
                    ACCENT
                };
                painter.line_segment([o, end], Stroke::new(2.0, col));
                painter.rect_filled(Rect::from_center_size(end, vec2(14.0, 14.0)), 2, col);
            }
            Tool::Select | Tool::Measure => {}
        }
        painter.circle_filled(o, 4.5, Color32::WHITE);

        // Перетаскивание. Мышь берём из ввода напрямую: ручки нарисованы поверх сцены,
        // а сцена — один большой виджет.
        let (pressed, down, released, esc) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if let (Some(h), true, Some(m)) = (hover, pressed, pointer) {
            let handle = match h {
                HitKind::AxisX => Handle::Move(Some(ax)),
                HitKind::AxisY => Handle::Move(Some(ay)),
                HitKind::Free => Handle::Move(None),
                HitKind::Ring => Handle::Rotate,
                HitKind::Scale => Handle::Scale,
            };
            self.drag = Some(Drag {
                path: p.path.clone(),
                handle,
                from: view.to_scene(m),
                origin,
                parent: p.parent,
                at: [node.at.0, node.at.1],
                angle: node.angle,
                scale: node.scale,
                last: None,
            });
        }
        let Some(drag) = self.drag.as_mut() else {
            if hover.is_some() {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
            return hover.is_some();
        };
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        if esc {
            self.drag = None;
            actions.push(Action::CancelEdit);
            return true;
        }
        if let Some(m) = pointer.filter(|_| down || released) {
            let change = drag_change(drag, view.to_scene(m), &self.snap);
            let edit = Edit {
                path: drag.path.clone(),
                change: change.clone(),
            };
            if released {
                actions.push(Action::Edit { edit, commit: true });
            } else if drag.last.as_ref() != Some(&change) {
                actions.push(Action::Edit {
                    edit,
                    commit: false,
                });
            }
            drag.last = Some(change);
        }
        if released || !down {
            if !released && drag.last.is_some() {
                // Кнопку отпустили вне окна: правка всё равно в силе.
                if let Some(change) = drag.last.clone() {
                    actions.push(Action::Edit {
                        edit: Edit {
                            path: drag.path.clone(),
                            change,
                        },
                        commit: true,
                    });
                }
            }
            self.drag = None;
        }
        true
    }

    fn hit(&self, m: Pos2, o: Pos2, ax: egui::Vec2, ay: egui::Vec2) -> Option<HitKind> {
        match self.tool {
            Tool::Move => {
                let sq = Rect::from_min_size(o + vec2(0.0, -24.0), vec2(24.0, 24.0));
                if sq.contains(m) {
                    return Some(HitKind::Free);
                }
                let near = |a: egui::Vec2| seg_dist(m, o, o + a * (ARROW + 14.0)) < GRAB;
                if near(ax) {
                    Some(HitKind::AxisX)
                } else if near(ay) {
                    Some(HitKind::AxisY)
                } else {
                    None
                }
            }
            Tool::Rotate => ((m - o).length() - RING)
                .abs()
                .lt(&GRAB)
                .then_some(HitKind::Ring),
            Tool::Scale => {
                let end = o + vec2(1.0, -1.0).normalized() * ARROW;
                Rect::from_center_size(end, vec2(14.0 + GRAB, 14.0 + GRAB))
                    .contains(m)
                    .then_some(HitKind::Scale)
            }
            Tool::Select | Tool::Measure => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HitKind {
    AxisX,
    AxisY,
    Free,
    Ring,
    Scale,
}

fn h_kind(h: Handle) -> Option<HitKind> {
    // Ось X/Y различаем по тому, какая из них тянется (вектор приходит из того же кадра).
    match h {
        Handle::Move(None) => Some(HitKind::Free),
        Handle::Move(Some(_)) => None,
        Handle::Rotate => Some(HitKind::Ring),
        Handle::Scale => Some(HitKind::Scale),
    }
}

/// Новое место, поворот и размер при мыши в точке `m` сцены.
fn drag_change(d: &Drag, m: [f64; 2], snap: &Snap) -> Change {
    match d.handle {
        Handle::Move(axis) => {
            let mut delta = [m[0] - d.from[0], m[1] - d.from[1]];
            if let Some(a) = axis {
                let k = delta[0] * a[0] + delta[1] * a[1];
                delta = [a[0] * k, a[1] * k];
            }
            let world = [d.origin[0] + delta[0], d.origin[1] + delta[1]];
            let local = d.parent.inverse().map_or(world, |inv| inv.apply(world));
            let mut at = d.at;
            for k in 0..2 {
                // Привязываем только то, что поменялось: ось, вдоль которой не тянут, не прыгает.
                if (local[k] - d.at[k]).abs() > 1e-9 {
                    at[k] = snap_to(local[k], snap.grid, snap.grid_on);
                }
            }
            Change::At(at)
        }
        Handle::Rotate => {
            let a0 = (d.from[1] - d.origin[1]).atan2(d.from[0] - d.origin[0]);
            let a1 = (m[1] - d.origin[1]).atan2(m[0] - d.origin[0]);
            let mut delta = (a1 - a0).to_degrees();
            if delta > 180.0 {
                delta -= 360.0;
            } else if delta < -180.0 {
                delta += 360.0;
            }
            Change::Angle(snap_to(d.angle + delta, snap.angle, snap.angle_on))
        }
        Handle::Scale => {
            let d0 = (d.from[0] - d.origin[0])
                .hypot(d.from[1] - d.origin[1])
                .max(1e-6);
            let d1 = (m[0] - d.origin[0]).hypot(m[1] - d.origin[1]);
            Change::Scale(snap_to(d.scale * d1 / d0, snap.scale, snap.scale_on).max(0.01))
        }
    }
}

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

fn dashed_rect(painter: &egui::Painter, r: Rect, stroke: Stroke) {
    let pts = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
}

/// Значки инструментов.
fn paint_tool(p: &egui::Painter, c: Pos2, tool: Tool, col: Color32) {
    let s = Stroke::new(1.3, col);
    match tool {
        Tool::Select => {
            p.add(egui::Shape::closed_line(
                vec![
                    c + vec2(-4.0, -6.0),
                    c + vec2(-4.0, 6.0),
                    c + vec2(-1.0, 3.0),
                    c + vec2(1.5, 7.0),
                    c + vec2(3.0, 6.0),
                    c + vec2(0.5, 2.0),
                    c + vec2(4.5, 2.0),
                ],
                s,
            ));
        }
        Tool::Move => {
            p.line_segment([c + vec2(-7.0, 0.0), c + vec2(7.0, 0.0)], s);
            p.line_segment([c + vec2(0.0, -7.0), c + vec2(0.0, 7.0)], s);
            for d in [
                vec2(1.0, 0.0),
                vec2(-1.0, 0.0),
                vec2(0.0, 1.0),
                vec2(0.0, -1.0),
            ] {
                let tip = c + d * 7.0;
                p.line_segment([tip, tip - d * 3.0 + d.rot90() * 2.5], s);
                p.line_segment([tip, tip - d * 3.0 - d.rot90() * 2.5], s);
            }
        }
        Tool::Rotate => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|k| {
                    let t = -0.6 + k as f32 * 0.25;
                    c + vec2(t.cos(), t.sin()) * 6.0
                })
                .collect();
            p.add(egui::Shape::line(pts, s));
            let tip = c + vec2((-0.6f32).cos(), (-0.6f32).sin()) * 6.0;
            p.line_segment([tip, tip + vec2(0.0, -3.5)], s);
            p.line_segment([tip, tip + vec2(3.5, 0.5)], s);
        }
        Tool::Scale => {
            p.rect_stroke(
                Rect::from_min_size(c + vec2(-6.0, -1.0), vec2(7.0, 7.0)),
                0,
                s,
                egui::StrokeKind::Middle,
            );
            p.line_segment([c + vec2(0.0, -6.0), c + vec2(6.0, -6.0)], s);
            p.line_segment([c + vec2(6.0, -6.0), c + vec2(6.0, 0.0)], s);
            p.line_segment([c + vec2(6.0, -6.0), c + vec2(1.0, -1.0)], s);
        }
        Tool::Measure => {
            // Линейка наискосок с делениями.
            let (a, b) = (c + vec2(-7.0, 4.0), c + vec2(7.0, -4.0));
            p.line_segment([a, b], s);
            let n = (b - a).normalized().rot90() * 3.0;
            for k in 0..=4 {
                let q = a + (b - a) * (k as f32 / 4.0);
                let len = if k % 2 == 0 { 1.0 } else { 0.6 };
                p.line_segment([q, q + n * len], s);
            }
        }
    }
}

/// Подпись предмета у рамки выделения.
pub(crate) fn label(painter: &egui::Painter, view: &View, p: &Placed, file: &SceneFile) {
    let Some(node) = outline::node_at(file, &p.path) else {
        return;
    };
    let at = view.to_ui([p.own.min[0], p.own.max[1]]) + vec2(0.0, -4.0);
    let text = outline::display_name(node);
    let galley = painter.layout_no_wrap(text, FontId::proportional(FONT_SMALL), Color32::WHITE);
    let rect = Rect::from_min_size(
        at - vec2(0.0, galley.size().y + 4.0),
        galley.size() + vec2(8.0, 4.0),
    );
    painter.rect_filled(rect, 3, SELECT);
    painter.galley(rect.min + vec2(4.0, 2.0), galley, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            center: [0.5, -0.25],
            zoom: 2.0,
            size: [1600.0, 900.0],
        }
    }

    #[test]
    fn view_round_trip() {
        let v = view();
        let p = [0.3, 0.7];
        let q = v.to_scene(v.to_ui(p));
        assert!((q[0] - p[0]).abs() < 1e-4 && (q[1] - p[1]).abs() < 1e-4);
        // Середина окна — центр камеры.
        let c = v.to_scene(pos2(800.0, 450.0));
        assert!((c[0] - 0.5).abs() < 1e-9 && (c[1] + 0.25).abs() < 1e-9);
        assert!((v.points_per_unit() - 900.0).abs() < 1e-9);
    }

    #[test]
    fn focus_centers_bounds_in_viewport() {
        let v = view();
        let b = geom::Bounds {
            min: [1.0, 1.0],
            max: [1.2, 1.1],
        };
        let vp = Rect::from_min_max(pos2(280.0, 82.0), pos2(1280.0, 656.0));
        let (center, zoom) = v.focus(&b, vp);
        let v2 = View {
            center,
            zoom,
            size: v.size,
        };
        let c = v2.to_ui(b.center());
        assert!((c - vp.center()).length() < 0.5, "{c:?}");
        let w = v2.to_ui(b.max).x - v2.to_ui(b.min).x;
        assert!(w < vp.width() && w > vp.width() * 0.3);
    }

    fn drag(handle: Handle) -> Drag {
        Drag {
            path: vec![0],
            handle,
            from: [0.0, 0.0],
            origin: [0.0, 0.0],
            parent: Xform::IDENTITY,
            at: [0.0, 0.0],
            angle: 0.0,
            scale: 1.0,
            last: None,
        }
    }

    #[test]
    fn move_along_axis_snaps_only_that_axis() {
        let snap = Snap::default();
        let c = drag_change(&drag(Handle::Move(Some([1.0, 0.0]))), [0.123, 0.4], &snap);
        assert_eq!(c, Change::At([0.1, 0.0]));
        // Внутри повёрнутого родителя место считается в его координатах.
        let mut d = drag(Handle::Move(None));
        d.parent = Xform::placement([1.0, 0.0], 90.0, 1.0);
        d.origin = [1.0, 0.0];
        let Change::At(at) = drag_change(
            &d,
            [0.0, 0.5],
            &Snap {
                grid_on: false,
                ..snap
            },
        ) else {
            panic!()
        };
        // Мышь сдвинута на +0.5 по y сцены: у родителя, повёрнутого на 90°, это +0.5 по его x.
        assert!((at[0] - 0.5).abs() < 1e-9 && at[1].abs() < 1e-9, "{at:?}");
    }

    #[test]
    fn rotate_and_scale_snap() {
        let snap = Snap::default();
        let mut d = drag(Handle::Rotate);
        d.from = [1.0, 0.0];
        let Change::Angle(angle) = drag_change(&d, [1.0, 0.9], &snap) else {
            panic!()
        };
        assert_eq!(angle, 45.0);
        let mut d = drag(Handle::Scale);
        d.from = [1.0, 0.0];
        let Change::Scale(scale) = drag_change(
            &d,
            [2.1, 0.0],
            &Snap {
                scale_on: true,
                ..snap
            },
        ) else {
            panic!()
        };
        assert_eq!(scale, 2.0);
    }
}
