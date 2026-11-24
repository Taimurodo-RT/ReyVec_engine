//! Окно «Проекты» (П.1): недавние проекты, «Создать проект» из заготовки и «Открыть
//! папку». Показывается при запуске (если не выключено) и из «Файл → Проекты…». Другой
//! проект открывается перезапуском редактора: папку сцен движок берёт при старте.

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use egui::{Color32, Id, RichText, Sense, Stroke, Ui, pos2, vec2};

use super::super::explorer::when_text;
use super::super::project::{self, Recent, Template};
use super::super::project_files as pf;
use super::super::theme::*;
use super::{Action, EditorUi, Info};

/// Что показывает окно.
#[derive(Clone, Debug, PartialEq)]
enum Page {
    List,
    Create,
    /// Выбор папки: для «Открыть» или «Где создать».
    Browse {
        open: bool,
    },
}

/// Состояние окна «Проекты».
#[derive(Clone, Debug)]
pub(crate) struct ProjectsScreen {
    recent: Recent,
    recent_path: PathBuf,
    current: Option<PathBuf>,
    page: Page,
    name: String,
    parent: String,
    template: Template,
    browse: PathBuf,
    browse_text: String,
    error: Option<String>,
    /// Окно открыто при запуске (тогда «Закрыть» = остаться в текущем проекте).
    at_start: bool,
}

impl ProjectsScreen {
    pub(crate) fn new(recent_path: PathBuf, current: Option<PathBuf>, at_start: bool) -> Self {
        let parent = project::default_parent();
        Self {
            recent: Recent::load(&recent_path),
            recent_path,
            current,
            page: Page::List,
            name: "Новая игра".into(),
            parent: parent.display().to_string(),
            template: Template::Hero,
            browse: parent,
            browse_text: String::new(),
            error: None,
            at_start,
        }
    }

    fn is_current(&self, p: &Path) -> bool {
        self.current
            .as_deref()
            .is_some_and(|c| pf::normalize(c) == pf::normalize(p))
    }
}

impl EditorUi {
    /// Открыть окно «Проекты».
    pub(crate) fn open_projects(&mut self, at_start: bool) {
        let current = self.explorer.project().map(Path::to_path_buf);
        self.projects = Some(ProjectsScreen::new(
            project::recent_path(),
            current,
            at_start,
        ));
    }

    pub(super) fn projects_dialog(&mut self, ctx: &egui::Context, info: &Info) {
        let Some(mut s) = self.projects.take() else {
            return;
        };
        // Другой проект — только без несохранённых правок и не во время игры.
        let busy = if info.dirty || self.code_dirty() {
            Some("сохраните правки (Ctrl+S)")
        } else if info.play != super::PlayState::Edit {
            Some("остановите игру")
        } else {
            None
        };
        let mut open: Option<PathBuf> = None;
        let mut close = false;
        let modal = egui::Modal::new(Id::unique("editor-projects")).show(ctx, |ui| {
            ui.set_width(640.0);
            match s.page.clone() {
                Page::List => list_page(ui, &mut s, busy, &mut open, &mut close),
                Page::Create => create_page(ui, &mut s, busy, &mut open),
                Page::Browse { open: for_open } => browse_page(ui, &mut s, for_open, &mut open),
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                if s.page == Page::List {
                    close = true;
                } else {
                    s.page = Page::List;
                    s.error = None;
                }
            }
        });
        if modal.should_close() && s.page == Page::List {
            close = true;
        }
        if let Some(dir) = open {
            if s.is_current(&dir) {
                close = true;
            } else if let Some(why) = busy {
                s.error = Some(format!("Чтобы открыть другой проект, {why}"));
            } else {
                self.actions.push(Action::OpenProject(dir));
                close = true;
            }
        }
        if !close {
            self.projects = Some(s);
        }
    }
}

fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(FONT_HEADING).color(Color32::WHITE));
}

fn error_line(ui: &mut Ui, s: &ProjectsScreen) {
    if let Some(e) = &s.error {
        ui.add_space(GAP_S);
        ui.label(RichText::new(e).color(BAD));
    }
}

fn list_page(
    ui: &mut Ui,
    s: &mut ProjectsScreen,
    busy: Option<&str>,
    open: &mut Option<PathBuf>,
    close: &mut bool,
) {
    title(ui, "Проекты");
    ui.label(
        RichText::new("Проект — папка со сценами (scenes) и ассетами (assets)")
            .small()
            .color(MUTED),
    );
    ui.add_space(GAP_L);
    let mut forget = None;
    egui::ScrollArea::vertical()
        .max_height(340.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = GAP_XS;
            if s.recent.projects.is_empty() {
                ui.label(RichText::new("Недавних проектов пока нет").color(MUTED));
            }
            for r in s.recent.projects.clone() {
                let alive = project::is_project(&r.path);
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
                let current = s.is_current(&r.path);
                let p = ui.painter();
                let fill = if current {
                    SELECT_BG
                } else if resp.hovered() && alive {
                    HOVER
                } else {
                    CARD
                };
                p.rect_filled(rect, RADIUS_M, fill);
                let name = if alive {
                    project::read(&r.path).name
                } else {
                    pf::file_name(&r.path)
                };
                let x = rect.left() + GAP_L;
                p.text(
                    pos2(x, rect.top() + 16.0),
                    egui::Align2::LEFT_CENTER,
                    &name,
                    egui::FontId::proportional(FONT_BODY + 1.0),
                    if alive { TEXT } else { MUTED },
                );
                let when = when_text(UNIX_EPOCH + Duration::from_secs(r.opened));
                let sub = if alive {
                    format!("{}  ·  {when}", r.path.display())
                } else {
                    format!("{}  ·  папка не найдена", r.path.display())
                };
                p.text(
                    pos2(x, rect.top() + 36.0),
                    egui::Align2::LEFT_CENTER,
                    sub,
                    egui::FontId::proportional(FONT_SMALL),
                    MUTED,
                );
                if current {
                    p.text(
                        pos2(rect.right() - GAP_L, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        "открыт сейчас",
                        egui::FontId::proportional(FONT_SMALL),
                        ACCENT,
                    );
                }
                let resp = resp.on_hover_text(if alive {
                    "Открыть этот проект"
                } else {
                    "Папки нет: её перенесли или удалили"
                });
                if resp.clicked() && alive {
                    *open = Some(r.path.clone());
                }
                resp.context_menu(|ui| {
                    if ui.button("Убрать из списка").clicked() {
                        forget = Some(r.path.clone());
                        ui.close();
                    }
                });
            }
        });
    if let Some(p) = forget {
        s.recent.forget(&p);
        s.recent.save(&s.recent_path);
    }
    error_line(ui, s);
    ui.add_space(GAP_L);
    ui.horizontal(|ui| {
        let b =
            egui::Button::new(RichText::new("+  Создать проект…").color(ON_ACCENT)).fill(ACCENT);
        if ui.add(b).clicked() {
            s.page = Page::Create;
            s.error = None;
        }
        if ui.button("Открыть папку…").clicked() {
            s.page = Page::Browse { open: true };
            s.browse_text.clear();
            s.error = None;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if s.at_start {
                "Продолжить"
            } else {
                "Закрыть"
            };
            if ui.button(label).clicked() {
                *close = true;
            }
        });
    });
    ui.add_space(GAP_M);
    let mut show = !s.recent.skip_start;
    if ui
        .checkbox(&mut show, "Показывать это окно при запуске")
        .on_hover_text("Иначе редактор сразу открывает последний проект")
        .changed()
    {
        s.recent.skip_start = !show;
        s.recent.save(&s.recent_path);
    }
    if let Some(why) = busy {
        ui.label(
            RichText::new(format!("Чтобы открыть другой проект, {why}"))
                .small()
                .color(MUTED),
        );
    }
}

fn create_page(
    ui: &mut Ui,
    s: &mut ProjectsScreen,
    busy: Option<&str>,
    open: &mut Option<PathBuf>,
) {
    title(ui, "Создать проект");
    ui.add_space(GAP_L);
    egui::Grid::new("project-create")
        .num_columns(2)
        .spacing(vec2(GAP_L, GAP_M))
        .show(ui, |ui| {
            ui.label("Имя");
            ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(420.0));
            ui.end_row();
            ui.label("Где");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut s.parent).desired_width(320.0));
                if ui.button("Выбрать…").clicked() {
                    let p = PathBuf::from(s.parent.trim());
                    s.browse = if p.is_dir() {
                        p
                    } else {
                        p.parent()
                            .filter(|q| q.is_dir())
                            .map_or_else(project::default_parent, Path::to_path_buf)
                    };
                    s.browse_text.clear();
                    s.page = Page::Browse { open: false };
                    s.error = None;
                }
            });
            ui.end_row();
        });
    ui.add_space(GAP_L);
    ui.label(RichText::new("С чего начать").color(TEXT));
    ui.add_space(GAP_S);
    ui.horizontal(|ui| {
        for (t, name, about) in [
            (
                Template::Hero,
                "С героем",
                "Герой, пол, ступеньки: F5 — бегать",
            ),
            (Template::Empty, "Пустой", "Только пол и лампа"),
        ] {
            let on = s.template == t;
            let (rect, r) = ui.allocate_exact_size(vec2(300.0, 64.0), Sense::click());
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
            p.text(
                pos2(rect.left() + GAP_L, rect.top() + 20.0),
                egui::Align2::LEFT_CENTER,
                name,
                egui::FontId::proportional(FONT_BODY + 1.0),
                Color32::WHITE,
            );
            p.text(
                pos2(rect.left() + GAP_L, rect.top() + 44.0),
                egui::Align2::LEFT_CENTER,
                about,
                egui::FontId::proportional(FONT_SMALL),
                MUTED,
            );
            if r.clicked() {
                s.template = t;
            }
        }
    });
    let parent = PathBuf::from(s.parent.trim());
    let dir = parent.join(s.name.trim());
    ui.add_space(GAP_L);
    ui.label(
        RichText::new(format!(
            "Будет создана папка {} — в ней scenes, assets и project.ron",
            dir.display()
        ))
        .small()
        .color(MUTED),
    );
    error_line(ui, s);
    ui.add_space(GAP_L);
    ui.horizontal(|ui| {
        let b = egui::Button::new(RichText::new("Создать и открыть").color(ON_ACCENT)).fill(ACCENT);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add(b).clicked() || enter {
            if let Some(why) = busy {
                s.error = Some(format!("Чтобы открыть другой проект, {why}"));
            } else {
                // «Документы\ReyVec» может ещё не быть: её создать можно, глубже — нет.
                if !parent.exists()
                    && parent.parent().is_some_and(Path::is_dir)
                    && let Err(e) = std::fs::create_dir(&parent)
                {
                    s.error = Some(format!("Не создать папку «{}»: {e}", parent.display()));
                    return;
                }
                match project::create(&parent, &s.name, s.template) {
                    Ok(dir) => *open = Some(dir),
                    Err(e) => s.error = Some(e),
                }
            }
        }
        if ui.button("Назад").clicked() {
            s.page = Page::List;
            s.error = None;
        }
    });
}

/// Свой простой выбор папки: путь, «вверх», подпапки (проекты отмечены).
fn browse_page(ui: &mut Ui, s: &mut ProjectsScreen, for_open: bool, open: &mut Option<PathBuf>) {
    title(
        ui,
        if for_open {
            "Открыть проект"
        } else {
            "Где создать проект"
        },
    );
    ui.add_space(GAP_M);
    let mut go: Option<PathBuf> = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(s.browse.parent().is_some(), egui::Button::new("↑ Вверх"))
            .clicked()
        {
            go = s.browse.parent().map(Path::to_path_buf);
        }
        let hint = s.browse.display().to_string();
        let r = ui.add(
            egui::TextEdit::singleline(&mut s.browse_text)
                .hint_text(hint)
                .desired_width(500.0),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let p = PathBuf::from(s.browse_text.trim().trim_matches('"'));
            if p.is_dir() {
                go = Some(p);
            } else {
                s.error = Some(format!("Нет папки «{}»", p.display()));
            }
        }
    });
    ui.label(
        RichText::new("Путь можно вставить из адресной строки проводника Windows и нажать Enter")
            .small()
            .color(MUTED),
    );
    ui.add_space(GAP_M);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&s.browse)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && !pf::file_name(p).starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    dirs.sort_by_key(|p| pf::file_name(p).to_lowercase());
    if s.browse.parent().is_none() && cfg!(windows) {
        // Корень диска: показать и другие диски.
        dirs.extend(
            (b'C'..=b'Z')
                .map(|c| PathBuf::from(format!("{}:\\", c as char)))
                .filter(|p| p.is_dir() && *p != s.browse),
        );
    }
    let mut pick: Option<PathBuf> = None;
    egui::ScrollArea::vertical()
        .max_height(300.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if dirs.is_empty() {
                ui.label(RichText::new("Папок нет").color(MUTED));
            }
            for d in &dirs {
                let is_proj = project::is_project(d);
                let (rect, r) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                if r.hovered() {
                    ui.painter().rect_filled(rect, RADIUS_S, HOVER);
                }
                let name = if d.file_name().is_none() {
                    d.display().to_string()
                } else {
                    pf::file_name(d)
                };
                ui.painter().text(
                    pos2(rect.left() + GAP_M, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(FONT_BODY),
                    TEXT,
                );
                if is_proj {
                    ui.painter().text(
                        pos2(rect.right() - GAP_M, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        "проект ReyVec",
                        egui::FontId::proportional(FONT_SMALL),
                        ACCENT,
                    );
                }
                let r = r.on_hover_text(if for_open && is_proj {
                    "Двойной щелчок — открыть проект"
                } else {
                    "Двойной щелчок — войти в папку"
                });
                if r.double_clicked() {
                    if for_open && is_proj {
                        pick = Some(d.clone());
                    } else {
                        go = Some(d.clone());
                    }
                }
            }
        });
    if let Some(d) = go {
        s.browse = d;
        s.browse_text.clear();
        s.error = None;
    }
    error_line(ui, s);
    ui.add_space(GAP_L);
    ui.horizontal(|ui| {
        let label = if for_open {
            "Открыть эту папку"
        } else {
            "Выбрать эту папку"
        };
        let b = egui::Button::new(RichText::new(label).color(ON_ACCENT)).fill(ACCENT);
        if ui.add(b).clicked() {
            pick = Some(s.browse.clone());
        }
        if ui.button("Назад").clicked() {
            s.page = if for_open { Page::List } else { Page::Create };
            s.error = None;
        }
    });
    if let Some(p) = pick {
        if !for_open {
            s.parent = p.display().to_string();
            s.page = Page::Create;
            s.error = None;
        } else if project::is_project(&p) {
            *open = Some(p);
        } else {
            s.error = Some(format!(
                "В «{}» нет проекта ReyVec: нужна папка scenes или файл project.ron",
                p.display()
            ));
        }
    }
}
