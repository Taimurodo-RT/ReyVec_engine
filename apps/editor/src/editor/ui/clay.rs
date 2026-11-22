//! Лепка глины в «Формах» (М.9б): карточка «Лепка» у глиняной детали (кисть, размер,
//! сила), кнопка «Превратить в глину» у остальных и кисть мышью на подставке: левая
//! кнопка лепит, кружок показывает, куда и какого размера.

use egui::{Pos2, Response, RichText, Stroke, Ui};
use glam::Vec3;
use reyvec_sdf::clay::Brush;

use super::super::clay_edit::{self, Dab};
use super::super::shape_edit::{self, Part};
use super::super::shapes;
use super::super::theme::*;
use super::{Action, EditorUi, ShapeInfo, card};

/// Что помнит лепка между кадрами.
pub(crate) struct ClayUi {
    /// Кисть включена: левая кнопка на глине лепит, а не выбирает и не тянет ручки.
    pub on: bool,
    pub brush: Brush,
    /// Радиус кисти в клетках глины.
    pub size: f32,
    /// Сила мазка (0…1).
    pub strength: f32,
    /// Где был прошлый мазок прохода (координаты глины): следующий — когда мышь ушла
    /// дальше трети радиуса.
    last: Option<Vec3>,
    /// Кнопку держат: проход идёт.
    stroke: bool,
    /// Запас коробки глины вокруг формы (доля длинной стороны).
    pub room: f32,
}

impl Default for ClayUi {
    fn default() -> Self {
        Self {
            on: true,
            brush: Brush::Add,
            size: 4.0,
            strength: 0.5,
            last: None,
            stroke: false,
            room: 0.2,
        }
    }
}

/// Кисти по порядку на кнопках.
pub(super) const BRUSHES: [Brush; 3] = [Brush::Add, Brush::Push, Brush::Smooth];

/// Кнопка-значок кисти: поверхность с бугром, с ямкой или ровная после волн.
pub(super) fn brush_button(ui: &mut Ui, brush: Brush, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 24.0), egui::Sense::click());
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, RADIUS_S, ACCENT.gamma_multiply(0.25));
    } else if resp.hovered() {
        painter.rect_filled(rect, RADIUS_S, HOVER);
    }
    let col = if on || resp.hovered() { TEXT } else { MUTED };
    let stroke = Stroke::new(1.5, col);
    let c = rect.center();
    // Линия поверхности слева направо, высота по точке.
    let line = |h: &dyn Fn(f32) -> f32| -> Vec<Pos2> {
        (0..=16)
            .map(|k| {
                let x = -9.0 + k as f32 * 18.0 / 16.0;
                c + egui::vec2(x, 4.0 - h(x))
            })
            .collect()
    };
    let bump = |x: f32, s: f32| (1.0 - (x / 5.0).powi(2)).max(0.0) * s;
    match brush {
        Brush::Add => {
            painter.add(egui::Shape::line(line(&|x| bump(x, 7.0)), stroke));
        }
        Brush::Push => {
            painter.add(egui::Shape::line(line(&|x| -bump(x, 5.0) + 3.0), stroke));
        }
        Brush::Smooth => {
            painter.add(egui::Shape::line(
                line(&|x| {
                    if x < 0.0 {
                        (x * 1.4).sin() * 3.0 + 3.0
                    } else {
                        3.0
                    }
                }),
                stroke,
            ));
        }
    }
    resp
}

/// Подсказка к кнопке кисти: имя и что делает.
pub(super) fn brush_hint(b: Brush) -> String {
    let (name, hint) = brush_name(b);
    format!("{name}: {hint}")
}

/// Кисть по-человечески.
fn brush_name(b: Brush) -> (&'static str, &'static str) {
    match b {
        Brush::Add => ("Нарастить", "Глина нарастает бугром под кистью"),
        Brush::Push => ("Вдавить", "Кисть вдавливает ямку"),
        Brush::Smooth => ("Сгладить", "Бугры и ямки под кистью выравниваются"),
    }
}

/// Глина под мышью: точка в координатах модели, та же точка в координатах глины и
/// радиус кисти в них.
struct Spot {
    model: Vec3,
    local: Vec3,
    radius: f32,
    /// Сколько глины в одной единице модели (масштаб сдвигов вокруг детали).
    scale: f32,
}

impl EditorUi {
    /// Файл глины, которую сейчас лепят: кисть включена, открыта модель (не сцена),
    /// выбрана глиняная деталь.
    pub(super) fn clay_file(&self, shape: &ShapeInfo) -> Option<String> {
        if !self.clay.on || shape.scene || !shape.preview.is_empty() {
            return None;
        }
        let root = shape_edit::parse(shape.text).ok()?;
        clay_edit::file_of(shape.text, root.at(&self.shape_selected)?)
    }

    /// Карточка глины у выбранной детали: у глины — кисть, у остальных — «Превратить».
    pub(super) fn clay_card(&mut self, ui: &mut Ui, text: &str, part: &Part, path: &[usize]) {
        let room_slider = |ui: &mut Ui, room: &mut f32| {
            ui.add(
                egui::Slider::new(room, 0.05..=1.0)
                    .text("запас")
                    .custom_formatter(|v, _| format!("{:.0} %", v * 100.0)),
            )
            .on_hover_text(
                "Насколько коробка глины шире формы с каждой стороны (доля длинной \
                 стороны): за край коробки глина не нарастает. Точек в сетке столько же, \
                 поэтому чем больше запас, тем крупнее клетка и мягче мелкие детали",
            );
        };
        if let Some(file) = clay_edit::file_of(text, part) {
            let c = &mut self.clay;
            let mut grow = false;
            card(ui, "Лепка", |ui| {
                ui.checkbox(&mut c.on, "Лепить кистью").on_hover_text(
                    "Левая кнопка на глине лепит. Выключите, чтобы снова тянуть ручки",
                );
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP_XS;
                    for b in BRUSHES {
                        if brush_button(ui, b, c.brush == b)
                            .on_hover_text(brush_name(b).1)
                            .clicked()
                        {
                            c.brush = b;
                        }
                    }
                    ui.add_space(GAP_M);
                    ui.label(RichText::new(brush_name(c.brush).0).color(TEXT));
                });
                let brick = clay_edit::brick(&file);
                let cell = brick.as_ref().map_or(0.0, |b| b.0.cell());
                ui.add(
                    egui::Slider::new(&mut c.size, 1.5..=16.0)
                        .text("размер")
                        .custom_formatter(|v, _| format!("{:.1} см", v as f32 * cell * 100.0)),
                );
                ui.add(
                    egui::Slider::new(&mut c.strength, 0.05..=1.0)
                        .text("сила")
                        .custom_formatter(|v, _| format!("{:.0} %", v * 100.0)),
                );
                ui.add_space(GAP_S);
                if let Some(b) = &brick {
                    let (lo, hi) = b.0.bounds_box();
                    let d = (hi - lo) * 100.0;
                    ui.label(
                        RichText::new(format!(
                            "Коробка глины {:.0}×{:.0}×{:.0} см, клетка {:.1} мм",
                            d.x,
                            d.y,
                            d.z,
                            cell * 1000.0
                        ))
                        .color(MUTED)
                        .size(FONT_SMALL),
                    );
                }
                room_slider(ui, &mut c.room);
                grow = ui
                    .button("Расширить коробку")
                    .on_hover_text(
                        "Коробка глины станет шире на запас с каждой стороны: можно \
                         нарастить дальше. Ctrl+Z вернёт как было",
                    )
                    .clicked();
                ui.label(
                    RichText::new(format!("Файл: models/{file}. Клавиши — в значке «?»"))
                        .color(MUTED)
                        .size(FONT_SMALL),
                );
            });
            if grow {
                self.actions.push(Action::ClayGrow {
                    file,
                    room: self.clay.room,
                });
            }
            return;
        }
        if !clay_edit::can_turn(part) {
            return;
        }
        let mut turn = false;
        let room = &mut self.clay.room;
        card(ui, "Глина", |ui| {
            room_slider(ui, room);
            turn = ui
                .button("Превратить в глину")
                .on_hover_text(
                    "Деталь станет куском глины: её можно мять кистью. Размеры и ручки \
                     у неё пропадут, форма останется. Ctrl+Z вернёт как было",
                )
                .clicked();
        });
        if turn {
            self.actions.push(Action::ToClay {
                path: path.to_vec(),
                room: self.clay.room,
            });
        }
    }

    /// Глина под точкой экрана `pos`.
    fn clay_spot(&self, ui: &Ui, shape: &ShapeInfo, file: &str, pos: Pos2) -> Option<Spot> {
        let (o, d) = self.scene_ray(ui, shape, pos);
        let p = shapes::hit(shape.text, shape.hidden, o, d, shape.camera.far)?;
        let frame = shapes::part_frame(shape.text, &self.shape_selected)?;
        let brick = clay_edit::brick(file)?;
        let local = frame.inverse().transform_point3(p);
        // Попали не в эту глину, а в другую деталь.
        if brick.0.sample(local).abs() > brick.0.cell() * 2.0 {
            return None;
        }
        let scale = frame.matrix3.x_axis.length().max(1e-6);
        Some(Spot {
            model: p,
            local,
            radius: self.clay.size * brick.0.cell(),
            scale,
        })
    }

    /// Мышь на подставке, пока лепят. Ответ — мышь занята кистью (выбор и ручки ждут).
    pub(super) fn clay_pointer(&mut self, ui: &Ui, response: &Response, shape: &ShapeInfo) -> bool {
        let Some(file) = self.clay_file(shape) else {
            if self.clay.stroke {
                self.clay.stroke = false;
                self.actions.push(Action::ClayEnd);
            }
            return false;
        };
        // Размер кисти колесом с Alt.
        if response.hovered() && ui.input(|i| i.modifiers.alt) {
            let steps = ui.input(super::wheel_steps);
            if steps != 0.0 {
                self.clay.size = (self.clay.size * 1.15f32.powf(steps)).clamp(1.5, 16.0);
            }
        }
        let held = response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
        let pos = response
            .hover_pos()
            .or_else(|| response.interact_pointer_pos());
        let spot = pos.and_then(|p| self.clay_spot(ui, shape, &file, p));
        if let Some(s) = &spot {
            self.clay_ring(ui, shape, s);
        }
        if !held {
            if self.clay.stroke {
                self.clay.stroke = false;
                self.clay.last = None;
                self.actions.push(Action::ClayEnd);
            }
            return spot.is_some();
        }
        if !self.clay.stroke {
            // Нажали мимо глины — это не лепка: выбор и ручки как обычно.
            if spot.is_none() {
                return false;
            }
            self.clay.stroke = true;
            self.clay.last = None;
        }
        if let Some(s) = spot {
            let far = self
                .clay
                .last
                .is_none_or(|l| (l - s.local).length() > s.radius * 0.3);
            if far {
                let kind = ui.input(|i| {
                    if i.modifiers.shift {
                        Brush::Smooth
                    } else if i.modifiers.command {
                        match self.clay.brush {
                            Brush::Add => Brush::Push,
                            Brush::Push => Brush::Add,
                            Brush::Smooth => Brush::Smooth,
                        }
                    } else {
                        self.clay.brush
                    }
                });
                self.clay.last = Some(s.local);
                self.actions.push(Action::ClayDab(Dab {
                    file,
                    kind,
                    at: s.local,
                    radius: s.radius,
                    strength: self.clay.strength,
                }));
            }
        }
        true
    }

    /// Кружок кисти на подставке: где и какого размера.
    fn clay_ring(&self, ui: &Ui, shape: &ShapeInfo, s: &Spot) {
        let screen = self.shape_screen(ui, shape);
        let aspect = screen.width() / screen.height().max(1.0);
        let shift = Vec3::from(shape.shift);
        let to_screen = |p: Vec3| {
            let n = shape.camera.project(p + shift, aspect)?;
            (n.z > 0.0 && n.z <= 1.0).then(|| {
                Pos2::new(
                    screen.left() + (n.x * 0.5 + 0.5) * screen.width(),
                    screen.top() + (0.5 - n.y * 0.5) * screen.height(),
                )
            })
        };
        let side = (s.model - shape.camera.eye + shift)
            .cross(Vec3::Y)
            .try_normalize()
            .unwrap_or(Vec3::X);
        let (Some(c), Some(e)) = (
            to_screen(s.model),
            to_screen(s.model + side * s.radius * s.scale),
        ) else {
            return;
        };
        let r = (e - c).length().max(3.0);
        let color = match self.clay.brush {
            Brush::Add => GOOD,
            Brush::Push => BAD,
            Brush::Smooth => ACCENT,
        };
        let painter = ui.painter_at(screen);
        painter.circle_stroke(c, r, Stroke::new(2.0, color));
        painter.circle_filled(c, 2.0, color);
    }
}
