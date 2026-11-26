//! Ссылки по именам во вкладке «Сюжет» (Сю.8): где имя встречается в скриптах проекта,
//! окно безопасного переименования и поиск по всем текстам сюжета.
//!
//! Само переименование — в `reyvec_story::rename`: здесь только то, что видит автор, и
//! файлы проекта (скрипты `.luau`, которые нужно поправить вместе с сюжетом).

use std::path::{Path, PathBuf};

use egui::{Align2, FontId, RichText, TextEdit};
use reyvec_story::rename::{Kind, luau_lines};
use reyvec_story::{Node, Story};

use super::story_ui::Item;
use super::theme::*;

/// Скрипт проекта: путь и текст.
pub(super) type Script = (PathBuf, String);

/// Папки, где скриптов не бывает (сборка, история, сам сюжет).
const SKIP: [&str; 5] = ["target", ".git", "story", "node_modules", ".cargo"];

/// Все `.luau` проекта (не глубже 6 папок).
pub(super) fn project_scripts(project: &Path) -> Vec<Script> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<Script>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
                if depth < 6 && name.is_some_and(|n| !SKIP.contains(&n.as_str())) {
                    walk(&p, depth + 1, out);
                }
            } else if p.extension().is_some_and(|e| e == "luau")
                && let Ok(text) = std::fs::read_to_string(&p)
            {
                out.push((p, text));
            }
        }
    }
    let mut out = Vec::new();
    walk(project, 0, &mut out);
    out
}

/// Имя заметки для кода: вид и имя (у «Мира» и переводов имени нет).
pub(super) fn name_of(item: &Item) -> Option<(Kind, &str)> {
    match item {
        Item::Dialog(id) => Some((Kind::Dialog, id)),
        Item::Quest(id) => Some((Kind::Quest, id)),
        Item::Character(id) => Some((Kind::Character, id)),
        Item::Vars | Item::Lang(_) => None,
    }
}

/// Место в скрипте: файл, строка (с 1), текст строки.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScriptUse {
    pub path: PathBuf,
    pub line: usize,
    pub text: String,
}

/// Где имя встречается в скриптах.
pub(super) fn script_uses(scripts: &[Script], kind: Kind, id: &str) -> Vec<ScriptUse> {
    scripts
        .iter()
        .flat_map(|(path, text)| {
            luau_lines(text, kind, id)
                .into_iter()
                .map(|(line, text)| ScriptUse {
                    path: path.clone(),
                    line,
                    text,
                })
        })
        .collect()
}

/// Открытое окно переименования.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Rename {
    pub kind: Kind,
    pub old: String,
    pub new: String,
}

impl Rename {
    pub(super) fn new(kind: Kind, old: &str) -> Self {
        Self {
            kind,
            old: old.into(),
            new: old.into(),
        }
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Dialog => "диалог",
        Kind::Quest => "задание",
        Kind::Character => "персонажа",
        Kind::Var => "переменную",
    }
}

/// Что ответило окно переименования.
pub(super) enum RenameAnswer {
    Keep,
    Cancel,
    Apply,
}

/// Окно «Переименовать»: новое имя, почему нельзя, что поменяется.
pub(super) fn rename_window(
    ctx: &egui::Context,
    r: &mut Rename,
    story: &Story,
    scripts: &[Script],
    project: Option<&Path>,
) -> RenameAnswer {
    let mut answer = RenameAnswer::Keep;
    let check = story.can_rename(r.kind, &r.old, r.new.trim());
    let uses = script_uses(scripts, r.kind, &r.old);
    egui::Window::new(format!("Переименовать {}", kind_name(r.kind)))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(format!("Сейчас: {}", r.old)).color(TEXT_DIM));
            let edit = ui.add(
                TextEdit::singleline(&mut r.new)
                    .font(FontId::monospace(FONT_MONO))
                    .desired_width(f32::INFINITY)
                    .margin(PAD_FIELD),
            );
            if !edit.has_focus() && r.new == r.old {
                edit.request_focus();
            }
            match &check {
                Err(e) if r.new.trim() != r.old => {
                    ui.label(RichText::new(e).color(BAD).size(FONT_SMALL));
                }
                _ => {}
            }
            ui.add_space(GAP_M);
            ui.label(
                RichText::new(
                    "Имя поменяется везде: в других диалогах, условиях, заданиях, переводах и \
                     скриптах.",
                )
                .color(MUTED)
                .size(FONT_SMALL),
            );
            if !uses.is_empty() {
                ui.label(RichText::new(format!("В скриптах: {}", uses.len())).color(TEXT_DIM));
                for u in uses.iter().take(6) {
                    let file = project
                        .and_then(|p| u.path.strip_prefix(p).ok())
                        .unwrap_or(&u.path);
                    ui.label(
                        RichText::new(format!("{}:{}  {}", file.display(), u.line, u.text))
                            .monospace()
                            .color(MUTED)
                            .size(FONT_SMALL),
                    );
                }
            }
            ui.add_space(GAP_M);
            ui.horizontal(|ui| {
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui
                    .add_enabled(check.is_ok(), egui::Button::new("Переименовать"))
                    .clicked()
                    || (enter && check.is_ok())
                {
                    answer = RenameAnswer::Apply;
                }
                if ui.button("Отмена").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    answer = RenameAnswer::Cancel;
                }
            });
        });
    answer
}

/// Найденное поиском по тексту: заметка, место (карточка, стадия) и строка с находкой.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Hit {
    pub item: Item,
    pub place: String,
    pub text: String,
}

/// Поиск по всем текстам сюжета: реплики, варианты, условия, журнал, характеры,
/// переводы, имена. Без учёта регистра.
pub(super) fn search(story: &Story, query: &str) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let mut out = Vec::new();
    if q.chars().count() < 2 {
        return out;
    }
    let mut add = |item: &Item, place: &str, text: &str| {
        if text.to_lowercase().contains(&q) {
            out.push(Hit {
                item: item.clone(),
                place: place.into(),
                text: text.into(),
            });
        }
    };
    for name in story.vars.keys() {
        add(&Item::Vars, "", name);
    }
    for c in story.characters.values() {
        let it = Item::Character(c.id.clone());
        add(&it, "", &c.name);
        add(&it, "", &c.id);
        for line in c.about.lines() {
            add(&it, "", line);
        }
    }
    for d in story.dialogs.values() {
        let it = Item::Dialog(d.id.clone());
        add(&it, "", &d.title);
        add(&it, "", &d.id);
        for n in &d.nodes {
            let place = n.id();
            match n {
                Node::Line { text, choices, .. } => {
                    add(&it, place, text);
                    for c in choices {
                        add(&it, place, &c.text);
                        add(&it, place, &c.when);
                    }
                }
                Node::Generate { task, fallback, .. } => {
                    add(&it, place, task);
                    add(&it, place, fallback);
                }
                Node::Check { when, .. } => add(&it, place, when),
                Node::Event { name, .. } => add(&it, place, name),
                Node::Run { func, .. } => add(&it, place, func),
                Node::Jump { dialog, .. } => add(&it, place, dialog),
                _ => {}
            }
        }
    }
    for q in story.quests.values() {
        let it = Item::Quest(q.id.clone());
        add(&it, "", &q.title);
        add(&it, "", &q.id);
        add(&it, "", &q.description);
        add(&it, "", &q.fail_when);
        for s in &q.stages {
            add(&it, &s.id, &s.journal);
            add(&it, &s.id, &s.done_when);
            for g in &s.goals {
                add(&it, &s.id, &g.text);
            }
        }
    }
    for (code, words) in &story.lang {
        let it = Item::Lang(code.clone());
        let mut keys: Vec<_> = words.iter().collect();
        keys.sort();
        for (k, v) in keys {
            add(&it, k, v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Story {
        Story::from_texts(reyvec_story::EXAMPLE).0
    }

    #[test]
    fn search_finds_lines_journal_and_translations() {
        let s = example();
        let hits = search(&s, "РУДУ");
        assert!(
            hits.iter()
                .any(|h| h.item == Item::Dialog("kuznec.privet".into())
                    && h.text == "Я принёс руду."),
            "{hits:?}"
        );
        assert!(hits.iter().any(|h| matches!(h.item, Item::Quest(_))));
        assert!(!search(&s, "ore").is_empty(), "перевод");
        assert!(search(&s, "р").is_empty(), "одна буква — не ищем");
    }

    #[test]
    fn scripts_of_the_example_project_are_found() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scripts = project_scripts(&root);
        let uses = script_uses(&scripts, Kind::Dialog, "kuznec.ruda");
        assert!(
            uses.iter()
                .any(|u| u.path.ends_with("scenes/scripts/kuznec.luau")),
            "{uses:?}"
        );
        assert!(!script_uses(&scripts, Kind::Quest, "mech").is_empty());
    }
}
