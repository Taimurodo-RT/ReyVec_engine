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

use super::luau_text::{
    self, Func, HERO, Hint, ME, Problem, QUEST_FUNCS, STORY_FUNCS, Tok, Warning,
};
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

/// Поиск и замена (Ctrl+F, Ctrl+H).
#[derive(Clone, Debug, Default)]
struct Find {
    query: String,
    replace: String,
    /// Показана строка замены.
    replacing: bool,
    /// Поставить фокус в поле поиска в следующем кадре.
    focus: bool,
}

/// Список подсказки под курсором.
#[derive(Clone, Debug)]
struct Popup {
    /// Где начинается набираемое слово (байт).
    at: usize,
    items: Vec<String>,
    pick: usize,
}

pub(crate) struct CodeEditor {
    pub(crate) path: PathBuf,
    project: Option<PathBuf>,
    pub(crate) text: String,
    saved: String,
    problem: Option<Problem>,
    /// Похоже на опечатку (С.2).
    warnings: Vec<Warning>,
    /// Имена диалогов, заданий и переменных сюжета проекта (Сю.5).
    story: luau_text::StoryNames,
    find: Option<Find>,
    /// Окошко «Перейти к строке» (Ctrl+G) и что в нём набрано.
    goto_input: Option<String>,
    /// Показать место: строка (с 1) и что выделить (байты).
    jump: Option<(usize, std::ops::Range<usize>)>,
    /// Текст, который уже проверен.
    checked: Option<String>,
    edited_at: f64,
    lua: mlua::Lua,
    /// Окно «Сохранить правки?» перед закрытием.
    asking_close: bool,
    popup: Option<Popup>,
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
            warnings: Vec::new(),
            story: project
                .map(luau_text::StoryNames::of_project)
                .unwrap_or_default(),
            find: None,
            goto_input: None,
            jump: None,
            checked: None,
            edited_at: f64::NEG_INFINITY,
            lua: mlua::Lua::new(),
            asking_close: false,
            popup: None,
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
        self.problem = luau_text::check(&self.lua, &self.name(), &self.text).map(|mut p| {
            p.text = luau_text::russian(&p.text);
            p
        });
        self.warnings = luau_text::warnings(&self.text);
        self.warnings
            .extend(luau_text::story_warnings(&self.text, &self.story));
        self.warnings.sort_by_key(|w| w.range.start);
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
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F)) {
                self.open_find(&ctx, false);
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::H)) {
                self.open_find(&ctx, true);
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::G)) {
                self.goto_input = Some(String::new());
            }
            if self.popup.is_none()
                && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
            {
                if self.find.is_some() || self.goto_input.is_some() {
                    self.find = None;
                    self.goto_input = None;
                    ctx.memory_mut(|m| m.request_focus(self.text_id()));
                } else {
                    self.ask_close();
                }
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
                        self.go_line(p.line);
                    }
                }
                None => match self.warnings.first().cloned() {
                    Some(w) => {
                        let more = if self.warnings.len() > 1 {
                            format!("  (и ещё {})", self.warnings.len() - 1)
                        } else {
                            String::new()
                        };
                        let text = format!("Строка {}: {}{more}", w.line, w.text);
                        let r = ui.add(
                            egui::Label::new(RichText::new(text).small().color(WARN))
                                .truncate()
                                .sense(Sense::click()),
                        );
                        if r.on_hover_text("Показать это место").clicked() {
                            self.jump = Some((w.line, w.range.clone()));
                        }
                    }
                    None => {
                        ui.label(RichText::new("Ошибок нет").small().color(GOOD));
                    }
                },
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
        hint_list(ui, "me — этот предмет", ME, true);
        ui.add_space(GAP_L);
        hint_list(ui, "hero — герой", HERO, false);
        ui.add_space(GAP_L);
        ui.label(RichText::new("dt — шаг игры, секунды").color(TEXT));
        ui.add_space(GAP_L);
        func_list(ui, "story — сюжет", "story", STORY_FUNCS);
        ui.add_space(GAP_L);
        func_list(ui, "quest — задания", "quest", QUEST_FUNCS);
        if !self.story.dialogs.is_empty() {
            ui.label(
                RichText::new("Имена диалогов и заданий подсказываются прямо в кавычках")
                    .small()
                    .color(MUTED),
            );
        }
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

    /// Перейти к строке: курсор в её начало.
    fn go_line(&mut self, line: usize) {
        let at = line_start(&self.text, line);
        self.jump = Some((line, at..at));
    }

    /// Открыть поиск: искать выделенное, если оно в одну строку.
    fn open_find(&mut self, ctx: &egui::Context, replacing: bool) {
        let picked = egui::TextEdit::load_state(ctx, self.text_id())
            .and_then(|s| s.cursor.char_range())
            .map(|r| {
                let (a, b) = (r.primary.index.0, r.secondary.index.0);
                let (a, b) = (byte_of(&self.text, a.min(b)), byte_of(&self.text, a.max(b)));
                self.text[a..b].to_owned()
            })
            .filter(|t| !t.is_empty() && !t.contains('\n'));
        let find = self.find.get_or_insert_with(Find::default);
        if let Some(t) = picked {
            find.query = t;
        }
        find.replacing |= replacing;
        find.focus = true;
    }

    /// Где в тексте найденное (байты).
    fn found(&self) -> Vec<std::ops::Range<usize>> {
        match &self.find {
            Some(f) if !f.query.is_empty() => self
                .text
                .match_indices(f.query.as_str())
                .map(|(i, m)| i..i + m.len())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Следующее (или прошлое) найденное от курсора: выделить и показать.
    fn find_next(&mut self, ctx: &egui::Context, back: bool) {
        let found = self.found();
        if found.is_empty() {
            return;
        }
        let at = egui::TextEdit::load_state(ctx, self.text_id())
            .and_then(|s| s.cursor.char_range())
            .map_or(0, |r| byte_of(&self.text, r.primary.index.0));
        let pick = if back {
            found.iter().rev().find(|r| r.end < at).or(found.last())
        } else {
            found.iter().find(|r| r.start >= at).or(found.first())
        };
        if let Some(r) = pick.cloned() {
            let line = self.text[..r.start].matches('\n').count() + 1;
            self.jump = Some((line, r));
        }
    }

    /// Полоса поиска и замены над кодом.
    fn find_bar(&mut self, ui: &mut Ui) {
        let Some(mut find) = self.find.clone() else {
            return;
        };
        let ctx = ui.ctx().clone();
        let n = self.found().len();
        let mut next = None;
        let mut replace_one = false;
        let mut replace_all = false;
        let mut close = false;
        Frame::NONE
            .fill(PANEL)
            .inner_margin(Margin::symmetric(GAP_XL as i8, GAP_M as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Найти").small().color(MUTED));
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut find.query)
                            .id(egui::Id::unique("code-find"))
                            .desired_width(220.0),
                    );
                    if find.focus {
                        r.request_focus();
                        find.focus = false;
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        next = Some(ui.input(|i| i.modifiers.shift));
                        r.request_focus();
                    }
                    let count = if find.query.is_empty() {
                        String::new()
                    } else if n == 0 {
                        "не найдено".to_owned()
                    } else {
                        format!("найдено: {n}")
                    };
                    ui.label(
                        RichText::new(count)
                            .small()
                            .color(if n == 0 { BAD } else { MUTED }),
                    );
                    if ui
                        .small_button("↑")
                        .on_hover_text("Прошлое (Shift+Enter)")
                        .clicked()
                    {
                        next = Some(true);
                    }
                    if ui
                        .small_button("↓")
                        .on_hover_text("Следующее (Enter)")
                        .clicked()
                    {
                        next = Some(false);
                    }
                    if !find.replacing && ui.small_button("Заменить…").clicked() {
                        find.replacing = true;
                    }
                    if ui
                        .small_button("✕")
                        .on_hover_text("Закрыть (Esc)")
                        .clicked()
                    {
                        close = true;
                    }
                });
                if find.replacing {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("На").small().color(MUTED));
                        ui.add(
                            egui::TextEdit::singleline(&mut find.replace)
                                .id(egui::Id::unique("code-replace"))
                                .desired_width(220.0),
                        );
                        if ui
                            .small_button("Заменить")
                            .on_hover_text("Выделенное найденное")
                            .clicked()
                        {
                            replace_one = true;
                        }
                        if ui.small_button("Заменить все").clicked() {
                            replace_all = true;
                        }
                    });
                }
            });
        if close {
            self.find = None;
            return;
        }
        self.find = Some(find.clone());
        if replace_all && n > 0 {
            self.text = self.text.replace(&find.query, &find.replace);
            self.edited_at = ui.input(|i| i.time);
            self.out
                .push(CodeOut::Log(TEXT_DIM, format!("Заменено: {n}")));
        } else if replace_one {
            let picked = egui::TextEdit::load_state(&ctx, self.text_id())
                .and_then(|s| s.cursor.char_range())
                .map(|r| {
                    let (a, b) = (r.primary.index.0, r.secondary.index.0);
                    (byte_of(&self.text, a.min(b)), byte_of(&self.text, a.max(b)))
                });
            if let Some((a, b)) = picked
                && self.text[a..b] == find.query
            {
                self.text.replace_range(a..b, &find.replace);
                self.place_cursor(&ctx, self.text_id(), a + find.replace.len());
                self.edited_at = ui.input(|i| i.time);
            }
            self.find_next(&ctx, false);
        } else if let Some(back) = next {
            self.find_next(&ctx, back);
        }
    }

    /// Окошко «Перейти к строке».
    fn goto_bar(&mut self, ui: &mut Ui) {
        let Some(mut text) = self.goto_input.clone() else {
            return;
        };
        let mut go = None;
        Frame::NONE
            .fill(PANEL)
            .inner_margin(Margin::symmetric(GAP_XL as i8, GAP_M as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Перейти к строке").small().color(MUTED));
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .id(egui::Id::unique("code-goto"))
                            .desired_width(80.0),
                    );
                    if !r.has_focus() && !r.lost_focus() {
                        r.request_focus();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        go = text.trim().parse::<usize>().ok();
                    }
                });
            });
        self.goto_input = Some(text);
        if let Some(line) = go {
            self.goto_input = None;
            self.go_line(line.max(1));
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
        self.find_bar(ui);
        self.goto_bar(ui);
        let found = self.found();
        let jump = self.jump.take();
        let mut scroll = egui::ScrollArea::both()
            .id_salt("code-scroll")
            .auto_shrink(false);
        if let Some((line, _)) = &jump {
            let line = *line;
            // Строку с ошибкой — в середину окна.
            let y = (line.saturating_sub(1) as f32 * row_h - ui.available_height() / 2.0).max(0.0);
            scroll = scroll.vertical_scroll_offset(y);
        }
        let problem_line = self.problem.as_ref().map_or(0, |p| p.line);
        let warn_lines: Vec<usize> = self.warnings.iter().map(|w| w.line).collect();
        let warn_ranges: Vec<std::ops::Range<usize>> =
            self.warnings.iter().map(|w| w.range.clone()).collect();
        // Что подсветить у курсора (по прошлому кадру): то же имя и пару блока.
        let cursor_byte = egui::TextEdit::load_state(ui.ctx(), id)
            .and_then(|s| s.cursor.char_range())
            .filter(|r| r.primary.index == r.secondary.index)
            .map(|r| byte_of(&self.text, r.primary.index.0));
        let (same, pair) = match cursor_byte.filter(|_| focused) {
            Some(at) => (
                luau_text::word_at(&self.text, at)
                    .map(|(w, _)| luau_text::same_words(&self.text, &w))
                    .filter(|v| v.len() > 1)
                    .unwrap_or_default(),
                luau_text::pair_at(&self.text, at),
            ),
            None => (Vec::new(), None),
        };
        let mut hover_text = None;
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
                // Подсветка мест: прямоугольник куска текста (в одну строку).
                let galley = output.galley.clone();
                let origin = output.galley_pos;
                let text_now = self.text.clone();
                let rect_of = |r: &std::ops::Range<usize>| {
                    let a = text_now[..r.start.min(text_now.len())].chars().count();
                    let b = text_now[..r.end.min(text_now.len())].chars().count();
                    let ra = galley.pos_from_cursor(CCursor::new(a));
                    let rb = galley.pos_from_cursor(CCursor::new(b));
                    Rect::from_min_max(origin + ra.min.to_vec2(), origin + vec2(rb.max.x, ra.max.y))
                };
                for r in &found {
                    p.rect_filled(rect_of(r), 2, ACCENT.gamma_multiply(0.22));
                }
                for r in &same {
                    p.rect_filled(rect_of(r), 2, SELECT.gamma_multiply(0.22));
                }
                if let Some((a, b)) = &pair {
                    for r in [a, b] {
                        p.rect_stroke(
                            rect_of(r).expand(1.0),
                            2,
                            Stroke::new(LINE, ACCENT),
                            egui::StrokeKind::Outside,
                        );
                    }
                }
                for r in &warn_ranges {
                    zigzag(p, rect_of(r), WARN);
                }
                // Подсказка при наведении на слово.
                if let Some(pos) = output.response.hover_pos() {
                    let c = galley.cursor_from_pos(pos - origin);
                    let at = byte_of(&self.text, c.index.0);
                    hover_text = luau_text::hover(&self.text, at).or_else(|| {
                        self.warnings
                            .iter()
                            .find(|w| w.range.start <= at && at < w.range.end)
                            .map(|w| w.text.clone())
                    });
                }
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
                        } else if warn_lines.contains(&line) {
                            WARN
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
        if let Some((_, range)) = jump {
            self.select(ui.ctx(), id, range);
            ui.memory_mut(|m| m.request_focus(id));
        }
        if let Some(text) = hover_text {
            egui::Tooltip::always_open(
                ui.ctx().clone(),
                ui.layer_id(),
                egui::Id::unique("code-hover"),
                egui::PopupAnchor::Pointer,
            )
            .show(|ui| {
                ui.set_max_width(360.0);
                ui.label(RichText::new(text).small());
            });
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
                self.popup = luau_text::complete(&self.text, at)
                    .or_else(|| luau_text::story_complete(&self.text, at, &self.story))
                    .map(|(at, _, items)| Popup { at, items, pick: 0 });
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
                    .replace_range(popup.at..at, &popup.items[popup.pick]);
                let end = popup.at + popup.items[popup.pick].len();
                self.place_cursor(ui.ctx(), id, end);
                self.edited_at = ui.input(|i| i.time);
            }
            return;
        }
        let one = range.primary.index.0 == range.secondary.index.0;
        let (sel_a, sel_b) = {
            let (a, b) = (range.primary.index.0, range.secondary.index.0);
            (byte_of(&self.text, a.min(b)), byte_of(&self.text, a.max(b)))
        };
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Slash)) {
            let (text, end) = luau_text::toggle_comment(&self.text, sel_a, sel_b);
            self.text = text;
            self.place_cursor(&ctx, id, end);
            self.edited_at = ui.input(|i| i.time);
            return;
        }
        // Скобки и кавычки закрываются сами; выделенное берётся в них.
        let typed = ctx.input_mut(|i| {
            let at = i.events.iter().position(|e| {
                matches!(e, egui::Event::Text(t) if matches!(t.as_str(), "(" | "[" | "{" | "\"" | "'" | ")" | "]" | "}"))
            })?;
            match i.events.remove(at) {
                egui::Event::Text(t) => Some(t),
                _ => None,
            }
        });
        if let Some(t) = typed {
            self.type_pair(&ctx, id, &t, sel_a, sel_b);
            self.edited_at = ui.input(|i| i.time);
        }
        if one && k(Key::Tab) {
            let at = byte_of(&self.text, range.primary.index.0);
            self.text.insert_str(at, "    ");
            self.place_cursor(ui.ctx(), id, at + 4);
            self.edited_at = ui.input(|i| i.time);
        }
        if one && k(Key::Enter) {
            let at = byte_of(&self.text, range.primary.index.0);
            let line = self.text[line_start_of(&self.text, at)..at].to_owned();
            let indent = luau_text::next_indent(&line);
            let base: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let code = line.split("--").next().unwrap_or("").trim_end();
            let opens_block = code.ends_with("then")
                || code.ends_with("do")
                || (code.contains("function") && code.ends_with(')'));
            let rest_empty = self.text[at..]
                .split('\n')
                .next()
                .unwrap_or("")
                .trim()
                .is_empty();
            let insert = format!("\n{indent}");
            self.text.insert_str(at, &insert);
            // Новый блок без end: end ставится сам, курсор — внутрь блока.
            if opens_block && rest_empty && luau_text::needs_end(&self.text) {
                self.text
                    .insert_str(at + insert.len(), &format!("\n{base}end"));
            }
            self.place_cursor(&ctx, id, at + insert.len());
            self.edited_at = ui.input(|i| i.time);
        }
    }

    /// Набрали скобку или кавычку.
    fn type_pair(&mut self, ctx: &egui::Context, id: egui::Id, t: &str, a: usize, b: usize) {
        let close = match t {
            "(" => Some(")"),
            "[" => Some("]"),
            "{" => Some("}"),
            "\"" | "'" => Some(t),
            _ => None,
        };
        let next = self.text[b..].chars().next();
        // Закрывающая, а следом уже такая же: просто шагнуть через неё.
        if a == b
            && next.is_some_and(|c| c.to_string() == t)
            && (close.is_none() || close == Some(t))
        {
            self.place_cursor(ctx, id, b + t.len());
            return;
        }
        match close {
            Some(c) if a != b => {
                let inner = self.text[a..b].to_owned();
                self.text.replace_range(a..b, &format!("{t}{inner}{c}"));
                self.select(ctx, id, a + t.len()..b + t.len());
            }
            // Пара только перед пробелом, концом строки или закрывающей.
            Some(c) if next.is_none_or(|n| n.is_whitespace() || ")]},".contains(n)) => {
                self.text.insert_str(a, &format!("{t}{c}"));
                self.place_cursor(ctx, id, a + t.len());
            }
            _ => {
                self.text.replace_range(a..b, t);
                self.place_cursor(ctx, id, a + t.len());
            }
        }
    }

    /// Поставить курсор на байт `at`.
    fn place_cursor(&self, ctx: &egui::Context, id: egui::Id, at: usize) {
        self.select(ctx, id, at..at);
    }

    /// Выделить кусок (байты).
    fn select(&self, ctx: &egui::Context, id: egui::Id, r: std::ops::Range<usize>) {
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        let chars = |b: usize| CCursor::new(self.text[..b.min(self.text.len())].chars().count());
        state
            .cursor
            .set_char_range(Some(CCursorRange::two(chars(r.start), chars(r.end))));
        state.store(ctx, id);
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
                                .find(|h| h.name == name.as_str())
                                .map(|h| h.about)
                                .or_else(|| {
                                    STORY_FUNCS
                                        .iter()
                                        .chain(QUEST_FUNCS.iter())
                                        .find(|f| f.name == name.as_str())
                                        .map(|f| f.about)
                                })
                                .unwrap_or("");
                            let picked = i == popup.pick;
                            let text = RichText::new(name.as_str()).monospace().color(if picked {
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
            self.text.replace_range(popup.at..at, &popup.items[i]);
            self.place_cursor(ui.ctx(), id, popup.at + popup.items[i].len());
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
            let color = if h.by_script { ACCENT } else { SELECT };
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
            RichText::new("Свои поля можно хранить тут же: me.home, me.dir")
                .small()
                .color(TEXT_DIM),
        );
        ui.label(
            RichText::new("синие даёт движок, оранжевые пишет скрипт")
                .small()
                .color(MUTED),
        );
    }
}

fn func_list(ui: &mut Ui, title: &str, table: &str, funcs: &[Func]) {
    ui.label(RichText::new(title).color(TEXT).strong());
    ui.add_space(GAP_XS);
    for f in funcs {
        ui.label(
            RichText::new(format!("{table}.{}{}", f.name, f.args))
                .monospace()
                .size(FONT_MONO)
                .color(SELECT),
        );
        ui.add(egui::Label::new(RichText::new(f.about).small().color(TEXT_DIM)).wrap());
    }
}

/// Цвет предупреждения (похоже на опечатку).
const WARN: Color32 = FILE_SOUND;

/// Волнистая черта под куском текста.
fn zigzag(p: &egui::Painter, r: Rect, color: Color32) {
    let y = r.bottom() - 1.0;
    let step = 3.0;
    let mut x = r.left();
    let mut up = true;
    let mut points = vec![pos2(x, y)];
    while x < r.right() {
        x = (x + step).min(r.right());
        points.push(pos2(x, if up { y - 2.0 } else { y }));
        up = !up;
    }
    p.add(egui::Shape::line(points, Stroke::new(LINE, color)));
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
        ed.place_cursor(&ctx, ed.text_id(), end);
        frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![key(Key::Enter, Modifiers::NONE)],
        );
        // Новый if без end: end ставится сам, курсор внутри блока.
        assert!(
            ed.text.ends_with("then\n        \n    end"),
            "{:?}",
            ed.text
        );
        // «me.mo» — подсказка «move», Enter вставляет.
        frame(&ctx, &mut ed, &mut t, vec![Event::Text("me.mo".into())]);
        assert_eq!(
            ed.popup.as_ref().map(|p| p.items.clone()),
            Some(vec!["move".to_owned()])
        );
        frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![key(Key::Enter, Modifiers::NONE)],
        );
        assert!(ed.text.ends_with("me.move\n    end"), "{:?}", ed.text);
        assert!(ed.popup.is_none());
        frame(&ctx, &mut ed, &mut t, vec![Event::Text(" = 1".into())]);
        let end = ed.text.len();
        ed.place_cursor(&ctx, ed.text_id(), end);
        frame(&ctx, &mut ed, &mut t, vec![Event::Text("\nend".into())]);
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

    /// Скобки закрываются сами, Ctrl+/ комментирует строку, Ctrl+H меняет все, Ctrl+G
    /// ставит курсор на строку, опечатка в поле видна предупреждением.
    #[test]
    fn pairs_comment_find_goto_and_warnings() {
        let dir = std::env::temp_dir().join(format!("reyvec-code2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("монета.luau");
        std::fs::write(&file, "function update(me, hero, dt)\n    me.y = 1\nend\n").unwrap();
        let ctx = ctx();
        let mut ed = CodeEditor::open(&file, Some(&dir)).unwrap();
        let mut t = 0.0;
        let id = ed.text_id();
        frame(&ctx, &mut ed, &mut t, vec![]);
        ctx.memory_mut(|m| m.request_focus(id));
        frame(&ctx, &mut ed, &mut t, vec![]);
        // В конец второй строки: «print(» даёт «print()», курсор внутри.
        let at = ed.text.find(" = 1").unwrap() + 4;
        ed.place_cursor(&ctx, id, at);
        frame(&ctx, &mut ed, &mut t, vec![Event::Text(" print".into())]);
        frame(&ctx, &mut ed, &mut t, vec![Event::Text("(".into())]);
        frame(&ctx, &mut ed, &mut t, vec![Event::Text("me.mvoe".into())]);
        frame(&ctx, &mut ed, &mut t, vec![Event::Text(")".into())]);
        assert!(
            ed.text.contains("me.y = 1 print(me.mvoe)\n"),
            "{:?}",
            ed.text
        );
        for _ in 0..10 {
            frame(&ctx, &mut ed, &mut t, vec![]);
        }
        assert_eq!(ed.problem, None);
        assert_eq!(ed.warnings.len(), 1);
        assert!(ed.warnings[0].text.contains("move"), "{:?}", ed.warnings);
        // Ctrl+/ — строка закомментирована, ещё раз — обратно.
        let before = ed.text.clone();
        frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![key(Key::Slash, Modifiers::COMMAND)],
        );
        assert!(ed.text.contains("    -- me.y = 1"), "{:?}", ed.text);
        frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![key(Key::Slash, Modifiers::COMMAND)],
        );
        assert_eq!(ed.text, before);
        // Ctrl+H: «mvoe» на «move» везде.
        frame(&ctx, &mut ed, &mut t, vec![key(Key::H, Modifiers::COMMAND)]);
        let f = ed.find.as_mut().unwrap();
        assert!(f.replacing);
        f.query = "mvoe".into();
        f.replace = "move".into();
        assert_eq!(ed.found().len(), 1);
        ed.text = ed.text.replace("mvoe", "move");
        for _ in 0..10 {
            frame(&ctx, &mut ed, &mut t, vec![]);
        }
        assert!(ed.warnings.is_empty());
        frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![key(Key::Escape, Modifiers::NONE)],
        );
        assert!(
            ed.find.is_none() && !ed.asking_close,
            "Esc закрывает поиск, а не скрипт"
        );
        // Ctrl+G 3 Enter — курсор в начале третьей строки.
        frame(&ctx, &mut ed, &mut t, vec![key(Key::G, Modifiers::COMMAND)]);
        frame(&ctx, &mut ed, &mut t, vec![]);
        frame(&ctx, &mut ed, &mut t, vec![Event::Text("3".into())]);
        frame(
            &ctx,
            &mut ed,
            &mut t,
            vec![key(Key::Enter, Modifiers::NONE)],
        );
        frame(&ctx, &mut ed, &mut t, vec![]);
        frame(&ctx, &mut ed, &mut t, vec![]);
        assert!(ed.goto_input.is_none());
        assert_eq!(ed.at.0, 3, "курсор на третьей строке");
        let _ = std::fs::remove_dir_all(&dir);
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
