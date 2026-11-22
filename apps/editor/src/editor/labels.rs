//! Цветные метки файлов (Э.5): у файла или папки может быть несколько меток из семи
//! цветов, как в проводнике macOS. Названия меток свои у проекта («Готово», «Переделать»).
//!
//! Всё лежит одним файлом `.metki.ron` в папке проекта: пути от проекта через «/» и номера
//! меток. Переименовали, перенесли, удалили в корзину, вернули или скопировали файл —
//! [`super::project_files`] сам переносит его метки (и отмена тоже).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::Color32;
use serde::{Deserialize, Serialize};

/// Файл с метками в папке проекта (скрытый: проводник не показывает имена с точкой).
pub(crate) const FILE: &str = ".metki.ron";
/// Сколько всего меток.
pub(crate) const COUNT: usize = 7;
pub(crate) const COLORS: [Color32; COUNT] = [
    Color32::from_rgb(244, 91, 91),
    Color32::from_rgb(247, 152, 62),
    Color32::from_rgb(240, 204, 64),
    Color32::from_rgb(98, 200, 106),
    Color32::from_rgb(76, 196, 230),
    Color32::from_rgb(84, 128, 240),
    Color32::from_rgb(176, 110, 232),
];
const NAMES: [&str; COUNT] = [
    "Красная",
    "Оранжевая",
    "Жёлтая",
    "Зелёная",
    "Голубая",
    "Синяя",
    "Фиолетовая",
];

/// Метки проекта.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Labels {
    /// Свои названия меток; пустое — название по цвету.
    #[serde(default)]
    names: Vec<String>,
    /// Путь от проекта → номера меток по порядку.
    #[serde(default)]
    files: BTreeMap<String, Vec<u8>>,
}

impl Labels {
    /// Прочитать метки проекта (нет файла или он испорчен — пусто).
    pub(crate) fn load(project: &Path) -> Self {
        std::fs::read_to_string(project.join(FILE))
            .ok()
            .and_then(|t| ron::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save(&self, project: &Path) -> Result<(), String> {
        let path = project.join(FILE);
        if self.files.is_empty() && self.names.iter().all(String::is_empty) {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| format!("Метки не сохранены: {e}"))?;
            }
            return Ok(());
        }
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| format!("Метки не сохранены: {e}"))?;
        std::fs::write(&path, text).map_err(|e| format!("Метки не сохранены: {e}"))
    }

    /// Название метки.
    pub(crate) fn name(&self, i: usize) -> String {
        match self.names.get(i) {
            Some(n) if !n.trim().is_empty() => n.trim().to_owned(),
            _ => NAMES[i].to_owned(),
        }
    }

    pub(crate) fn rename(&mut self, i: usize, name: &str) {
        if self.names.len() < COUNT {
            self.names.resize(COUNT, String::new());
        }
        self.names[i] = if name.trim() == NAMES[i] {
            String::new()
        } else {
            name.trim().to_owned()
        };
        while self.names.last().is_some_and(String::is_empty) {
            self.names.pop();
        }
    }

    /// Метки файла.
    pub(crate) fn of(&self, project: &Path, path: &Path) -> &[u8] {
        rel(project, path)
            .and_then(|r| self.files.get(&r))
            .map_or(&[], Vec::as_slice)
    }

    /// Есть ли метка `i` у всех `paths`.
    pub(crate) fn all_have(&self, project: &Path, paths: &[PathBuf], i: u8) -> bool {
        !paths.is_empty() && paths.iter().all(|p| self.of(project, p).contains(&i))
    }

    /// Поставить (`on`) или снять метку `i` у файлов.
    pub(crate) fn set(&mut self, project: &Path, paths: &[PathBuf], i: u8, on: bool) {
        for p in paths {
            let Some(r) = rel(project, p) else {
                continue;
            };
            let v = self.files.entry(r.clone()).or_default();
            v.retain(|&x| x != i);
            if on {
                v.push(i);
                v.sort_unstable();
            }
            if v.is_empty() {
                self.files.remove(&r);
            }
        }
    }

    /// Файлы с меткой `i` (пути от проекта), кроме лежащих в корзине.
    pub(crate) fn with(&self, project: &Path, i: u8) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|(r, v)| v.contains(&i) && !r.starts_with('.'))
            .map(|(r, _)| project.join(r))
            .collect()
    }

    /// Сколько файлов с каждой меткой (без корзины).
    pub(crate) fn counts(&self) -> [usize; COUNT] {
        let mut n = [0; COUNT];
        for (r, v) in &self.files {
            if r.starts_with('.') {
                continue;
            }
            for &i in v {
                if let Some(c) = n.get_mut(i as usize) {
                    *c += 1;
                }
            }
        }
        n
    }

    /// Метки уехали вместе с файлами: `(откуда, куда)`; папка уносит и всё, что в ней.
    /// С `copy` метки остаются и на старом месте.
    fn carry(&mut self, project: &Path, moves: &[(PathBuf, PathBuf)], copy: bool) -> bool {
        let mut changed = false;
        for (from, to) in moves {
            let (Some(f), Some(t)) = (rel(project, from), rel(project, to)) else {
                continue;
            };
            let hit: Vec<String> = self
                .files
                .keys()
                .filter(|k| under(k, &f))
                .cloned()
                .collect();
            for k in hit {
                let v = if copy {
                    self.files[&k].clone()
                } else {
                    self.files.remove(&k).unwrap_or_default()
                };
                self.files.insert(format!("{t}{}", &k[f.len()..]), v);
                changed = true;
            }
        }
        changed
    }

    /// Забыть метки стёртого насовсем.
    fn drop_under(&mut self, project: &Path, paths: &[PathBuf]) -> bool {
        let before = self.files.len();
        for p in paths {
            if let Some(r) = rel(project, p) {
                self.files.retain(|k, _| !under(k, &r));
            }
        }
        self.files.len() != before
    }
}

/// Файлы перенесли (или удалили в корзину, или вернули): метки едут следом.
pub(crate) fn follow(project: &Path, moves: &[(PathBuf, PathBuf)]) {
    update(project, |l| l.carry(project, moves, false));
}

/// Файлы скопировали: у копий те же метки.
pub(crate) fn copied(project: &Path, pairs: &[(PathBuf, PathBuf)]) {
    update(project, |l| l.carry(project, pairs, true));
}

/// Стёрли насовсем.
pub(crate) fn erased(project: &Path, paths: &[PathBuf]) {
    update(project, |l| l.drop_under(project, paths));
}

fn update(project: &Path, f: impl FnOnce(&mut Labels) -> bool) {
    if !project.join(FILE).exists() {
        return;
    }
    let mut l = Labels::load(project);
    if f(&mut l)
        && let Err(e) = l.save(project)
    {
        tracing::warn!("{e}");
    }
}

/// `k` — это `dir` или лежит в нём.
fn under(k: &str, dir: &str) -> bool {
    let (a, b) = if cfg!(windows) {
        (k.to_lowercase(), dir.to_lowercase())
    } else {
        (k.to_owned(), dir.to_owned())
    };
    a == b || (a.starts_with(&b) && a.as_bytes().get(b.len()) == Some(&b'/'))
}

/// Путь от проекта через «/».
fn rel(project: &Path, p: &Path) -> Option<String> {
    let p = super::project_files::normalize(p);
    let r = p
        .strip_prefix(super::project_files::normalize(project))
        .ok()?;
    let s = r.to_string_lossy().replace('\\', "/");
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_follow_copy_and_erase() {
        let dir = std::env::temp_dir().join(format!("reyvec-labels-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = |s: &str| dir.join(s);
        let mut l = Labels::default();
        l.set(&dir, &[p("assets/a.svg"), p("assets/d")], 2, true);
        l.set(&dir, &[p("assets/d/x.png")], 0, true);
        l.set(&dir, &[p("assets/a.svg")], 5, true);
        l.set(&dir, &[p("assets/a.svg")], 2, false);
        assert_eq!(l.of(&dir, &p("assets/a.svg")), &[5]);
        assert!(l.all_have(&dir, &[p("assets/d")], 2));
        assert!(!l.all_have(&dir, &[p("assets/d"), p("assets/a.svg")], 2));
        l.rename(1, "  Переделать ");
        assert_eq!(l.name(1), "Переделать");
        assert_eq!(l.name(3), "Зелёная");
        l.save(&dir).unwrap();
        assert_eq!(Labels::load(&dir), l);

        // Папка переехала — её метка и метки внутри тоже; похожее имя «dd» не задето.
        let mut l2 = l.clone();
        l2.set(&dir, &[p("assets/dd")], 4, true);
        l2.save(&dir).unwrap();
        follow(&dir, &[(p("assets/d"), p("scenes/d2"))]);
        let l3 = Labels::load(&dir);
        assert_eq!(l3.of(&dir, &p("scenes/d2/x.png")), &[0]);
        assert_eq!(l3.of(&dir, &p("scenes/d2")), &[2]);
        assert!(l3.of(&dir, &p("assets/d")).is_empty());
        assert_eq!(l3.of(&dir, &p("assets/dd")), &[4]);

        // Корзина: метки уехали туда и в списках не видны; копия — с теми же метками.
        follow(&dir, &[(p("assets/a.svg"), p(".trash/1/a.svg"))]);
        copied(&dir, &[(p("assets/dd"), p("assets/dd - копия"))]);
        let l4 = Labels::load(&dir);
        assert!(l4.with(&dir, 5).is_empty());
        assert_eq!(l4.of(&dir, &p(".trash/1/a.svg")), &[5]);
        assert_eq!(l4.of(&dir, &p("assets/dd - копия")), &[4]);
        assert_eq!(l4.counts()[4], 2);
        erased(&dir, &[p(".trash/1/a.svg")]);
        assert!(Labels::load(&dir).of(&dir, &p(".trash/1/a.svg")).is_empty());

        // Сняли всё — файл меток пропадает.
        let mut empty = Labels::load(&dir);
        empty.files.clear();
        empty.names.clear();
        empty.save(&dir).unwrap();
        assert!(!dir.join(FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
