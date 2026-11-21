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
