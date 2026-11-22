//! Корзина проекта в проводнике (Э.4): что удалили, откуда и когда. Удалённое можно
//! вернуть на старое место (Ctrl+Z унесёт обратно) или стереть насовсем.

use super::*;
use crate::editor::project_files::Binned;

impl Explorer {
    /// Открыть корзину вместо папки.
    pub(super) fn open_bin(&mut self) {
        if self.in_bin {
            return;
        }
        self.in_bin = true;
        self.bin_picked.clear();
        self.renaming = None;
        self.band = None;
        self.reload_bin();
    }

    /// Вернуться из корзины к папке.
    pub(super) fn close_bin(&mut self) {
        if self.in_bin {
            self.in_bin = false;
            self.bin_picked.clear();
            self.erasing = None;
            self.refresh();
        }
    }

    /// Перечитать корзину с диска.
    pub(super) fn reload_bin(&mut self) {
        self.bin = self
            .project
            .as_deref()
            .map(pf::trash_list)
            .unwrap_or_default();
        let alive: HashSet<&Path> = self.bin.iter().map(|b| b.item.as_path()).collect();
        self.bin_picked.retain(|p| alive.contains(p.as_path()));
    }

    fn picked_bin(&self) -> Vec<Binned> {
        self.bin
            .iter()
            .filter(|b| self.bin_picked.contains(&b.item))
            .cloned()
            .collect()
    }

    /// Вернуть выбранное в корзине на старые места.
    pub(super) fn restore_picked(&mut self) {
        let items = self.picked_bin();
        let Some(project) = self.project.clone() else {
            return;
        };
        if items.is_empty() {
            return;
        }
        match pf::restore_binned(&project, &items) {
            Ok(done) => {
                let n = match &done {
                    Done::Created(p) => p.len(),
                    _ => 0,
                };
                self.did(done);
                self.say(
                    format!("Возвращено: {} (Ctrl+Z унесёт обратно)", things_word(n)),
                    false,
                );
                self.out.push(Out::Changed);
                self.refresh();
                self.reload_bin();
            }
            Err(e) => self.say(e, true),
        }
    }

    /// Спросить, стереть ли насовсем.
    fn ask_erase(&mut self, items: Vec<Binned>) {
        if !items.is_empty() {
            self.erasing = Some(items);
        }
    }

    pub(super) fn erase_now(&mut self, items: &[Binned]) {
        match pf::erase(items) {
            Ok(n) => self.say(format!("Стёрто насовсем: {}", things_word(n)), false),
            Err(e) => self.say(e, true),
        }
        // Стёртое в отмене не вернуть: шаги, которые на него ссылаются, убрать.
        let gone: HashSet<&Path> = items.iter().map(|b| b.item.as_path()).collect();
        let lost = |d: &Done| match d {
            Done::Trashed(g) => g.iter().any(|(_, at)| gone.contains(at.as_path())),
            _ => false,
        };
        self.undo.retain(|d| !lost(d));
        self.redo.retain(|r| match r {
            pf::Redo::Restore(g) => !g.iter().any(|(_, at)| gone.contains(at.as_path())),
            _ => true,
        });
        self.reload_bin();
    }

    /// Окно «Стереть насовсем?».
    pub(super) fn erase_dialog(&mut self, ctx: &egui::Context) {
        let Some(items) = self.erasing.clone() else {
            return;
        };
        let mut close = false;
        let mut erase = false;
        let modal = egui::Modal::new(egui::Id::unique("explorer-erase")).show(ctx, |ui| {
            ui.set_width(360.0);
            heading(ui, "Стереть насовсем?");
            ui.add_space(GAP_M);
            let what = if items.len() == 1 {
                format!("«{}»", pf::file_name(&items[0].item))
            } else {
                things_word(items.len())
            };
            ui.label(RichText::new(format!("Будет стёрто: {what}.")).color(TEXT));
            ui.label(
                RichText::new("Это нельзя отменить, Ctrl+Z не поможет.")
                    .small()
                    .color(BAD),
            );
            ui.add_space(GAP_L);
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new("Стереть").color(ON_ACCENT)).fill(BAD);
                if ui.add(b).clicked() {
                    erase = true;
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
        if erase {
            self.erasing = None;
            self.erase_now(&items);
        } else if close {
            self.erasing = None;
        }
    }

    /// Клавиши в корзине.
    pub(super) fn bin_keys(&mut self, ui: &Ui) {
        let ctx = ui.ctx().clone();
        if ctx.egui_wants_keyboard_input() || self.erasing.is_some() || self.creating.is_some() {
            return;
        }
        let k = |m: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(m, key));
        if k(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || k(Modifiers::COMMAND, Key::Y) {
            self.redo();
            self.reload_bin();
        }
        if k(Modifiers::COMMAND, Key::Z) {
            self.undo();
            self.reload_bin();
        }
        if k(Modifiers::COMMAND, Key::A) {
            self.bin_picked = self.bin.iter().map(|b| b.item.clone()).collect();
        }
        if k(Modifiers::NONE, Key::Delete) {
            self.ask_erase(self.picked_bin());
        }
        if k(Modifiers::NONE, Key::Enter) {
            self.restore_picked();
        }
        if k(Modifiers::NONE, Key::Escape)
            || k(Modifiers::NONE, Key::Backspace)
            || k(Modifiers::ALT, Key::ArrowLeft)
        {
            self.close_bin();
        }
    }

    /// Команды корзины: вернуть, стереть, отмена, очистить.
    pub(super) fn bin_commands(&mut self, ui: &mut Ui) {
        let any = !self.bin_picked.is_empty();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_XS;
            ui.add_enabled_ui(any, |ui| {
                let b = egui::Button::new(RichText::new("Вернуть").color(ON_ACCENT)).fill(ACCENT);
                if ui
                    .add(b)
                    .on_hover_text("Вернуть выбранное на старое место (Enter)")
                    .clicked()
                {
                    self.restore_picked();
                }
            });
            ui.add_space(GAP_L);
            sep(ui);
            if icon(ui, Glyph::Trash, "Стереть выбранное насовсем (Delete)", any).clicked()
            {
                self.ask_erase(self.picked_bin());
            }
            sep(ui);
            if icon(ui, Glyph::Undo, "Отменить (Ctrl+Z)", !self.undo.is_empty()).clicked() {
                self.undo();
                self.reload_bin();
            }
            if icon(ui, Glyph::Redo, "Повторить (Ctrl+Y)", !self.redo.is_empty()).clicked()
            {
                self.redo();
                self.reload_bin();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(!self.bin.is_empty(), |ui| {
                    if ui
                        .button("Очистить корзину")
                        .on_hover_text("Стереть всё в корзине насовсем")
                        .clicked()
                    {
                        self.ask_erase(self.bin.clone());
                    }
                });
            });
        });
    }

    /// Строка «Корзина» в дереве: открывает её; брошенные на неё файлы удаляются.
    pub(super) fn bin_tree_row(&mut self, ui: &mut Ui, busy: &Busy) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
        let hovering = response.dnd_hover_payload::<Dragged>().is_some();
        if self.in_bin {
            ui.painter().rect_filled(rect, RADIUS_S, SELECT_BG);
        } else if response.hovered() || hovering {
            ui.painter().rect_filled(rect, RADIUS_S, HOVER);
        }
        if hovering {
            ui.painter().rect_stroke(
                rect,
                RADIUS_S,
                Stroke::new(LINE_FOCUS, BAD),
                egui::StrokeKind::Inside,
            );
        }
        let at = pos2(rect.left() + GAP_S + INDENT + 9.0, rect.center().y);
        paint_glyph(
            ui.painter(),
            at,
            Glyph::Trash,
            if self.in_bin { ACCENT } else { MUTED },
        );
        let n = self.bin_count();
        let name = if n > 0 {
            format!("Корзина ({n})")
        } else {
            "Корзина".to_owned()
        };
        ui.painter().text(
            pos2(at.x + 12.0, rect.center().y),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(FONT_BODY),
            if self.in_bin { Color32::WHITE } else { TEXT },
        );
        let response = response.on_hover_text("Удалённое: вернуть или стереть насовсем");
        if response.clicked() {
            self.open_bin();
        }
        if let Some(d) = response.dnd_release_payload::<Dragged>() {
            self.delete_items(d.0.clone(), busy);
            if self.in_bin {
                self.reload_bin();
            }
        }
    }

    /// Сколько в корзине (для дерева; перечитывается вместе с папкой).
    fn bin_count(&mut self) -> usize {
        if !self.in_bin && self.bin_counted.is_none() {
            let n = self
                .project
                .as_deref()
                .map_or(0, |p| pf::trash_list(p).len());
            self.bin_counted = Some(n);
        }
        if self.in_bin {
            self.bin.len()
        } else {
            self.bin_counted.unwrap_or(0)
        }
    }

    /// Список удалённого.
    pub(super) fn bin_files(&mut self, ui: &mut Ui) {
        let full = ui.available_rect_before_wrap();
        let bg = ui.interact(full, egui::Id::unique("explorer-bin-bg"), Sense::click());
        if bg.clicked() {
            self.bin_picked.clear();
        }
        if self.bin.is_empty() {
            ui.label(RichText::new("Корзина пуста").color(MUTED));
            return;
        }
        // Заголовок столбцов.
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        for (text, col) in ["Имя", "Откуда", "Удалено", "Размер"]
            .into_iter()
            .zip(columns(head))
        {
            ui.painter().text(
                pos2(col.left() + GAP_M, col.center().y),
                Align2::LEFT_CENTER,
                text,
                FontId::proportional(FONT_SMALL),
                MUTED,
            );
        }
        ui.painter().hline(
            head.x_range(),
            head.bottom() - 0.5,
            Stroke::new(LINE, BORDER),
        );
        let bin = self.bin.clone();
        let mut restore_one = None;
        egui::ScrollArea::vertical()
            .id_salt("explorer-bin")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for b in &bin {
                    let picked = self.bin_picked.contains(&b.item);
                    let r = self.bin_row(ui, b, picked);
                    if r.clicked() {
                        let mods = ui.input(|i| i.modifiers);
                        self.bin_click(&b.item, mods);
                    }
                    if r.double_clicked() {
                        restore_one = Some(b.item.clone());
                    }
                    r.context_menu(|ui| {
                        if !self.bin_picked.contains(&b.item) {
                            self.bin_picked = vec![b.item.clone()];
                        }
                        if ui.button("Вернуть на место").clicked() {
                            self.restore_picked();
                            ui.close();
                        }
                        if ui.button("Стереть насовсем…").clicked() {
                            self.ask_erase(self.picked_bin());
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Показать в проводнике Windows").clicked() {
                            self.out.push(Out::Reveal(b.item.clone()));
                            ui.close();
                        }
                    });
                }
            });
        if let Some(item) = restore_one {
            self.bin_picked = vec![item];
            self.restore_picked();
        }
    }

    fn bin_click(&mut self, item: &Path, mods: Modifiers) {
        if mods.command {
            if let Some(i) = self.bin_picked.iter().position(|p| p == item) {
                self.bin_picked.remove(i);
            } else {
                self.bin_picked.push(item.to_path_buf());
            }
        } else if mods.shift
            && let Some(last) = self.bin_picked.last().cloned()
        {
            let pos = |p: &Path| self.bin.iter().position(|b| b.item == p);
            if let (Some(a), Some(b)) = (pos(&last), pos(item)) {
                let (lo, hi) = (a.min(b), a.max(b));
                self.bin_picked = self.bin[lo..=hi].iter().map(|b| b.item.clone()).collect();
            }
        } else {
            self.bin_picked = vec![item.to_path_buf()];
        }
    }

    fn bin_row(&self, ui: &mut Ui, b: &Binned, picked: bool) -> egui::Response {
        let (rect, r) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
        let p = ui.painter();
        if picked {
            p.rect_filled(rect, RADIUS_S, SELECT_BG);
        } else if r.hovered() {
            p.rect_filled(rect, RADIUS_S, HOVER);
        }
        let name = pf::file_name(&b.item);
        let (_, color) = kind_of(&name, b.folder, false);
        let cols = columns(rect);
        let dot = pos2(cols[0].left() + 14.0, rect.center().y);
        if b.folder {
            paint_glyph(p, dot, Glyph::Folder, color);
        } else {
            p.rect_filled(
                Rect::from_center_size(dot, vec2(10.0, 10.0)),
                3,
                color.gamma_multiply(0.85),
            );
        }
        p.text(
            pos2(dot.x + 7.0 + GAP_L, rect.center().y),
            Align2::LEFT_CENTER,
            &name,
            FontId::proportional(FONT_BODY),
            if picked { Color32::WHITE } else { TEXT },
        );
        let cells = [
            self.bin_from(b),
            b.when.map(when_text).unwrap_or_default(),
            size_text(b.size),
        ];
        for (text, col) in cells.into_iter().zip(&cols[1..]) {
            p.with_clip_rect(col.intersect(rect)).text(
                pos2(col.left() + GAP_M, rect.center().y),
                Align2::LEFT_CENTER,
                text,
                FontId::proportional(FONT_SMALL),
                MUTED,
            );
        }
        r
    }

    /// Откуда удалили: папка от проекта.
    fn bin_from(&self, b: &Binned) -> String {
        match b.from.as_deref().and_then(Path::parent) {
            Some(dir) => self.rel(dir).unwrap_or_else(|| "Проект".into()),
            None => "неизвестно".into(),
        }
    }

    /// Сведения справа в корзине.
    pub(super) fn bin_details(&mut self, ui: &mut Ui) {
        let picked = self.picked_bin();
        match picked.as_slice() {
            [] => {
                heading(ui, "Корзина");
                ui.label(RichText::new("Удалённое из проекта").color(MUTED));
                ui.add_space(GAP_L);
                fact(ui, "Внутри", &things_word(self.bin.len()));
                let total: u64 = self.bin.iter().map(|b| b.size).sum();
                fact(ui, "Размер", &size_text(total));
                ui.add_space(GAP_XL);
                ui.label(
                    RichText::new(
                        "Двойной щелчок или Enter возвращает файл на старое место.\n\
                         Delete стирает насовсем.",
                    )
                    .small()
                    .color(MUTED),
                );
            }
            [b] => {
                heading(ui, &pf::file_name(&b.item));
                ui.label(
                    RichText::new(if b.folder {
                        "Папка в корзине"
                    } else {
                        "Файл в корзине"
                    })
                    .color(MUTED),
                );
                ui.add_space(GAP_L);
                fact(ui, "Откуда", &self.bin_from(b));
                if let Some(w) = b.when {
                    fact(ui, "Удалено", &when_text(w));
                }
                fact(ui, "Размер", &size_text(b.size));
                if let Some(from) = &b.from
                    && from.exists()
                {
                    ui.add_space(GAP_M);
                    ui.label(
                        RichText::new("На старом месте уже есть файл с таким именем: вернётся рядом с номером.")
                            .small()
                            .color(MUTED),
                    );
                }
            }
            many => {
                heading(ui, &format!("Выбрано: {}", many.len()));
                let total: u64 = many.iter().map(|b| b.size).sum();
                fact(ui, "Размер", &size_text(total));
            }
        }
        if !picked.is_empty() {
            ui.add_space(GAP_L);
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new("Вернуть").color(ON_ACCENT)).fill(ACCENT);
                if ui.add(b).clicked() {
                    self.restore_picked();
                }
                if ui.button("Стереть насовсем…").clicked() {
                    self.ask_erase(picked.clone());
                }
            });
        }
    }
}
