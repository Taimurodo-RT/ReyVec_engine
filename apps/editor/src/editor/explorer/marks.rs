//! Цветные метки в проводнике (Э.5): точки на плитках и в списке, пункт «Метки» в меню,
//! кружки в свойствах, раздел «Метки» в дереве (все файлы с меткой из всех папок) и окно
//! названий. Ctrl+1…7 ставит или снимает метку у выбранного. Хранение — в
//! [`crate::editor::labels`].

use super::*;
use crate::editor::labels::{COLORS, COUNT, Labels};

/// Поперечник точки метки на плитке и в списке.
const DOT: f32 = 8.0;

impl Explorer {
    pub(super) fn reload_labels(&mut self) {
        self.labels = self
            .project
            .as_deref()
            .map(Labels::load)
            .unwrap_or_default();
    }

    /// Метки файла.
    pub(super) fn marks(&self, path: &Path) -> Vec<u8> {
        self.project
            .as_deref()
            .map(|p| self.labels.of(p, path).to_vec())
            .unwrap_or_default()
    }

    /// Поставить метку `i` выбранным или снять, если она уже у всех них.
    pub(super) fn toggle_label(&mut self, i: u8) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let paths = self.selected.clone();
        if paths.is_empty() {
            return;
        }
        let on = !self.labels.all_have(&project, &paths, i);
        self.labels.set(&project, &paths, i, on);
        let name = self.labels.name(i as usize);
        let text = if on {
            format!("Метка «{name}»: {}", things_word(paths.len()))
        } else {
            format!("Метка «{name}» снята: {}", things_word(paths.len()))
        };
        self.save_labels(text);
    }

    /// Снять с выбранного все метки.
    pub(super) fn clear_labels(&mut self) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let paths = self.selected.clone();
        for i in 0..COUNT as u8 {
            self.labels.set(&project, &paths, i, false);
        }
        self.save_labels(format!("Метки сняты: {}", things_word(paths.len())));
    }

    fn save_labels(&mut self, text: String) {
        let Some(project) = self.project.clone() else {
            return;
        };
        match self.labels.save(&project) {
            Ok(()) => {
                self.say(text, false);
                if self.label_view.is_some() {
                    self.refresh();
                }
            }
            Err(e) => self.say(e, true),
        }
    }

    /// Показать все файлы проекта с меткой `i`.
    pub(super) fn open_label(&mut self, i: u8) {
        self.close_bin();
        self.label_view = Some(i);
        self.selected.clear();
        self.anchor = None;
        self.renaming = None;
        self.search.clear();
        self.refresh();
    }

    /// Вернуться из списка меченых к папке.
    pub(super) fn close_label(&mut self) {
        if self.label_view.take().is_some() {
            self.selected.clear();
            self.refresh();
        }
    }

    /// Заголовок списка меченых.
    pub(super) fn label_title(&self, i: u8) -> String {
        format!("Метка «{}»", self.labels.name(i as usize))
    }

    /// Пункты подменю «Метки» у файла.
    pub(super) fn label_menu(&mut self, ui: &mut Ui) {
        let Some(project) = self.project.clone() else {
            return;
        };
        for i in 0..COUNT as u8 {
            let on = self.labels.all_have(&project, &self.selected, i);
            let r = ui.add(
                egui::Button::new(format!(
                    "{}  {}",
                    if on { "✓" } else { "  " },
                    self.labels.name(i as usize)
                ))
                .shortcut_text(format!("Ctrl+{}", i + 1))
                .min_size(vec2(180.0, 0.0)),
            );
            ui.painter().circle_filled(
                pos2(r.rect.left() + 34.0, r.rect.center().y),
                DOT / 2.0,
                COLORS[i as usize],
            );
            if r.clicked() {
                self.toggle_label(i);
                ui.close();
            }
        }
        ui.separator();
        let any = self.selected.iter().any(|p| !self.marks(p).is_empty());
        if ui
            .add_enabled(any, egui::Button::new("Снять все метки"))
            .clicked()
        {
            self.clear_labels();
            ui.close();
        }
        if ui.button("Названия меток…").clicked() {
            self.naming = Some((0..COUNT).map(|i| self.labels.name(i)).collect());
            ui.close();
        }
    }

    /// Кружки меток в свойствах: щелчок ставит или снимает метку у выбранного.
    pub(super) fn label_picker(&mut self, ui: &mut Ui) {
        let Some(project) = self.project.clone() else {
            return;
        };
        ui.add_space(GAP_XL);
        ui.label(RichText::new("Метки").color(TEXT).strong());
        ui.add_space(GAP_S);
        let mut toggle = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_S;
            for i in 0..COUNT as u8 {
                let on = self.labels.all_have(&project, &self.selected, i);
                let some = !on
                    && self
                        .selected
                        .iter()
                        .any(|p| self.labels.of(&project, p).contains(&i));
                let (rect, r) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                let c = COLORS[i as usize];
                let p = ui.painter();
                let radius = if r.hovered() { 9.0 } else { 8.0 };
                if on {
                    p.circle_filled(rect.center(), radius, c);
                    p.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        "✓",
                        FontId::proportional(FONT_SMALL),
                        Color32::BLACK,
                    );
                } else {
                    p.circle_filled(rect.center(), radius, c.gamma_multiply(0.18));
                    p.circle_stroke(rect.center(), radius - 0.5, Stroke::new(LINE_FOCUS, c));
                    if some {
                        p.circle_filled(rect.center(), 3.0, c);
                    }
                }
                let hint = format!(
                    "{} (Ctrl+{}){}",
                    self.labels.name(i as usize),
                    i + 1,
                    if some {
                        "\nЕсть не у всех выбранных"
                    } else {
                        ""
                    }
                );
                if r.on_hover_text(hint).clicked() {
                    toggle = Some(i);
                }
            }
        });
        if let Some(i) = toggle {
            self.toggle_label(i);
        }
        let names: Vec<String> = (0..COUNT as u8)
            .filter(|&i| self.labels.all_have(&project, &self.selected, i))
            .map(|i| self.labels.name(i as usize))
            .collect();
        ui.add_space(GAP_XS);
        let text = if names.is_empty() {
            "Меток нет. Щёлкните по цвету, чтобы поставить".to_owned()
        } else {
            names.join(", ")
        };
        ui.label(RichText::new(text).small().color(MUTED));
    }

    /// Раздел «Метки» в дереве: метки, которые где-то стоят, и сколько файлов с ними.
    pub(super) fn label_tree(&mut self, ui: &mut Ui) {
        ui.add_space(GAP_L);
        ui.label(RichText::new("МЕТКИ").small().color(MUTED));
        ui.add_space(GAP_XS);
        let counts = self.labels.counts();
        if counts.iter().all(|&n| n == 0) {
            ui.label(
                RichText::new("Пока нет. Правая кнопка по файлу → «Метки»")
                    .small()
                    .color(MUTED),
            );
            return;
        }
        let mut open = None;
        let mut naming = false;
        for (i, &n) in counts.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            let current = self.label_view == Some(i as u8);
            let p = ui.painter();
            if current {
                p.rect_filled(rect, RADIUS_S, SELECT_BG);
            } else if r.hovered() {
                p.rect_filled(rect, RADIUS_S, HOVER);
            }
            let at = pos2(rect.left() + GAP_S + INDENT + 9.0, rect.center().y);
            p.circle_filled(at, DOT / 2.0 + 1.0, COLORS[i]);
            p.text(
                pos2(at.x + 12.0, rect.center().y),
                Align2::LEFT_CENTER,
                format!("{} ({n})", self.labels.name(i)),
                FontId::proportional(FONT_BODY),
                if current { Color32::WHITE } else { TEXT },
            );
            let r = r.on_hover_text("Все файлы проекта с этой меткой");
            if r.clicked() {
                open = Some(i as u8);
            }
            r.context_menu(|ui| {
                if ui.button("Названия меток…").clicked() {
                    naming = true;
                    ui.close();
                }
            });
        }
        if let Some(i) = open {
            self.open_label(i);
        }
        if naming {
            self.naming = Some((0..COUNT).map(|i| self.labels.name(i)).collect());
        }
    }

    /// Окно «Названия меток».
    pub(super) fn labels_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut names) = self.naming.take() else {
            return;
        };
        let mut close = false;
        let mut done = false;
        let modal = egui::Modal::new(egui::Id::unique("explorer-labels")).show(ctx, |ui| {
            ui.set_width(340.0);
            heading(ui, "Названия меток");
            ui.label(
                RichText::new("Например: «Готово», «Переделать», «Для уровня 2»")
                    .small()
                    .color(MUTED),
            );
            ui.add_space(GAP_L);
            for (i, name) in names.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), DOT / 2.0 + 1.0, COLORS[i]);
                    ui.add(egui::TextEdit::singleline(name).desired_width(260.0));
                });
            }
            ui.add_space(GAP_L);
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new("Готово").color(ON_ACCENT)).fill(ACCENT);
                if ui.add(b).clicked() {
                    done = true;
                }
                if ui.button("Отмена").clicked() {
                    close = true;
                }
            });
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                close = true;
            }
        });
        if modal.should_close() {
            close = true;
        }
        if done {
            for (i, n) in names.iter().enumerate() {
                self.labels.rename(i, n);
            }
            self.save_labels("Названия меток сохранены".into());
        } else if !close {
            self.naming = Some(names);
        }
    }
}

/// Точки меток справа налево от точки `right` (её середина по высоте).
pub(super) fn paint_marks(p: &egui::Painter, right: Pos2, marks: &[u8], back: Color32) {
    for (k, &i) in marks.iter().rev().enumerate() {
        let c = pos2(right.x - DOT / 2.0 - k as f32 * (DOT - 1.0), right.y);
        p.circle_filled(c, DOT / 2.0 + 1.0, back);
        p.circle_filled(c, DOT / 2.0, COLORS[i as usize % COUNT]);
    }
}
