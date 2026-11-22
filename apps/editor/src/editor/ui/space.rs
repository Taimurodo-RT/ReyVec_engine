//! Сцена 3D в «Формах» (М.8б): новые предметы (из палитры и моделями из «Ассетов»),
//! лампы (кружки в окне, их тянут мышью) и материалы сцены. Правки — текстом файла
//! сцены (`space_edit`), как и у предметов.

use std::time::{Duration, Instant};

use egui::{Align, Color32, CursorIcon, Layout, Pos2, Rect, Response, RichText, Stroke, Ui, vec2};
use glam::Vec3;

use super::super::assets::AssetFile;
use super::super::ron_edit::{self, EditError};
use super::super::shape_edit::{self, Part};
use super::super::shapes;
use super::super::space_edit;
use super::super::theme::*;
use super::{Action, EditorUi, FieldEdit, ShapeInfo, card, chip, prop, step_field, vec3_fields};

/// Сколько висит подсказка в окне.
const NOTE: Duration = Duration::from_secs(5);

/// Радиус кружка лампы в окне, точки.
const LAMP_DOT: f32 = 7.0;

/// Что помнит окно сцены 3D между кадрами.
#[derive(Default)]
pub(crate) struct SceneUi {
    /// Выбранная лампа (номер в списке `lamps`).
    pub lamp: Option<usize>,
    drag: Option<LampDrag>,
    /// Подсказка в окне (например, почему модель не встала).
    note: Option<(String, Instant)>,
    /// Имя материала в поле правки (номер, текст).
    material_name: Option<(usize, String)>,
}

/// Лампу тянут мышью.
struct LampDrag {
    index: usize,
    text: String,
    at: Vec3,
    /// По вертикали (Shift в начале), иначе по полу.
    vertical: bool,
    /// Плоскость, по которой ходит лампа (нормаль): пол на её высоте, а если камера
    /// смотрит почти вдоль пола или тянут вверх — стена к камере.
    normal: Vec3,
    /// Точка плоскости под мышью в начале.
    grab: Vec3,
}

/// Точка, где луч встречает плоскость через `point` с нормалью `normal`.
fn plane_hit(o: Vec3, d: Vec3, point: Vec3, normal: Vec3) -> Option<Vec3> {
    let den = d.dot(normal);
    if den.abs() < 1e-6 {
        return None;
    }
    let t = (point - o).dot(normal) / den;
    (t > 0.0).then(|| o + d * t)
}

impl LampDrag {
    /// Начать тянуть лампу с места `at` лучом `o + d·t`.
    fn new(index: usize, text: String, at: Vec3, vertical: bool, o: Vec3, d: Vec3) -> Self {
        let mut wall = at - o;
        wall.y = 0.0;
        let wall = wall.try_normalize().unwrap_or(Vec3::Z);
        // Вдоль пола под малым углом точка на нём убегает далеко: тогда — по стене.
        let normal = if vertical || d.y.abs() < 0.2 {
            wall
        } else {
            Vec3::Y
        };
        let grab = plane_hit(o, d, at, normal).unwrap_or(at);
        Self {
            index,
            text,
            at,
            vertical,
            normal,
            grab,
        }
    }

    fn at(&self, o: Vec3, d: Vec3) -> Option<Vec3> {
        let p = plane_hit(o, d, self.at, self.normal)?;
        let moved = self.at + (p - self.grab);
        Some(if self.vertical {
            Vec3::new(self.at.x, moved.y, self.at.z)
        } else {
            Vec3::new(moved.x, self.at.y, moved.z)
        })
    }
}

fn lamp_color(l: &reyvec_sdf::space_file::LampEntry) -> Color32 {
    let [r, g, b] = space_edit::srgb(&l.color);
    Color32::from_rgb(r, g, b)
}

impl EditorUi {
    /// Луч из точки экрана в координатах сцены.
    pub(super) fn scene_ray(&self, ui: &Ui, shape: &ShapeInfo, pos: Pos2) -> (Vec3, Vec3) {
        let screen = self.shape_screen(ui, shape);
        let aspect = screen.width() / screen.height().max(1.0);
        let x = (pos.x - screen.left()) / screen.width() * 2.0 - 1.0;
        let y = 1.0 - (pos.y - screen.top()) / screen.height() * 2.0;
        let (o, d) = shape.camera.ray([x, y], aspect);
        (o - Vec3::from(shape.shift), d)
    }

    /// Куда в сцене попадает точка экрана: поверхность предмета, иначе пол, иначе три
    /// метра перед камерой.
    fn scene_point(&self, ui: &Ui, shape: &ShapeInfo, pos: Pos2) -> Vec3 {
        let (o, d) = self.scene_ray(ui, shape, pos);
        let far = shape.camera.far;
        shapes::hit(shape.text, shape.hidden, o, d, far)
            .or_else(|| space_edit::on_floor(o, d, far))
            .unwrap_or(o + d * 3.0)
    }

    /// Подсказка в окне на несколько секунд.
    fn scene_note(&mut self, text: impl Into<String>) {
        self.space.note = Some((text.into(), Instant::now()));
    }

    /// Правка сцены: в файл или подсказкой, почему нельзя.
    fn scene_send(&mut self, r: Result<String, EditError>, commit: bool) {
        match r {
            Ok(text) => self.actions.push(Action::ShapeText { text, commit }),
            Err(e) => self.scene_note(e.0),
        }
    }

    /// Модель из «Ассетов» тащат в окно сцены: рамка, отпустили — новый предмет там, куда
    /// показывает мышь (на поверхность предмета или на пол).
    pub(super) fn scene_drop(
        &mut self,
        ui: &Ui,
        response: &Response,
        shape: &ShapeInfo,
        rect: Rect,
    ) {
        let is_model = |f: &AssetFile| {
            f.0.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(shapes::is_shape)
        };
        if response
            .dnd_hover_payload::<AssetFile>()
            .is_some_and(|f| is_model(&f))
        {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                0,
                Stroke::new(LINE_FOCUS, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        let Some(f) = response.dnd_release_payload::<AssetFile>() else {
            return;
        };
        if !is_model(&f) {
            return;
        }
        let Some(pos) = response.hover_pos() else {
            return;
        };
        let Some(model) = space_edit::model_for_scene(shape.root, &f.0) else {
            self.scene_note(format!(
                "Модель не встала: сцена берёт модели только из своей папки models ({})",
                shape.root.join("models").display()
            ));
            return;
        };
        let at = space_edit::rounded(self.scene_point(ui, shape, pos));
        match space_edit::add_model(shape.text, &model, at) {
            Ok((text, i)) => {
                self.shape_selected = vec![i];
                self.space.lamp = None;
                self.actions.push(Action::ShapeText { text, commit: true });
            }
            Err(e) => self.scene_note(format!("Модель не встала: {}", e.0)),
        }
    }

    /// Кружки ламп в окне. Ответ — где какая лампа на экране.
    pub(super) fn lamp_dots(&self, ui: &Ui, shape: &ShapeInfo, rect: Rect) -> Vec<(usize, Pos2)> {
        let screen = self.shape_screen(ui, shape);
        let aspect = screen.width() / screen.height().max(1.0);
        let painter = ui.painter_at(rect);
        let shift = Vec3::from(shape.shift);
        let mut out = Vec::new();
        for (i, lamp) in space_edit::lamps(shape.text).iter().enumerate() {
            let Some(l) = lamp else { continue };
            let Some(n) = shape.camera.project(Vec3::from(l.at) + shift, aspect) else {
                continue;
            };
            if !(n.z > 0.0 && n.z <= 1.0) {
                continue;
            }
            let p = Pos2::new(
                screen.left() + (n.x * 0.5 + 0.5) * screen.width(),
                screen.top() + (0.5 - n.y * 0.5) * screen.height(),
            );
            let selected = self.space.lamp == Some(i) && self.shape_selected.is_empty();
            let ring = if selected { ACCENT } else { TEXT };
            // Лучики, кружок цвета лампы и обводка.
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let dir = vec2(a.cos(), a.sin());
                painter.line_segment(
                    [p + dir * (LAMP_DOT + 2.0), p + dir * (LAMP_DOT + 6.0)],
                    Stroke::new(1.5, ring),
                );
            }
            painter.circle_filled(p, LAMP_DOT, lamp_color(l));
            painter.circle_stroke(
                p,
                LAMP_DOT,
                Stroke::new(if selected { 2.5 } else { 1.5 }, ring),
            );
            out.push((i, p));
        }
        out
    }

    /// Мышь у ламп: щелчок выбирает лампу, левая тянет её по полу (с Shift — вверх и
    /// вниз). `true` — мышь занята лампой, деталям её не отдавать.
    pub(super) fn lamp_pointer(
        &mut self,
        ui: &Ui,
        response: &Response,
        shape: &ShapeInfo,
        dots: &[(usize, Pos2)],
    ) -> bool {
        if let Some(drag) = &self.space.drag {
            if response.dragged_by(egui::PointerButton::Primary) {
                if let Some(pos) = response.interact_pointer_pos() {
                    let (o, d) = self.scene_ray(ui, shape, pos);
                    if let Some(at) = drag.at(o, d) {
                        let r =
                            space_edit::move_lamp(&drag.text, drag.index, space_edit::rounded(at));
                        self.scene_send(r, false);
                    }
                }
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            } else {
                self.space.drag = None;
                self.actions.push(Action::EndEdit);
            }
            return true;
        }
        let hover = response.hover_pos();
        let under = |p: Option<Pos2>| {
            let p = p?;
            dots.iter()
                .filter(|(_, d)| d.distance(p) <= LAMP_DOT + 4.0)
                .min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p)))
                .map(|(i, _)| *i)
        };
        if under(hover).is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        // Тянуть начинают, сдвинув мышь: лампа — та, где кнопку нажали.
        let pressed = ui.input(|i| i.pointer.press_origin());
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(i) = under(pressed)
            && let Some(Some(l)) = space_edit::lamps(shape.text).get(i)
            && let Some(pos) = pressed
        {
            let vertical = ui.input(|i| i.modifiers.shift);
            let (o, d) = self.scene_ray(ui, shape, pos);
            let drag = LampDrag::new(i, shape.text.to_owned(), Vec3::from(l.at), vertical, o, d);
            self.space.lamp = Some(i);
            self.shape_selected.clear();
            self.space.drag = Some(drag);
            return true;
        }
        if response.clicked_by(egui::PointerButton::Primary)
            && let Some(i) = under(response.interact_pointer_pos())
        {
            self.space.lamp = Some(i);
            self.shape_selected.clear();
            return true;
        }
        false
    }

    /// Подсказка в углу окна сцены, пока не устарела.
    pub(super) fn scene_note_chip(&mut self, painter: &egui::Painter, at: Pos2) {
        if let Some((text, when)) = &self.space.note {
            if when.elapsed() > NOTE {
                self.space.note = None;
            } else {
                chip(painter, at, text, ACCENT);
            }
        }
    }

    /// Лампы строками под предметами в иерархии.
    pub(super) fn lamp_rows(&mut self, ui: &mut Ui, text: &str) {
        let lamps = space_edit::lamps(text);
        if lamps.is_empty() {
            return;
        }
        ui.add_space(GAP_M);
        ui.label(RichText::new("Лампы").color(MUTED).size(FONT_SMALL));
        for i in 0..lamps.len() {
            let on = self.space.lamp == Some(i) && self.shape_selected.is_empty();
            if ui
                .selectable_label(on, format!("    Лампа {}", i + 1))
                .on_hover_text("Выбрать лампу (её кружок в окне тянут мышью)")
                .clicked()
            {
                self.space.lamp = Some(i);
                self.shape_selected.clear();
            }
        }
    }

    /// Инспектор всей сцены: новый предмет, материалы, лампы.
    pub(super) fn scene_inspector(&mut self, ui: &mut Ui, shape: &ShapeInfo, root: &Part) {
        if self.shape_selected.is_empty()
            && let Some(i) = self.space.lamp
            && i < space_edit::lamps(shape.text).len()
        {
            self.lamp_inspector(ui, shape, i);
            return;
        }
        ui.label(RichText::new("Сцена 3D").size(FONT_TITLE).color(TEXT));
        ui.label(
            RichText::new(format!(
                "Предметов: {}. Выберите предмет щелчком в окне или в иерархии.",
                root.children.len()
            ))
            .color(MUTED)
            .size(FONT_SMALL),
        );
        ui.add_space(GAP_M);
        let mut new = None;
        card(ui, "Новый предмет", |ui| {
            ui.label(
                RichText::new(
                    "Модель — перетащите её файл из «Ассетов» в окно: встанет, куда показывает \
                     мышь. Простая форма — кнопкой: встанет на пол посреди окна.",
                )
                .color(MUTED)
                .size(FONT_SMALL),
            );
            ui.horizontal_wrapped(|ui| {
                for kind in shape_edit::PALETTE {
                    if ui
                        .button(format!("+ {}", shape_edit::label(kind)))
                        .clicked()
                    {
                        new = Some(kind);
                    }
                }
            });
        });
        if let Some(kind) = new
            && let Some(rect) = self.viewport
        {
            let p = self.scene_point(ui, shape, rect.center());
            // Простая форма 20 см — серединой на 10 см выше точки, чтобы стоять на полу.
            let at = space_edit::rounded(p + Vec3::Y * 0.1);
            let shape_text = shape_edit::part_shape(kind, 0.2);
            match space_edit::add_object(shape.text, shape_edit::label(kind), &shape_text, at) {
                Ok((text, i)) => {
                    self.shape_selected = vec![i];
                    self.actions.push(Action::ShapeText { text, commit: true });
                }
                Err(e) => self.scene_note(e.0),
            }
        }
        self.materials_card(ui, shape.text);
        let lamps = space_edit::lamps(shape.text);
        let mut add = false;
        let mut pick = None;
        card(ui, "Лампы", |ui| {
            for (i, l) in lamps.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
                    let c = l.as_ref().map_or(MUTED, lamp_color);
                    ui.painter().circle_filled(rect.center(), 5.0, c);
                    if ui.link(format!("Лампа {}", i + 1)).clicked() {
                        pick = Some(i);
                    }
                    if let Some(l) = l {
                        ui.label(
                            RichText::new(format!("сила {}, на {} м", l.power, l.range))
                                .color(MUTED)
                                .size(FONT_SMALL),
                        );
                    }
                });
            }
            if ui
                .button("+ Лампа")
                .on_hover_text("Новая лампа над серединой окна, в 1,5 м от пола")
                .clicked()
            {
                add = true;
            }
        });
        if let Some(i) = pick {
            self.space.lamp = Some(i);
        }
        if add && let Some(rect) = self.viewport {
            let mut p = self.scene_point(ui, shape, rect.center());
            p.y = p.y.max(0.0) + 1.5;
            let r = space_edit::add_lamp(shape.text, space_edit::rounded(p));
            if r.is_ok() {
                self.space.lamp = Some(lamps.len());
            }
            self.scene_send(r, true);
        }
    }

    /// Карточка «Материалы»: имя, цвет, свечение, сетка; новый и убрать.
    fn materials_card(&mut self, ui: &mut Ui, text: &str) {
        let all = space_edit::materials(text);
        let naming = &mut self.space.material_name;
        let mut out: Vec<(Result<String, EditError>, bool)> = Vec::new();
        let mut end = false;
        let mut add = false;
        card(ui, "Материалы", |ui| {
            for (i, m) in all.iter().enumerate() {
                let Some(m) = m else {
                    ui.label(RichText::new("Материал не читается: правьте файл").color(BAD));
                    continue;
                };
                ui.horizontal(|ui| {
                    let mut c = space_edit::srgb(&m.color);
                    let r = egui::color_picker::color_edit_button_srgb(ui, &mut c);
                    if r.changed() {
                        out.push((
                            space_edit::set_material(text, i, "color", &space_edit::hex(c)),
                            false,
                        ));
                    }
                    let mut buf = match naming {
                        Some((at, b)) if *at == i => b.clone(),
                        _ => m.name.clone(),
                    };
                    let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(110.0));
                    if r.has_focus() {
                        *naming = Some((i, buf.clone()));
                    }
                    if r.lost_focus() {
                        *naming = None;
                        if buf != m.name {
                            out.push((space_edit::rename_material(text, i, &buf), true));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let uses = space_edit::uses_of(text, &m.name);
                        let hint = if uses == 0 {
                            "Убрать материал".to_owned()
                        } else {
                            format!("Им покрашено предметов: {uses}. Сначала смените им материал")
                        };
                        if ui
                            .add_enabled(
                                uses == 0,
                                egui::Button::new(RichText::new("✕").color(BAD)),
                            )
                            .on_hover_text(&hint)
                            .on_disabled_hover_text(&hint)
                            .clicked()
                        {
                            out.push((space_edit::remove_material(text, i), true));
                        }
                        let mut grid = m.grid;
                        if ui
                            .checkbox(&mut grid, "сетка")
                            .on_hover_text("Сетка по материалу (как у пола)")
                            .changed()
                        {
                            out.push((space_edit::set_grid(text, i, grid), true));
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.add_space(26.0);
                    prop(ui, "Свечение");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match step_field(
                        ui,
                        f64::from(m.glow),
                        0.02,
                        0.5,
                        0.0,
                        None,
                    ) {
                        Some(FieldEdit::Value(x, commit)) => {
                            out.push((space_edit::set_glow(text, i, x), commit));
                        }
                        Some(FieldEdit::End) => end = true,
                        None => {}
                    });
                });
            }
            if ui.button("+ Материал").clicked() {
                add = true;
            }
        });
        // Цвет в палитре правят протяжкой: конец правки — когда кнопку мыши отпустили.
        if ui.input(|i| i.pointer.any_released()) && !out.iter().any(|(_, c)| *c) {
            end = true;
        }
        if add {
            out.push((space_edit::add_material(text).map(|(t, _)| t), true));
        }
        if let Some((r, commit)) = out.pop() {
            self.scene_send(r, commit);
        }
        if end {
            self.actions.push(Action::EndEdit);
        }
    }

    /// Инспектор лампы: место, цвет, сила, дальность; убрать.
    fn lamp_inspector(&mut self, ui: &mut Ui, shape: &ShapeInfo, i: usize) {
        let text = shape.text;
        let Some(Some(l)) = space_edit::lamps(text).get(i).cloned() else {
            ui.label(RichText::new("Лампа не читается: правьте файл").color(BAD));
            return;
        };
        ui.label(
            RichText::new(format!("Лампа {}", i + 1))
                .size(FONT_TITLE)
                .color(TEXT),
        );
        ui.label(
            RichText::new("Кружок лампы в окне тянут мышью по полу, с Shift — вверх и вниз.")
                .color(MUTED)
                .size(FONT_SMALL),
        );
        ui.add_space(GAP_M);
        let mut out: Vec<(Result<String, EditError>, bool)> = Vec::new();
        let mut end = false;
        card(ui, "Лампа", |ui| {
            prop(ui, "Положение, м");
            let at = l.at.map(f64::from);
            match vec3_fields(ui, at, 0.005, 0.1) {
                Some((Some((x, commit)), _)) => {
                    out.push((space_edit::move_lamp(text, i, x), commit))
                }
                Some((None, true)) => end = true,
                _ => {}
            }
            ui.horizontal(|ui| {
                prop(ui, "Цвет");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let mut c = space_edit::srgb(&l.color);
                    if egui::color_picker::color_edit_button_srgb(ui, &mut c).changed() {
                        out.push((
                            space_edit::set_lamp(text, i, "color", &space_edit::hex(c)),
                            false,
                        ));
                    }
                });
            });
            for (field, name, v, speed, step) in [
                ("power", "Сила", l.power, 0.02, 0.5),
                ("range", "Светит на, м", l.range, 0.02, 0.5),
            ] {
                ui.horizontal(|ui| {
                    prop(ui, name);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match step_field(
                        ui,
                        f64::from(v),
                        speed,
                        step,
                        0.0,
                        None,
                    ) {
                        Some(FieldEdit::Value(x, commit)) => out.push((
                            space_edit::set_lamp(text, i, field, &ron_edit::num(x.max(0.0))),
                            commit,
                        )),
                        Some(FieldEdit::End) => end = true,
                        None => {}
                    });
                });
            }
        });
        if ui.input(|i| i.pointer.any_released()) && !out.iter().any(|(_, c)| *c) {
            end = true;
        }
        if ui
            .button(RichText::new("Убрать лампу").color(BAD))
            .on_hover_text("Убрать лампу из сцены (Delete, Ctrl+Z вернёт)")
            .clicked()
        {
            out.push((space_edit::remove_lamp(text, i), true));
            self.space.lamp = None;
        }
        if ui.button("← Вся сцена").clicked() {
            self.space.lamp = None;
        }
        if let Some((r, commit)) = out.pop() {
            self.scene_send(r, commit);
        }
        if end {
            self.actions.push(Action::EndEdit);
        }
    }

    /// Delete при выбранной лампе: убрать её. `true` — клавиша ушла лампе.
    pub(super) fn lamp_delete(&mut self, text: &str) -> bool {
        let Some(i) = self.space.lamp.filter(|_| self.shape_selected.is_empty()) else {
            return false;
        };
        let r = space_edit::remove_lamp(text, i);
        self.space.lamp = None;
        self.scene_send(r, true);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamp_moves_along_the_floor_or_up() {
        let lamp = Vec3::new(0.0, 2.0, 0.0);
        let eye = Vec3::new(0.0, 4.0, 4.0);
        let ray = |to: Vec3| (to - eye).normalize();
        // Сверху под углом — по полу на высоте лампы.
        let d = LampDrag::new(0, String::new(), lamp, false, eye, ray(lamp));
        assert_eq!(d.normal, Vec3::Y);
        let at = d.at(eye, ray(Vec3::new(1.0, 2.0, 0.0))).unwrap();
        assert!((at - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-4, "{at}");
        // С Shift — только вверх и вниз.
        let d = LampDrag::new(0, String::new(), lamp, true, eye, ray(lamp));
        let at = d.at(eye, ray(Vec3::new(0.3, 3.0, 0.0))).unwrap();
        assert!((at - Vec3::new(0.0, 3.0, 0.0)).length() < 1e-4, "{at}");
        // Камера смотрит вдоль пола — лампа ходит вбок по стене к камере, высота та же.
        let eye = Vec3::new(0.0, 2.0, 4.0);
        let ray = |to: Vec3| (to - eye).normalize();
        let d = LampDrag::new(0, String::new(), lamp, false, eye, ray(lamp));
        let at = d.at(eye, ray(Vec3::new(0.5, 2.2, 0.0))).unwrap();
        assert!((at - Vec3::new(0.5, 2.0, 0.0)).length() < 1e-4, "{at}");
    }
}
