//! Проводник «Ассетов» (Э.1): файлы проекта на всё окно, как в проводнике Windows.
//!
//! Слева дерево папок, сверху «назад, вперёд, вверх», адрес и поиск, под ними команды
//! (создать, вырезать, копировать, вставить, переименовать, удалить, сортировка, вид),
//! посередине плитки или список, справа свойства выбранного: вид файла, размер, когда
//! менялся и где используется. Двойной щелчок открывает ассет в его редакторе: модель
//! формы и сцену 3D — прямо здесь, сцену 2D — в «Мире», остальное — программой Windows.
//!
//! Сами действия с файлами (и отмена, и починка ссылок) — в [`super::project_files`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::{
    Align, Align2, Color32, FontId, Frame, Key, Layout, Margin, Modifiers, Pos2, Rect, RichText,
    Sense, Stroke, Ui, pos2, vec2,
};

use super::assets::{PREFABS, is_space_scene, kind_of};
use super::project_files::{self as pf, Done, NewKind, ROOTS};
use super::shapes::is_shape;
use super::theme::*;

/// Как часто перечитывать открытую папку: файлы могли поменять в другой программе.
const REFRESH_S: f64 = 1.5;
/// Сколько действий помнит отмена.
const UNDO_DEPTH: usize = 50;
/// Больше стольких найденных поиск не показывает.
const SEARCH_LIMIT: usize = 500;
const TILE: egui::Vec2 = vec2(104.0, 104.0);
const THUMB_H: f32 = 70.0;

/// Что проводник просит у редактора.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Out {
    /// Модель формы или сцену 3D — в редактор моделей.
    OpenShape(PathBuf),
    /// Сцену 2D — в «Мир».
    OpenScene(PathBuf),
    /// Открыть программой Windows (блокнот, просмотр картинок…).
    OpenFile(PathBuf),
    /// Показать в проводнике Windows.
    Reveal(PathBuf),
    /// Строка в консоль.
    Log(Color32, String),
    /// Файлы на диске поменялись: перечитать нижнюю панель «Ассеты» в «Мире».
    Changed,
}

/// Что сейчас открыто в редакторе: такие файлы не переносят и не удаляют.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Busy<'a> {
    /// Сцена «Мира».
    pub scene: Option<&'a Path>,
    /// В ней несохранённые правки (тогда её текст нельзя переписывать ссылками).
    pub scene_dirty: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewKind {
    Tiles,
    List,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SortBy {
    Name,
    Kind,
    Date,
    Size,
}

impl SortBy {
    const ALL: [SortBy; 4] = [SortBy::Name, SortBy::Kind, SortBy::Date, SortBy::Size];

    fn label(self) -> &'static str {
        match self {
            SortBy::Name => "Имя",
            SortBy::Kind => "Вид",
            SortBy::Date => "Изменён",
            SortBy::Size => "Размер",
        }
    }
}

/// Файл или папка в открытой папке.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub path: PathBuf,
    pub name: String,
    pub folder: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Вид файла словами и его цвет.
    pub kind: &'static str,
    pub color: Color32,
}

impl Item {
    fn read(path: PathBuf) -> Option<Self> {
        let name = pf::file_name(&path);
        if name.starts_with('.') {
            return None;
        }
        let meta = std::fs::metadata(&path).ok()?;
        let folder = meta.is_dir();
        let prefab = path.parent().is_some_and(|p| pf::file_name(p) == PREFABS);
        let (mut kind, color) = kind_of(&name, folder, prefab && !folder);
        if kind == "сцена" && is_space_scene(&path) {
            kind = "сцена 3D";
        }
        Some(Item {
            size: if folder { 0 } else { meta.len() },
            modified: meta.modified().ok(),
            path,
            name,
            folder,
            kind,
            color,
        })
    }
}

/// Окно «Создать»: в какую папку, имя без окончания, вид, первый ли кадр.
#[derive(Clone, Debug)]
pub(crate) struct Creating {
    pub dir: PathBuf,
    pub name: String,
    pub kind: NewKind,
    /// Имя ещё не трогали: при смене вида оно меняется на имя по виду.
    touched: bool,
    first: bool,
    error: Option<String>,
}

/// Файлы, которые тащат мышью внутри проводника.
#[derive(Clone, Debug)]
struct Dragged(Vec<PathBuf>);

/// Состояние проводника.
pub(crate) struct Explorer {
    project: Option<PathBuf>,
    /// Открытая папка; сама папка проекта — корень «Проект».
    dir: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    items: Vec<Item>,
    /// Когда перечитали папку (время egui); `None` — перечитать сейчас.
    listed_at: Option<f64>,
    /// Подпапки для дерева (кэш до следующего перечитывания).
    subdirs: HashMap<PathBuf, Vec<PathBuf>>,
    /// Раскрытые папки дерева.
    open: HashSet<PathBuf>,
    pub(crate) selected: Vec<PathBuf>,
    /// От какого файла выделяют с Shift.
    anchor: Option<PathBuf>,
    /// Буфер: файлы и «вырезаны ли».
    clipboard: Option<(Vec<PathBuf>, bool)>,
    pub(crate) view: ViewKind,
    sort: SortBy,
    desc: bool,
    pub(crate) search: String,
    /// Переименование: файл, текст, первый ли кадр (тогда выделить имя без окончания).
    renaming: Option<(PathBuf, String, bool)>,
    undo: Vec<Done>,
    /// Отменённое, которое можно повторить (Ctrl+Y); новое действие его стирает.
    redo: Vec<pf::Redo>,
    /// Окно «Создать»: куда, имя и вид.
    pub(crate) creating: Option<Creating>,
    /// Рамка выделения мышью: откуда тянут (точка экрана) и выбранное до неё (с Ctrl).
    band: Option<(Pos2, Vec<PathBuf>)>,
    /// Где используется выбранный: для какого файла посчитано и что нашлось.
    used: Option<(PathBuf, Vec<(PathBuf, usize)>)>,
    /// Последнее сообщение внизу: текст и ошибка ли.
    note: Option<(String, bool)>,
    /// Прокрутить к выбранному в следующем кадре.
    scroll_to: bool,
    /// Файлы бросили на звено адреса: перенести после рисования.
    pending_drop: Option<(Vec<PathBuf>, PathBuf)>,
    out: Vec<Out>,
}

impl Explorer {
    pub(crate) fn new(project: Option<&Path>) -> Self {
        let project = project.map(Path::to_path_buf);
        let dir = project
            .as_ref()
            .map(|p| p.join(ROOTS[1]))
            .filter(|p| p.is_dir())
            .or_else(|| project.clone())
            .unwrap_or_default();
        let open = project
            .iter()
            .flat_map(|p| ROOTS.map(|r| p.join(r)))
            .chain(project.clone())
            .collect();
        Self {
            project,
            dir,
            back: Vec::new(),
            forward: Vec::new(),
            items: Vec::new(),
            listed_at: None,
            subdirs: HashMap::new(),
            open,
            selected: Vec::new(),
            anchor: None,
            clipboard: None,
            view: ViewKind::Tiles,
            sort: SortBy::Name,
            desc: false,
            search: String::new(),
            renaming: None,
            undo: Vec::new(),
            redo: Vec::new(),
            creating: None,
            band: None,
            used: None,
            note: None,
            scroll_to: false,
            pending_drop: None,
            out: Vec::new(),
        }
    }

    pub(crate) fn project(&self) -> Option<&Path> {
        self.project.as_deref()
    }

    /// Перечитать папку в следующем кадре.
    pub(crate) fn refresh(&mut self) {
        self.listed_at = None;
        self.used = None;
    }

    /// Открытая папка.
    #[cfg(test)]
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    #[cfg(test)]
    pub(crate) fn items(&self) -> &[Item] {
        &self.items
    }

    fn is_root(&self, p: &Path) -> bool {
        self.project.as_deref() == Some(p)
    }

    /// Папки `scenes` и `assets` и сам проект: их не переименовывают и не удаляют.
    fn is_fixed(&self, p: &Path) -> bool {
        self.project
            .as_ref()
            .is_some_and(|r| p == r || ROOTS.iter().any(|n| p == r.join(n)))
    }

    /// Открыть папку (запомнив прошлую для «назад»).
    pub(crate) fn go(&mut self, dir: PathBuf) {
        if dir == self.dir {
            return;
        }
        let prev = std::mem::replace(&mut self.dir, dir);
        self.back.push(prev);
        self.forward.clear();
        self.after_go();
    }

    fn after_go(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.renaming = None;
        self.search.clear();
        // Путь до открытой папки раскрыт в дереве.
        let mut p = self.dir.clone();
        while let Some(parent) = p.parent().map(Path::to_path_buf) {
            self.open.insert(parent.clone());
            if self.is_root(&parent) {
                break;
            }
            p = parent;
        }
        self.refresh();
    }

    fn go_back(&mut self) {
        if let Some(d) = self.back.pop() {
            let cur = std::mem::replace(&mut self.dir, d);
            self.forward.push(cur);
            self.after_go();
        }
    }

    fn go_forward(&mut self) {
        if let Some(d) = self.forward.pop() {
            let cur = std::mem::replace(&mut self.dir, d);
            self.back.push(cur);
            self.after_go();
        }
    }

    fn up_dir(&self) -> Option<PathBuf> {
        if self.is_root(&self.dir) {
            return None;
        }
        self.dir.parent().map(Path::to_path_buf)
    }

    /// Перечитать открытую папку (или найти по поиску).
    fn list(&mut self, now: f64) {
        self.listed_at = Some(now);
        self.subdirs.clear();
        // Открытую папку удалили в другой программе — подняться до живой.
        while !self.dir.is_dir() && !self.is_root(&self.dir) {
            match self.dir.parent() {
                Some(p) => self.dir = p.to_path_buf(),
                None => break,
            }
        }
        let mut items: Vec<Item> = if !self.search.trim().is_empty() {
            let q = self.search.trim().to_lowercase();
            let mut found = Vec::new();
            find(&self.dir, &q, &mut found, 0);
            found.into_iter().filter_map(Item::read).collect()
        } else if self.is_root(&self.dir) {
            ROOTS
                .iter()
                .filter_map(|r| Item::read(self.dir.join(r)))
                .collect()
        } else {
            read_dir(&self.dir)
                .into_iter()
                .filter_map(Item::read)
                .collect()
        };
        sort_items(&mut items, self.sort, self.desc);
        self.items = items;
        // Выбранные, которых больше нет, снимаются.
        let alive: HashSet<&Path> = self.items.iter().map(|i| i.path.as_path()).collect();
        self.selected.retain(|p| alive.contains(p.as_path()));
    }

    fn subdirs(&mut self, dir: &Path) -> Vec<PathBuf> {
        if let Some(v) = self.subdirs.get(dir) {
            return v.clone();
        }
        let v: Vec<PathBuf> = if self.is_root(dir) {
            ROOTS
                .iter()
                .map(|r| dir.join(r))
                .filter(|p| p.is_dir())
                .collect()
        } else {
            let mut v: Vec<PathBuf> = read_dir(dir)
                .into_iter()
                .filter(|p| p.is_dir() && !pf::file_name(p).starts_with('.'))
                .collect();
            v.sort_by_key(|p| pf::file_name(p).to_lowercase());
            v
        };
        self.subdirs.insert(dir.to_path_buf(), v.clone());
        v
    }

    fn say(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        self.out
            .push(Out::Log(if error { BAD } else { TEXT_DIM }, text.clone()));
        self.note = Some((text, error));
    }

    /// Запомнить сделанное для отмены и перечитать папки.
    fn did(&mut self, done: Done) {
        self.redo.clear();
        self.remember(done);
    }

    /// Запомнить для отмены, не трогая повтор.
    fn remember(&mut self, done: Done) {
        self.undo.push(done);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.out.push(Out::Changed);
        self.refresh();
    }

    /// Нельзя трогать: открыто в редакторе или папка проекта.
    fn blocked(&self, paths: &[PathBuf], busy: &Busy) -> Option<String> {
        for p in paths {
            if self.is_fixed(p) {
                return Some(format!(
                    "Папку «{}» проекта не переименовать и не удалить",
                    pf::file_name(p)
                ));
            }
            if let Some(scene) = busy.scene
                && (p == scene || pf::inside(scene, p))
            {
                return Some(format!(
                    "Сцена «{}» открыта в «Мире»: сначала откройте другую",
                    pf::file_name(scene)
                ));
            }
        }
        None
    }

    /// Переписывание ссылок не должно задеть сцену с несохранёнными правками.
    fn refs_blocked(&self, moves: &[(PathBuf, PathBuf)], busy: &Busy) -> Option<String> {
        let (Some(project), Some(scene)) = (&self.project, busy.scene) else {
            return None;
        };
        if !busy.scene_dirty {
            return None;
        }
        let touches = pf::plan_refs(project, moves)
            .iter()
            .any(|e| e.file == scene);
        touches.then(|| {
            format!(
                "В сцене «{}» есть ссылки на эти файлы и несохранённые правки: сначала Ctrl+S в «Мире»",
                pf::file_name(scene)
            )
        })
    }

    fn move_into(&mut self, items: Vec<PathBuf>, dir: &Path, busy: &Busy) {
        let Some(project) = self.project.clone() else {
            return;
        };
        if self.is_root(dir) {
            return self.say(
                "Прямо в «Проект» класть нельзя: только в scenes или assets",
                true,
            );
        }
        let moves: Vec<(PathBuf, PathBuf)> = items
            .into_iter()
            .filter(|p| p.parent() != Some(dir))
            .map(|p| {
                let to = dir.join(pf::file_name(&p));
                (p, to)
            })
            .collect();
        if moves.is_empty() {
            return;
        }
        let from: Vec<PathBuf> = moves.iter().map(|(f, _)| f.clone()).collect();
        if let Some(why) = self
            .blocked(&from, busy)
            .or_else(|| self.refs_blocked(&moves, busy))
        {
            return self.say(why, true);
        }
        let n = moves.len();
        let to: Vec<PathBuf> = moves.iter().map(|(_, t)| t.clone()).collect();
        match pf::move_all(&project, moves) {
            Ok(done) => {
                let refs = match &done {
                    Done::Moved { refs, .. } => refs.len(),
                    _ => 0,
                };
                self.did(done);
                self.say(moved_note(n, &pf::file_name(dir), refs), false);
                if dir == self.dir {
                    self.selected = to;
                }
            }
            Err(e) => self.say(e, true),
        }
    }

    fn rename(&mut self, path: PathBuf, name: String, busy: &Busy) {
        let name = name.trim().to_owned();
        if name == pf::file_name(&path) {
            return;
        }
        if let Err(e) = pf::check_name(&name) {
            return self.say(e, true);
        }
        let Some(project) = self.project.clone() else {
            return;
        };
        let to = path.with_file_name(&name);
        let moves = vec![(path.clone(), to.clone())];
        if let Some(why) = self
            .blocked(std::slice::from_ref(&path), busy)
            .or_else(|| self.refs_blocked(&moves, busy))
        {
            return self.say(why, true);
        }
        match pf::move_all(&project, moves) {
            Ok(done) => {
                let refs = match &done {
                    Done::Moved { refs, .. } => refs.len(),
                    _ => 0,
                };
                self.did(done);
                let mut note = format!("Переименовано в «{name}»");
                if refs > 0 {
                    note.push_str(&format!(", ссылки исправлены в {}", files_word(refs)));
                }
                self.say(note, false);
                self.selected = vec![to.clone()];
                self.anchor = Some(to);
            }
            Err(e) => self.say(e, true),
        }
    }

    fn delete(&mut self, busy: &Busy) {
        let items = self.selected.clone();
        self.delete_items(items, busy);
    }

    fn delete_items(&mut self, items: Vec<PathBuf>, busy: &Busy) {
        if items.is_empty() {
            return;
        }
        if let Some(why) = self.blocked(&items, busy) {
            return self.say(why, true);
        }
        let Some(project) = self.project.clone() else {
            return;
        };
        // Удалённое, на которое ссылаются сцены, сломает их: предупредить словами.
        let used: usize = items.iter().map(|p| pf::used_in(&project, p).len()).sum();
        match pf::trash(&project, &items) {
            Ok(done) => {
                self.did(done);
                let mut note = format!("Удалено: {} (Ctrl+Z вернёт)", things_word(items.len()));
                if used > 0 {
                    note.push_str(&format!(
                        ". Внимание: на удалённое ссылаются {}",
                        files_word(used)
                    ));
                }
                self.say(note, used > 0);
                self.selected.clear();
            }
            Err(e) => self.say(e, true),
        }
    }

    fn paste(&mut self, busy: &Busy) {
        self.paste_into(self.dir.clone(), busy);
    }

    fn paste_into(&mut self, dir: PathBuf, busy: &Busy) {
        let Some((items, cut)) = self.clipboard.clone() else {
            return;
        };
        if self.is_root(&dir) {
            return self.say(
                "Прямо в «Проект» класть нельзя: откройте scenes или assets",
                true,
            );
        }
        if cut {
            self.move_into(items, &dir, busy);
            self.clipboard = None;
            return;
        }
        match pf::copy_into(&dir, &items) {
            Ok(done) => {
                if let Done::Created(made) = &done {
                    self.selected = made.clone();
                }
                self.did(done);
                self.say(format!("Скопировано: {}", things_word(items.len())), false);
            }
            Err(e) => self.say(e, true),
        }
    }

    /// Открыть окно «Создать» для папки `dir`.
    pub(crate) fn ask_create(&mut self, dir: PathBuf, kind: NewKind) {
        if self.is_root(&dir) {
            return self.say(
                "Прямо в «Проект» класть нельзя: откройте scenes или assets",
                true,
            );
        }
        self.renaming = None;
        self.creating = Some(Creating {
            dir,
            name: kind.stem().to_owned(),
            kind,
            touched: false,
            first: true,
            error: None,
        });
    }

    /// Создать из окна: имя без окончания и вид. Ошибка — что сказать в окне.
    fn create_named(&mut self, dir: &Path, stem: &str, kind: NewKind) -> Result<(), String> {
        let name = format!("{}{}", stem.trim(), kind.ext());
        pf::check_name(&name)?;
        if dir.join(&name).exists() {
            return Err(format!("В «{}» уже есть «{name}»", pf::file_name(dir)));
        }
        let (done, path) = pf::create(dir, &name, kind.text())?;
        self.did(done);
        self.search.clear();
        if dir != self.dir {
            self.go(dir.to_path_buf());
        }
        self.selected = vec![path.clone()];
        self.anchor = Some(path);
        self.scroll_to = true;
        self.say(format!("Создано: «{name}»"), false);
        Ok(())
    }

    /// Окно «Создать»: вид значками и имя; Enter — создать, Esc — закрыть.
    fn create_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut c) = self.creating.take() else {
            return;
        };
        let mut make = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("explorer-create")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(
                RichText::new(format!("Создать в «{}»", pf::file_name(&c.dir)))
                    .size(FONT_HEADING)
                    .color(Color32::WHITE),
            );
            ui.add_space(GAP_L);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(GAP_M, GAP_M);
                for kind in NewKind::ALL {
                    let on = c.kind == kind;
                    let (_, color) = kind_of(kind.file_name(), kind == NewKind::Folder, false);
                    let (rect, r) = ui.allocate_exact_size(vec2(84.0, 64.0), Sense::click());
                    let p = ui.painter();
                    p.rect(
                        rect,
                        RADIUS_M,
                        if on {
                            SELECT_BG
                        } else if r.hovered() {
                            HOVER
                        } else {
                            CARD
                        },
                        Stroke::new(LINE, if on { SELECT } else { BORDER_STRONG }),
                        egui::StrokeKind::Inside,
                    );
                    let top = rect.center() - vec2(0.0, 10.0);
                    if kind == NewKind::Folder {
                        paint_folder(p, top, 1.1, color);
                    } else {
                        p.rect_filled(Rect::from_center_size(top, vec2(16.0, 16.0)), 3, color);
                    }
                    p.text(
                        rect.center() + vec2(0.0, 16.0),
                        Align2::CENTER_CENTER,
                        kind.label(),
                        FontId::proportional(FONT_SMALL),
                        if on { Color32::WHITE } else { TEXT_DIM },
                    );
                    if r.clicked() {
                        c.kind = kind;
                        if !c.touched {
                            c.name = kind.stem().to_owned();
                        }
                        c.first = true;
                    }
                }
            });
            ui.add_space(GAP_L);
            ui.horizontal(|ui| {
                let ext = c.kind.ext();
                let id = egui::Id::unique("explorer-create-name");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut c.name)
                        .id(id)
                        .desired_width(460.0 - 90.0)
                        .hint_text("Имя"),
                );
                ui.label(RichText::new(ext).monospace().color(MUTED));
                if edit.changed() {
                    c.touched = true;
                    c.error = None;
                }
                if c.first {
                    // Имя выделено целиком: печать сразу заменяет его.
                    c.first = false;
                    edit.request_focus();
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                        let n = c.name.chars().count();
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(n),
                            )));
                        state.store(ui.ctx(), id);
                    }
                }
                if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    make = true;
                }
            });
            if let Some(e) = &c.error {
                ui.label(RichText::new(e).small().color(BAD));
            }
            ui.add_space(GAP_L);
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new("Создать").color(ON_ACCENT)).fill(ACCENT);
                if ui.add(b).clicked() {
                    make = true;
                }
                if ui.button("Отмена").clicked() {
                    close = true;
                }
                ui.label(
                    RichText::new("Enter — создать, Esc — закрыть")
                        .small()
                        .color(MUTED),
                );
            });
        });
        if modal.should_close() {
            close = true;
        }
        // Поле имени отдаёт клавиатуру, как только окно закрылось: иначе первый Ctrl+Z
        // после окна достался бы пустому полю.
        let unfocus = |ctx: &egui::Context| {
            ctx.memory_mut(|m| m.surrender_focus(egui::Id::unique("explorer-create-name")));
        };
        if make {
            let dir = c.dir.clone();
            match self.create_named(&dir, &c.name, c.kind) {
                Ok(()) => return unfocus(ctx),
                Err(e) => {
                    c.error = Some(e);
                    c.first = true;
                }
            }
        }
        if close {
            unfocus(ctx);
        } else {
            self.creating = Some(c);
        }
    }

    /// Повторить отменённое (Ctrl+Y).
    fn redo(&mut self) {
        let (Some(again), Some(project)) = (self.redo.pop(), self.project.clone()) else {
            return self.say("Повторять нечего", false);
        };
        match pf::redo(&project, again) {
            Ok(done) => {
                let select = match &done {
                    Done::Moved { moves, .. } => moves.iter().map(|(_, t)| t.clone()).collect(),
                    Done::Created(paths) => paths.clone(),
                    Done::Trashed(_) => Vec::new(),
                };
                self.remember(done);
                self.say("Повторено", false);
                self.show(select);
                self.out.push(Out::Changed);
                self.refresh();
            }
            Err(e) => self.say(e, true),
        }
    }

    /// Открыть папку первого из путей и выбрать их.
    fn show(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if let Some(dir) = paths[0].parent()
            && dir != self.dir
        {
            self.go(dir.to_path_buf());
        }
        self.selected = paths;
        self.scroll_to = true;
    }

    fn undo(&mut self) {
        let (Some(done), Some(project)) = (self.undo.pop(), self.project.clone()) else {
            return self.say("Отменять нечего", false);
        };
        match pf::undo(&project, done) {
            Ok((back, again)) => {
                self.say("Отменено", false);
                self.redo.push(again);
                self.show(back);
                self.out.push(Out::Changed);
                self.refresh();
            }
            Err(e) => self.say(e, true),
        }
    }

    fn open_item(&mut self, path: &Path) {
        if path.is_dir() {
            return self.go(path.to_path_buf());
        }
        let name = pf::file_name(path).to_lowercase();
        let prefab = path.parent().is_some_and(|p| pf::file_name(p) == PREFABS);
        let out = if is_shape(&name) || (name.ends_with(".ron") && is_space_scene(path)) {
            Out::OpenShape(path.to_path_buf())
        } else if name.ends_with(".ron") && !prefab && is_scene_2d(path) {
            Out::OpenScene(path.to_path_buf())
        } else {
            Out::OpenFile(path.to_path_buf())
        };
        self.out.push(out);
    }

    fn select_click(&mut self, path: &Path, mods: Modifiers) {
        if mods.command {
            if let Some(i) = self.selected.iter().position(|p| p == path) {
                self.selected.remove(i);
            } else {
                self.selected.push(path.to_path_buf());
            }
            self.anchor = Some(path.to_path_buf());
        } else if mods.shift
            && let Some(a) = self.anchor.clone()
        {
            let pos = |p: &Path| self.items.iter().position(|i| i.path == p);
            if let (Some(a), Some(b)) = (pos(&a), pos(path)) {
                let (lo, hi) = (a.min(b), a.max(b));
                self.selected = self.items[lo..=hi].iter().map(|i| i.path.clone()).collect();
            }
        } else {
            self.selected = vec![path.to_path_buf()];
            self.anchor = Some(path.to_path_buf());
        }
    }

    /// Стрелки: следующий или прошлый файл (в плитках вверх-вниз — через ряд).
    fn step(&mut self, by: isize, shift: bool) {
        if self.items.is_empty() {
            return;
        }
        let cur = self
            .selected
            .last()
            .and_then(|p| self.items.iter().position(|i| &i.path == p));
        let next = match cur {
            Some(c) => (c as isize + by).clamp(0, self.items.len() as isize - 1) as usize,
            None => 0,
        };
        let path = self.items[next].path.clone();
        let mods = Modifiers {
            shift,
            ..Default::default()
        };
        self.select_click(&path, mods);
        if !shift {
            self.anchor = Some(path);
        }
        self.scroll_to = true;
    }

    /// Рисует проводник на всё место `ui`; возвращает просьбы к редактору.
    pub(crate) fn draw(&mut self, ui: &mut Ui, busy: &Busy) -> Vec<Out> {
        let now = ui.input(|i| i.time);
        if self.project.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Папка проекта не найдена").color(MUTED));
            });
            return std::mem::take(&mut self.out);
        }
        if self.listed_at.is_none_or(|t| now - t > REFRESH_S) {
            self.list(now);
        }
        let per_row = self.keys(ui, busy);
        let frame = Frame::NONE.fill(PANEL);
        egui::Panel::top("explorer-nav")
            .exact_size(44.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(frame.inner_margin(Margin::symmetric(GAP_XL as i8, 0)))
            .show(ui, |ui| self.nav_bar(ui));
        egui::Panel::top("explorer-commands")
            .exact_size(40.0)
            .resizable(false)
            .frame(
                Frame::NONE
                    .fill(BAR)
                    .inner_margin(Margin::symmetric(GAP_XL as i8, 0)),
            )
            .show(ui, |ui| self.command_bar(ui, busy));
        egui::Panel::bottom("explorer-status")
            .exact_size(28.0)
            .resizable(false)
            .frame(
                Frame::NONE
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(GAP_XL as i8, 0)),
            )
            .show(ui, |ui| self.status(ui));
        egui::Panel::left("explorer-tree")
            .resizable(true)
            .default_size(250.0)
            .size_range(160.0..=480.0)
            .frame(frame.inner_margin(PAD_PANEL))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("explorer-tree")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if let Some(root) = self.project.clone() {
                            self.tree_row(ui, &root, 0, busy);
                        }
                    });
            });
        egui::Panel::right("explorer-details")
            .resizable(true)
            .default_size(RIGHT_WIDTH)
            .size_range(SIDE_RANGE)
            .frame(frame.inner_margin(PAD_CARD))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("explorer-details")
                    .auto_shrink(false)
                    .show(ui, |ui| self.details(ui));
            });
        egui::CentralPanel::default()
            .frame(
                Frame::NONE
                    .fill(BG)
                    .inner_margin(Margin::same(GAP_XL as i8)),
            )
            .show(ui, |ui| self.files(ui, busy, per_row));
        let ctx = ui.ctx().clone();
        self.create_dialog(&ctx);
        std::mem::take(&mut self.out)
    }

    /// Клавиши проводника. Возвращает, сколько плиток в ряду (для стрелок вверх-вниз).
    fn keys(&mut self, ui: &Ui, busy: &Busy) -> usize {
        let per_row = ui
            .data(|d| d.get_temp::<usize>(egui::Id::unique("explorer-per-row")))
            .unwrap_or(1)
            .max(1);
        let ctx = ui.ctx().clone();
        if ctx.egui_wants_keyboard_input() || self.creating.is_some() {
            // Пока печатают имя или поиск, клавиши принадлежат полю.
            return per_row;
        }
        let (copy, cut, paste) = ctx.input_mut(|i| {
            let ev = |want: fn(&egui::Event) -> bool| i.events.iter().any(want);
            let copy = ev(|e| matches!(e, egui::Event::Copy));
            let cut = ev(|e| matches!(e, egui::Event::Cut));
            let paste = ev(|e| matches!(e, egui::Event::Paste(_) | egui::Event::PasteImage(_)));
            let mut cmd = |k| i.consume_key(Modifiers::COMMAND, k);
            (
                copy || cmd(Key::C),
                cut || cmd(Key::X),
                paste || cmd(Key::V),
            )
        });
        let k = |m: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(m, key));
        if k(Modifiers::COMMAND | Modifiers::SHIFT, Key::N) {
            self.ask_create(self.dir.clone(), NewKind::Folder);
        }
        if k(Modifiers::COMMAND, Key::N) {
            let kind = self.creating.as_ref().map_or(NewKind::Folder, |c| c.kind);
            self.ask_create(self.dir.clone(), kind);
        }
        if k(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || k(Modifiers::COMMAND, Key::Y) {
            self.redo();
        }
        if k(Modifiers::COMMAND, Key::Z) {
            self.undo();
        }
        if k(Modifiers::COMMAND, Key::A) {
            self.selected = self.items.iter().map(|i| i.path.clone()).collect();
        }
        if copy && !self.selected.is_empty() {
            self.clipboard = Some((self.selected.clone(), false));
            self.say(
                format!("Скопировано в буфер: {}", things_word(self.selected.len())),
                false,
            );
        }
        if cut && !self.selected.is_empty() {
            self.clipboard = Some((self.selected.clone(), true));
            self.say(
                format!(
                    "Вырезано: {}. Ctrl+V — положить",
                    things_word(self.selected.len())
                ),
                false,
            );
        }
        if paste {
            self.paste(busy);
        }
        if k(Modifiers::NONE, Key::Delete) {
            self.delete(busy);
        }
        if k(Modifiers::NONE, Key::F2)
            && let Some(p) = self.selected.last().cloned()
            && !self.is_fixed(&p)
        {
            self.renaming = Some((p.clone(), pf::file_name(&p), true));
        }
        if k(Modifiers::NONE, Key::Enter)
            && let Some(p) = self.selected.last().cloned()
        {
            self.open_item(&p);
        }
        if k(Modifiers::NONE, Key::Backspace) || k(Modifiers::ALT, Key::ArrowLeft) {
            self.go_back();
        }
        if k(Modifiers::ALT, Key::ArrowRight) {
            self.go_forward();
        }
        if k(Modifiers::ALT, Key::ArrowUp)
            && let Some(up) = self.up_dir()
        {
            self.go(up);
        }
        if k(Modifiers::COMMAND, Key::F) {
            ctx.memory_mut(|m| m.request_focus(egui::Id::unique("explorer-search")));
        }
        let row = if self.view == ViewKind::Tiles {
            per_row as isize
        } else {
            1
        };
        for shift in [false, true] {
            let m = if shift {
                Modifiers::SHIFT
            } else {
                Modifiers::NONE
            };
            for (key, by) in [
                (Key::ArrowLeft, -1),
                (Key::ArrowRight, 1),
                (Key::ArrowUp, -row),
                (Key::ArrowDown, row),
            ] {
                if k(m, key) {
                    self.step(by, shift);
                }
            }
        }
        per_row
    }

    /// «Назад, вперёд, вверх», адрес и поиск.
    fn nav_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_XS;
            if icon(
                ui,
                Glyph::Back,
                "Назад (Alt+←, Backspace)",
                !self.back.is_empty(),
            )
            .clicked()
            {
                self.go_back();
            }
            if icon(
                ui,
                Glyph::Forward,
                "Вперёд (Alt+→)",
                !self.forward.is_empty(),
            )
            .clicked()
            {
                self.go_forward();
            }
            let up = self.up_dir();
            if icon(ui, Glyph::Up, "На папку выше (Alt+↑)", up.is_some()).clicked()
                && let Some(up) = up
            {
                self.go(up);
            }
            if icon(ui, Glyph::Refresh, "Перечитать папку с диска", true).clicked()
            {
                self.refresh();
            }
            ui.add_space(GAP_L);
            let search_w = 260.0_f32.min(ui.available_width() * 0.35);
            let addr_w = (ui.available_width() - search_w - GAP_L).max(120.0);
            Frame::NONE
                .fill(BG)
                .stroke(Stroke::new(LINE, BORDER))
                .corner_radius(RADIUS_S)
                .inner_margin(Margin::symmetric(GAP_M as i8, 3))
                .show(ui, |ui| {
                    ui.set_width(addr_w - 2.0 * GAP_M);
                    self.crumbs(ui);
                });
            ui.add_space(GAP_L);
            Frame::NONE
                .fill(BG)
                .stroke(Stroke::new(LINE, BORDER))
                .corner_radius(RADIUS_S)
                .inner_margin(Margin::symmetric(GAP_M as i8, 3))
                .show(ui, |ui| {
                    ui.set_width(search_w - 2.0 * GAP_M);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 20.0), Sense::hover());
                        paint_glyph(ui.painter(), r.center(), Glyph::Search, MUTED);
                        let hint = format!("Поиск в «{}»", self.dir_title());
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .id(egui::Id::unique("explorer-search"))
                                .hint_text(hint)
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY),
                        );
                        if edit.changed() {
                            self.listed_at = None;
                        }
                    });
                });
        });
    }

    fn dir_title(&self) -> String {
        if self.is_root(&self.dir) {
            "Проект".into()
        } else {
            pf::file_name(&self.dir)
        }
    }

    /// Адрес: Проект › assets › svg; щелчок по звену открывает его.
    fn crumbs(&mut self, ui: &mut Ui) {
        let Some(root) = self.project.clone() else {
            return;
        };
        let mut chain = vec![self.dir.clone()];
        let mut p = self.dir.clone();
        while p != root {
            match p.parent() {
                Some(parent) => {
                    chain.push(parent.to_path_buf());
                    p = parent.to_path_buf();
                }
                None => break,
            }
        }
        chain.reverse();
        let mut go = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_XS;
            let last = chain.len() - 1;
            for (n, dir) in chain.iter().enumerate() {
                if n > 0 {
                    ui.label(RichText::new("›").color(MUTED));
                }
                let name = if *dir == root {
                    "Проект".to_owned()
                } else {
                    pf::file_name(dir)
                };
                let text = RichText::new(name).color(if n == last { TEXT } else { TEXT_DIM });
                let b = egui::Button::new(text)
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .corner_radius(RADIUS_S);
                let r = ui.add(b);
                if r.clicked() && n != last {
                    go = Some(dir.clone());
                }
                drop_target(&r, ui);
                if let Some(d) = r.dnd_release_payload::<Dragged>() {
                    self.pending_drop = Some((d.0.clone(), dir.clone()));
                }
            }
        });
        if let Some(d) = go {
            self.go(d);
        }
    }

    /// Команды: создать, вырезать, копировать, вставить, переименовать, удалить, отмена,
    /// сортировка, вид.
    fn command_bar(&mut self, ui: &mut Ui, busy: &Busy) {
        let any = !self.selected.is_empty();
        let movable = any && self.selected.iter().all(|p| !self.is_fixed(p));
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_XS;
            let can_make = !self.is_root(&self.dir);
            ui.add_enabled_ui(can_make, |ui| {
                let label = RichText::new("+  Создать").color(ON_ACCENT);
                let b = egui::Button::new(label).fill(ACCENT);
                let r = ui
                    .add(b)
                    .on_hover_text("Новая папка, сцена, модель или скрипт (Ctrl+N)");
                if r.clicked() {
                    self.ask_create(self.dir.clone(), NewKind::Folder);
                }
                r.on_disabled_hover_text(
                    "Откройте scenes или assets: прямо в «Проект» класть нельзя",
                );
            });
            ui.add_space(GAP_L);
            sep(ui);
            if icon(ui, Glyph::Cut, "Вырезать (Ctrl+X)", movable).clicked() {
                self.clipboard = Some((self.selected.clone(), true));
            }
            if icon(ui, Glyph::Copy, "Копировать (Ctrl+C)", any).clicked() {
                self.clipboard = Some((self.selected.clone(), false));
            }
            let can_paste = self.clipboard.is_some() && can_make;
            if icon(ui, Glyph::Paste, "Вставить (Ctrl+V)", can_paste).clicked() {
                self.paste(busy);
            }
            if icon(
                ui,
                Glyph::Rename,
                "Переименовать (F2)",
                movable && self.selected.len() == 1,
            )
            .clicked()
                && let Some(p) = self.selected.last().cloned()
            {
                self.renaming = Some((p.clone(), pf::file_name(&p), true));
            }
            if icon(ui, Glyph::Trash, "Удалить (Delete). Ctrl+Z вернёт", movable).clicked()
            {
                self.delete(busy);
            }
            sep(ui);
            if icon(
                ui,
                Glyph::Undo,
                "Отменить последнее действие с файлами (Ctrl+Z)",
                !self.undo.is_empty(),
            )
            .clicked()
            {
                self.undo();
            }
            if icon(
                ui,
                Glyph::Redo,
                "Повторить отменённое (Ctrl+Y)",
                !self.redo.is_empty(),
            )
            .clicked()
            {
                self.redo();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = GAP_XS;
                let reveal = self.selected.last().cloned().unwrap_or(self.dir.clone());
                if icon(ui, Glyph::Folder, "Показать в проводнике Windows", true).clicked()
                {
                    self.out.push(Out::Reveal(reveal));
                }
                sep(ui);
                if toggle(
                    ui,
                    Glyph::List,
                    "Список: имя, вид, дата, размер",
                    self.view == ViewKind::List,
                )
                .clicked()
                {
                    self.view = ViewKind::List;
                }
                if toggle(ui, Glyph::Tiles, "Плитки", self.view == ViewKind::Tiles).clicked()
                {
                    self.view = ViewKind::Tiles;
                }
                sep(ui);
                let arrow = if self.desc { "↓" } else { "↑" };
                ui.menu_button(
                    format!("Сортировка: {} {arrow}", self.sort.label()),
                    |ui| {
                        for s in SortBy::ALL {
                            if ui.radio(self.sort == s, s.label()).clicked() {
                                self.set_sort(s, self.desc);
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.radio(!self.desc, "По возрастанию").clicked() {
                            self.set_sort(self.sort, false);
                            ui.close();
                        }
                        if ui.radio(self.desc, "По убыванию").clicked() {
                            self.set_sort(self.sort, true);
                            ui.close();
                        }
                    },
                );
            });
        });
    }

    fn set_sort(&mut self, by: SortBy, desc: bool) {
        self.sort = by;
        self.desc = desc;
        sort_items(&mut self.items, by, desc);
    }

    /// Строка дерева папок и раскрытые под ней.
    fn tree_row(&mut self, ui: &mut Ui, dir: &Path, depth: usize, busy: &Busy) {
        let subs = self.subdirs(dir);
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
        let current = *dir == self.dir;
        let hovering = response.dnd_hover_payload::<Dragged>().is_some();
        if current {
            ui.painter().rect_filled(rect, RADIUS_S, SELECT_BG);
        } else if response.hovered() || hovering {
            ui.painter().rect_filled(rect, RADIUS_S, HOVER);
        }
        if hovering {
            ui.painter().rect_stroke(
                rect,
                RADIUS_S,
                Stroke::new(LINE_FOCUS, SELECT),
                egui::StrokeKind::Inside,
            );
        }
        let is_open = self.open.contains(dir);
        let x = rect.left() + GAP_S + depth as f32 * INDENT;
        let arrow = Rect::from_min_size(pos2(x, rect.top()), vec2(INDENT, rect.height()));
        if !subs.is_empty() {
            chevron(ui.painter(), arrow.center(), is_open);
        }
        let folder_at = pos2(arrow.right() + 9.0, rect.center().y);
        paint_glyph(
            ui.painter(),
            folder_at,
            Glyph::Folder,
            if current { ACCENT } else { MUTED },
        );
        let name = if self.is_root(dir) {
            "Проект".to_owned()
        } else {
            pf::file_name(dir)
        };
        ui.painter().text(
            pos2(folder_at.x + 12.0, rect.center().y),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(FONT_BODY),
            if current { Color32::WHITE } else { TEXT },
        );
        if response.clicked() {
            let on_arrow = response
                .interact_pointer_pos()
                .is_some_and(|p| arrow.contains(p));
            if on_arrow {
                if !self.open.remove(dir) {
                    self.open.insert(dir.to_path_buf());
                }
            } else {
                self.go(dir.to_path_buf());
            }
        }
        if response.double_clicked() && !self.open.remove(dir) {
            self.open.insert(dir.to_path_buf());
        }
        if let Some(d) = response.dnd_release_payload::<Dragged>() {
            self.move_into(d.0.clone(), dir, busy);
        }
        response.context_menu(|ui| self.tree_menu(ui, dir, busy));
        if self.open.contains(dir) {
            for s in subs {
                self.tree_row(ui, &s, depth + 1, busy);
            }
        }
    }

    /// Меню папки в дереве: открыть, создать, вставить, переименовать, удалить.
    fn tree_menu(&mut self, ui: &mut Ui, dir: &Path, busy: &Busy) {
        let root = self.is_root(dir);
        let fixed = self.is_fixed(dir);
        if ui.button("Открыть").clicked() {
            self.go(dir.to_path_buf());
            ui.close();
        }
        if ui
            .add_enabled(!root, egui::Button::new("Создать здесь…"))
            .clicked()
        {
            self.ask_create(dir.to_path_buf(), NewKind::Folder);
            ui.close();
        }
        if ui
            .add_enabled(
                !root && self.clipboard.is_some(),
                egui::Button::new("Вставить сюда"),
            )
            .clicked()
        {
            self.paste_into(dir.to_path_buf(), busy);
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(
                !fixed,
                egui::Button::new("Переименовать").shortcut_text("F2"),
            )
            .clicked()
            && let Some(parent) = dir.parent()
        {
            // Имя правится в списке папки выше, как в проводнике Windows.
            self.go(parent.to_path_buf());
            self.selected = vec![dir.to_path_buf()];
            self.renaming = Some((dir.to_path_buf(), pf::file_name(dir), true));
            self.scroll_to = true;
            ui.close();
        }
        if ui
            .add_enabled(!fixed, egui::Button::new("Удалить").shortcut_text("Delete"))
            .clicked()
        {
            self.delete_items(vec![dir.to_path_buf()], busy);
            ui.close();
        }
        ui.separator();
        if ui.button("Показать в проводнике Windows").clicked() {
            self.out.push(Out::Reveal(dir.to_path_buf()));
            ui.close();
        }
    }

    /// Плитки или список файлов открытой папки.
    fn files(&mut self, ui: &mut Ui, busy: &Busy, per_row: usize) {
        let full = ui.max_rect();
        // Пустое место: щелчок снимает выбор, правая кнопка — меню папки, тянуть мышью —
        // рамка выделения (с Ctrl — добавить к выбранному).
        let bg = ui.interact(
            full,
            egui::Id::unique("explorer-bg"),
            Sense::click_and_drag(),
        );
        if bg.clicked() {
            self.selected.clear();
            self.renaming = None;
        }
        if bg.drag_started_by(egui::PointerButton::Primary)
            && let Some(at) = bg.interact_pointer_pos()
        {
            let keep = if ui.input(|i| i.modifiers.command) {
                self.selected.clone()
            } else {
                Vec::new()
            };
            self.band = Some((at, keep));
            self.renaming = None;
        }
        if !bg.dragged() {
            self.band = None;
        }
        let band = self.band.as_ref().and_then(|(from, keep)| {
            let to = ui.input(|i| i.pointer.latest_pos())?;
            Some((Rect::from_two_pos(*from, to), keep.clone()))
        });
        let mut rects: Vec<(PathBuf, Rect)> = Vec::new();
        bg.context_menu(|ui| self.folder_menu(ui, busy));
        if self.items.is_empty() {
            let text = if self.search.trim().is_empty() {
                "Папка пуста. «Создать» — новая папка или ассет"
            } else {
                "Ничего не нашлось"
            };
            ui.label(RichText::new(text).color(MUTED));
            return;
        }
        if !self.search.trim().is_empty() {
            ui.label(
                RichText::new(format!(
                    "Найдено в «{}» и вложенных папках: {}",
                    self.dir_title(),
                    self.items.len()
                ))
                .color(MUTED),
            );
            ui.add_space(GAP_M);
        }
        let mut clicked: Option<(PathBuf, Modifiers)> = None;
        let mut opened: Option<PathBuf> = None;
        let mut drop: Option<(Vec<PathBuf>, PathBuf)> = None;
        let mut rename_done: Option<(PathBuf, String)> = None;
        let scroll_to = std::mem::take(&mut self.scroll_to);
        let items = self.items.clone();
        let list = self.view == ViewKind::List;
        if list {
            self.list_header(ui);
        }
        egui::ScrollArea::vertical()
            .id_salt(("explorer-files", list))
            .auto_shrink(false)
            .show(ui, |ui| {
                let mut body = |ui: &mut Ui| {
                    for item in &items {
                        let selected = self.selected.contains(&item.path);
                        let r = if list {
                            row(ui, item, selected, self.renaming_text(&item.path))
                        } else {
                            tile(ui, item, selected, self.renaming_text(&item.path))
                        };
                        let (r, edit) = r;
                        if let Some(edit) = edit {
                            self.rename_field(ui, &edit, &item.path, &mut rename_done);
                        }
                        if scroll_to && selected {
                            r.scroll_to_me(None);
                        }
                        rects.push((item.path.clone(), r.rect));
                        // Тащить: выбранные (или только этот, если он не выбран).
                        if r.dragged() {
                            let pack = if selected {
                                self.selected.clone()
                            } else {
                                vec![item.path.clone()]
                            };
                            r.dnd_set_drag_payload(Dragged(pack));
                        }
                        if item.folder {
                            drop_target(&r, ui);
                            if let Some(d) = r.dnd_release_payload::<Dragged>()
                                && !d.0.contains(&item.path)
                            {
                                drop = Some((d.0.clone(), item.path.clone()));
                            }
                        }
                        if r.double_clicked() {
                            opened = Some(item.path.clone());
                        } else if r.clicked() {
                            clicked = Some((item.path.clone(), ui.input(|i| i.modifiers)));
                        }
                        if r.secondary_clicked() && !selected {
                            clicked = Some((item.path.clone(), Modifiers::NONE));
                        }
                        let tip = format!("{}\n{}", item.name, capital(item.kind));
                        let r = r.on_hover_text(tip);
                        r.context_menu(|ui| {
                            self.item_menu(ui, item, busy);
                        });
                    }
                };
                if list {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    body(ui);
                } else {
                    let w = ui.available_width();
                    let n = ((w + GAP_XL) / (TILE.x + GAP_XL)).floor().max(1.0) as usize;
                    if n != per_row {
                        ui.data_mut(|d| d.insert_temp(egui::Id::unique("explorer-per-row"), n));
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(GAP_XL, GAP_XL);
                        body(ui);
                    });
                }
            });
        if let Some((rect, keep)) = band {
            let p = ui.painter();
            p.rect_filled(rect, 2, SELECT.gamma_multiply(0.12));
            p.rect_stroke(rect, 2, Stroke::new(LINE, SELECT), egui::StrokeKind::Inside);
            let mut picked = keep;
            for (path, r) in rects {
                if r.intersects(rect) && !picked.contains(&path) {
                    picked.push(path);
                }
            }
            self.anchor = picked.last().cloned();
            self.selected = picked;
        }
        if let Some((path, name)) = rename_done {
            self.rename(path, name, busy);
        }
        if let Some((items, dir)) = drop.or(self.pending_drop.take()) {
            self.move_into(items, &dir, busy);
        }
        if let Some(p) = opened {
            self.selected = vec![p.clone()];
            self.open_item(&p);
        } else if let Some((p, m)) = clicked {
            self.select_click(&p, m);
            self.renaming = None;
        }
    }

    fn renaming_text(&self, path: &Path) -> bool {
        matches!(&self.renaming, Some((p, _, _)) if p == path)
    }

    /// Поле нового имени поверх подписи файла.
    fn rename_field(
        &mut self,
        ui: &mut Ui,
        rect: &Rect,
        path: &Path,
        done: &mut Option<(PathBuf, String)>,
    ) {
        let Some((_, text, first)) = self.renaming.as_mut() else {
            return;
        };
        let id = egui::Id::unique("explorer-rename");
        let edit = ui.put(
            *rect,
            egui::TextEdit::singleline(text)
                .id(id)
                .font(FontId::proportional(FONT_SMALL + 1.0))
                .margin(Margin::symmetric(4, 1)),
        );
        if *first {
            // Как в Windows: выделено имя без окончания, его и печатают поверх.
            *first = false;
            edit.request_focus();
            let (stem, _) = pf::split_name(text);
            let n = stem.chars().count();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(n),
                    )));
                state.store(ui.ctx(), id);
            }
            return;
        }
        if edit.lost_focus() {
            let esc = ui.input(|i| i.key_pressed(Key::Escape));
            if let Some((_, text, _)) = self.renaming.take()
                && !esc
            {
                *done = Some((path.to_path_buf(), text));
            }
        } else if !edit.has_focus() {
            edit.request_focus();
        }
    }

    fn list_header(&mut self, ui: &mut Ui) {
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
        let cols = columns(rect);
        for (s, col) in SortBy::ALL.into_iter().zip(cols) {
            let r = ui.interact(
                col,
                egui::Id::unique(("explorer-col", s.label())),
                Sense::click(),
            );
            let on = self.sort == s;
            let mut text = s.label().to_owned();
            if on {
                text.push_str(if self.desc { " ↓" } else { " ↑" });
            }
            ui.painter().text(
                pos2(col.left() + GAP_M, col.center().y),
                Align2::LEFT_CENTER,
                text,
                FontId::proportional(FONT_SMALL),
                if on || r.hovered() { TEXT } else { MUTED },
            );
            if r.on_hover_text("Сортировать по этому столбцу").clicked() {
                let desc = on && !self.desc;
                self.set_sort(s, desc);
            }
        }
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(LINE, BORDER),
        );
    }

    fn item_menu(&mut self, ui: &mut Ui, item: &Item, busy: &Busy) {
        let fixed = self.is_fixed(&item.path);
        let open_label = if item.folder {
            "Открыть папку"
        } else {
            match self.open_kind(&item.path) {
                OpenKind::Editor => "Открыть в редакторе",
                OpenKind::World => "Открыть в «Мире»",
                OpenKind::System => "Открыть программой Windows",
            }
        };
        if ui.button(open_label).clicked() {
            self.open_item(&item.path);
            ui.close();
        }
        if !item.folder
            && self.open_kind(&item.path) != OpenKind::System
            && ui
                .button("Открыть как текст (программой Windows)")
                .clicked()
        {
            self.out.push(Out::OpenFile(item.path.clone()));
            ui.close();
        }
        if item.folder
            && !self.is_root(&item.path)
            && ui
                .add(egui::Button::new("Создать здесь…"))
                .on_hover_text("Новая папка, сцена, модель или скрипт в этой папке")
                .clicked()
        {
            self.ask_create(item.path.clone(), NewKind::Folder);
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(
                !fixed,
                egui::Button::new("Вырезать").shortcut_text("Ctrl+X"),
            )
            .clicked()
        {
            self.clipboard = Some((self.selected.clone(), true));
            ui.close();
        }
        if ui
            .add(egui::Button::new("Копировать").shortcut_text("Ctrl+C"))
            .clicked()
        {
            self.clipboard = Some((self.selected.clone(), false));
            ui.close();
        }
        if item.folder
            && self.clipboard.is_some()
            && !self.is_root(&item.path)
            && ui.button("Вставить в эту папку").clicked()
        {
            let dir = item.path.clone();
            if let Some((items, cut)) = self.clipboard.clone() {
                if cut {
                    self.move_into(items, &dir, busy);
                    self.clipboard = None;
                } else {
                    match pf::copy_into(&dir, &items) {
                        Ok(done) => self.did(done),
                        Err(e) => self.say(e, true),
                    }
                }
            }
            ui.close();
        }
        ui.separator();
        let one = self.selected.len() <= 1;
        if ui
            .add_enabled(
                !fixed && one,
                egui::Button::new("Переименовать").shortcut_text("F2"),
            )
            .clicked()
        {
            self.renaming = Some((item.path.clone(), item.name.clone(), true));
            ui.close();
        }
        if ui
            .add_enabled(!fixed, egui::Button::new("Удалить").shortcut_text("Delete"))
            .clicked()
        {
            self.delete(busy);
            ui.close();
        }
        ui.separator();
        if ui.button("Показать в проводнике Windows").clicked() {
            self.out.push(Out::Reveal(item.path.clone()));
            ui.close();
        }
    }

    fn folder_menu(&mut self, ui: &mut Ui, busy: &Busy) {
        let can_make = !self.is_root(&self.dir);
        if ui
            .add_enabled(
                can_make,
                egui::Button::new("Создать…").shortcut_text("Ctrl+N"),
            )
            .clicked()
        {
            self.ask_create(self.dir.clone(), NewKind::Folder);
            ui.close();
        }
        if ui
            .add_enabled(
                can_make && self.clipboard.is_some(),
                egui::Button::new("Вставить").shortcut_text("Ctrl+V"),
            )
            .clicked()
        {
            self.paste(busy);
            ui.close();
        }
        if ui
            .add_enabled(
                !self.undo.is_empty(),
                egui::Button::new("Отменить").shortcut_text("Ctrl+Z"),
            )
            .clicked()
        {
            self.undo();
            ui.close();
        }
        ui.separator();
        if ui.button("Перечитать папку").clicked() {
            self.refresh();
            ui.close();
        }
        if ui.button("Показать в проводнике Windows").clicked() {
            self.out.push(Out::Reveal(self.dir.clone()));
            ui.close();
        }
    }

    fn open_kind(&self, path: &Path) -> OpenKind {
        let name = pf::file_name(path).to_lowercase();
        let prefab = path.parent().is_some_and(|p| pf::file_name(p) == PREFABS);
        if is_shape(&name) || (name.ends_with(".ron") && is_space_scene(path)) {
            OpenKind::Editor
        } else if name.ends_with(".ron") && !prefab && is_scene_2d(path) {
            OpenKind::World
        } else {
            OpenKind::System
        }
    }

    /// Свойства выбранного справа.
    fn details(&mut self, ui: &mut Ui) {
        let picked: Vec<Item> = self
            .selected
            .iter()
            .filter_map(|p| self.items.iter().find(|i| &i.path == p).cloned())
            .collect();
        match picked.as_slice() {
            [] => {
                let title = self.dir_title();
                let files = self.items.iter().filter(|i| !i.folder).count();
                let dirs = self.items.len() - files;
                heading(ui, &title);
                ui.label(RichText::new("Папка").color(MUTED));
                ui.add_space(GAP_L);
                fact(ui, "Папок", &dirs.to_string());
                fact(ui, "Файлов", &files.to_string());
                let total: u64 = self.items.iter().map(|i| i.size).sum();
                fact(ui, "Размер файлов", &size_text(total));
                if let Some(rel) = self.rel(&self.dir) {
                    fact(ui, "Где", &rel);
                }
                ui.add_space(GAP_XL);
                ui.label(
                    RichText::new(
                        "Двойной щелчок по файлу открывает его редактор. Правая кнопка — меню.\n\
                         Перетащите файлы на папку, чтобы перенести их.",
                    )
                    .small()
                    .color(MUTED),
                );
            }
            [item] => self.one_details(ui, item),
            many => {
                heading(ui, &format!("Выбрано: {}", many.len()));
                let dirs = many.iter().filter(|i| i.folder).count();
                fact(ui, "Папок", &dirs.to_string());
                fact(ui, "Файлов", &(many.len() - dirs).to_string());
                let total: u64 = many.iter().map(|i| i.size).sum();
                fact(ui, "Размер файлов", &size_text(total));
            }
        }
    }

    fn one_details(&mut self, ui: &mut Ui, item: &Item) {
        // Большая плитка-образец вида файла.
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 120.0), Sense::hover());
        thumb(ui.painter(), rect, item, false);
        ui.add_space(GAP_L);
        heading(ui, &item.name);
        ui.label(RichText::new(capital(item.kind)).color(item.color));
        ui.add_space(GAP_L);
        if item.folder {
            let n = read_dir(&item.path)
                .iter()
                .filter(|p| !pf::file_name(p).starts_with('.'))
                .count();
            fact(ui, "Внутри", &things_word(n));
        } else {
            fact(ui, "Размер", &size_text(item.size));
        }
        if let Some(m) = item.modified {
            fact(ui, "Изменён", &when_text(m));
        }
        if let Some(rel) = self.rel(&item.path) {
            fact(ui, "Где", &rel);
        }
        ui.add_space(GAP_L);
        ui.horizontal(|ui| {
            let label = match (item.folder, self.open_kind(&item.path)) {
                (true, _) => "Открыть",
                (_, OpenKind::Editor) => "Открыть в редакторе",
                (_, OpenKind::World) => "Открыть в «Мире»",
                (_, OpenKind::System) => "Открыть",
            };
            let b = egui::Button::new(RichText::new(label).color(ON_ACCENT)).fill(ACCENT);
            if ui.add(b).clicked() {
                self.open_item(&item.path);
            }
            if ui
                .button("В проводнике")
                .on_hover_text("Показать в проводнике Windows")
                .clicked()
            {
                self.out.push(Out::Reveal(item.path.clone()));
            }
        });
        // Где используется (считается раз на выбор: просмотр всех .ron проекта).
        if self.used.as_ref().is_none_or(|(p, _)| p != &item.path)
            && let Some(project) = &self.project
        {
            self.used = Some((item.path.clone(), pf::used_in(project, &item.path)));
        }
        ui.add_space(GAP_XL);
        ui.label(RichText::new("Где используется").color(TEXT).strong());
        ui.add_space(GAP_S);
        let used = self
            .used
            .as_ref()
            .map(|(_, u)| u.clone())
            .unwrap_or_default();
        if used.is_empty() {
            ui.label(
                RichText::new("Ни одна сцена или модель на него не ссылается")
                    .small()
                    .color(MUTED),
            );
        }
        for (file, n) in used {
            let name = self.rel(&file).unwrap_or_else(|| pf::file_name(&file));
            let text = if n > 1 {
                format!("{name}  ×{n}")
            } else {
                name
            };
            let r = ui.add(
                egui::Button::new(RichText::new(text).color(SELECT))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE),
            );
            if r.on_hover_text("Показать этот файл").clicked()
                && let Some(dir) = file.parent()
            {
                self.go(dir.to_path_buf());
                self.selected = vec![file.clone()];
                self.anchor = Some(file.clone());
                self.scroll_to = true;
            }
        }
        if !item.folder {
            ui.add_space(GAP_M);
            ui.label(
                RichText::new("Переименование и перенос сами поправят пути в сценах и моделях.")
                    .small()
                    .color(MUTED),
            );
        }
    }

    /// Путь от папки проекта: «scenes/3d/models».
    fn rel(&self, p: &Path) -> Option<String> {
        let r = p.strip_prefix(self.project.as_ref()?).ok()?;
        let s = r.to_string_lossy().replace('\\', "/");
        Some(if s.is_empty() {
            "Проект".into()
        } else {
            s
        })
    }

    fn status(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            let n = self.items.len();
            let mut text = things_word(n);
            if !self.selected.is_empty() {
                text.push_str(&format!("  ·  выбрано {}", self.selected.len()));
            }
            if let Some((items, cut)) = &self.clipboard {
                text.push_str(&format!(
                    "  ·  в буфере {} ({})",
                    items.len(),
                    if *cut {
                        "вырезано"
                    } else {
                        "копия"
                    }
                ));
            }
            ui.label(RichText::new(text).small().color(MUTED));
            if let Some((note, error)) = &self.note {
                ui.add_space(GAP_XL);
                ui.label(
                    RichText::new(note)
                        .small()
                        .color(if *error { BAD } else { TEXT_DIM }),
                );
            }
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenKind {
    Editor,
    World,
    System,
}

/// Файл — сцена 2D.
fn is_scene_2d(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.contains("\"reyvec-scene\""))
}

fn read_dir(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// Поиск по имени во всех вложенных папках.
fn find(dir: &Path, q: &str, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 10 || out.len() >= SEARCH_LIMIT {
        return;
    }
    for p in read_dir(dir) {
        let name = pf::file_name(&p);
        if name.starts_with('.') {
            continue;
        }
        if name.to_lowercase().contains(q) {
            out.push(p.clone());
        }
        if p.is_dir() {
            find(&p, q, out, depth + 1);
        }
    }
}

/// Папки всегда первыми, дальше — как выбрано.
pub(crate) fn sort_items(items: &mut [Item], by: SortBy, desc: bool) {
    items.sort_by(|a, b| {
        let order = match by {
            SortBy::Name => natural(&a.name, &b.name),
            SortBy::Kind => a.kind.cmp(b.kind).then_with(|| natural(&a.name, &b.name)),
            SortBy::Date => a.modified.cmp(&b.modified),
            SortBy::Size => a.size.cmp(&b.size),
        };
        b.folder
            .cmp(&a.folder)
            .then(if desc { order.reverse() } else { order })
    });
}

/// Сравнение имён «по-человечески»: «сцена 2» раньше «сцена 10», регистр не важен.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.to_lowercase().chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, t)) if *d == digit => t.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (pa, pb) = (parts(a), parts(b));
    for ((da, ta), (db, tb)) in pa.iter().zip(&pb) {
        let o = if *da && *db {
            let (na, nb) = (ta.trim_start_matches('0'), tb.trim_start_matches('0'));
            na.len().cmp(&nb.len()).then_with(|| na.cmp(nb))
        } else {
            ta.cmp(tb)
        };
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    pa.len().cmp(&pb.len())
}

pub(crate) fn size_text(n: u64) -> String {
    let n = n as f64;
    if n < 1024.0 {
        format!("{n} Б")
    } else if n < 1024.0 * 1024.0 {
        format!("{:.1} КБ", n / 1024.0)
    } else {
        format!("{:.1} МБ", n / 1024.0 / 1024.0)
    }
}

/// Когда менялся: «только что», «5 мин назад», «вчера», «12.10.2026».
pub(crate) fn when_text(t: SystemTime) -> String {
    let Ok(ago) = SystemTime::now().duration_since(t) else {
        return "только что".into();
    };
    let s = ago.as_secs();
    match s {
        0..60 => "только что".into(),
        60..3600 => format!("{} мин назад", s / 60),
        3600..86_400 => format!("{} ч назад", s / 3600),
        86_400..172_800 => "вчера".into(),
        172_800..604_800 => format!("{} дн. назад", s / 86_400),
        _ => {
            // Дата по UTC: часовой пояс здесь не важен.
            let days = t
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() / 86_400) as i64;
            let (y, m, d) = civil(days);
            format!("{d:02}.{m:02}.{y}")
        }
    }
}

/// День от 1970-01-01 → (год, месяц, число).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn moved_note(n: usize, dir: &str, refs: usize) -> String {
    let mut s = format!("Перенесено в «{dir}»: {}", things_word(n));
    if refs > 0 {
        s.push_str(&format!(", ссылки исправлены в {}", files_word(refs)));
    }
    s
}

/// «1 элемент», «3 элемента», «5 элементов».
pub(crate) fn things_word(n: usize) -> String {
    format!("{n} {}", plural(n, "элемент", "элемента", "элементов"))
}

fn files_word(n: usize) -> String {
    format!("{n} {}", plural(n, "файле", "файлах", "файлах"))
}

fn plural(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    let (d, h) = (n % 10, n % 100);
    if d == 1 && h != 11 {
        one
    } else if (2..=4).contains(&d) && !(12..=14).contains(&h) {
        few
    } else {
        many
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(FONT_HEADING).color(Color32::WHITE));
}

fn fact(ui: &mut Ui, name: &str, value: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(110.0, 18.0), Sense::hover());
        ui.painter().text(
            r.left_center(),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(FONT_BODY),
            MUTED,
        );
        ui.add(egui::Label::new(RichText::new(value).color(TEXT_DIM)).wrap());
    });
}

fn sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(GAP_L * 2.0, 22.0), Sense::hover());
    ui.painter()
        .vline(r.center().x, r.y_range(), Stroke::new(LINE, BORDER_STRONG));
}

/// Под мышью с файлами — рамка: сюда можно бросить.
fn drop_target(r: &egui::Response, ui: &Ui) {
    if r.dnd_hover_payload::<Dragged>().is_some() {
        ui.painter().rect_stroke(
            r.rect.expand(2.0),
            RADIUS_M,
            Stroke::new(LINE_FOCUS, SELECT),
            egui::StrokeKind::Outside,
        );
    }
}

/// Столбцы списка: имя, вид, изменён, размер.
fn columns(rect: Rect) -> [Rect; 4] {
    let w = rect.width();
    let widths = [w - 3.0 * (w * 0.16).clamp(100.0, 180.0), 0.0, 0.0, 0.0];
    let side = (w * 0.16).clamp(100.0, 180.0);
    let mut x = rect.left();
    let mut out = [Rect::NOTHING; 4];
    for (i, o) in out.iter_mut().enumerate() {
        let cw = if i == 0 { widths[0] } else { side };
        *o = Rect::from_min_size(pos2(x, rect.top()), vec2(cw, rect.height()));
        x += cw;
    }
    out
}

/// Строка списка. Второе — где поле имени, если файл переименовывают.
fn row(ui: &mut Ui, item: &Item, selected: bool, renaming: bool) -> (egui::Response, Option<Rect>) {
    let (rect, r) = ui.allocate_exact_size(
        vec2(ui.available_width(), ROW_HEIGHT),
        Sense::click_and_drag(),
    );
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, RADIUS_S, SELECT_BG);
    } else if r.hovered() {
        p.rect_filled(rect, RADIUS_S, HOVER);
    }
    let cols = columns(rect);
    let dot = Rect::from_center_size(
        pos2(cols[0].left() + 14.0, rect.center().y),
        vec2(14.0, 14.0),
    );
    if item.folder {
        paint_glyph(p, dot.center(), Glyph::Folder, item.color);
    } else {
        p.rect_filled(dot.shrink(2.0), 3, item.color.gamma_multiply(0.85));
    }
    let name_rect = Rect::from_min_max(
        pos2(dot.right() + GAP_L, rect.top() + 2.0),
        pos2(cols[0].right() - GAP_L, rect.bottom() - 2.0),
    );
    if !renaming {
        p.text(
            pos2(name_rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            &item.name,
            FontId::proportional(FONT_BODY),
            if selected { Color32::WHITE } else { TEXT },
        );
    }
    let cells = [
        capital(item.kind),
        item.modified.map(when_text).unwrap_or_default(),
        if item.folder {
            String::new()
        } else {
            size_text(item.size)
        },
    ];
    for (text, col) in cells.into_iter().zip(&cols[1..]) {
        p.text(
            pos2(col.left() + GAP_M, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(FONT_SMALL),
            MUTED,
        );
    }
    (r, renaming.then_some(name_rect))
}

/// Плитка: образец вида файла и имя под ним.
fn tile(
    ui: &mut Ui,
    item: &Item,
    selected: bool,
    renaming: bool,
) -> (egui::Response, Option<Rect>) {
    let (rect, r) = ui.allocate_exact_size(TILE, Sense::click_and_drag());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect.expand(3.0), RADIUS_M, SELECT_BG);
    } else if r.hovered() {
        p.rect_filled(rect.expand(3.0), RADIUS_M, HOVER);
    }
    let t = Rect::from_min_size(rect.min, vec2(rect.width(), THUMB_H));
    thumb(p, t, item, r.hovered());
    let name_rect = Rect::from_min_max(
        pos2(rect.left() - 2.0, t.bottom() + GAP_S),
        pos2(rect.right() + 2.0, t.bottom() + GAP_S + 22.0),
    );
    if !renaming {
        let galley = p.layout(
            item.name.clone(),
            FontId::proportional(FONT_SMALL),
            if selected { Color32::WHITE } else { TEXT_DIM },
            rect.width(),
        );
        // Не больше двух строк: длинное имя целиком — в подсказке.
        let clip = Rect::from_min_max(
            pos2(rect.left(), t.bottom() + GAP_S),
            pos2(rect.right(), rect.bottom() + 4.0),
        );
        let x = rect.center().x - galley.size().x / 2.0;
        p.with_clip_rect(clip)
            .galley(pos2(x, t.bottom() + GAP_S), galley, TEXT_DIM);
    }
    (r, renaming.then_some(name_rect))
}

/// Образец вида файла: цветная полоса сверху, значок папки или окончание и вид.
fn thumb(p: &egui::Painter, t: Rect, item: &Item, hovered: bool) {
    let fill = if hovered { RAISED } else { CARD };
    p.rect(
        t,
        RADIUS_M,
        fill,
        Stroke::new(LINE, BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
    p.rect_filled(t.shrink(LINE), RADIUS_M, item.color.gamma_multiply(0.08));
    if item.folder {
        let s = (t.height() / 70.0).clamp(1.0, 2.0);
        paint_folder(p, t.center(), s * 1.9, item.color);
        return;
    }
    let band = Rect::from_min_size(t.min + vec2(LINE, LINE), vec2(t.width() - 2.0 * LINE, 3.0));
    p.rect_filled(
        band,
        egui::CornerRadius {
            nw: RADIUS_M - 1,
            ne: RADIUS_M - 1,
            sw: 0,
            se: 0,
        },
        item.color,
    );
    let (_, ext) = pf::split_name(&item.name);
    let big = t.height() > 90.0;
    p.text(
        t.center() - vec2(0.0, if big { 10.0 } else { 7.0 }),
        Align2::CENTER_CENTER,
        if ext.is_empty() { "файл" } else { ext },
        FontId::monospace(if big { FONT_HEADING } else { FONT_SMALL }),
        item.color,
    );
    p.text(
        t.center() + vec2(0.0, if big { 14.0 } else { 10.0 }),
        Align2::CENTER_CENTER,
        item.kind,
        FontId::proportional(if big { FONT_BODY } else { FONT_SMALL - 1.5 }),
        MUTED,
    );
}

fn paint_folder(p: &egui::Painter, c: Pos2, s: f32, color: Color32) {
    let body = Rect::from_center_size(c + vec2(0.0, 1.0 * s), vec2(16.0 * s, 11.0 * s));
    let tab = Rect::from_min_size(body.min - vec2(0.0, 2.5 * s), vec2(7.0 * s, 3.5 * s));
    p.rect_filled(tab, (1.5 * s) as u8, color.gamma_multiply(0.75));
    p.rect_filled(body, (2.0 * s) as u8, color);
}

fn chevron(p: &egui::Painter, c: Pos2, open: bool) {
    let s = Stroke::new(1.3, MUTED);
    let pts = if open {
        [
            c + vec2(-4.0, -2.0),
            c + vec2(0.0, 2.0),
            c + vec2(4.0, -2.0),
        ]
    } else {
        [
            c + vec2(-2.0, -4.0),
            c + vec2(2.0, 0.0),
            c + vec2(-2.0, 4.0),
        ]
    };
    p.line(pts.to_vec(), s);
}

/// Значки кнопок проводника (рисуются линиями, без картинок).
#[derive(Clone, Copy, Debug)]
enum Glyph {
    Back,
    Forward,
    Up,
    Refresh,
    Search,
    Cut,
    Copy,
    Paste,
    Rename,
    Trash,
    Undo,
    Redo,
    Folder,
    Tiles,
    List,
}

fn paint_glyph(p: &egui::Painter, c: Pos2, g: Glyph, color: Color32) {
    let s = Stroke::new(1.4, color);
    let v = |x: f32, y: f32| c + vec2(x, y);
    match g {
        Glyph::Back | Glyph::Forward | Glyph::Up => {
            let (tip, a, b, tail) = match g {
                Glyph::Back => (v(-6.0, 0.0), v(-1.0, -5.0), v(-1.0, 5.0), v(6.0, 0.0)),
                Glyph::Forward => (v(6.0, 0.0), v(1.0, -5.0), v(1.0, 5.0), v(-6.0, 0.0)),
                _ => (v(0.0, -6.0), v(-5.0, -1.0), v(5.0, -1.0), v(0.0, 6.0)),
            };
            p.line_segment([tail, tip], s);
            p.line(vec![a, tip, b], s);
        }
        Glyph::Refresh => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let t = 0.5 + i as f32 / 20.0 * 5.0;
                    c + vec2(t.cos(), t.sin()) * 5.5
                })
                .collect();
            let end = *pts.last().unwrap_or(&c);
            p.line(pts, s);
            p.line(vec![end + vec2(-3.5, -1.0), end, end + vec2(0.5, -3.8)], s);
        }
        Glyph::Search => {
            p.circle_stroke(v(-1.5, -1.5), 4.5, s);
            p.line_segment([v(2.0, 2.0), v(6.0, 6.0)], s);
        }
        Glyph::Cut => {
            p.circle_stroke(v(-3.5, 4.0), 2.3, s);
            p.circle_stroke(v(3.5, 4.0), 2.3, s);
            p.line_segment([v(-2.2, 2.0), v(4.0, -6.0)], s);
            p.line_segment([v(2.2, 2.0), v(-4.0, -6.0)], s);
        }
        Glyph::Copy => {
            p.rect_stroke(
                Rect::from_min_size(v(-6.0, -6.0), vec2(8.5, 9.5)),
                2,
                s,
                egui::StrokeKind::Middle,
            );
            p.rect_stroke(
                Rect::from_min_size(v(-2.5, -2.5), vec2(8.5, 9.5)),
                2,
                s,
                egui::StrokeKind::Middle,
            );
        }
        Glyph::Paste => {
            p.rect_stroke(
                Rect::from_min_size(v(-5.5, -5.0), vec2(11.0, 12.0)),
                2,
                s,
                egui::StrokeKind::Middle,
            );
            p.rect_filled(Rect::from_min_size(v(-2.5, -7.0), vec2(5.0, 3.0)), 1, color);
        }
        Glyph::Rename => {
            p.line_segment([v(-5.0, 5.0), v(4.0, -4.0)], Stroke::new(2.2, color));
            p.line_segment([v(-6.0, 6.5), v(6.0, 6.5)], s);
        }
        Glyph::Trash => {
            p.line_segment([v(-6.0, -4.0), v(6.0, -4.0)], s);
            p.line_segment([v(-2.0, -6.0), v(2.0, -6.0)], s);
            p.line(
                vec![v(-4.5, -4.0), v(-3.5, 6.0), v(3.5, 6.0), v(4.5, -4.0)],
                s,
            );
        }
        Glyph::Undo => {
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = -1.6 + i as f32 / 16.0 * 3.2;
                    c + vec2(t.sin() * 5.0 + 1.0, -t.cos() * 4.5 + 1.0)
                })
                .collect();
            p.line(pts, s);
            p.line(vec![v(-6.0, -6.0), v(-3.0, -3.5), v(-6.5, -1.5)], s);
        }
        Glyph::Redo => {
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = -1.6 + i as f32 / 16.0 * 3.2;
                    c + vec2(-(t.sin() * 5.0 + 1.0), -t.cos() * 4.5 + 1.0)
                })
                .collect();
            p.line(pts, s);
            p.line(vec![v(6.0, -6.0), v(3.0, -3.5), v(6.5, -1.5)], s);
        }
        Glyph::Folder => paint_folder(p, c, 0.85, color),
        Glyph::Tiles => {
            for (x, y) in [(-5.5, -5.5), (0.5, -5.5), (-5.5, 0.5), (0.5, 0.5)] {
                p.rect_filled(Rect::from_min_size(v(x, y), vec2(5.0, 5.0)), 1, color);
            }
        }
        Glyph::List => {
            for y in [-4.5, 0.0, 4.5] {
                p.rect_filled(
                    Rect::from_min_size(v(-6.0, y - 1.0), vec2(2.0, 2.0)),
                    0,
                    color,
                );
                p.line_segment([v(-2.5, y), v(6.0, y)], s);
            }
        }
    }
}

/// Кнопка-значок с подсказкой.
fn icon(ui: &mut Ui, g: Glyph, hint: &str, enabled: bool) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, r) = ui.allocate_exact_size(vec2(30.0, 30.0), sense);
    if enabled && r.hovered() {
        ui.painter().rect_filled(rect, RADIUS_S, HOVER);
    }
    let color = if !enabled {
        MUTED.gamma_multiply(0.5)
    } else if r.hovered() {
        Color32::WHITE
    } else {
        TEXT_DIM
    };
    paint_glyph(ui.painter(), rect.center(), g, color);
    r.on_hover_text(hint)
}

/// Кнопка-значок, которая бывает включена (вид плиток или списка).
fn toggle(ui: &mut Ui, g: Glyph, hint: &str, on: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    if on {
        ui.painter().rect_filled(rect, RADIUS_S, SELECT_BG);
    } else if r.hovered() {
        ui.painter().rect_filled(rect, RADIUS_S, HOVER);
    }
    paint_glyph(
        ui.painter(),
        rect.center(),
        g,
        if on { Color32::WHITE } else { TEXT_DIM },
    );
    r.on_hover_text(hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn project(files: &[(&str, &str)]) -> Tmp {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("reyvec-explorer-ui-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        Tmp(root)
    }

    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Кадр проводника; время растёт, чтобы egui не путался.
    fn frame(
        ctx: &egui::Context,
        ex: &mut Explorer,
        t: &mut f64,
        events: Vec<egui::Event>,
    ) -> Vec<Out> {
        *t += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 900.0))),
            time: Some(*t),
            events,
            ..Default::default()
        };
        let mut outs = Vec::new();
        let mut out = ctx.run_ui(input, |ui| outs = ex.draw(ui, &Busy::default()));
        out.textures_delta.clear();
        outs
    }

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(reyvec_app::for_editor::fonts());
        super::super::theme::apply(&ctx);
        ctx
    }

    #[test]
    fn keys_rename_delete_undo_and_open() {
        let t = project(&[
            (
                "scenes/room.ron",
                "(format: \"reyvec-scene\", version: 1, nodes: [(svg: \"../assets/a.svg\")])",
            ),
            ("assets/a.svg", "<svg/>"),
            ("assets/m.shape.ron", "(format: \"reyvec-shape\")"),
        ]);
        let ctx = ctx();
        let mut ex = Explorer::new(Some(&t.0));
        let mut time = 0.0;
        frame(&ctx, &mut ex, &mut time, vec![]);
        assert_eq!(ex.dir(), t.0.join("assets"));
        let names: Vec<_> = ex.items().iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["a.svg", "m.shape.ron"]);
        // F2, «b», Enter: выделено имя без окончания, печать заменяет его.
        ex.selected = vec![t.0.join("assets/a.svg")];
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::F2, Modifiers::NONE)],
        );
        frame(&ctx, &mut ex, &mut time, vec![]);
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![egui::Event::Text("b".into())],
        );
        let outs = frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::Enter, Modifiers::NONE)],
        );
        frame(&ctx, &mut ex, &mut time, vec![]);
        assert!(t.0.join("assets/b.svg").is_file(), "{outs:?}");
        assert!(outs.contains(&Out::Changed));
        let scene = std::fs::read_to_string(t.0.join("scenes/room.ron")).unwrap();
        assert!(scene.contains("\"../assets/b.svg\""), "{scene}");
        // Delete уносит в корзину, Ctrl+Z возвращает.
        assert_eq!(ex.selected, vec![t.0.join("assets/b.svg")]);
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::Delete, Modifiers::NONE)],
        );
        assert!(!t.0.join("assets/b.svg").exists());
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::Z, Modifiers::COMMAND)],
        );
        assert!(t.0.join("assets/b.svg").is_file());
        // Enter на модели — открыть её в редакторе; на сцене 2D — в «Мире».
        ex.selected = vec![t.0.join("assets/m.shape.ron")];
        let outs = frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::Enter, Modifiers::NONE)],
        );
        assert!(
            outs.contains(&Out::OpenShape(t.0.join("assets/m.shape.ron"))),
            "{outs:?}"
        );
        ex.go(t.0.join("scenes"));
        frame(&ctx, &mut ex, &mut time, vec![]);
        ex.selected = vec![t.0.join("scenes/room.ron")];
        let outs = frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::Enter, Modifiers::NONE)],
        );
        assert!(
            outs.contains(&Out::OpenScene(t.0.join("scenes/room.ron"))),
            "{outs:?}"
        );
        // Назад — снова assets; новая папка сразу ждёт имя.
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::ArrowLeft, Modifiers::ALT)],
        );
        assert_eq!(ex.dir(), t.0.join("assets"));
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::N, Modifiers::COMMAND | Modifiers::SHIFT)],
        );
        // Окно «Создать»: имя выделено, печать заменяет его, Enter создаёт.
        assert_eq!(ex.creating.as_ref().map(|c| c.kind), Some(NewKind::Folder));
        frame(&ctx, &mut ex, &mut time, vec![]);
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![egui::Event::Text("ящики".into())],
        );
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::Enter, Modifiers::NONE)],
        );
        frame(&ctx, &mut ex, &mut time, vec![]);
        assert!(t.0.join("assets/ящики").is_dir());
        assert!(ex.creating.is_none());
        // Модель с тем же именем через окно: вид меняется, окончание дописывается.
        ex.ask_create(t.0.join("assets"), NewKind::Shape);
        frame(&ctx, &mut ex, &mut time, vec![]);
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![egui::Event::Text("m".into())],
        );
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::Enter, Modifiers::NONE)],
        );
        frame(&ctx, &mut ex, &mut time, vec![]);
        // «m.shape.ron» уже есть: окно остаётся и говорит почему.
        let c = ex.creating.as_ref().expect("окно открыто");
        assert!(
            c.error.as_deref().is_some_and(|e| e.contains("уже есть")),
            "{c:?}"
        );
        ex.creating = None;
        frame(&ctx, &mut ex, &mut time, vec![]);
        // Отмена убирает папку, повтор (Ctrl+Y) возвращает её.
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::Z, Modifiers::COMMAND)],
        );
        assert!(!t.0.join("assets/ящики").exists(), "{:?}", ex.note);
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![key(Key::Y, Modifiers::COMMAND)],
        );
        assert!(t.0.join("assets/ящики").is_dir());
        // Поиск находит во вложенных папках.
        ex.search = "room".into();
        ex.go(t.0.clone());
        ex.search = "room".into();
        ex.refresh();
        frame(&ctx, &mut ex, &mut time, vec![]);
        let names: Vec<_> = ex.items().iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["room.ron"]);
    }

    /// Рамка мышью с пустого места выбирает плитки, которых коснулась.
    #[test]
    fn band_selects_tiles() {
        let t = project(&[
            ("assets/a.svg", ""),
            ("assets/b.svg", ""),
            ("assets/c.svg", ""),
        ]);
        let ctx = ctx();
        let mut ex = Explorer::new(Some(&t.0));
        let mut time = 0.0;
        frame(&ctx, &mut ex, &mut time, vec![]);
        frame(&ctx, &mut ex, &mut time, vec![]);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let start = pos2(900.0, 600.0);
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![egui::Event::PointerMoved(start)],
        );
        frame(&ctx, &mut ex, &mut time, vec![button(start, true)]);
        for p in [pos2(850.0, 500.0), pos2(500.0, 300.0), pos2(380.0, 150.0)] {
            frame(&ctx, &mut ex, &mut time, vec![egui::Event::PointerMoved(p)]);
        }
        // Рамка от (380, 150) до (900, 600) задевает вторую и третью плитки, но не первую.
        let mut got: Vec<_> = ex.selected.iter().map(|p| pf::file_name(p)).collect();
        got.sort();
        assert_eq!(got, ["b.svg", "c.svg"]);
        frame(
            &ctx,
            &mut ex,
            &mut time,
            vec![button(pos2(380.0, 150.0), false)],
        );
        frame(&ctx, &mut ex, &mut time, vec![]);
        assert!(ex.band.is_none());
        assert_eq!(ex.selected.len(), 2);
    }

    #[test]
    fn open_scene_cannot_move_and_root_is_fixed() {
        let t = project(&[("scenes/room.ron", "x"), ("assets/a.svg", "")]);
        let ex = Explorer::new(Some(&t.0));
        let scene = t.0.join("scenes/room.ron");
        let busy = Busy {
            scene: Some(&scene),
            scene_dirty: false,
        };
        assert!(ex.blocked(std::slice::from_ref(&scene), &busy).is_some());
        assert!(ex.blocked(&[t.0.join("scenes")], &busy).is_some());
        assert!(ex.blocked(&[t.0.join("assets/a.svg")], &busy).is_none());
    }

    #[test]
    fn words_sizes_and_order() {
        assert_eq!(things_word(1), "1 элемент");
        assert_eq!(things_word(3), "3 элемента");
        assert_eq!(things_word(11), "11 элементов");
        assert_eq!(size_text(2048), "2.0 КБ");
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_000), (2024, 10, 4));
        assert_eq!(natural("сцена 2", "сцена 10"), std::cmp::Ordering::Less);
        assert_eq!(natural("B", "a"), std::cmp::Ordering::Greater);
    }
}
