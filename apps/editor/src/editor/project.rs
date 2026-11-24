//! Проекты (П.1): папка с `scenes`, `assets` и файлом `project.ron`, создание нового
//! проекта из заготовки и список недавних.
//!
//! Проект — это просто папка: в ней `scenes` (сцены и скрипты), `assets` (картинки, звук,
//! заготовки) и `project.ron` с именем проекта и первой сценой. Папка без `project.ron`,
//! но со `scenes` тоже считается проектом (так устроен пример движка до П.1). Список
//! недавних проектов лежит рядом с программой: `settings/projects.ron`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::project_files as pf;

/// Файл проекта в его папке.
pub(crate) const FILE: &str = "project.ron";
/// Сколько недавних проектов помнить.
const RECENT_MAX: usize = 12;

/// Что записано в `project.ron`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProjectFile {
    /// Имя проекта (видно в заголовке окна и в списке проектов).
    pub name: String,
    /// Какую сцену открывать первой: путь от папки проекта.
    #[serde(default)]
    pub first_scene: Option<String>,
    /// Версией какого движка создан (для будущих переходов между версиями).
    #[serde(default)]
    pub engine: String,
}

/// Папка — проект ReyVec.
pub(crate) fn is_project(dir: &Path) -> bool {
    dir.join(FILE).is_file() || dir.join("scenes").is_dir()
}

/// Прочитать `project.ron`; нет его или испорчен — имя по папке.
pub(crate) fn read(dir: &Path) -> ProjectFile {
    let file = std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|t| ron::from_str::<ProjectFile>(&t).ok());
    let mut f = file.unwrap_or_default();
    if f.name.trim().is_empty() {
        f.name = pf::file_name(dir);
    }
    f
}

/// Сцена, с которой открыть проект: из `project.ron`, иначе `scenes/first-room.ron`,
/// иначе первая сцена 2D в `scenes`.
pub(crate) fn first_scene(dir: &Path) -> Option<PathBuf> {
    if let Some(rel) = read(dir).first_scene {
        let p = dir.join(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    let room = dir.join("scenes").join("first-room.ron");
    if room.is_file() {
        return Some(room);
    }
    super::files::list_scenes(&dir.join("scenes"))
        .into_iter()
        .find(|p| std::fs::read_to_string(p).is_ok_and(|t| t.contains("\"reyvec-scene\"")))
}

/// С чего начать новый проект.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Template {
    /// Пол и лампа.
    Empty,
    /// Пол, ступеньки, лампа и герой: можно сразу нажать F5 и бегать.
    Hero,
}

/// Имя первой сцены нового проекта.
pub(crate) const FIRST_SCENE: &str = "первая сцена.ron";

/// Создать проект `name` в папке `parent`. Возвращает папку проекта.
pub(crate) fn create(parent: &Path, name: &str, template: Template) -> Result<PathBuf, String> {
    let name = name.trim();
    pf::check_name(name)?;
    if !parent.is_dir() {
        return Err(format!("Нет папки «{}»", parent.display()));
    }
    let dir = parent.join(name);
    if dir.exists() {
        return Err(format!("В «{}» уже есть «{name}»", pf::file_name(parent)));
    }
    let made = (|| -> std::io::Result<()> {
        for sub in [
            "scenes",
            "scenes/scripts",
            "assets",
            "assets/svg",
            "assets/prefabs",
        ] {
            std::fs::create_dir_all(dir.join(sub))?;
        }
        let scene = match template {
            Template::Empty => super::files::NEW_SCENE,
            Template::Hero => HERO_SCENE,
        };
        std::fs::write(dir.join("scenes").join(FIRST_SCENE), scene)?;
        let file = ProjectFile {
            name: name.to_owned(),
            first_scene: Some(format!("scenes/{FIRST_SCENE}")),
            engine: env!("CARGO_PKG_VERSION").to_owned(),
        };
        let text = ron::ser::to_string_pretty(&file, ron::ser::PrettyConfig::default())
            .map_err(std::io::Error::other)?;
        std::fs::write(
            dir.join(FILE),
            format!("// Проект ReyVec. Имя и первую сцену можно поменять здесь.\n{text}\n"),
        )
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("Не создать проект «{name}»: {e}"));
    }
    Ok(dir)
}

/// Куда класть новые проекты по умолчанию: «Документы\ReyVec».
pub(crate) fn default_parent() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let docs = ["Documents", "Документы"]
        .iter()
        .map(|d| home.join(d))
        .find(|p| p.is_dir())
        .unwrap_or(home);
    docs.join("ReyVec")
}

/// Недавний проект.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecentProject {
    pub path: PathBuf,
    /// Когда открывали: секунды от 1970 года.
    pub opened: u64,
}

/// Недавние проекты и настройка стартового окна.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Recent {
    #[serde(default)]
    pub projects: Vec<RecentProject>,
    /// Не показывать стартовое окно: сразу открывать последний проект.
    #[serde(default)]
    pub skip_start: bool,
}

/// Где лежит список недавних: `settings/projects.ron` рядом с программой.
pub(crate) fn recent_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join("settings")
        .join("projects.ron")
}

impl Recent {
    pub(crate) fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| ron::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save(&self, path: &Path) {
        let text = match ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()) {
            Ok(t) => t,
            Err(e) => return tracing::warn!("Список проектов не записан: {e}"),
        };
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, text));
        if let Err(e) = written {
            tracing::warn!("Список проектов не записан в «{}»: {e}", path.display());
        }
    }

    /// Проект открыли: наверх списка.
    pub(crate) fn touch(&mut self, dir: &Path) {
        let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        let key = |p: &Path| pf::normalize(p).to_string_lossy().to_lowercase();
        self.projects.retain(|r| key(&r.path) != key(&dir));
        let opened = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.projects.insert(
            0,
            RecentProject {
                path: pf::normalize(&dir),
                opened,
            },
        );
        self.projects.truncate(RECENT_MAX);
    }

    /// Последний открытый проект, если его папка ещё на месте.
    pub(crate) fn last(&self) -> Option<&Path> {
        self.projects
            .iter()
            .map(|r| r.path.as_path())
            .find(|p| is_project(p))
    }

    /// Убрать из списка (сама папка не трогается).
    pub(crate) fn forget(&mut self, dir: &Path) {
        self.projects.retain(|r| r.path != dir);
    }
}

/// Новый проект с героем: пол, ступеньки, ящик, лампа и герой без костей.
pub(crate) const HERO_SCENE: &str = r##"(
    // Первая сцена проекта. F5 — играть: стрелки — бег, пробел — прыжок, X — удар.
    // Её можно править в редакторе («Мир») или здесь, в блокноте.
    format: "reyvec-scene",
    version: 1,
    background: Lab,
    ambient: "#3c4048",
    nodes: [
        (
            name: "герой",
            at: (-0.6, -0.2),
            depth: 3,
            shadow: true,
            player: (size: (0.14, 0.28), speed: 1.6, jump: 0.6, jump_time: 0.36, hp: 5),
            shapes: [
                Rect(at: (0.0, -0.02), size: (0.12, 0.2), corner: 0.04, color: "#3b5f8a"),
                Rect(at: (0.0, 0.1), size: (0.1, 0.09), corner: 0.04, color: "#e8c39e"),
                Rect(at: (0.0, 0.045), size: (0.13, 0.03), corner: 0.012, color: "#c0392b"),
            ],
        ),
        (
            name: "пол",
            at: (0.0, -0.45),
            shapes: [
                Rect(at: (0.0, 0.0), size: (4.0, 0.1), color: "#4a4e58", solid: true),
            ],
        ),
        (
            name: "ступенька",
            at: (0.45, -0.33),
            shapes: [
                Rect(at: (0.0, 0.0), size: (0.4, 0.14), corner: 0.02, color: "#5b606c", solid: true),
            ],
        ),
        (
            name: "полка",
            at: (1.1, -0.12),
            shapes: [
                Rect(at: (0.0, 0.0), size: (0.5, 0.06), corner: 0.02, color: "#5b606c", solid: true),
            ],
        ),
        (
            name: "ящик",
            at: (-1.2, -0.32),
            shapes: [
                Rect(at: (0.0, 0.0), size: (0.16, 0.16), corner: 0.015, color: "#a06030", solid: true),
            ],
        ),
        (
            name: "лампа",
            at: (0.0, 0.45),
            lights: [
                Point(at: (0.0, 0.0), color: "#ffd9a0", power: 1.4, reach: 1.6, size: 0.03),
            ],
        ),
    ],
)
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use reyvec_scene::SceneFile;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("reyvec-project-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn create_read_and_first_scene() {
        let parent = tmp("create");
        let dir = create(&parent, " Моя игра ", Template::Hero).unwrap();
        assert_eq!(dir, parent.join("Моя игра"));
        assert!(is_project(&dir));
        let f = read(&dir);
        assert_eq!(f.name, "Моя игра");
        assert_eq!(f.engine, env!("CARGO_PKG_VERSION"));
        let scene = first_scene(&dir).unwrap();
        assert_eq!(scene, dir.join("scenes").join(FIRST_SCENE));
        assert!(dir.join("assets/prefabs").is_dir());
        // Заготовки — настоящие сцены, у «с героем» есть герой.
        let hero = SceneFile::parse(&std::fs::read_to_string(&scene).unwrap()).unwrap();
        assert!(hero.nodes.iter().any(|n| n.player.is_some()));
        SceneFile::parse(super::super::files::NEW_SCENE).unwrap();

        // Второй раз то же имя — ошибка; плохое имя — ошибка; папки не остаётся.
        assert!(create(&parent, "Моя игра", Template::Empty).is_err());
        assert!(create(&parent, "a/b", Template::Empty).is_err());
        assert!(!parent.join("a").exists());

        // Без project.ron: имя по папке, сцена — первая сцена 2D в scenes.
        let old = parent.join("старый");
        std::fs::create_dir_all(old.join("scenes")).unwrap();
        std::fs::write(old.join("scenes/b.ron"), super::super::files::NEW_SCENE).unwrap();
        std::fs::write(old.join("scenes/a.ron"), "(format: \"reyvec-space\")").unwrap();
        assert!(is_project(&old));
        assert_eq!(read(&old).name, "старый");
        assert_eq!(first_scene(&old), Some(old.join("scenes/b.ron")));
        assert!(!is_project(&parent));
        let _ = std::fs::remove_dir_all(&parent);
    }

    /// Герой заготовки падает на пол и стоит на нём.
    #[test]
    fn hero_template_stands_on_floor() {
        let file = SceneFile::parse(HERO_SCENE).unwrap();
        let mut lib = reyvec_vector::ShapeLibrary::new(0.001);
        let mut w =
            reyvec_scene::SceneWorld::new(&file, &mut lib, &mut reyvec_scene::ShapeCache::new())
                .unwrap();
        for i in 0..120 {
            w.update_at(i as f64 / 60.0);
        }
        let p = w.snapshot().player.expect("герой есть");
        assert!(p.on_ground, "{:?}", p.at);
        // Верх пола −0,4, половина роста героя 0,14.
        assert!((p.at[1] - -0.26).abs() < 1e-3, "{:?}", p.at);
    }

    #[test]
    fn recent_list() {
        let d = tmp("recent");
        let a = d.join("a");
        let b = d.join("b");
        for p in [&a, &b] {
            std::fs::create_dir_all(p.join("scenes")).unwrap();
        }
        let path = d.join("settings/projects.ron");
        let mut r = Recent::load(&path);
        assert!(r.projects.is_empty());
        r.touch(&a);
        r.touch(&b);
        r.touch(&a);
        assert_eq!(r.projects.len(), 2);
        assert_eq!(r.last(), Some(a.as_path()));
        r.skip_start = true;
        r.save(&path);
        let mut back = Recent::load(&path);
        assert_eq!(back, r);
        // Папку убрали — последним считается следующий живой проект.
        std::fs::remove_dir_all(&a).unwrap();
        assert_eq!(back.last(), Some(b.as_path()));
        back.forget(&b);
        assert_eq!(back.projects.len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
