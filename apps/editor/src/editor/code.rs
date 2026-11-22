//! Редактор скриптов Luau (С.1): открывается двойным щелчком по `.luau` в «Ассетах» и
//! занимает место проводника.
//!
//! Слева номера строк, посередине код с подсветкой, справа подсказка, какие поля `me` и
//! `hero` даёт движок и где скрипт используется. Код проверяется прямо при наборе
//! (Luau переводит его в байт-код без запуска): строка с ошибкой подсвечена, текст
//! ошибки внизу, щелчок по нему ведёт к строке. После `me.` и `hero.` всплывает список
//! полей. Ctrl+S сохраняет файл, и запущенная игра подхватывает его на лету (движок сам
//! следит за файлами скриптов).

use std::path::{Path, PathBuf};

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, FontId, Frame, Key, Layout, Margin, Modifiers, Rect, RichText, Sense,
    Stroke, Ui, pos2, vec2,
};

use super::luau_text::{self, HERO, Hint, ME, Problem, Tok};
use super::project_files as pf;
use super::theme::*;

/// Через сколько секунд после последней буквы проверять код.
const CHECK_AFTER_S: f64 = 0.3;
/// Размер шрифта кода.
const CODE_FONT: f32 = 14.0;

/// Что редактор кода просит у окна.
#[derive(Debug)]
pub(crate) enum CodeOut {
    /// Закрыть и вернуться к проводнику.
    Close,
    /// Файл записан (проводнику — перечитать папку).
    Saved,
    /// Показать файл в проводнике Windows.
    Reveal(PathBuf),
    Log(Color32, String),
}

/// Список подсказки под курсором.
#[derive(Clone, Debug)]
struct Popup {
    /// Где начинается набираемое слово (байт).
    at: usize,
    items: Vec<&'static str>,
    pick: usize,
}

pub(crate) struct CodeEditor {
    pub(crate) path: PathBuf,
    project: Option<PathBuf>,
    pub(crate) text: String,
    saved: String,
    problem: Option<Problem>,
    /// Текст, который уже проверен.
    checked: Option<String>,
    edited_at: f64,
    lua: mlua::Lua,
    /// Окно «Сохранить правки?» перед закрытием.
    asking_close: bool,
    popup: Option<Popup>,
    /// Попросили перейти к строке (с 1).
    goto: Option<usize>,
    used: Option<Vec<(PathBuf, usize)>>,
    /// Где курсор: строка и столбец (с 1).
    at: (usize, usize),
    out: Vec<CodeOut>,
}

impl CodeEditor {
    pub(crate) fn open(path: &Path, project: Option<&Path>) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("Скрипт «{}» не открыть: {e}", pf::file_name(path)))?;
        Ok(Self {
            path: path.to_path_buf(),
            project: project.map(Path::to_path_buf),
            saved: text.clone(),
            text,
            problem: None,
            checked: None,
            edited_at: f64::NEG_INFINITY,
            lua: mlua::Lua::new(),
            asking_close: false,
            popup: None,
            goto: None,
            used: None,
            at: (1, 1),
            out: Vec::new(),
        })
    }

    /// Поле кода.
    fn text_id(&self) -> egui::Id {
        egui::Id::unique(("code-text", self.path.clone()))
    }

    pub(crate) fn dirty(&self) -> bool {
        self.text != self.saved
    }

    fn name(&self) -> String {
        pf::file_name(&self.path)
    }

    /// Записать файл.
    pub(crate) fn save(&mut self) -> Result<(), String> {
        std::fs::write(&self.path, &self.text)
            .map_err(|e| format!("Скрипт «{}» не сохранён: {e}", self.name()))?;
        self.saved = self.text.clone();
        let note = match &self.problem {
            Some(p) => {
                format!(
                    "Скрипт «{}» сохранён, но в нём ошибка: игра пока работает по прежней версии",
                    self.name()
                ) + &line_note(p)
            }
            None => format!(
                "Скрипт «{}» сохранён: игра подхватит его на лету",
                self.name()
            ),
        };
        let color = if self.problem.is_some() {
            BAD
        } else {
            TEXT_DIM
        };
        self.out.push(CodeOut::Log(color, note));
        self.out.push(CodeOut::Saved);
        Ok(())
    }

    fn save_or_say(&mut self) {
        if let Err(e) = self.save() {
            self.out.push(CodeOut::Log(BAD, e));
        }
    }

    /// Закрыть: с несохранёнными правками сначала спросить.
    fn ask_close(&mut self) {
        if self.dirty() {
            self.asking_close = true;
        } else {
            self.out.push(CodeOut::Close);
        }
    }

    /// Проверить код, если его поменяли и печатать перестали.
    fn recheck(&mut self, now: f64) {
        if self.checked.as_deref() == Some(self.text.as_str()) {
            return;
        }
        if self.checked.is_some() && now - self.edited_at < CHECK_AFTER_S {
            return;
        }
        self.problem = luau_text::check(&self.lua, &self.name(), &self.text);
        self.checked = Some(self.text.clone());
    }

    /// Рисует редактор на всё место `ui`.
    pub(crate) fn draw(&mut self, ui: &mut Ui) -> Vec<CodeOut> {
        let now = ui.input(|i| i.time);
        self.recheck(now);
        let ctx = ui.ctx().clone();
        if !self.asking_close {
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S)) {
                self.save_or_say();
            }
            if self.popup.is_none()
                && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
            {
                self.ask_close();
            }
        }
        egui::Panel::top("code-bar")
            .exact_size(44.0)
            .resizable(false)
            .frame(
                Frame::NONE
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(GAP_XL as i8, 0)),
            )
            .show(ui, |ui| self.bar(ui));
        egui::Panel::bottom("code-status")
            .exact_size(28.0)
            .resizable(false)
            .frame(
                Frame::NONE
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(GAP_XL as i8, 0)),
            )
            .show(ui, |ui| self.status(ui));
        egui::Panel::right("code-hints")
            .resizable(true)
            .default_size(RIGHT_WIDTH)
            .size_range(SIDE_RANGE)
            .frame(Frame::NONE.fill(PANEL).inner_margin(PAD_CARD))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("code-hints")
                    .auto_shrink(false)
                    .show(ui, |ui| self.hints(ui));
            });
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG))
            .show(ui, |ui| self.code(ui));
        self.close_dialog(&ctx);
        std::mem::take(&mut self.out)
    }

    fn bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_L;
            if ui
                .button("←  Файлы")
                .on_hover_text("Вернуться к проводнику (Esc)")
                .clicked()
            {
                self.ask_close();
            }
            ui.add_space(GAP_L);
            ui.label(
                RichText::new(self.name())
                    .size(FONT_HEADING)
                    .color(Color32::WHITE),
            );
            ui.label(RichText::new("скрипт Luau").color(KIND_SCRIPT));
            if self.dirty() {
                ui.label(RichText::new("●  не сохранён").small().color(ACCENT));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = GAP_M;
                if ui
                    .button("В проводнике")
                    .on_hover_text("Показать файл в проводнике Windows")
                    .clicked()
                {
                    self.out.push(CodeOut::Reveal(self.path.clone()));
                }
                let save =
                    egui::Button::new(RichText::new("Сохранить").color(ON_ACCENT)).fill(ACCENT);
                if ui
                    .add_enabled(self.dirty(), save)
                    .on_hover_text("Ctrl+S. Запущенная игра подхватит скрипт сразу")
                    .clicked()
                {
                    self.save_or_say();
                }
            });
        });
    }

    fn status(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            match self.problem.clone() {
                Some(p) => {
                    let text = format!("Ошибка{}: {}", line_note(&p), p.text);
                    let r = ui.add(
                        egui::Label::new(RichText::new(text).small().color(BAD))
                            .truncate()
                            .sense(Sense::click()),
                    );
                    if p.line > 0 && r.on_hover_text("Перейти к строке").clicked() {
                        self.goto = Some(p.line);
                    }
                }
                None => {
                    ui.label(RichText::new("Ошибок нет").small().color(GOOD));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("Строка {}, столбец {}", self.at.0, self.at.1))
                        .small()
                        .color(MUTED),
                );
            });
        });
    }

    /// Подсказка справа: что даёт движок и где скрипт используется.
    fn hints(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("Как устроен скрипт")
                .size(FONT_HEADING)
                .color(Color32::WHITE),
        );
        ui.add_space(GAP_S);
        ui.label(
            RichText::new(
                "Движок 60 раз в секунду зовёт update(me, hero, dt). Скрипт читает поля и \
                 пишет, что делать. После «me.» и «hero.» появится список полей.",
            )
            .small()
            .color(TEXT_DIM),
        );
        ui.add_space(GAP_L);
        hint_list(ui, "me — этот предмет", &ME, true);
        ui.add_space(GAP_L);
        hint_list(ui, "hero — герой", &HERO, false);
        ui.add_space(GAP_L);
        ui.label(RichText::new("dt — шаг игры, секунды").color(TEXT));
        ui.add_space(GAP_XL);
        ui.label(RichText::new("Где используется").color(TEXT).strong());
        ui.add_space(GAP_S);
        if self.used.is_none() {
            self.used = Some(
                self.project
                    .as_deref()
                    .map(|p| pf::used_in(p, &self.path))
                    .unwrap_or_default(),
            );
        }
        let used = self.used.clone().unwrap_or_default();
        if used.is_empty() {
            ui.label(
                RichText::new("Ни одна сцена на него не ссылается")
                    .small()
                    .color(MUTED),
            );
        }
        for (file, n) in used {
            let name = self
                .project
                .as_deref()
                .and_then(|p| file.strip_prefix(p).ok())
                .map_or_else(
                    || pf::file_name(&file),
                    |r| r.to_string_lossy().replace('\\', "/"),
                );
            let text = if n > 1 {
                format!("{name}  ×{n}")
            } else {
                name
            };
            ui.label(RichText::new(text).small().color(SELECT));
        }
    }

    /// Код: номера строк, текст с подсветкой, подсветка строки с ошибкой, подсказка.
    fn code(&mut self, ui: &mut Ui) {
        let id = self.text_id();
        let font = FontId::monospace(CODE_FONT);
        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        let focused = ui.memory(|m| m.has_focus(id));
        if focused {
            self.keys_before_text(ui, id);
        }
        let lines = self.text.split('\n').count().max(1);
        let digits = lines.to_string().len().max(2);
        let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let gutter_w = digits as f32 * char_w + 2.0 * GAP_XL;
        let goto = self.goto.take();
        let mut scroll = egui::ScrollArea::both()
            .id_salt("code-scroll")
            .auto_shrink(false);
        if let Some(line) = goto {
            // Строку с ошибкой — в середину окна.
            let y = (line.saturating_sub(1) as f32 * row_h - ui.available_height() / 2.0).max(0.0);
            scroll = scroll.vertical_scroll_offset(y);
        }
        let problem_line = self.problem.as_ref().map_or(0, |p| p.line);
        let mut edited = false;
        let mut cursor_after = None;
        let mut popup_at = None;
        scroll.show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let (gutter, _) = ui.allocate_exact_size(
                    vec2(gutter_w, lines as f32 * row_h + 2.0 * GAP_L),
                    Sense::hover(),
                );
                let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                    let job = highlight(buf.as_str(), &font);
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                let output = egui::TextEdit::multiline(&mut self.text)
                    .id(id)
                    .code_editor()
                    .frame(Frame::NONE)
                    .margin(Margin::symmetric(GAP_L as i8, GAP_L as i8))
                    .desired_width(f32::INFINITY)
                    .desired_rows(lines.max(30))
                    .layouter(&mut layouter)
                    .show(ui);
                edited = output.response.changed();
                cursor_after = output.cursor_range;
                let p = ui.painter();
                p.rect_filled(gutter, 0, PANEL);
                // Номера строк: по строкам раскладки (перенос выключен — строка = ряд).
                let mut line = 1;
                let mut new_line = true;
                let cur_line = self.at.0;
                for row in &output.galley.rows {
                    let y = output.galley_pos.y + row.pos.y;
                    if new_line {
                        let color = if line == problem_line {
                            BAD
                        } else if line == cur_line {
                            TEXT
                        } else {
                            MUTED
                        };
                        p.text(
                            pos2(gutter.right() - GAP_XL, y + row_h / 2.0),
                            Align2::RIGHT_CENTER,
                            line.to_string(),
                            font.clone(),
                            color,
                        );
                        if line == problem_line {
                            let band = Rect::from_min_size(
                                pos2(gutter.left(), y),
                                vec2(ui.clip_rect().right() - gutter.left(), row_h),
                            );
                            p.rect_filled(band, 0, BAD.gamma_multiply(0.12));
                        }
                        line += 1;
                    }
                    new_line = row.ends_with_newline;
                }
                if let Some(range) = output.cursor_range
                    && focused
                {
                    let r = output.galley.pos_from_cursor(range.primary);
                    popup_at = Some(output.galley_pos + r.left_bottom().to_vec2());
                }
            });
        });
        if let Some(line) = goto {
            self.place_cursor(ui, id, line_start(&self.text, line));
            ui.memory_mut(|m| m.request_focus(id));
        }
        if edited {
            self.edited_at = ui.input(|i| i.time);
        }
        if let Some(range) = cursor_after {
            let at = byte_of(&self.text, range.primary.index.0);
            let before = &self.text[..at];
            let line = before.matches('\n').count() + 1;
            let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
            self.at = (line, col);
            if edited {
                self.popup = luau_text::complete(&self.text, at).map(|(at, _, items)| Popup {
                    at,
                    items,
                    pick: 0,
                });
            } else if range.primary.index.0 != range.secondary.index.0 {
                self.popup = None;
            }
        }
        if !focused {
            self.popup = None;
        }
        if let (Some(popup), Some(at)) = (self.popup.clone(), popup_at) {
            self.popup_list(ui, id, &popup, at);
        }
    }

    /// Клавиши, которые редактор забирает у поля: подсказка (стрелки, Enter, Tab, Esc),
    /// Tab — четыре пробела, Enter — новая строка с тем же отступом.
    fn keys_before_text(&mut self, ui: &Ui, id: egui::Id) {
        let ctx = ui.ctx().clone();
        let Some(state) = egui::TextEdit::load_state(&ctx, id) else {
            return;
        };
        let Some(range) = state.cursor.char_range() else {
            return;
        };
        let k = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        if let Some(popup) = self.popup.as_mut() {
            if k(Key::ArrowDown) {
                popup.pick = (popup.pick + 1) % popup.items.len();
            }
            if k(Key::ArrowUp) {
                popup.pick = (popup.pick + popup.items.len() - 1) % popup.items.len();
            }
            if k(Key::Escape) {
                self.popup = None;
                return;
            }
            if k(Key::Enter) || k(Key::Tab) {
                let popup = self.popup.take().unwrap_or_else(|| unreachable!());
                let at = byte_of(&self.text, range.primary.index.0);
                self.text
                    .replace_range(popup.at..at, popup.items[popup.pick]);
                let end = popup.at + popup.items[popup.pick].len();
                self.place_cursor(ui, id, end);
                self.edited_at = ui.input(|i| i.time);
            }
            return;
        }
        let one = range.primary.index.0 == range.secondary.index.0;
        if one && k(Key::Tab) {
            let at = byte_of(&self.text, range.primary.index.0);
            self.text.insert_str(at, "    ");
            self.place_cursor(ui, id, at + 4);
            self.edited_at = ui.input(|i| i.time);
        }
        if one && k(Key::Enter) {
            let at = byte_of(&self.text, range.primary.index.0);
            let line = &self.text[line_start_of(&self.text, at)..at];
            let insert = format!("\n{}", luau_text::next_indent(line));
            self.text.insert_str(at, &insert);
            self.place_cursor(ui, id, at + insert.len());
            self.edited_at = ui.input(|i| i.time);
        }
    }

    /// Поставить курсор на байт `at`.
    fn place_cursor(&self, ui: &Ui, id: egui::Id, at: usize) {
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let c = CCursor::new(self.text[..at.min(self.text.len())].chars().count());
        state.cursor.set_char_range(Some(CCursorRange::one(c)));
        state.store(ui.ctx(), id);
    }

    /// Список подсказки под курсором; щелчок выбирает.
    fn popup_list(&mut self, ui: &Ui, id: egui::Id, popup: &Popup, at: egui::Pos2) {
        let mut chosen = None;
        egui::Area::new(egui::Id::unique("code-popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(at + vec2(0.0, GAP_S))
            .show(ui.ctx(), |ui| {
                Frame::NONE
                    .fill(RAISED)
                    .stroke(Stroke::new(LINE, BORDER_STRONG))
                    .corner_radius(RADIUS_S)
                    .inner_margin(Margin::same(GAP_S as i8))
                    .show(ui, |ui| {
                        ui.set_min_width(200.0);
                        for (i, name) in popup.items.iter().enumerate() {
                            let about = ME
                                .iter()
                                .chain(HERO.iter())
                                .find(|h| h.name == *name)
                                .map_or("", |h| h.about);
                            let picked = i == popup.pick;
                            let text = RichText::new(*name).monospace().color(if picked {
                                Color32::WHITE
                            } else {
                                TEXT
                            });
                            let r = ui.add(
                                egui::Button::new(text)
                                    .fill(if picked {
                                        SELECT_BG
                                    } else {
                                        Color32::TRANSPARENT
                                    })
                                    .stroke(Stroke::NONE)
                                    .min_size(vec2(ui.available_width(), 0.0)),
                            );
                            if picked && !about.is_empty() {
                                ui.label(RichText::new(about).small().color(MUTED));
                            }
                            if r.clicked() {
                                chosen = Some(i);
                            }
                        }
                    });
            });
        if let Some(i) = chosen
            && let Some(state) = egui::TextEdit::load_state(ui.ctx(), id)
            && let Some(range) = state.cursor.char_range()
        {
            let at = byte_of(&self.text, range.primary.index.0);
            self.text.replace_range(popup.at..at, popup.items[i]);
            self.place_cursor(ui, id, popup.at + popup.items[i].len());
            self.popup = None;
            ui.memory_mut(|m| m.request_focus(id));
        }
    }

    /// «Сохранить правки в скрипте?»
    fn close_dialog(&mut self, ctx: &egui::Context) {
        if !self.asking_close {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("code-close")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                RichText::new("Сохранить правки?")
                    .size(FONT_HEADING)
                    .color(Color32::WHITE),
            );
            ui.add_space(GAP_M);
            ui.label(
                RichText::new(format!(
                    "В скрипте {} есть несохранённые правки. Без сохранения они пропадут.",
                    self.name()
                ))
                .color(TEXT_DIM),
            );
            ui.add_space(GAP_XL);
            ui.horizontal(|ui| {
                let save =
                    egui::Button::new(RichText::new("Сохранить").color(ON_ACCENT)).fill(ACCENT);
                if ui.add(save).clicked() {
                    self.asking_close = false;
                    if self.save().is_ok() {
                        self.out.push(CodeOut::Close);
                    }
                }
                if ui.button("Не сохранять").clicked() {
                    self.asking_close = false;
                    self.out.push(CodeOut::Close);
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
}

/// « в строке N» для сообщения (пусто, если строка не известна).
fn line_note(p: &Problem) -> String {
    if p.line > 0 {
        format!(" в строке {}", p.line)
    } else {
        String::new()
    }
}

fn hint_list(ui: &mut Ui, title: &str, hints: &[Hint], legend: bool) {
    ui.label(RichText::new(title).color(TEXT).strong());
    ui.add_space(GAP_XS);
    for h in hints {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_M;
            let color = if h.write { ACCENT } else { SELECT };
            let (r, _) = ui.allocate_exact_size(vec2(84.0, 16.0), Sense::hover());
            ui.painter().text(
                r.left_center(),
                Align2::LEFT_CENTER,
                h.name,
                FontId::monospace(FONT_MONO),
                color,
            );
            ui.add(egui::Label::new(RichText::new(h.about).small().color(TEXT_DIM)).wrap());
        });
    }
    if legend {
        ui.label(
            RichText::new("синие даёт движок, оранжевые пишет скрипт")
                .small()
                .color(MUTED),
        );
    }
}

/// Цвет куска кода.
fn color_of(tok: Tok) -> Color32 {
    match tok {
        Tok::Keyword => KIND_SCRIPT,
        Tok::Literal | Tok::Number => FILE_SHAPE,
        Tok::Str => GOOD,
        Tok::Comment => MUTED,
        Tok::Known => KIND_PLAYER,
        Tok::Field => TEXT_DIM,
        Tok::FnName => ACCENT,
        Tok::Ident | Tok::Space => TEXT,
        Tok::Punct => TEXT_DIM,
    }
}

/// Раскладка текста с подсветкой.
fn highlight(text: &str, font: &FontId) -> LayoutJob {
    let mut job = LayoutJob {
        text: text.to_owned(),
        wrap: egui::text::TextWrapping {
            max_width: f32::INFINITY,
            ..Default::default()
        },
        ..Default::default()
    };
    for (range, tok) in luau_text::tokens(text) {
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: egui::text::ByteIndex(range.start)..egui::text::ByteIndex(range.end),
            format: TextFormat::simple(font.clone(), color_of(tok)),
        });
    }
    job
}

/// Байт, на котором стоит символ номер `chars`.
fn byte_of(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i)
}

/// Начало строки `line` (с 1), байт.
fn line_start(text: &str, line: usize) -> usize {
    if line <= 1 {
        return 0;
    }
    text.match_indices('\n')
        .nth(line - 2)
        .map_or(text.len(), |(i, _)| i + 1)
}

/// Начало строки, в которой стоит байт `at`.
fn line_start_of(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Pos2};

    fn frame(
        ctx: &egui::Context,
        ed: &mut CodeEditor,
        t: &mut f64,
        events: Vec<Event>,
    ) -> Vec<CodeOut> {
        *t += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0))),
            time: Some(*t),
            events,
            ..Default::default()
        };
        let mut outs = Vec::new();
        let mut out = ctx.run_ui(input, |ui| outs = ed.draw(ui));
        out.textures_delta.clear();
        outs
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(reyvec_app::for_editor::fonts());
        super::super::theme::apply(&ctx);
        ctx
    }

    /// Набор кода: ошибка видна через долю секунды и с номером строки, подсказка после
    /// «me.» вставляет поле, Enter держит отступ, Ctrl+S пишет файл, Esc с правками
    /// спрашивает, без правок закрывает.
    #[test]
    fn type_check_complete_save_and_close() {
        let dir = std::env::temp_dir().join(format!("reyvec-code-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("жук.luau");
        std::fs::write(&file, "function update(me, hero, dt)\n    if me.alive then").unwrap();
        let ctx = ctx();
        let mut ed = CodeEditor::open(&file, Some(&dir)).unwrap();
        let mut t = 0.0;
        frame(&ctx, &mut ed, &mut t, vec![]);
        // Нет «end»: Luau находит ошибку сразу при открытии.
        assert!(ed.problem.is_some(), "незакрытый if — ошибка");
        // Курсор в конец, Enter: новая строка с отступом на шаг больше.
        ctx.memory_mut(|m| m.request_focus(ed.text_id()));
        let end = ed.text.len();
        frame(&ctx, &mut ed, &mut t, vec![]);
        ed.place_cursor(&ctx_ui_dummy(&ctx), ed.text_id(), end);
        frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![key(Key::Enter, Modifiers::NONE)],
        );
        assert!(ed.text.ends_with("then\n        "), "{:?}", ed.text);
        // «me.mo» — подсказка «move», Enter вставляет.
        frame(&ctx, &mut ed, &mut t, vec![Event::Text("me.mo".into())]);
        assert_eq!(
            ed.popup.as_ref().map(|p| p.items.clone()),
            Some(vec!["move"])
        );
        frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![key(Key::Enter, Modifiers::NONE)],
        );
        assert!(ed.text.ends_with("me.move"), "{:?}", ed.text);
        assert!(ed.popup.is_none());
        frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![Event::Text(" = 1\n    end\nend".into())],
        );
        // Проверка ждёт, пока перестанут печатать.
        for _ in 0..10 {
            frame(&ctx, &mut ed, &mut t, vec![]);
        }
        assert_eq!(ed.problem, None, "{:?}\n{}", ed.problem, ed.text);
        assert!(ed.dirty());
        // Esc с правками — вопрос, а не закрытие.
        ctx.memory_mut(|m| m.surrender_focus(ed.text_id()));
        let out = frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![key(Key::Escape, Modifiers::NONE)],
        );
        assert!(ed.asking_close && !out.iter().any(|o| matches!(o, CodeOut::Close)));
        ed.asking_close = false;
        let out = frame(&ctx, &mut ed, &mut t, vec![key(Key::S, Modifiers::COMMAND)]);
        assert!(out.iter().any(|o| matches!(o, CodeOut::Saved)));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ed.text);
        let out = frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![key(Key::Escape, Modifiers::NONE)],
        );
        assert!(out.iter().any(|o| matches!(o, CodeOut::Close)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ui для вызова помощников вне кадра.
    fn ctx_ui_dummy(ctx: &egui::Context) -> Ui {
        Ui::new(
            ctx.clone(),
            egui::Id::unique("code-test-ui"),
            egui::UiBuilder::new(),
        )
    }

    #[test]
    fn lines_and_bytes() {
        let t = "аб\nв\nгд";
        assert_eq!(byte_of(t, 2), 4);
        assert_eq!(byte_of(t, 100), t.len());
        assert_eq!(line_start(t, 1), 0);
        assert_eq!(line_start(t, 2), 5);
        assert_eq!(line_start(t, 3), 8);
        assert_eq!(line_start_of(t, 7), 5);
    }
}
