//! Рабочее пространство «Сюжет» (Сю.2): персонажи, диалоги, задания, переменные мира и
//! переводы из папки `story/` проекта.
//!
//! Вид и раскладка — как у Obsidian (пожелание Романа): узкая полоса значков слева, дерево
//! заметок с поиском, вкладки открытых заметок посередине, крупный заголовок прямо в тексте,
//! блок «Свойства» и текст под ним, справа — «Ссылки сюда», «Структура» и «Проблемы».
//! Правки сохраняются сами через секунду после последней, как в Obsidian (Ctrl+S — сразу).
//! Схема карточек диалога — шаг Сю.3; пока диалог виден как сценарий.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use egui::{
    Align, Color32, CornerRadius, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Pos2, Rect,
    RichText, Sense, Stroke, TextEdit, Ui, UiBuilder, vec2,
};
use reyvec_story::format::{self, Change, Choice, Goal, Stage, Value, VarsFile};
use reyvec_story::{Character, Dialog, LineSettings, Node, Problem, Quest, Story, View};

use super::dialog_graph::{Around, DialogGraph, KINDS as NODE_KINDS, Speaker, change_row};
use super::story_play::Player;
use super::story_refs::{self, Rename, RenameAnswer, Script};
use super::theme::*;
use reyvec_story::rename::Kind;

/// Через сколько секунд после последней правки заметка сохраняется сама.
const AUTOSAVE_S: f64 = 1.0;
/// Полоса значков слева.
const RIBBON: f32 = 44.0;
/// Дерево заметок.
const SIDEBAR: f32 = 260.0;
/// Правая панель.
const RIGHTBAR: f32 = 290.0;
/// Правая панель с проигрывателем шире: в ней лента разговора.
const PLAYBAR: f32 = 380.0;
/// Ширина текста посередине (как «читаемая длина строки» Obsidian).
const READABLE: f32 = 720.0;
/// Заголовок заметки.
const FONT_INLINE_TITLE: f32 = 28.0;
/// Подзаголовки в тексте.
const FONT_H2: f32 = 18.0;
/// Основной текст заметки — чуть крупнее, чем у панелей: с ним работают подолгу.
const FONT_NOTE: f32 = 15.0;

/// Что попросили у окна редактора.
pub(crate) enum StoryOut {
    Log(Color32, String),
    /// Открыть скрипт на строке (из «Ссылок» и поиска, Сю.8).
    OpenScript(PathBuf, usize),
}

/// Заметка: что можно открыть во вкладке.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Item {
    Vars,
    Character(String),
    Dialog(String),
    Quest(String),
    Lang(String),
}

/// Папки дерева.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Folder {
    Characters,
    Dialogs,
    Quests,
    Lang,
}

const FOLDERS: [(Folder, &str); 4] = [
    (Folder::Characters, "Персонажи"),
    (Folder::Dialogs, "Диалоги"),
    (Folder::Quests, "Задания"),
    (Folder::Lang, "Переводы"),
];

/// Вкладка правой панели.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Links,
    Outline,
    Problems,
    Play,
    Search,
}

/// Значки, нарисованные линиями.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Glyph {
    Person,
    Bubble,
    Flag,
    Globe,
    Lang,
    Search,
    Switch,
    Plus,
    Back,
    Forward,
    Close,
    Chevron(bool),
}

pub(crate) struct StoryUi {
    /// Папка `story/` проекта.
    dir: Option<PathBuf>,
    /// Для какого проекта прочитан сюжет.
    loaded_for: Option<PathBuf>,
    story: Story,
    problems: Vec<Problem>,
    tabs: Vec<Item>,
    active: usize,
    dirty: BTreeSet<Item>,
    last_edit: f64,
    filter: String,
    closed: BTreeSet<u8>,
    side: Side,
    /// Окно быстрого перехода (Ctrl+O): строка поиска.
    switcher: Option<String>,
    back: Vec<Item>,
    forward: Vec<Item>,
    /// Прокрутить к карточке диалога или стадии задания.
    scroll_to: Option<String>,
    new_var: (String, usize),
    out: Vec<StoryOut>,
    /// Схема карточек диалога (Сю.3).
    graph: DialogGraph,
    /// Проигрыватель диалога (Сю.4).
    player: Player,
    /// Диалог показан сценарием, а не схемой.
    script: bool,
    /// Скрипты проекта (Сю.8): где имена сюжета используются в коде.
    scripts: Vec<Script>,
    /// Когда скрипты перечитаны.
    scripts_at: f64,
    /// Открытое окно переименования.
    renaming: Option<Rename>,
    /// Строка поиска по тексту (вкладка «Поиск»).
    search: String,
}

impl Default for StoryUi {
    fn default() -> Self {
        Self {
            dir: None,
            loaded_for: None,
            story: Story::default(),
            problems: Vec::new(),
            tabs: Vec::new(),
            active: 0,
            dirty: BTreeSet::new(),
            last_edit: 0.0,
            filter: String::new(),
            closed: BTreeSet::new(),
            side: Side::Links,
            switcher: None,
            back: Vec::new(),
            forward: Vec::new(),
            scroll_to: None,
            new_var: (String::new(), 0),
            out: Vec::new(),
            graph: DialogGraph::default(),
            player: Player::default(),
            script: false,
            scripts: Vec::new(),
            scripts_at: f64::NEG_INFINITY,
            renaming: None,
            search: String::new(),
        }
    }
}

impl Drop for StoryUi {
    fn drop(&mut self) {
        self.save_dirty();
    }
}

impl StoryUi {
    /// Прочитать сюжет проекта, если проект сменился.
    pub(crate) fn sync(&mut self, project: Option<&Path>) {
        let project = project.map(Path::to_path_buf);
        if project == self.loaded_for {
            return;
        }
        self.save_dirty();
        self.loaded_for = project.clone();
        self.dir = project.map(|p| p.join("story"));
        self.reload();
    }

    /// Перечитать папку `story/` (правки, которых нет на диске, теряются — их нет, потому что
    /// перед этим всё сохраняется).
    pub(crate) fn reload(&mut self) {
        let Some(dir) = self.dir.clone() else {
            self.story = Story::default();
            self.problems.clear();
            self.tabs.clear();
            return;
        };
        let (story, problems) = Story::load_dir(&dir);
        self.story = story;
        self.problems = problems;
        self.dirty.clear();
        self.tabs.retain(|t| exists(&self.story, t));
        if self.tabs.is_empty()
            && let Some(id) = self.story.dialogs.keys().next()
        {
            self.tabs.push(Item::Dialog(id.clone()));
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
    }

    /// Открытая заметка.
    fn current(&self) -> Option<Item> {
        self.tabs.get(self.active).cloned()
    }

    /// Открыть заметку: уже открытую — сделать активной, иначе заменить текущую вкладку
    /// (как обычный щелчок в Obsidian; новая вкладка — с Ctrl).
    pub(crate) fn open(&mut self, item: Item, new_tab: bool) {
        if let Some(cur) = self.current()
            && cur != item
        {
            self.back.push(cur);
            self.forward.clear();
        }
        self.show(item, new_tab);
    }

    fn show(&mut self, item: Item, new_tab: bool) {
        if let Some(i) = self.tabs.iter().position(|t| *t == item) {
            self.active = i;
        } else if new_tab || self.tabs.is_empty() {
            self.tabs.push(item);
            self.active = self.tabs.len() - 1;
        } else {
            self.tabs[self.active] = item;
        }
    }

    fn edited(&mut self, item: &Item, now: f64) {
        self.dirty.insert(item.clone());
        self.last_edit = now;
    }

    /// Сохранить все правки и проверить сюжет заново.
    pub(crate) fn save_dirty(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let Some(dir) = self.dir.clone() else {
            return;
        };
        for item in std::mem::take(&mut self.dirty) {
            let (rel, text) = match &item {
                Item::Vars => (
                    "vars.ron".to_string(),
                    format::write(&VarsFile {
                        format: "reyvec-story-vars".into(),
                        version: format::VERSION,
                        vars: self.story.vars.clone(),
                    }),
                ),
                Item::Character(id) => match self.story.characters.get(id) {
                    Some(c) => (file_of(&self.story, &item), format::write(c)),
                    None => continue,
                },
                Item::Dialog(id) => match self.story.dialogs.get(id) {
                    Some(d) => (file_of(&self.story, &item), format::write(d)),
                    None => continue,
                },
                Item::Quest(id) => match self.story.quests.get(id) {
                    Some(q) => (file_of(&self.story, &item), format::write(q)),
                    None => continue,
                },
                Item::Lang(code) => {
                    let words: BTreeMap<_, _> = self
                        .story
                        .lang
                        .get(code)
                        .map(|w| w.iter().filter(|(_, v)| !v.is_empty()).collect())
                        .unwrap_or_default();
                    (file_of(&self.story, &item), format::write(&words))
                }
            };
            let path = dir.join(&rel);
            let res = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, text));
            if let Err(e) = res {
                self.out
                    .push((BAD, format!("Сюжет: не записать {rel}: {e}")).into());
            }
        }
        self.problems = reyvec_story::check::check(&mut self.story);
    }

    fn reload_scripts(&mut self, now: f64) {
        self.scripts = self
            .loaded_for
            .as_deref()
            .map(story_refs::project_scripts)
            .unwrap_or_default();
        self.scripts_at = now;
    }

    /// Открыть окно переименования.
    fn ask_rename(&mut self, kind: Kind, old: &str) {
        self.renaming = Some(Rename::new(kind, old));
        self.scripts_at = f64::NEG_INFINITY;
    }

    fn rename_window(&mut self, ctx: &egui::Context) {
        let Some(mut r) = self.renaming.take() else {
            return;
        };
        match story_refs::rename_window(
            ctx,
            &mut r,
            &self.story,
            &self.scripts,
            self.loaded_for.as_deref(),
        ) {
            RenameAnswer::Keep => self.renaming = Some(r),
            RenameAnswer::Cancel => {}
            RenameAnswer::Apply => self.rename(r.kind, &r.old, r.new.trim()),
        }
    }

    /// Переименовать во всём сюжете и в скриптах проекта (Сю.8) и сразу сохранить.
    pub(crate) fn rename(&mut self, kind: Kind, old: &str, new: &str) {
        self.save_dirty();
        let r = match self.story.rename(kind, old, new) {
            Ok(r) => r,
            Err(e) => {
                self.out
                    .push((BAD, format!("Не переименовать «{old}»: {e}")).into());
                return;
            }
        };
        for id in &r.dialogs {
            self.dirty.insert(Item::Dialog(id.clone()));
        }
        for id in &r.quests {
            self.dirty.insert(Item::Quest(id.clone()));
        }
        for id in &r.characters {
            self.dirty.insert(Item::Character(id.clone()));
        }
        for code in &r.langs {
            self.dirty.insert(Item::Lang(code.clone()));
        }
        if r.vars {
            self.dirty.insert(Item::Vars);
        }
        self.save_dirty();
        if let (Some(dir), Some((from, to))) = (&self.dir, &r.moved)
            && from != to
            && let Err(e) = std::fs::remove_file(dir.join(from))
        {
            self.out
                .push((BAD, format!("Сюжет: не убрать старый файл {from}: {e}")).into());
        }
        // Скрипты проекта: свежие с диска, чтобы не затереть правки из редактора кода.
        self.reload_scripts(self.scripts_at.max(0.0));
        let mut in_scripts = 0;
        let mut files = 0;
        for (path, text) in &self.scripts {
            let (fixed, n) = reyvec_story::rename::luau_rename(text, kind, old, new);
            if n == 0 {
                continue;
            }
            match std::fs::write(path, fixed) {
                Ok(()) => {
                    in_scripts += n;
                    files += 1;
                }
                Err(e) => self
                    .out
                    .push((BAD, format!("Не записать {}: {e}", path.display())).into()),
            }
        }
        self.scripts_at = f64::NEG_INFINITY;
        // Открытые вкладки и история переходов — на новое имя.
        let swap = |it: &mut Item| {
            let new_item = match (&*it, kind) {
                (Item::Dialog(id), Kind::Dialog) if id == old => Item::Dialog(new.into()),
                (Item::Quest(id), Kind::Quest) if id == old => Item::Quest(new.into()),
                (Item::Character(id), Kind::Character) if id == old => Item::Character(new.into()),
                _ => return,
            };
            *it = new_item;
        };
        self.tabs.iter_mut().for_each(swap);
        self.back.iter_mut().for_each(swap);
        self.forward.iter_mut().for_each(swap);
        let scripts = if in_scripts > 0 {
            format!(", в скриптах — {in_scripts} (файлов: {files})")
        } else {
            String::new()
        };
        self.out.push(
            (
                GOOD,
                format!(
                    "Переименовано «{old}» → «{new}»: мест в сюжете — {}{scripts}",
                    r.places
                ),
            )
                .into(),
        );
    }

    /// Рисует всё пространство «Сюжет» на месте `ui`.
    pub(crate) fn draw(&mut self, ui: &mut Ui) -> Vec<StoryOut> {
        let now = ui.input(|i| i.time);
        self.keys(ui);
        if !self.dirty.is_empty() {
            if now - self.last_edit > AUTOSAVE_S {
                self.save_dirty();
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(300));
            }
        }
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0, BG);
        let ribbon = Rect::from_min_size(full.min, vec2(RIBBON, full.height()));
        let left = Rect::from_min_size(
            Pos2::new(ribbon.max.x, full.min.y),
            vec2(SIDEBAR, full.height()),
        );
        let rw = if self.side == Side::Play {
            PLAYBAR
        } else {
            RIGHTBAR
        };
        let right = Rect::from_min_max(Pos2::new(full.max.x - rw, full.min.y), full.max);
        let center = Rect::from_min_max(
            Pos2::new(left.max.x, full.min.y),
            Pos2::new(right.min.x, full.max.y),
        );
        ui.painter().rect_filled(ribbon, 0, BAR);
        ui.painter().rect_filled(left, 0, PANEL);
        ui.painter().rect_filled(right, 0, PANEL);
        for x in [ribbon.max.x, left.max.x, right.min.x] {
            ui.painter()
                .vline(x, full.y_range(), Stroke::new(LINE, BORDER));
        }
        self.ribbon(&mut ui.new_child(UiBuilder::new().max_rect(ribbon.shrink2(vec2(6.0, 8.0)))));
        let mut l = ui.new_child(
            UiBuilder::new()
                .max_rect(left.shrink2(vec2(8.0, 8.0)))
                .layout(Layout::top_down(Align::Min)),
        );
        self.sidebar(&mut l);
        let mut c = ui.new_child(
            UiBuilder::new()
                .max_rect(center)
                .layout(Layout::top_down(Align::Min)),
        );
        self.center(&mut c, now);
        let mut r = ui.new_child(
            UiBuilder::new()
                .max_rect(right.shrink2(vec2(8.0, 8.0)))
                .layout(Layout::top_down(Align::Min)),
        );
        self.rightbar(&mut r);
        self.switcher(ui.ctx());
        self.rename_window(ui.ctx());
        // Скрипты могли поменяться в редакторе кода: перечитываются раз в пару секунд,
        // пока видны ссылки или открыто переименование.
        if (self.side == Side::Links || self.renaming.is_some()) && now - self.scripts_at > 2.0 {
            self.reload_scripts(now);
        }
        std::mem::take(&mut self.out)
    }

    fn keys(&mut self, ui: &Ui) {
        let ctx = ui.ctx();
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S)) {
            self.save_dirty();
            self.out.push((GOOD, "Сюжет сохранён".to_string()).into());
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::O)) {
            self.switcher = Some(String::new());
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::W)) && !self.tabs.is_empty() {
            self.close_tab(self.active);
        }
    }

    fn close_tab(&mut self, i: usize) {
        self.save_dirty();
        self.tabs.remove(i);
        if self.active >= i && self.active > 0 {
            self.active -= 1;
        }
    }

    // ---- Полоса значков.

    fn ribbon(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = GAP_S;
            if glyph_button(ui, Glyph::Switch, "Быстрый переход (Ctrl+O)", false).clicked()
            {
                self.switcher = Some(String::new());
            }
            if glyph_button(
                ui,
                Glyph::Search,
                "Поиск по сюжету: имена и тексты реплик",
                false,
            )
            .clicked()
            {
                self.filter.clear();
                ui.memory_mut(|m| m.request_focus(Id::unique("story-filter")));
            }
            ui.add_space(GAP_L);
            let cur = self.current();
            for (g, hint, item) in [
                (Glyph::Globe, "Мир: переменные сюжета", Some(Item::Vars)),
                (Glyph::Person, "Персонажи", self.first(Folder::Characters)),
                (Glyph::Bubble, "Диалоги", self.first(Folder::Dialogs)),
                (Glyph::Flag, "Задания", self.first(Folder::Quests)),
                (Glyph::Lang, "Переводы", self.first(Folder::Lang)),
            ] {
                let on = match (&cur, &item) {
                    (Some(a), Some(b)) => std::mem::discriminant(a) == std::mem::discriminant(b),
                    _ => false,
                };
                if glyph_button(ui, g, hint, on).clicked()
                    && let Some(item) = item
                {
                    self.open(item, false);
                }
            }
        });
    }

    fn first(&self, f: Folder) -> Option<Item> {
        self.items(f).into_iter().next().map(|(i, _)| i)
    }

    // ---- Дерево заметок.

    fn items(&self, f: Folder) -> Vec<(Item, String)> {
        let s = &self.story;
        match f {
            Folder::Characters => s
                .characters
                .values()
                .map(|c| (Item::Character(c.id.clone()), c.name.clone()))
                .collect(),
            Folder::Dialogs => s
                .dialogs
                .values()
                .map(|d| (Item::Dialog(d.id.clone()), title_or_id(&d.title, &d.id)))
                .collect(),
            Folder::Quests => s
                .quests
                .values()
                .map(|q| (Item::Quest(q.id.clone()), title_or_id(&q.title, &q.id)))
                .collect(),
            Folder::Lang => s
                .lang
                .keys()
                .map(|code| (Item::Lang(code.clone()), lang_name(code)))
                .collect(),
        }
    }

    /// Подходит ли заметка под строку поиска: имя, заголовок или текст реплик.
    fn matches(&self, item: &Item, title: &str) -> bool {
        let q = self.filter.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        let has = |t: &str| t.to_lowercase().contains(&q);
        if has(title) {
            return true;
        }
        match item {
            Item::Character(id) => {
                has(id) || self.story.characters.get(id).is_some_and(|c| has(&c.about))
            }
            Item::Dialog(id) => {
                has(id)
                    || self.story.dialogs.get(id).is_some_and(|d| {
                        d.nodes.iter().any(|n| match n {
                            Node::Line { text, choices, .. } => {
                                has(text) || choices.iter().any(|c| has(&c.text))
                            }
                            _ => false,
                        })
                    })
            }
            Item::Quest(id) => {
                has(id)
                    || self
                        .story
                        .quests
                        .get(id)
                        .is_some_and(|q| q.stages.iter().any(|s| has(&s.journal)))
            }
            Item::Vars => self.story.vars.keys().any(|k| has(k)),
            Item::Lang(code) => has(code),
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(GAP_S);
            ui.label(RichText::new("Сюжет").size(FONT_TITLE).color(TEXT).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let plus = glyph_button(
                    ui,
                    Glyph::Plus,
                    "Создать: персонажа, диалог, задание",
                    false,
                );
                egui::Popup::menu(&plus).show(|ui| {
                    ui.set_min_width(170.0);
                    if ui.button("Новый персонаж").clicked() {
                        self.create(Folder::Characters);
                    }
                    if ui.button("Новый диалог").clicked() {
                        self.create(Folder::Dialogs);
                    }
                    if ui.button("Новое задание").clicked() {
                        self.create(Folder::Quests);
                    }
                });
            });
        });
        ui.add_space(GAP_S);
        ui.add(
            TextEdit::singleline(&mut self.filter)
                .id(Id::unique("story-filter"))
                .hint_text("Поиск: имя или слова из реплик")
                .desired_width(f32::INFINITY)
                .margin(PAD_FIELD),
        );
        ui.add_space(GAP_L);
        if self.dir.as_ref().is_none_or(|d| !d.exists()) {
            ui.label(
                RichText::new("В проекте ещё нет папки story.")
                    .color(MUTED)
                    .size(FONT_BODY),
            );
            if ui.button("Создать сюжет").clicked() {
                self.edited(&Item::Vars, 0.0);
                self.save_dirty();
                self.reload();
                self.open(Item::Vars, false);
            }
            return;
        }
        let cur = self.current();
        egui::ScrollArea::vertical()
            .id_salt("story-tree")
            .auto_shrink([false, false])
            .max_height(ui.available_height() - 28.0)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let vars_title = format!("Мир · {} переменных", self.story.vars.len());
                if self.matches(&Item::Vars, &vars_title)
                    && tree_row(
                        ui,
                        &vars_title,
                        0.0,
                        cur == Some(Item::Vars),
                        self.dirty.contains(&Item::Vars),
                    )
                    .clicked()
                {
                    self.open(Item::Vars, false);
                }
                for (i, (f, name)) in FOLDERS.iter().enumerate() {
                    let items: Vec<_> = self
                        .items(*f)
                        .into_iter()
                        .filter(|(it, t)| self.matches(it, t))
                        .collect();
                    if items.is_empty() && !self.filter.trim().is_empty() {
                        continue;
                    }
                    let open = !self.closed.contains(&(i as u8)) || !self.filter.trim().is_empty();
                    if folder_row(ui, name, items.len(), open).clicked() {
                        if open {
                            self.closed.insert(i as u8);
                        } else {
                            self.closed.remove(&(i as u8));
                        }
                    }
                    if open {
                        for (item, title) in items {
                            let r = tree_row(
                                ui,
                                &title,
                                INDENT,
                                cur.as_ref() == Some(&item),
                                self.dirty.contains(&item),
                            );
                            if r.clicked() {
                                let ctrl = ui.input(|i| i.modifiers.command);
                                self.open(item, ctrl);
                            }
                        }
                    }
                }
            });
        // Внизу — имя проекта, как имя хранилища в Obsidian.
        let name = self
            .loaded_for
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "проект".into(), |n| n.to_string_lossy().into_owned());
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.label(
                RichText::new(format!("⌂ {name}"))
                    .color(MUTED)
                    .size(FONT_SMALL),
            );
        });
    }

    /// Новая заметка с именем, которого ещё нет.
    fn create(&mut self, f: Folder) {
        let s = &mut self.story;
        let free = |base: &str, taken: &dyn Fn(&str) -> bool| {
            (1..)
                .map(|n| {
                    if n == 1 {
                        base.to_string()
                    } else {
                        format!("{base}_{n}")
                    }
                })
                .find(|id| !taken(id))
                .unwrap_or_default()
        };
        let item = match f {
            Folder::Characters => {
                let id = free("novyi_personazh", &|id| s.characters.contains_key(id));
                s.characters.insert(
                    id.clone(),
                    Character {
                        format: "reyvec-character".into(),
                        version: format::VERSION,
                        id: id.clone(),
                        name: "Новый персонаж".into(),
                        color: "#AEB5C4".into(),
                        about: String::new(),
                        lines: LineSettings::default(),
                        relations: BTreeMap::new(),
                    },
                );
                s.files
                    .insert(format!("c:{id}"), format!("characters/{id}.ron"));
                Item::Character(id)
            }
            Folder::Dialogs => {
                let id = free("novyi_dialog", &|id| s.dialogs.contains_key(id));
                let who = s
                    .characters
                    .keys()
                    .find(|k| *k != "hero")
                    .cloned()
                    .unwrap_or_default();
                s.dialogs.insert(
                    id.clone(),
                    Dialog {
                        format: "reyvec-dialog".into(),
                        version: format::VERSION,
                        id: id.clone(),
                        title: "Новый диалог".into(),
                        with: if who.is_empty() {
                            Vec::new()
                        } else {
                            vec![who.clone()]
                        },
                        start: String::new(),
                        lines: LineSettings::default(),
                        nodes: vec![
                            Node::Line {
                                id: "nachalo".into(),
                                at: (0.0, 0.0),
                                who,
                                text: "Привет.".into(),
                                settings: LineSettings::default(),
                                choices: Vec::new(),
                                next: "konec".into(),
                                key: String::new(),
                            },
                            Node::End {
                                id: "konec".into(),
                                at: (240.0, 0.0),
                            },
                        ],
                    },
                );
                s.files
                    .insert(format!("d:{id}"), format!("dialogs/{id}.ron"));
                Item::Dialog(id)
            }
            Folder::Quests => {
                let id = free("novoe_zadanie", &|id| s.quests.contains_key(id));
                s.quests.insert(
                    id.clone(),
                    Quest {
                        format: "reyvec-quest".into(),
                        version: format::VERSION,
                        id: id.clone(),
                        title: "Новое задание".into(),
                        description: String::new(),
                        stages: vec![new_stage("nachalo")],
                        rewards: Vec::new(),
                        fail_when: String::new(),
                    },
                );
                s.files
                    .insert(format!("q:{id}"), format!("quests/{id}.ron"));
                Item::Quest(id)
            }
            Folder::Lang => return,
        };
        self.edited(&item, 0.0);
        self.save_dirty();
        self.open(item, false);
    }

    // ---- Середина: вкладки, путь, текст.

    fn center(&mut self, ui: &mut Ui, now: f64) {
        self.tab_bar(ui);
        let Some(item) = self.current() else {
            ui.add_space(80.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Нет открытых заметок")
                        .size(FONT_H2)
                        .color(TEXT_DIM),
                );
                ui.label(RichText::new("Выберите заметку слева или нажмите Ctrl+O").color(MUTED));
            });
            return;
        };
        self.nav_bar(ui, &item);
        if let Item::Dialog(id) = &item {
            self.dialog_bar(ui, id, now);
            if !self.script {
                if let Some(node) = self.scroll_to.take() {
                    self.graph.focus(id, &node);
                }
                if self.dialog_schema(ui, id) {
                    self.edited(&item, now);
                }
                return;
            }
        }
        let width = ui.available_width();
        let pad = ((width - READABLE) / 2.0).max(GAP_XXL * 2.0);
        egui::ScrollArea::vertical()
            .id_salt(("story-note", format!("{item:?}")))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Frame::NONE
                    .inner_margin(Margin {
                        left: pad as i8,
                        right: pad as i8,
                        top: 20,
                        bottom: 60,
                    })
                    .show(ui, |ui| {
                        ui.set_max_width(READABLE.min(width - 2.0 * pad));
                        ui.spacing_mut().item_spacing.y = GAP_L;
                        let changed = match &item {
                            Item::Vars => self.vars_view(ui),
                            Item::Character(id) => self.character_view(ui, id),
                            Item::Dialog(id) => self.dialog_view(ui, id),
                            Item::Quest(id) => self.quest_view(ui, id),
                            Item::Lang(code) => self.lang_view(ui, code),
                        };
                        if changed {
                            self.edited(&item, now);
                        }
                    });
            });
    }

    /// Полоса над диалогом: «Схема / Сценарий» и, на схеме, кнопки новых карточек.
    fn dialog_bar(&mut self, ui: &mut Ui, id: &str, now: f64) {
        ui.add_space(GAP_S);
        ui.horizontal(|ui| {
            ui.add_space(GAP_L);
            for (script, name) in [(false, "Схема"), (true, "Сценарий")] {
                if ui.selectable_label(self.script == script, name).clicked() {
                    self.script = script;
                }
            }
            if ui
                .button("▶ Проиграть")
                .on_hover_text("Пройти диалог как в игре, справа")
                .clicked()
            {
                self.side = Side::Play;
                self.player.start(&mut self.story, id);
            }
            if ui
                .button(RichText::new(format!("✎ {id}")).monospace())
                .on_hover_text("Переименовать диалог везде: в других диалогах и скриптах")
                .clicked()
            {
                self.ask_rename(Kind::Dialog, id);
            }
            if self.script {
                return;
            }
            if ui
                .button("Разложить")
                .on_hover_text("Расставить карточки столбцами от начала разговора")
                .clicked()
                && let Some(d) = self.story.dialogs.get_mut(id)
            {
                self.graph.arrange(d);
                self.edited(&Item::Dialog(id.to_string()), now);
            }
        });
        if self.script {
            ui.add_space(GAP_S);
            return;
        }
        ui.add_space(GAP_S);
        ui.horizontal_wrapped(|ui| {
            ui.add_space(GAP_L);
            ui.label(RichText::new("Добавить:").color(MUTED).size(FONT_CHIP));
            ui.spacing_mut().button_padding = vec2(GAP_M, 2.0);
            ui.spacing_mut().item_spacing.x = GAP_S;
            let who = self
                .story
                .dialogs
                .get(id)
                .and_then(|d| d.with.first().cloned())
                .unwrap_or_default();
            for kind in NODE_KINDS {
                let b = egui::Button::new(
                    RichText::new(kind.title())
                        .size(FONT_SMALL + 1.0)
                        .color(Color32::WHITE),
                )
                .fill(kind.color())
                .corner_radius(RADIUS_M);
                if ui.add(b).on_hover_text(kind.hint()).clicked()
                    && let Some(d) = self.story.dialogs.get_mut(id)
                {
                    self.graph.add(d, kind, &who);
                    self.edited(&Item::Dialog(id.to_string()), now);
                }
            }
        });
        ui.add_space(GAP_S);
    }

    /// Схема карточек на всё оставшееся место. `true` — диалог поменялся.
    fn dialog_schema(&mut self, ui: &mut Ui, id: &str) -> bool {
        let speakers: Vec<Speaker> = self
            .story
            .characters
            .values()
            .map(|c| Speaker {
                id: c.id.clone(),
                name: c.name.clone(),
                color: parse_hex(&c.color).unwrap_or(TEXT),
            })
            .collect();
        let dialogs: Vec<String> = self.story.dialogs.keys().cloned().collect();
        let Some(d) = self.story.dialogs.get_mut(id) else {
            return false;
        };
        self.graph.playing = self
            .player
            .at()
            .filter(|(dlg, _)| *dlg == id)
            .map(|(_, n)| n.to_string());
        let around = Around {
            speakers: &speakers,
            dialogs: &dialogs,
        };
        self.graph.draw(ui, d, &around)
    }

    /// Для снимка вида: проигрыватель справа, разговор дошёл до вариантов.
    #[cfg(test)]
    pub(crate) fn preview_play(&mut self, dialog: &str) {
        self.side = Side::Play;
        self.player.start(&mut self.story, dialog);
        self.player.go_on(&mut self.story);
    }

    fn tab_bar(&mut self, ui: &mut Ui) {
        let (bar, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), TAB_HEIGHT + 6.0), Sense::hover());
        ui.painter().rect_filled(bar, 0, PANEL);
        ui.painter()
            .hline(bar.x_range(), bar.max.y - 0.5, Stroke::new(LINE, BORDER));
        let mut x = bar.min.x + GAP_L;
        let mut close = None;
        let mut pick = None;
        for (i, item) in self.tabs.iter().enumerate() {
            let title = item_title(&self.story, item);
            let font = FontId::proportional(FONT_BODY);
            let galley = ui.painter().layout_no_wrap(title, font, TEXT);
            let w = (galley.size().x + 44.0).clamp(110.0, 220.0);
            let rect = Rect::from_min_size(Pos2::new(x, bar.min.y + 6.0), vec2(w, TAB_HEIGHT));
            let r = ui.interact(rect, Id::unique(("story-tab", i)), Sense::click());
            let on = i == self.active;
            if on {
                ui.painter().rect_filled(
                    rect,
                    CornerRadius {
                        nw: RADIUS_M,
                        ne: RADIUS_M,
                        sw: 0,
                        se: 0,
                    },
                    BG,
                );
            } else if r.hovered() {
                ui.painter().rect_filled(rect.shrink(3.0), RADIUS_S, HOVER);
            }
            let color = if on { TEXT } else { TEXT_DIM };
            ui.painter()
                .with_clip_rect(rect.shrink2(vec2(12.0, 0.0)))
                .galley(
                    Pos2::new(rect.min.x + 12.0, rect.center().y - galley.size().y / 2.0),
                    galley,
                    color,
                );
            let xr = Rect::from_center_size(
                Pos2::new(rect.max.x - 15.0, rect.center().y),
                vec2(18.0, 18.0),
            );
            if self.dirty.contains(item) && !r.hovered() {
                ui.painter().circle_filled(xr.center(), 3.5, TEXT_DIM);
            } else if r.hovered() || on {
                let xresp = ui.interact(xr, Id::unique(("story-tab-x", i)), Sense::click());
                if xresp.hovered() {
                    ui.painter().rect_filled(xr, RADIUS_S, HOVER);
                }
                paint_glyph(ui.painter(), xr.center(), Glyph::Close, MUTED);
                if xresp.clicked() {
                    close = Some(i);
                }
            }
            if r.clicked() {
                pick = Some(i);
            }
            if r.middle_clicked() {
                close = Some(i);
            }
            x += w + 2.0;
        }
        if let Some(i) = close {
            self.close_tab(i);
        } else if let Some(i) = pick {
            self.active = i;
        }
    }

    fn nav_bar(&mut self, ui: &mut Ui, item: &Item) {
        ui.add_space(GAP_S);
        ui.horizontal(|ui| {
            ui.add_space(GAP_L);
            if glyph_enabled(ui, Glyph::Back, "Назад", !self.back.is_empty()).clicked()
                && let Some(prev) = self.back.pop()
            {
                if let Some(cur) = self.current() {
                    self.forward.push(cur);
                }
                self.show(prev, false);
            }
            if glyph_enabled(ui, Glyph::Forward, "Вперёд", !self.forward.is_empty()).clicked()
                && let Some(next) = self.forward.pop()
            {
                if let Some(cur) = self.current() {
                    self.back.push(cur);
                }
                self.show(next, false);
            }
            let folder = match item {
                Item::Vars => "Мир",
                Item::Character(_) => "Персонажи",
                Item::Dialog(_) => "Диалоги",
                Item::Quest(_) => "Задания",
                Item::Lang(_) => "Переводы",
            };
            let title = item_title(&self.story, item);
            ui.with_layout(
                Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    ui.label(
                        RichText::new(format!("{folder}  /  {title}"))
                            .color(MUTED)
                            .size(FONT_CHIP),
                    );
                },
            );
        });
    }

    fn vars_view(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        inline_title_static(ui, "Мир");
        ui.label(
            RichText::new(
                "Переменные сюжета: что произошло и что известно. Условия в диалогах читают их \
                 по имени: kuznec.znakom, hero.money >= 10. Здесь — значения в начале игры.",
            )
            .color(TEXT_DIM)
            .size(FONT_NOTE),
        );
        ui.add_space(GAP_L);
        let mut remove = None;
        let mut rename = None;
        egui::Grid::new("story-vars")
            .num_columns(4)
            .spacing(vec2(GAP_XL, GAP_M))
            .striped(false)
            .show(ui, |ui| {
                for h in ["Имя", "Тип", "В начале игры", ""] {
                    ui.label(RichText::new(h).color(MUTED).size(FONT_SMALL));
                }
                ui.end_row();
                let names: Vec<String> = self.story.vars.keys().cloned().collect();
                for name in names {
                    ui.label(RichText::new(&name).monospace().color(TEXT));
                    let v = self.story.vars.get_mut(&name).expect("имя из списка");
                    let mut kind = kind_index(v);
                    egui::ComboBox::from_id_salt(("var-kind", &name))
                        .width(110.0)
                        .selected_text(KINDS[kind])
                        .show_ui(ui, |ui| {
                            for (i, k) in KINDS.iter().enumerate() {
                                ui.selectable_value(&mut kind, i, *k);
                            }
                        });
                    if kind != kind_index(v) {
                        *v = convert(v, kind);
                        changed = true;
                    }
                    changed |= value_edit(ui, v, &name);
                    ui.horizontal(|ui| {
                        if rename_button(ui) {
                            rename = Some(name.clone());
                        }
                        if glyph_button(ui, Glyph::Close, "Удалить переменную", false).clicked()
                        {
                            remove = Some(name.clone());
                        }
                    });
                    ui.end_row();
                }
            });
        if let Some(name) = remove {
            self.story.vars.remove(&name);
            changed = true;
        }
        if let Some(name) = rename {
            self.ask_rename(Kind::Var, &name);
        }
        ui.add_space(GAP_L);
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.new_var.0)
                    .hint_text("новая.переменная")
                    .desired_width(200.0)
                    .font(FontId::monospace(FONT_MONO))
                    .margin(PAD_FIELD),
            );
            egui::ComboBox::from_id_salt("new-var-kind")
                .width(110.0)
                .selected_text(KINDS[self.new_var.1])
                .show_ui(ui, |ui| {
                    for (i, k) in KINDS.iter().enumerate() {
                        ui.selectable_value(&mut self.new_var.1, i, *k);
                    }
                });
            let name = self.new_var.0.trim().to_string();
            let ok = reyvec_story::valid_id(&name) && !self.story.vars.contains_key(&name);
            if ui.add_enabled(ok, egui::Button::new("Добавить")).clicked() {
                self.story
                    .vars
                    .insert(name, convert(&Value::Bool(false), self.new_var.1));
                self.new_var.0.clear();
                changed = true;
            }
        });
        if !self.new_var.0.trim().is_empty() && !reyvec_story::valid_id(self.new_var.0.trim()) {
            ui.label(
                RichText::new("Имя: латиница в нижнем регистре, цифры, _ и точка (kuznec.znakom)")
                    .color(BAD)
                    .size(FONT_SMALL),
            );
        }
        changed
    }

    fn character_view(&mut self, ui: &mut Ui, id: &str) -> bool {
        let mut changed = false;
        let mut rename = false;
        let others: Vec<(String, String)> = self
            .story
            .characters
            .values()
            .filter(|o| o.id != id)
            .map(|o| (o.id.clone(), o.name.clone()))
            .collect();
        let Some(c) = self.story.characters.get_mut(id) else {
            return false;
        };
        changed |= inline_title(ui, &mut c.name, "Имя персонажа");
        properties(ui, |ui| {
            prop(ui, "Имя в коде", |ui| {
                ui.label(RichText::new(&c.id).monospace().color(TEXT_DIM));
                rename |= rename_button(ui);
            });
            prop(ui, "Цвет имени", |ui| {
                let mut rgb = parse_hex(&c.color).unwrap_or(TEXT_DIM);
                if ui.color_edit_button_srgba(&mut rgb).changed() {
                    c.color = format!("#{:02X}{:02X}{:02X}", rgb.r(), rgb.g(), rgb.b());
                    changed = true;
                }
                ui.label(RichText::new(&c.color).monospace().color(MUTED));
            });
            changed |= line_settings(ui, &mut c.lines, "char");
        });
        ui.add_space(GAP_L);
        h2(ui, "Характер");
        changed |= ui
            .add(
                TextEdit::multiline(&mut c.about)
                    .frame(Frame::NONE)
                    .font(FontId::proportional(FONT_NOTE))
                    .desired_width(f32::INFINITY)
                    .desired_rows(6)
                    .hint_text(
                        "Кто он, как говорит, чего хочет, что знает. Это читает автор, а позже \
                         и модель для живых реплик (Сю.10).",
                    ),
            )
            .changed();
        changed |= relations_edit(ui, &mut c.relations, &others);
        ui.add_space(GAP_L);
        h2(ui, "Память");
        muted(
            ui,
            &format!(
                "В игре персонаж запоминает факты: событие «memory.add» в диалоге \
                 (персонаж, имя факта, текст, важность 1–10) или npc.remember в скрипте. \
                 Условия читают их так: memory.{id}.ruda_prinesena, relation.{id}.hero >= 2. \
                 Неважное забывается: важность 1 — через 10 минут игры, 10 — никогда."
            ),
        );
        if rename {
            self.ask_rename(Kind::Character, id);
        }
        changed
    }

    fn dialog_view(&mut self, ui: &mut Ui, id: &str) -> bool {
        let mut changed = false;
        let names: BTreeMap<String, (String, Color32)> = self
            .story
            .characters
            .values()
            .map(|c| {
                (
                    c.id.clone(),
                    (c.name.clone(), parse_hex(&c.color).unwrap_or(TEXT)),
                )
            })
            .collect();
        let dialogs: Vec<String> = self.story.dialogs.keys().cloned().collect();
        let mut rename = false;
        let Some(d) = self.story.dialogs.get_mut(id) else {
            return false;
        };
        changed |= inline_title(ui, &mut d.title, "Заголовок диалога");
        let node_ids: Vec<String> = d.nodes.iter().map(|n| n.id().to_string()).collect();
        properties(ui, |ui| {
            prop(ui, "Имя в коде", |ui| {
                ui.label(RichText::new(&d.id).monospace().color(TEXT_DIM));
                rename |= rename_button(ui);
            });
            prop(ui, "Участники", |ui| {
                let mut with = d.with.join(", ");
                if ui
                    .add(
                        TextEdit::singleline(&mut with)
                            .frame(Frame::NONE)
                            .desired_width(f32::INFINITY),
                    )
                    .changed()
                {
                    d.with = with
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    changed = true;
                }
            });
            prop(ui, "Начало", |ui| {
                let shown = d.start_node().unwrap_or("").to_string();
                egui::ComboBox::from_id_salt("dialog-start")
                    .selected_text(&shown)
                    .show_ui(ui, |ui| {
                        for n in &node_ids {
                            if ui.selectable_label(*n == shown, n).clicked() {
                                d.start = n.clone();
                                changed = true;
                            }
                        }
                    });
            });
            prop(ui, "Карточек", |ui| {
                ui.label(RichText::new(d.nodes.len().to_string()).color(TEXT_DIM));
            });
            changed |= line_settings(ui, &mut d.lines, "dialog");
        });
        callout(
            ui,
            "Сценарий — тот же диалог текстом: реплики и варианты можно править здесь. \
             Новые карточки и стрелки — на схеме.",
        );
        let mut jump_to = None;
        let mut open = None;
        for node in &mut d.nodes {
            let nid = node.id().to_string();
            let anchor = ui.label(
                RichText::new(format!("#{nid}"))
                    .monospace()
                    .size(FONT_SMALL)
                    .color(MUTED),
            );
            if self.scroll_to.as_deref() == Some(nid.as_str()) {
                anchor.scroll_to_me(Some(Align::Min));
                self.scroll_to = None;
            }
            match node {
                Node::Line {
                    who,
                    text,
                    choices,
                    next,
                    ..
                } => {
                    let (name, color) = names
                        .get(who.as_str())
                        .cloned()
                        .unwrap_or_else(|| (who.clone(), BAD));
                    ui.label(RichText::new(name).color(color).strong().size(FONT_NOTE));
                    changed |= ui
                        .add(
                            TextEdit::multiline(text)
                                .frame(Frame::NONE)
                                .font(FontId::proportional(FONT_NOTE))
                                .desired_width(f32::INFINITY)
                                .desired_rows(1),
                        )
                        .changed();
                    for (i, c) in choices.iter_mut().enumerate() {
                        changed |= choice_row(ui, c, i, &nid, &mut jump_to);
                    }
                    if !next.is_empty() {
                        arrow(ui, "дальше", next, &mut jump_to);
                    }
                }
                Node::Check { when, yes, no, .. } => {
                    ui.horizontal_wrapped(|ui| {
                        muted(ui, "◇ если");
                        code(ui, when);
                        link(ui, &format!("да → {yes}"), yes, &mut jump_to);
                        link(ui, &format!("нет → {no}"), no, &mut jump_to);
                    });
                }
                Node::Set { changes, next, .. } => {
                    for ch in changes.iter() {
                        ui.horizontal_wrapped(|ui| {
                            muted(ui, "=");
                            code(ui, &change_text(ch));
                        });
                    }
                    arrow(ui, "дальше", next, &mut jump_to);
                }
                Node::Event {
                    name, args, next, ..
                } => {
                    ui.horizontal_wrapped(|ui| {
                        muted(ui, "→ событие");
                        code(ui, &format!("{name}({})", args_text(args)));
                    });
                    arrow(ui, "дальше", next, &mut jump_to);
                }
                Node::Run {
                    func, args, next, ..
                } => {
                    ui.horizontal_wrapped(|ui| {
                        muted(ui, "→ выполнить");
                        code(ui, &format!("{func}({})", args_text(args)));
                    });
                    arrow(ui, "дальше", next, &mut jump_to);
                }
                Node::Jump { dialog, node, .. } => {
                    ui.horizontal_wrapped(|ui| {
                        muted(ui, "→ перейти в диалог");
                        let known = dialogs.contains(dialog);
                        let label = if node.is_empty() {
                            dialog.clone()
                        } else {
                            format!("{dialog} #{node}")
                        };
                        if ui
                            .link(RichText::new(label).color(if known { SELECT } else { BAD }))
                            .clicked()
                            && known
                        {
                            open = Some(Item::Dialog(dialog.clone()));
                        }
                    });
                }
                Node::Random { next, .. } => {
                    ui.horizontal_wrapped(|ui| {
                        muted(ui, "◇ наугад:");
                        for n in next.iter() {
                            link(ui, n, n, &mut jump_to);
                        }
                    });
                }
                Node::Chance {
                    percent, yes, no, ..
                } => {
                    ui.horizontal_wrapped(|ui| {
                        muted(ui, &format!("◇ с вероятностью {percent}%"));
                        link(ui, &format!("да → {yes}"), yes, &mut jump_to);
                        link(ui, &format!("нет → {no}"), no, &mut jump_to);
                    });
                }
                Node::Wait { seconds, next, .. } => {
                    let t = seconds.map_or_else(|| "до нажатия".to_string(), |s| format!("{s} с"));
                    ui.horizontal_wrapped(|ui| muted(ui, &format!("… пауза {t}")));
                    arrow(ui, "дальше", next, &mut jump_to);
                }
                Node::End { .. } => {
                    muted(ui, "■ конец разговора");
                }
            }
            ui.add_space(GAP_M);
        }
        if let Some(n) = jump_to {
            self.scroll_to = Some(n);
        }
        if let Some(item) = open {
            self.open(item, false);
        }
        if rename {
            self.ask_rename(Kind::Dialog, id);
        }
        changed
    }

    fn quest_view(&mut self, ui: &mut Ui, id: &str) -> bool {
        let mut changed = false;
        let mut rename_quest = false;
        let Some(q) = self.story.quests.get_mut(id) else {
            return false;
        };
        changed |= inline_title(ui, &mut q.title, "Название задания");
        properties(ui, |ui| {
            prop(ui, "Имя в коде", |ui| {
                ui.label(RichText::new(&q.id).monospace().color(TEXT_DIM));
                rename_quest |= rename_button(ui);
            });
            prop(ui, "Описание", |ui| {
                changed |= ui
                    .add(
                        TextEdit::multiline(&mut q.description)
                            .frame(Frame::NONE)
                            .desired_rows(1)
                            .desired_width(f32::INFINITY)
                            .hint_text("О чём задание — видно в журнале"),
                    )
                    .changed();
            });
            prop(ui, "Провал, если", |ui| {
                changed |= ui
                    .add(
                        TextEdit::singleline(&mut q.fail_when)
                            .frame(Frame::NONE)
                            .font(FontId::monospace(FONT_MONO))
                            .desired_width(f32::INFINITY)
                            .hint_text("условие Luau, например kuznec.zloy"),
                    )
                    .on_hover_text("Стало верным, пока задание идёт, — задание провалено")
                    .changed();
            });
        });

        ui.add_space(GAP_M);
        h2(ui, "Награда");
        muted(ui, "Что меняется в мире, когда задание выполнено");
        let mut drop = None;
        for (i, c) in q.rewards.iter_mut().enumerate() {
            let (ch, del) = change_row(ui, c, ("reward", i), 180.0);
            changed |= ch;
            if del {
                drop = Some(i);
            }
        }
        if let Some(i) = drop {
            q.rewards.remove(i);
            changed = true;
        }
        if add_button(ui, "+ награда") {
            q.rewards.push(format::Change {
                var: String::new(),
                to: Some(Value::Bool(true)),
                add: None,
            });
            changed = true;
        }

        ui.add_space(GAP_M);
        h2(ui, "Стадии");
        let ids: Vec<String> = q.stages.iter().map(|s| s.id.clone()).collect();
        let n = q.stages.len();
        let mut action = None;
        let mut rename = None;
        for (i, s) in q.stages.iter_mut().enumerate() {
            ui.add_space(GAP_S);
            let r = ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}.", i + 1))
                        .size(FONT_H2)
                        .color(TEXT)
                        .strong(),
                );
                let mut sid = s.id.clone();
                if ui
                    .add(
                        TextEdit::singleline(&mut sid)
                            .id_salt(("stage-id", i))
                            .frame(Frame::NONE)
                            .font(FontId::monospace(FONT_H2 - 2.0))
                            .desired_width(180.0),
                    )
                    .on_hover_text("Имя стадии в коде: quest.stage(\"задание\") его вернёт")
                    .changed()
                {
                    rename = Some((s.id.clone(), sid.clone()));
                    s.id = sid;
                    changed = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("×").color(MUTED)).frame(false))
                        .on_hover_text("Удалить стадию")
                        .clicked()
                    {
                        action = Some(StageAction::Remove(i));
                    }
                    if ui
                        .add_enabled(i + 1 < n, egui::Button::new("↓").frame(false))
                        .on_hover_text("Ниже")
                        .clicked()
                    {
                        action = Some(StageAction::Swap(i, i + 1));
                    }
                    if ui
                        .add_enabled(i > 0, egui::Button::new("↑").frame(false))
                        .on_hover_text("Выше")
                        .clicked()
                    {
                        action = Some(StageAction::Swap(i - 1, i));
                    }
                });
            });
            if self.scroll_to.as_deref() == Some(s.id.as_str()) {
                r.response.scroll_to_me(Some(Align::Min));
                self.scroll_to = None;
            }
            changed |= ui
                .add(
                    TextEdit::multiline(&mut s.journal)
                        .id_salt(("stage-journal", i))
                        .frame(Frame::NONE)
                        .font(FontId::proportional(FONT_NOTE))
                        .desired_width(f32::INFINITY)
                        .desired_rows(1)
                        .hint_text("Запись в журнале игрока"),
                )
                .changed();
            let mut drop = None;
            for (gi, g) in s.goals.iter_mut().enumerate() {
                let (ch, del) = goal_row(ui, g);
                changed |= ch;
                if del {
                    drop = Some(gi);
                }
            }
            if let Some(gi) = drop {
                s.goals.remove(gi);
                changed = true;
            }
            ui.horizontal(|ui| {
                ui.add_space(GAP_L);
                if add_button(ui, "+ цель") {
                    let gid = (1..)
                        .map(|k| format!("cel_{k}"))
                        .find(|g| !s.goals.iter().any(|x| x.id == *g))
                        .unwrap_or_default();
                    s.goals.push(format::Goal {
                        id: gid,
                        text: String::new(),
                        count: 1,
                    });
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(GAP_L);
                muted(ui, "и если");
                changed |= ui
                    .add(
                        TextEdit::singleline(&mut s.done_when)
                            .id_salt(("stage-when", i))
                            .font(FontId::monospace(FONT_MONO))
                            .desired_width(260.0)
                            .margin(PAD_FIELD)
                            .hint_text("условие Luau (можно пусто)"),
                    )
                    .on_hover_text(
                        "Стадия закончится, когда выполнены цели и это условие верно. \
                         Без целей и условия стадию заканчивает скрипт или событие \
                         quest.stage / quest.complete",
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.add_space(GAP_L);
                muted(ui, "↓ дальше");
                let shown = if s.next.is_empty() {
                    "■ задание выполнено".to_string()
                } else {
                    s.next.clone()
                };
                egui::ComboBox::from_id_salt(("stage-next", i))
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(s.next.is_empty(), "■ задание выполнено")
                            .clicked()
                        {
                            s.next.clear();
                            changed = true;
                        }
                        for other in ids.iter().filter(|o| **o != ids[i]) {
                            if ui.selectable_label(s.next == *other, other).clicked() {
                                s.next = other.clone();
                                changed = true;
                            }
                        }
                    });
            });
        }
        if let Some((old, new)) = rename {
            for s in &mut q.stages {
                if s.next == old {
                    s.next = new.clone();
                }
            }
        }
        match action {
            Some(StageAction::Remove(i)) => {
                let gone = q.stages.remove(i).id;
                for s in &mut q.stages {
                    if s.next == gone {
                        s.next.clear();
                    }
                }
                changed = true;
            }
            Some(StageAction::Swap(a, b)) => {
                q.stages.swap(a, b);
                changed = true;
            }
            None => {}
        }
        if add_button(ui, "+ стадия") {
            let sid = (1..)
                .map(|k| format!("stadiya_{k}"))
                .find(|x| q.stage(x).is_none())
                .unwrap_or_default();
            // Новая стадия встаёт в конец цепочки: последняя теперь ведёт в неё.
            if let Some(last) = q.stages.iter_mut().find(|s| s.next.is_empty()) {
                last.next = sid.clone();
            }
            q.stages.push(new_stage(&sid));
            changed = true;
        }

        ui.add_space(GAP_L);
        h2(ui, "Как увидит игрок");
        journal_preview(ui, q);
        if rename_quest {
            self.ask_rename(Kind::Quest, id);
        }
        changed
    }

    fn lang_view(&mut self, ui: &mut Ui, code: &str) -> bool {
        let mut changed = false;
        inline_title_static(ui, &lang_name(code));
        ui.label(
            RichText::new(
                "Слева — основной текст, справа — перевод. Пустой перевод — в игре звучит \
                 основной текст.",
            )
            .color(TEXT_DIM)
            .size(FONT_NOTE),
        );
        let rows: Vec<(String, Vec<(String, String)>)> = self
            .story
            .dialogs
            .values()
            .map(|d| (title_or_id(&d.title, &d.id), dialog_keys(d)))
            .collect();
        let words = self.story.lang.entry(code.to_string()).or_default();
        for (title, keys) in rows {
            ui.add_space(GAP_M);
            h2(ui, &title);
            for (key, main) in keys {
                ui.label(
                    RichText::new(&key)
                        .monospace()
                        .size(FONT_SMALL)
                        .color(MUTED),
                );
                ui.columns(2, |cols| {
                    cols[0].label(RichText::new(&main).color(TEXT_DIM).size(FONT_NOTE));
                    let t = words.entry(key.clone()).or_default();
                    changed |= cols[1]
                        .add(
                            TextEdit::multiline(t)
                                .font(FontId::proportional(FONT_NOTE))
                                .desired_rows(1)
                                .desired_width(f32::INFINITY)
                                .hint_text("перевод"),
                        )
                        .changed();
                });
            }
        }
        changed
    }

    // ---- Правая панель.

    fn rightbar(&mut self, ui: &mut Ui) {
        let mine = self.current().map(|i| file_of(&self.story, &i));
        let errors = self.problems.iter().filter(|p| !p.warning).count();
        ui.horizontal(|ui| {
            for (s, t) in [
                (Side::Links, "Ссылки".to_string()),
                (Side::Outline, "Структура".to_string()),
                (Side::Play, "Проиграть".to_string()),
                (Side::Search, "Поиск".to_string()),
                (
                    Side::Problems,
                    if self.problems.is_empty() {
                        "Проблемы".to_string()
                    } else {
                        format!("Проблемы {}", self.problems.len())
                    },
                ),
            ] {
                let on = self.side == s;
                let color = if on {
                    TEXT
                } else if s == Side::Problems && errors > 0 {
                    BAD
                } else {
                    MUTED
                };
                if ui
                    .add(egui::Button::new(RichText::new(t).color(color).size(FONT_CHIP)).frame(on))
                    .clicked()
                {
                    self.side = s;
                }
            }
        });
        ui.separator();
        let Some(item) = self.current() else {
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("story-right")
            .auto_shrink([false, false])
            .show(ui, |ui| match self.side {
                Side::Links => {
                    let links = backlinks(&self.story, &item);
                    caption(ui, "Ссылки сюда");
                    if links.is_empty() {
                        muted(ui, "Нигде не используется");
                    }
                    for (it, why) in links {
                        let r = ui.add(
                            egui::Button::new(
                                RichText::new(item_title(&self.story, &it)).color(TEXT),
                            )
                            .frame(false),
                        );
                        ui.label(RichText::new(why).color(MUTED).size(FONT_SMALL));
                        if r.clicked() {
                            self.open(it, false);
                        }
                    }
                    self.script_links(ui, &item);
                    let out = outlinks(&self.story, &item);
                    if !out.is_empty() {
                        ui.add_space(GAP_L);
                        caption(ui, "Ссылки отсюда");
                        for it in out {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(item_title(&self.story, &it)).color(TEXT_DIM),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                self.open(it, false);
                            }
                        }
                    }
                }
                Side::Search => self.search_view(ui),
                Side::Outline => {
                    caption(ui, "Структура");
                    let marks: Vec<(String, String)> = match &item {
                        Item::Dialog(id) => self.story.dialogs.get(id).map_or_else(Vec::new, |d| {
                            d.nodes
                                .iter()
                                .map(|n| (n.id().to_string(), node_kind(n).to_string()))
                                .collect()
                        }),
                        Item::Quest(id) => self.story.quests.get(id).map_or_else(Vec::new, |q| {
                            q.stages
                                .iter()
                                .map(|s| (s.id.clone(), s.journal.chars().take(40).collect()))
                                .collect()
                        }),
                        _ => Vec::new(),
                    };
                    if marks.is_empty() {
                        muted(ui, "У этой заметки нет частей");
                    }
                    for (id, kind) in marks {
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::new(RichText::new(&id).color(TEXT)).frame(false))
                                .clicked()
                            {
                                self.scroll_to = Some(id.clone());
                            }
                            ui.label(RichText::new(kind).color(MUTED).size(FONT_SMALL));
                        });
                    }
                }
                Side::Play => {
                    let dialog = match &item {
                        Item::Dialog(id) => Some(id.as_str()),
                        _ => None,
                    };
                    self.player.show(ui, &mut self.story, dialog);
                }
                Side::Problems => {
                    if self.problems.is_empty() {
                        ui.label(RichText::new("✓ Сюжет без ошибок").color(GOOD));
                    }
                    let mut sorted: Vec<&Problem> = self.problems.iter().collect();
                    sorted.sort_by_key(|p| (Some(&p.file) != mine.as_ref(), p.warning));
                    let mut open = None;
                    for p in sorted {
                        let color = if p.warning { ACCENT } else { BAD };
                        let r = ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{}{}",
                                    p.file,
                                    if p.place.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" #{}", p.place)
                                    }
                                ))
                                .color(TEXT_DIM)
                                .size(FONT_SMALL),
                            )
                            .sense(Sense::click()),
                        );
                        ui.label(RichText::new(&p.text).color(color));
                        ui.add_space(GAP_S);
                        if r.clicked() {
                            open = item_of_file(&self.story, &p.file).map(|i| (i, p.place.clone()));
                        }
                    }
                    if let Some((it, place)) = open {
                        self.open(it, false);
                        if !place.is_empty() {
                            self.scroll_to = Some(place);
                        }
                    }
                }
            });
    }

    /// «В скриптах»: строки скриптов проекта, где стоит имя заметки (у «Мира» — имена
    /// переменных). Щелчок открывает скрипт на этой строке.
    fn script_links(&mut self, ui: &mut Ui, item: &Item) {
        let mut uses = Vec::new();
        match story_refs::name_of(item) {
            Some((kind, id)) => {
                for u in story_refs::script_uses(&self.scripts, kind, id) {
                    uses.push((String::new(), u));
                }
            }
            None if *item == Item::Vars => {
                for var in self.story.vars.keys() {
                    for u in story_refs::script_uses(&self.scripts, Kind::Var, var) {
                        uses.push((format!("{var}: "), u));
                    }
                }
            }
            None => {}
        }
        if uses.is_empty() {
            return;
        }
        ui.add_space(GAP_L);
        caption(ui, "В скриптах");
        for (what, u) in uses {
            let file = self
                .loaded_for
                .as_deref()
                .and_then(|p| u.path.strip_prefix(p).ok())
                .unwrap_or(&u.path)
                .display()
                .to_string();
            let r = ui.add(
                egui::Button::new(RichText::new(format!("{what}{file}:{}", u.line)).color(TEXT))
                    .frame(false),
            );
            ui.label(
                RichText::new(&u.text)
                    .monospace()
                    .color(MUTED)
                    .size(FONT_SMALL),
            );
            if r.clicked() {
                self.out.push(StoryOut::OpenScript(u.path.clone(), u.line));
            }
        }
    }

    /// Поиск по всем текстам сюжета (Сю.8).
    fn search_view(&mut self, ui: &mut Ui) {
        caption(ui, "Поиск по тексту");
        ui.add(
            TextEdit::singleline(&mut self.search)
                .id(Id::unique("story-search"))
                .hint_text("слова из реплик, журнала, условий…")
                .desired_width(f32::INFINITY)
                .margin(PAD_FIELD),
        );
        let hits = story_refs::search(&self.story, &self.search);
        if self.search.trim().chars().count() >= 2 {
            muted(ui, &format!("Найдено: {}", hits.len()));
        }
        let mut open = None;
        for h in hits.iter().take(200) {
            let place = if h.place.is_empty() {
                String::new()
            } else {
                format!(" #{}", h.place)
            };
            let r = ui.add(
                egui::Button::new(
                    RichText::new(format!("{}{place}", item_title(&self.story, &h.item)))
                        .color(TEXT_DIM)
                        .size(FONT_SMALL),
                )
                .frame(false),
            );
            ui.label(RichText::new(&h.text).color(TEXT));
            ui.add_space(GAP_S);
            if r.clicked() {
                open = Some(h.clone());
            }
        }
        if let Some(h) = open {
            self.open(h.item, false);
            if !h.place.is_empty() {
                self.scroll_to = Some(h.place);
            }
        }
    }

    // ---- Быстрый переход.

    fn switcher(&mut self, ctx: &egui::Context) {
        let Some(mut q) = self.switcher.take() else {
            return;
        };
        let mut all: Vec<(Item, String)> = vec![(Item::Vars, "Мир".into())];
        for (f, _) in FOLDERS {
            all.extend(self.items(f));
        }
        let ql = q.to_lowercase();
        let found: Vec<(Item, String)> = all
            .into_iter()
            .filter(|(i, t)| {
                ql.is_empty()
                    || t.to_lowercase().contains(&ql)
                    || format!("{i:?}").to_lowercase().contains(&ql)
            })
            .take(12)
            .collect();
        let mut keep = true;
        let mut pick = None;
        let modal = egui::Modal::new(Id::unique("story-switcher")).show(ctx, |ui| {
            ui.set_width(480.0);
            let r = ui.add(
                TextEdit::singleline(&mut q)
                    .hint_text("Найти заметку…")
                    .desired_width(f32::INFINITY)
                    .margin(PAD_FIELD),
            );
            r.request_focus();
            ui.add_space(GAP_S);
            for (i, (item, title)) in found.iter().enumerate() {
                let folder = match item {
                    Item::Vars => "Мир",
                    Item::Character(_) => "Персонажи",
                    Item::Dialog(_) => "Диалоги",
                    Item::Quest(_) => "Задания",
                    Item::Lang(_) => "Переводы",
                };
                let r = ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::Button::new(RichText::new(title).color(if i == 0 {
                            TEXT
                        } else {
                            TEXT_DIM
                        }))
                        .frame(i == 0),
                    );
                    ui.label(RichText::new(folder).color(MUTED).size(FONT_SMALL));
                    r
                });
                if r.inner.clicked() {
                    pick = Some(item.clone());
                }
            }
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                pick = found.first().map(|(i, _)| i.clone());
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                keep = false;
            }
        });
        if modal.should_close() {
            keep = false;
        }
        if let Some(item) = pick {
            self.open(item, false);
            keep = false;
        }
        if keep {
            self.switcher = Some(q);
        }
    }
}

impl From<(Color32, String)> for StoryOut {
    fn from((c, t): (Color32, String)) -> Self {
        StoryOut::Log(c, t)
    }
}

// ---- Связи между заметками.

fn exists(s: &Story, item: &Item) -> bool {
    match item {
        Item::Vars => true,
        Item::Character(id) => s.characters.contains_key(id),
        Item::Dialog(id) => s.dialogs.contains_key(id),
        Item::Quest(id) => s.quests.contains_key(id),
        Item::Lang(c) => s.lang.contains_key(c),
    }
}

/// Файл заметки в `story/`.
fn file_of(s: &Story, item: &Item) -> String {
    let (key, dir, id) = match item {
        Item::Vars => return "vars.ron".into(),
        Item::Lang(code) => return format!("lang/{code}.ron"),
        Item::Character(id) => ("c", "characters", id),
        Item::Dialog(id) => ("d", "dialogs", id),
        Item::Quest(id) => ("q", "quests", id),
    };
    s.files
        .get(&format!("{key}:{id}"))
        .cloned()
        .unwrap_or_else(|| format!("{dir}/{id}.ron"))
}

fn item_of_file(s: &Story, file: &str) -> Option<Item> {
    if file == "vars.ron" {
        return Some(Item::Vars);
    }
    if let Some(code) = file.strip_prefix("lang/") {
        return Some(Item::Lang(code.trim_end_matches(".ron").into()));
    }
    s.files.iter().find(|(_, f)| *f == file).and_then(|(k, _)| {
        let (kind, id) = k.split_once(':')?;
        Some(match kind {
            "c" => Item::Character(id.into()),
            "d" => Item::Dialog(id.into()),
            _ => Item::Quest(id.into()),
        })
    })
}

fn item_title(s: &Story, item: &Item) -> String {
    match item {
        Item::Vars => "Мир".into(),
        Item::Character(id) => s
            .characters
            .get(id)
            .map_or_else(|| id.clone(), |c| c.name.clone()),
        Item::Dialog(id) => s
            .dialogs
            .get(id)
            .map_or_else(|| id.clone(), |d| title_or_id(&d.title, &d.id)),
        Item::Quest(id) => s
            .quests
            .get(id)
            .map_or_else(|| id.clone(), |q| title_or_id(&q.title, &q.id)),
        Item::Lang(code) => lang_name(code),
    }
}

/// Где упоминается заметка: (заметка, чем именно).
pub(crate) fn backlinks(s: &Story, item: &Item) -> Vec<(Item, String)> {
    let mut out = Vec::new();
    for d in s.dialogs.values() {
        let mut why = Vec::new();
        for n in &d.nodes {
            match (item, n) {
                (Item::Character(id), Node::Line { who, .. }) if who == id => {
                    why.push(format!("говорит в #{}", n.id()));
                }
                (Item::Quest(id), Node::Event { name, args, .. })
                    if name.starts_with("quest.")
                        && matches!(args.first(), Some(Value::Text(t)) if t == id) =>
                {
                    why.push(format!("{name} в #{}", n.id()));
                }
                (Item::Character(id), Node::Event { name, args, .. })
                    if reyvec_story::memory::texts_needed(name).is_some()
                        && args
                            .iter()
                            .take(2)
                            .any(|a| matches!(a, Value::Text(t) if t == id)) =>
                {
                    why.push(format!("{name} в #{}", n.id()));
                }
                (Item::Dialog(id), Node::Jump { dialog, .. }) if dialog == id && d.id != *id => {
                    why.push(format!("переход в #{}", n.id()));
                }
                (Item::Vars, _) => {
                    for var in s.vars.keys() {
                        let reads = node_conditions(n).iter().any(|t| t.contains(var.as_str()))
                            || matches!(n, Node::Line { text, .. } if text.contains(&format!("{{{var}}}")));
                        let writes = matches!(n, Node::Set { changes, .. } if changes.iter().any(|c| c.var == *var));
                        if writes {
                            why.push(format!("меняет {var} в #{}", n.id()));
                        } else if reads {
                            why.push(format!("читает {var} в #{}", n.id()));
                        }
                    }
                }
                (Item::Quest(id), _) => {
                    let needle = format!("quest.{id}");
                    if node_conditions(n).iter().any(|t| t.contains(&needle)) {
                        why.push(format!("условие в #{}", n.id()));
                    }
                }
                _ => {}
            }
        }
        if let Item::Character(id) = item
            && why.is_empty()
            && d.with.contains(id)
        {
            why.push("среди участников".into());
        }
        if !why.is_empty() {
            // «говорит в #a», «говорит в #b» → «говорит: #a, #b».
            let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
            for w in why {
                let (what, place) = w
                    .rsplit_once(" в #")
                    .map_or((w.clone(), String::new()), |(a, b)| {
                        (a.to_string(), format!("#{b}"))
                    });
                match grouped.iter_mut().find(|(k, _)| *k == what) {
                    Some((_, v)) => v.push(place),
                    None => grouped.push((what, vec![place])),
                }
            }
            let text = grouped
                .into_iter()
                .map(|(k, v)| {
                    let v: Vec<_> = v.into_iter().filter(|p| !p.is_empty()).collect();
                    if v.is_empty() {
                        k
                    } else {
                        format!("{k}: {}", v.join(", "))
                    }
                })
                .collect::<Vec<_>>()
                .join("; ");
            out.push((Item::Dialog(d.id.clone()), text));
        }
    }
    out
}

/// На что ссылается заметка.
fn outlinks(s: &Story, item: &Item) -> Vec<Item> {
    let mut out = BTreeSet::new();
    if let Item::Dialog(id) = item
        && let Some(d) = s.dialogs.get(id)
    {
        for w in &d.with {
            out.insert(Item::Character(w.clone()));
        }
        for n in &d.nodes {
            match n {
                Node::Line { who, .. } => {
                    out.insert(Item::Character(who.clone()));
                }
                Node::Jump { dialog, .. } => {
                    out.insert(Item::Dialog(dialog.clone()));
                }
                Node::Event { name, args, .. } if name.starts_with("quest.") => {
                    if let Some(Value::Text(q)) = args.first() {
                        out.insert(Item::Quest(q.clone()));
                    }
                }
                _ => {}
            }
        }
    }
    out.into_iter()
        .filter(|i| exists(s, i) && i != item)
        .collect()
}

fn node_conditions(n: &Node) -> Vec<&str> {
    match n {
        Node::Check { when, .. } => vec![when],
        Node::Line { choices, .. } => choices.iter().map(|c| c.when.as_str()).collect(),
        _ => Vec::new(),
    }
}

/// Ключи перевода диалога с основным текстом.
fn dialog_keys(d: &Dialog) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for n in &d.nodes {
        if let Node::Line {
            id,
            text,
            choices,
            key,
            ..
        } = n
        {
            let k = if key.is_empty() {
                format!("{}.{id}", d.id)
            } else {
                key.clone()
            };
            for (i, c) in choices.iter().enumerate() {
                let ck = if c.key.is_empty() {
                    format!("{k}.{}", i + 1)
                } else {
                    c.key.clone()
                };
                out.push((ck, c.text.clone()));
            }
            out.insert(out.len() - choices.len(), (k, text.clone()));
        }
    }
    out
}

fn node_kind(n: &Node) -> &'static str {
    match n {
        Node::Line { .. } => "реплика",
        Node::Check { .. } => "развилка",
        Node::Set { .. } => "изменить мир",
        Node::Event { .. } => "событие",
        Node::Run { .. } => "выполнить",
        Node::Jump { .. } => "переход",
        Node::Random { .. } => "наугад",
        Node::Chance { .. } => "процент",
        Node::Wait { .. } => "пауза",
        Node::End { .. } => "конец",
    }
}

fn title_or_id(title: &str, id: &str) -> String {
    if title.trim().is_empty() {
        id.to_string()
    } else {
        title.to_string()
    }
}

pub(super) fn lang_name(code: &str) -> String {
    match code {
        "en" => "Английский".into(),
        "ru" => "Русский".into(),
        "de" => "Немецкий".into(),
        "fr" => "Французский".into(),
        "es" => "Испанский".into(),
        "zh" => "Китайский".into(),
        "ja" => "Японский".into(),
        other => format!("Язык {}", other.to_uppercase()),
    }
}

fn change_text(c: &Change) -> String {
    match (&c.to, c.add) {
        (Some(v), _) => format!("{} = {}", c.var, value_text(v)),
        (None, Some(a)) if a < 0.0 => format!("{} − {}", c.var, -a),
        (None, Some(a)) => format!("{} + {a}", c.var),
        _ => c.var.clone(),
    }
}

pub(super) fn value_text(v: &Value) -> String {
    match v {
        Value::Text(t) => format!("\"{t}\""),
        other => other.show(),
    }
}

pub(super) fn args_text(args: &[Value]) -> String {
    args.iter().map(value_text).collect::<Vec<_>>().join(", ")
}

pub(super) fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

const KINDS: [&str; 4] = ["да/нет", "целое", "дробное", "текст"];

fn kind_index(v: &Value) -> usize {
    match v {
        Value::Bool(_) => 0,
        Value::Int(_) => 1,
        Value::Float(_) => 2,
        Value::Text(_) => 3,
    }
}

fn convert(v: &Value, kind: usize) -> Value {
    let n = v.number().unwrap_or(0.0);
    match kind {
        0 => Value::Bool(matches!(v, Value::Bool(true)) || n != 0.0),
        1 => Value::Int(n.round() as i64),
        2 => Value::Float(n),
        _ => Value::Text(match v {
            Value::Text(t) => t.clone(),
            _ => String::new(),
        }),
    }
}

// ---- Кусочки вида.

pub(super) fn value_edit(ui: &mut Ui, v: &mut Value, salt: &str) -> bool {
    match v {
        Value::Bool(b) => ui.checkbox(b, if *b { "да" } else { "нет" }).changed(),
        Value::Int(i) => ui.add(egui::DragValue::new(i).speed(0.2)).changed(),
        Value::Float(f) => ui.add(egui::DragValue::new(f).speed(0.05)).changed(),
        Value::Text(t) => ui
            .add(
                TextEdit::singleline(t)
                    .id_salt(("var-text", salt))
                    .desired_width(220.0)
                    .margin(PAD_FIELD),
            )
            .changed(),
    }
}

/// Крупный заголовок прямо в тексте (как «inline title» Obsidian).
fn inline_title(ui: &mut Ui, text: &mut String, hint: &str) -> bool {
    ui.add(
        TextEdit::singleline(text)
            .frame(Frame::NONE)
            .font(FontId::proportional(FONT_INLINE_TITLE))
            .text_color(Color32::WHITE)
            .desired_width(f32::INFINITY)
            .hint_text(hint),
    )
    .changed()
}

fn inline_title_static(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(FONT_INLINE_TITLE)
            .color(Color32::WHITE),
    );
}

fn h2(ui: &mut Ui, text: &str) -> egui::Response {
    ui.label(RichText::new(text).size(FONT_H2).color(TEXT).strong())
}

pub(super) fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(FONT_SMALL).color(MUTED).strong());
}

pub(super) fn muted(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(MUTED).size(FONT_BODY));
}

fn code(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .monospace()
            .size(FONT_MONO)
            .color(TEXT_DIM)
            .background_color(RAISED),
    );
}

/// Ссылка на карточку внутри заметки.
fn link(ui: &mut Ui, label: &str, target: &str, jump: &mut Option<String>) {
    if ui
        .link(RichText::new(label).color(SELECT).size(FONT_BODY))
        .clicked()
    {
        *jump = Some(target.to_string());
    }
}

fn arrow(ui: &mut Ui, what: &str, target: &str, jump: &mut Option<String>) {
    ui.horizontal(|ui| {
        muted(ui, &format!("↓ {what}"));
        link(ui, &format!("#{target}"), target, jump);
    });
}

/// Блок-выноска (как callout в Obsidian).
fn callout(ui: &mut Ui, text: &str) {
    Frame::NONE
        .fill(SELECT_BG.gamma_multiply(0.6))
        .stroke(Stroke::new(LINE, SELECT.gamma_multiply(0.5)))
        .corner_radius(RADIUS_M)
        .inner_margin(PAD_CARD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(TEXT_DIM).size(FONT_BODY));
        });
}

/// Блок «Свойства», как у Obsidian: подпись слева, значение справа.
/// «Отношения»: к кому и насколько (в начале игры). Сю.9.
fn relations_edit(
    ui: &mut Ui,
    rel: &mut BTreeMap<String, i64>,
    others: &[(String, String)],
) -> bool {
    let mut changed = false;
    ui.add_space(GAP_L);
    h2(ui, "Отношения");
    muted(
        ui,
        "Как персонаж относится к другим в начале игры: 0 — никак, больше — лучше, \
         меньше — хуже. Меняются в игре событием «relation.add».",
    );
    let mut remove = None;
    for (whom, n) in rel.iter_mut() {
        ui.horizontal(|ui| {
            let name = others
                .iter()
                .find(|(id, _)| id == whom)
                .map_or(whom.as_str(), |(_, n)| n.as_str());
            ui.label(RichText::new(name).color(TEXT));
            ui.label(RichText::new(whom.as_str()).monospace().color(MUTED));
            changed |= ui.add(egui::DragValue::new(n).range(-100..=100)).changed();
            if glyph_button(ui, Glyph::Close, "Убрать", false).clicked() {
                remove = Some(whom.clone());
            }
        });
    }
    if let Some(w) = remove {
        rel.remove(&w);
        changed = true;
    }
    let free: Vec<&(String, String)> = others
        .iter()
        .filter(|(id, _)| !rel.contains_key(id))
        .collect();
    if !free.is_empty() {
        let mut add = None;
        egui::ComboBox::from_id_salt("story-relation-add")
            .selected_text("+ отношение к…")
            .show_ui(ui, |ui| {
                for (id, name) in free {
                    if ui.selectable_label(false, name.as_str()).clicked() {
                        add = Some(id.clone());
                    }
                }
            });
        if let Some(id) = add {
            rel.insert(id, 0);
            changed = true;
        }
    }
    changed
}

/// Маленькая кнопка «переименовать» рядом с именем в коде.
fn rename_button(ui: &mut Ui) -> bool {
    ui.add(egui::Button::new(RichText::new("✎").color(MUTED)).frame(false))
        .on_hover_text("Переименовать везде: в сюжете и в скриптах проекта")
        .clicked()
}

fn properties(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    caption(ui, "Свойства");
    Frame::NONE
        .inner_margin(Margin::symmetric(0, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = GAP_XS;
            add(ui);
        });
    ui.add_space(GAP_XS);
    ui.painter().hline(
        ui.max_rect().x_range(),
        ui.cursor().min.y,
        Stroke::new(LINE, BORDER),
    );
}

fn prop(ui: &mut Ui, key: &str, value: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(ROW_HEIGHT);
        let (rect, _) = ui.allocate_exact_size(vec2(150.0, ROW_HEIGHT), Sense::hover());
        ui.painter().text(
            Pos2::new(rect.min.x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            key,
            FontId::proportional(FONT_BODY),
            MUTED,
        );
        value(ui);
    });
}

/// Настройки реплик: вид, настроение, голос, камера, пауза. Пусто — «как у …».
fn line_settings(ui: &mut Ui, s: &mut LineSettings, salt: &str) -> bool {
    let mut changed = false;
    prop(ui, "Вид реплики", |ui| {
        let name = |v: Option<View>| match v {
            None => "—",
            Some(View::Window) => "окно внизу",
            Some(View::Bubble) => "над головой",
            Some(View::Voiceover) => "за кадром",
        };
        egui::ComboBox::from_id_salt(("view", salt))
            .selected_text(name(s.view))
            .show_ui(ui, |ui| {
                for v in [
                    None,
                    Some(View::Window),
                    Some(View::Bubble),
                    Some(View::Voiceover),
                ] {
                    changed |= ui.selectable_value(&mut s.view, v, name(v)).changed();
                }
            });
    });
    for (key, field, hint) in [
        ("Настроение", &mut s.mood, "—"),
        ("Голос", &mut s.voice, "—"),
    ] {
        prop(ui, key, |ui| {
            let mut t = field.clone().unwrap_or_default();
            if ui
                .add(
                    TextEdit::singleline(&mut t)
                        .id_salt((key, salt))
                        .frame(Frame::NONE)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                *field = (!t.trim().is_empty()).then_some(t);
                changed = true;
            }
        });
    }
    prop(ui, "Камера к говорящему", |ui| {
        let name = |v: Option<bool>| match v {
            None => "—",
            Some(true) => "да",
            Some(false) => "нет",
        };
        egui::ComboBox::from_id_salt(("camera", salt))
            .selected_text(name(s.camera))
            .show_ui(ui, |ui| {
                for v in [None, Some(true), Some(false)] {
                    changed |= ui.selectable_value(&mut s.camera, v, name(v)).changed();
                }
            });
    });
    prop(ui, "Пауза после, с", |ui| {
        let mut on = s.pause.is_some();
        if ui.checkbox(&mut on, "").changed() {
            s.pause = on.then_some(0.5);
            changed = true;
        }
        if let Some(p) = s.pause.as_mut() {
            changed |= ui
                .add(egui::DragValue::new(p).speed(0.05).range(0.0..=30.0))
                .changed();
        }
    });
    changed
}

fn choice_row(
    ui: &mut Ui,
    c: &mut Choice,
    i: usize,
    node: &str,
    jump: &mut Option<String>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_space(GAP_XL);
        ui.label(
            RichText::new(format!("{}.", i + 1))
                .color(ACCENT)
                .size(FONT_NOTE),
        );
        let w = (ui.available_width() - 140.0).max(120.0);
        changed |= ui
            .add(
                TextEdit::singleline(&mut c.text)
                    .id_salt(("choice", node, i))
                    .frame(Frame::NONE)
                    .font(FontId::proportional(FONT_NOTE))
                    .desired_width(w),
            )
            .changed();
        link(ui, &format!("→ #{}", c.next), &c.next, jump);
    });
    if !c.when.trim().is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(GAP_XL + 18.0);
            muted(ui, "если");
            code(ui, &c.when);
        });
    }
    changed
}

fn goal_row(ui: &mut Ui, g: &mut Goal) -> (bool, bool) {
    let mut changed = false;
    let mut drop = false;
    ui.horizontal(|ui| {
        ui.add_space(GAP_L);
        let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        ui.painter().rect_stroke(
            Rect::from_center_size(r.center(), vec2(12.0, 12.0)),
            3,
            Stroke::new(1.3, TEXT_DIM),
            egui::StrokeKind::Middle,
        );
        changed |= ui
            .add(
                TextEdit::singleline(&mut g.text)
                    .id_salt(("goal", &g.id))
                    .frame(Frame::NONE)
                    .font(FontId::proportional(FONT_NOTE))
                    .hint_text("что сделать")
                    .desired_width(200.0),
            )
            .changed();
        muted(ui, "нужно");
        changed |= ui
            .add(egui::DragValue::new(&mut g.count).range(1..=9999))
            .changed();
        changed |= ui
            .add(
                TextEdit::singleline(&mut g.id)
                    .id_salt(("goal-id", &g.text))
                    .font(FontId::monospace(FONT_SMALL))
                    .desired_width(70.0),
            )
            .on_hover_text("Имя цели в коде: quest.add(\"задание\", \"цель\", 1)")
            .changed();
        if ui
            .add(egui::Button::new(RichText::new("×").color(MUTED)).frame(false))
            .on_hover_text("Убрать цель")
            .clicked()
        {
            drop = true;
        }
    });
    (changed, drop)
}

/// Что делать со стадией после рисования списка.
enum StageAction {
    Remove(usize),
    Swap(usize, usize),
}

fn new_stage(id: &str) -> Stage {
    Stage {
        id: id.into(),
        journal: "Что нужно сделать.".into(),
        goals: Vec::new(),
        next: String::new(),
        done_when: String::new(),
    }
}

/// Кнопка-ссылка «+ …» зелёным.
fn add_button(ui: &mut Ui, text: &str) -> bool {
    ui.add(egui::Button::new(RichText::new(text).color(GOOD)).frame(false))
        .clicked()
}

/// Журнал так, как его увидит игрок: задание по стадиям, цели с «0 из N», награда.
fn journal_preview(ui: &mut Ui, q: &Quest) {
    Frame::NONE
        .fill(CARD)
        .stroke(Stroke::new(LINE, BORDER))
        .corner_radius(RADIUS_M)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(&q.title).size(FONT_H2).color(TEXT).strong());
            if !q.description.is_empty() {
                ui.label(RichText::new(&q.description).color(TEXT_DIM));
            }
            ui.add_space(GAP_S);
            for (i, s) in q.stages.iter().enumerate() {
                let first = i == 0;
                let color = if first { TEXT } else { MUTED };
                ui.label(
                    RichText::new(format!("{}  {}", if first { "▶" } else { "·" }, s.journal))
                        .color(color),
                );
                for g in &s.goals {
                    ui.label(
                        RichText::new(format!("      ☐ {} — 0 из {}", g.text, g.count))
                            .color(color)
                            .size(FONT_SMALL + 1.0),
                    );
                }
            }
            if !q.rewards.is_empty() {
                ui.add_space(GAP_S);
                let list: Vec<String> = q.rewards.iter().map(change_text).collect();
                ui.label(
                    RichText::new(format!("Награда: {}", list.join(", ")))
                        .color(GOOD)
                        .size(FONT_SMALL + 1.0),
                );
            }
            ui.label(
                RichText::new("Сначала видна только первая стадия; следующие открываются по ходу.")
                    .color(MUTED)
                    .size(FONT_SMALL),
            );
        });
}

fn folder_row(ui: &mut Ui, name: &str, count: usize, open: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect, RADIUS_S, HOVER);
    }
    paint_glyph(
        ui.painter(),
        Pos2::new(rect.min.x + 10.0, rect.center().y),
        Glyph::Chevron(open),
        MUTED,
    );
    ui.painter().text(
        Pos2::new(rect.min.x + 22.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        FontId::proportional(FONT_BODY),
        TEXT_DIM,
    );
    ui.painter().text(
        Pos2::new(rect.max.x - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        FontId::proportional(FONT_SMALL),
        MUTED,
    );
    r
}

fn tree_row(ui: &mut Ui, title: &str, indent: f32, on: bool, dirty: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    if on {
        ui.painter().rect_filled(rect, RADIUS_S, SELECT_BG);
    } else if r.hovered() {
        ui.painter().rect_filled(rect, RADIUS_S, HOVER);
    }
    if indent > 0.0 {
        ui.painter().vline(
            rect.min.x + 10.0,
            rect.y_range(),
            Stroke::new(LINE, BORDER_STRONG),
        );
    }
    ui.painter()
        .with_clip_rect(rect.shrink2(vec2(4.0, 0.0)))
        .text(
            Pos2::new(rect.min.x + 8.0 + indent + 6.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            title,
            FontId::proportional(FONT_BODY),
            if on { Color32::WHITE } else { TEXT },
        );
    if dirty {
        ui.painter()
            .circle_filled(Pos2::new(rect.max.x - 10.0, rect.center().y), 3.0, TEXT_DIM);
    }
    r
}

fn glyph_button(ui: &mut Ui, g: Glyph, hint: &str, on: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    if on {
        ui.painter().rect_filled(rect, RADIUS_S, SELECT_BG);
    } else if r.hovered() {
        ui.painter().rect_filled(rect, RADIUS_S, HOVER);
    }
    let color = if on || r.hovered() {
        Color32::WHITE
    } else {
        TEXT_DIM
    };
    paint_glyph(ui.painter(), rect.center(), g, color);
    r.on_hover_text(hint)
}

fn glyph_enabled(ui: &mut Ui, g: Glyph, hint: &str, enabled: bool) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, r) = ui.allocate_exact_size(vec2(26.0, 26.0), sense);
    if enabled && r.hovered() {
        ui.painter().rect_filled(rect, RADIUS_S, HOVER);
    }
    let color = if enabled {
        TEXT_DIM
    } else {
        MUTED.gamma_multiply(0.5)
    };
    paint_glyph(ui.painter(), rect.center(), g, color);
    r.on_hover_text(hint)
}

fn paint_glyph(p: &egui::Painter, c: Pos2, g: Glyph, color: Color32) {
    let s = Stroke::new(1.4, color);
    let v = |x: f32, y: f32| c + vec2(x, y);
    match g {
        Glyph::Person => {
            p.circle_stroke(v(0.0, -3.5), 3.0, s);
            let arc: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let t = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
                    v(0.0, 7.0) + vec2(t.cos() * 6.0, t.sin() * 5.0)
                })
                .collect();
            p.line(arc, s);
        }
        Glyph::Bubble => {
            p.rect_stroke(
                Rect::from_center_size(v(0.0, -1.5), vec2(14.0, 9.0)),
                3,
                s,
                egui::StrokeKind::Middle,
            );
            p.line(vec![v(-3.0, 3.0), v(-5.0, 6.5), v(0.0, 3.0)], s);
        }
        Glyph::Flag => {
            p.line_segment([v(-5.0, -7.0), v(-5.0, 7.0)], s);
            p.line(vec![v(-5.0, -6.0), v(5.0, -3.5), v(-5.0, 0.5)], s);
        }
        Glyph::Globe => {
            p.circle_stroke(c, 6.5, s);
            p.line_segment([v(-6.5, 0.0), v(6.5, 0.0)], s);
            let mer: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 16.0;
                    v(t.cos() * 3.0, t.sin() * 6.5)
                })
                .collect();
            p.line(mer.clone(), s);
            p.line(
                mer.into_iter()
                    .map(|q| Pos2::new(2.0 * c.x - q.x, q.y))
                    .collect(),
                s,
            );
        }
        Glyph::Lang => {
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                "Aa",
                FontId::proportional(12.0),
                color,
            );
        }
        Glyph::Search => {
            p.circle_stroke(v(-1.5, -1.5), 4.5, s);
            p.line_segment([v(2.0, 2.0), v(6.0, 6.0)], s);
        }
        Glyph::Switch => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(14.0, 12.0)),
                2,
                s,
                egui::StrokeKind::Middle,
            );
            p.line(vec![v(-3.0, -2.0), v(0.0, 1.0), v(3.0, -2.0)], s);
        }
        Glyph::Plus => {
            p.line_segment([v(-5.0, 0.0), v(5.0, 0.0)], s);
            p.line_segment([v(0.0, -5.0), v(0.0, 5.0)], s);
        }
        Glyph::Back | Glyph::Forward => {
            let d = if g == Glyph::Back { -1.0 } else { 1.0 };
            p.line(
                vec![v(-2.0 * d, -5.0), v(3.0 * d, 0.0), v(-2.0 * d, 5.0)],
                s,
            );
        }
        Glyph::Close => {
            p.line_segment([v(-4.0, -4.0), v(4.0, 4.0)], s);
            p.line_segment([v(-4.0, 4.0), v(4.0, -4.0)], s);
        }
        Glyph::Chevron(open) => {
            if open {
                p.line(vec![v(-3.5, -1.5), v(0.0, 2.0), v(3.5, -1.5)], s);
            } else {
                p.line(vec![v(-1.5, -3.5), v(2.0, 0.0), v(-1.5, 3.5)], s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn backlinks_of_example() {
        let (s, problems) = Story::from_texts(reyvec_story::EXAMPLE);
        assert!(problems.is_empty());
        let smith = backlinks(&s, &Item::Character("kuznec".into()));
        assert_eq!(smith.len(), 2, "{smith:?}");
        let vars = backlinks(&s, &Item::Vars);
        assert!(
            vars[0].1.contains("меняет kuznec.znakom: #znakomstvo"),
            "{vars:?}"
        );
        assert!(
            vars[0].1.contains("читает kuznec.znakom: #nachalo"),
            "{vars:?}"
        );
        let quest = backlinks(&s, &Item::Quest("mech".into()));
        let ids: Vec<_> = quest.iter().map(|(i, _)| i.clone()).collect();
        assert!(ids.contains(&Item::Dialog("kuznec.privet".into())));
        assert!(ids.contains(&Item::Dialog("kuznec.ruda".into())));
        let ruda = backlinks(&s, &Item::Dialog("kuznec.ruda".into()));
        assert_eq!(ruda.len(), 1);
        assert!(ruda[0].1.contains("переход"), "{ruda:?}");
        assert_eq!(
            smith[0].1,
            "говорит: #spasibo, #chuzhak, #po_imeni, #vopros, #pro_mech, #proshchai"
        );
        let out = outlinks(&s, &Item::Dialog("kuznec.privet".into()));
        assert!(out.contains(&Item::Quest("mech".into())));
        assert!(out.contains(&Item::Dialog("kuznec.ruda".into())));
    }

    #[test]
    fn translation_keys_match_player() {
        let (s, _) = Story::from_texts(reyvec_story::EXAMPLE);
        let keys = dialog_keys(&s.dialogs["kuznec.privet"]);
        let names: Vec<&str> = keys.iter().map(|(k, _)| k.as_str()).collect();
        let at = names
            .iter()
            .position(|k| *k == "kuznec.privet.vopros")
            .unwrap();
        assert_eq!(names[at + 1], "kuznec.privet.vopros.1");
        // Все ключи английского примера есть среди ключей диалогов.
        let all: BTreeSet<String> = s
            .dialogs
            .values()
            .flat_map(|d| dialog_keys(d).into_iter().map(|(k, _)| k))
            .collect();
        for k in s.lang["en"].keys() {
            assert!(all.contains(k), "{k}");
        }
    }

    #[test]
    fn edit_and_autosave_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("reyvec-story-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let src = example_dir().join("story");
        for (rel, text) in reyvec_story::EXAMPLE {
            let p = tmp.join("story").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
        }
        assert!(src.exists());
        let mut ui = StoryUi::default();
        ui.sync(Some(&tmp));
        assert!(ui.problems.is_empty(), "{:?}", ui.problems);
        assert_eq!(ui.current(), Some(Item::Dialog("kuznec.privet".into())));
        ui.story.characters.get_mut("kuznec").unwrap().about = "Новый характер".into();
        ui.story
            .vars
            .insert("derevnya.sgorela".into(), Value::Bool(false));
        ui.edited(&Item::Character("kuznec".into()), 0.0);
        ui.edited(&Item::Vars, 0.0);
        ui.create(Folder::Quests);
        assert_eq!(ui.current(), Some(Item::Quest("novoe_zadanie".into())));
        ui.save_dirty();
        // Перечитали с диска — правки на месте, проблем нет.
        let mut again = StoryUi::default();
        again.sync(Some(&tmp));
        assert!(again.problems.is_empty(), "{:?}", again.problems);
        assert_eq!(again.story.characters["kuznec"].about, "Новый характер");
        assert_eq!(again.story.vars["derevnya.sgorela"], Value::Bool(false));
        assert!(again.story.quests.contains_key("novoe_zadanie"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Сю.8: переименовали диалог — поправились переход в другом диалоге, перевод, файл и
    /// скрипт кузнеца; перечитанный сюжет без ошибок.
    #[test]
    fn rename_fixes_story_files_and_scripts() {
        let tmp = std::env::temp_dir().join(format!("reyvec-story-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for (rel, text) in reyvec_story::EXAMPLE {
            let p = tmp.join("story").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
        }
        let script = tmp.join("scenes/scripts/kuznec.luau");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(
            &script,
            include_str!("../../../../scenes/scripts/kuznec.luau"),
        )
        .unwrap();
        let mut ui = StoryUi::default();
        ui.sync(Some(&tmp));
        ui.open(Item::Dialog("kuznec.ruda".into()), true);
        ui.rename(Kind::Dialog, "kuznec.ruda", "kuznec.ruda_prinesena");
        assert_eq!(
            ui.current(),
            Some(Item::Dialog("kuznec.ruda_prinesena".into()))
        );
        assert!(!tmp.join("story/dialogs/kuznec.ruda.ron").exists());
        assert!(tmp.join("story/dialogs/kuznec.ruda_prinesena.ron").exists());
        let code = std::fs::read_to_string(&script).unwrap();
        assert!(
            code.contains("story.talk(\"kuznec.ruda_prinesena\")"),
            "{code}"
        );
        ui.rename(Kind::Var, "hero.ruda", "hero.kuski_rudy");
        let mut again = StoryUi::default();
        again.sync(Some(&tmp));
        assert!(again.problems.is_empty(), "{:?}", again.problems);
        assert!(again.story.lang["en"].contains_key("kuznec.ruda_prinesena.nachalo"));
        assert!(again.story.vars.contains_key("hero.kuski_rudy"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
