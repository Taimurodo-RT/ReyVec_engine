//! Файлы сцен: список сцен рядом с открытой (меню «Файл → Открыть»), новая сцена и
//! автосохранение в скрытую папку `.autosave` рядом со сценой.

use std::path::{Path, PathBuf};

/// Текст новой сцены: пол-опора и лампа, чтобы сразу было что видеть и на что ставить.
pub(crate) const NEW_SCENE: &str = r##"(
    // Новая сцена. Её можно править в редакторе или здесь, в блокноте.
    format: "reyvec-scene",
    version: 1,
    nodes: [
        (
            name: "пол",
            at: (0.0, -0.45),
            shapes: [
                Rect(at: (0.0, 0.0), size: (2.0, 0.06), color: "#4a4e58", solid: true),
            ],
        ),
        (
            name: "лампа",
            at: (0.0, 0.3),
            lights: [
                Point(at: (0.0, 0.0), color: "#ffd9a0", power: 1.2, reach: 1.0, size: 0.02),
            ],
        ),
    ],
)
"##;

/// Сцены `.ron` в папке, по имени.
pub(crate) fn list_scenes(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ron"))
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    out.sort();
    out
}

/// Уровень проекта для меню «Файл → Открыть уровень»: сцена 2D или 3D.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Level {
    pub path: PathBuf,
    /// Путь от папки уровней через «/» без `.ron`: «3d/склад».
    pub name: String,
    pub three_d: bool,
}

/// Что за уровень в файле: `Some(false)` — сцена 2D, `Some(true)` — сцена 3D, `None` —
/// не уровень.
pub(crate) fn level_kind(path: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(path).ok()?;
    if text.contains("\"reyvec-scene\"") {
        Some(false)
    } else if text.contains("\"reyvec-space\"") {
        Some(true)
    } else {
        None
    }
}

/// Все уровни в папке `dir` и вложенных (кроме скрытых, `models` и `scripts`), по пути.
pub(crate) fn list_levels(dir: &Path) -> Vec<Level> {
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Level>) {
        if depth > 6 || out.len() >= 500 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                if name != "models" && name != "scripts" {
                    walk(root, &p, depth + 1, out);
                }
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ron"))
                && let Some(three_d) = level_kind(&p)
            {
                let rel = p.strip_prefix(root).unwrap_or(&p).with_extension("");
                out.push(Level {
                    name: rel.to_string_lossy().replace('\\', "/"),
                    path: p,
                    three_d,
                });
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out);
    out
}

/// Свободное имя для новой сцены: «новая сцена.ron», «новая сцена 2.ron»…
pub(crate) fn new_scene_path(dir: &Path) -> PathBuf {
    let mut p = dir.join("новая сцена.ron");
    let mut k = 2;
    while p.exists() {
        p = dir.join(format!("новая сцена {k}.ron"));
        k += 1;
    }
    p
}

/// Куда автосохраняется сцена: `.autosave/<имя>` рядом с ней (в Ассетах не видно).
pub(crate) fn autosave_path(scene: &Path) -> Option<PathBuf> {
    let name = scene.file_name()?;
    Some(scene.parent()?.join(".autosave").join(name))
}

/// Записать автосохранение.
pub(crate) fn write_autosave(scene: &Path, text: &str) -> std::io::Result<()> {
    let Some(p) = autosave_path(scene) else {
        return Ok(());
    };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(p, text)
}

/// Автосохранение, которое отличается от сцены на диске (правки не дошли до файла).
pub(crate) fn pending_autosave(scene: &Path, saved: &str) -> Option<String> {
    let text = std::fs::read_to_string(autosave_path(scene)?).ok()?;
    (text != saved).then_some(text)
}

/// Сцену сохранили: автосохранение больше не нужно.
pub(crate) fn drop_autosave(scene: &Path) {
    if let Some(p) = autosave_path(scene) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_2d_and_3d_in_subfolders() {
        let dir = std::env::temp_dir().join(format!("reyvec-levels-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("3d/models")).unwrap();
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("комната.ron"), NEW_SCENE).unwrap();
        std::fs::write(dir.join("3d/зал.ron"), "(format: \"reyvec-space\")").unwrap();
        std::fs::write(dir.join("3d/models/m.ron"), "(format: \"reyvec-space\")").unwrap();
        std::fs::write(dir.join("3d/заметка.ron"), "(просто: 1)").unwrap();
        let levels = list_levels(&dir);
        let names: Vec<_> = levels
            .iter()
            .map(|l| (l.name.as_str(), l.three_d))
            .collect();
        assert_eq!(names, [("3d/зал", true), ("комната", false)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_scene_lists_and_autosaves() {
        reyvec_scene::SceneFile::parse(NEW_SCENE).expect("новая сцена читается");
        let dir = std::env::temp_dir().join(format!("reyvec-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = new_scene_path(&dir);
        std::fs::write(&a, NEW_SCENE).unwrap();
        let b = new_scene_path(&dir);
        assert_ne!(a, b);
        std::fs::write(&b, NEW_SCENE).unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        // По имени: «новая сцена 2» раньше «новая сцена.ron» (пробел меньше точки).
        assert_eq!(list_scenes(&dir), [b, a.clone()]);
        // Автосохранение: видно, пока отличается от файла; после сохранения убирается.
        assert!(pending_autosave(&a, NEW_SCENE).is_none());
        write_autosave(&a, "(правка)").unwrap();
        assert_eq!(list_scenes(&dir).len(), 2, "папка .autosave не сцена");
        assert_eq!(pending_autosave(&a, NEW_SCENE).as_deref(), Some("(правка)"));
        assert!(pending_autosave(&a, "(правка)").is_none());
        drop_autosave(&a);
        assert!(pending_autosave(&a, NEW_SCENE).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
