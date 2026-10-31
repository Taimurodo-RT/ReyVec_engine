//! Библиотека шейдеров: SPIR-V, собранный при компиляции, и перезагрузка на лету.
//!
//! Если рядом есть исходники шейдеров (папка `crates/reyvec-gpu/shaders` в копии
//! репозитория, из которой собрана программа) и найден slangc, движок следит за
//! этими файлами. После сохранения файла шейдер пересобирается, и картинка
//! меняется без перезапуска. Если в шейдере ошибка, продолжает работать прошлая
//! версия, а ошибка видна в заголовке окна, красной рамкой в окне и в логе.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use crate::slangc;

include!(concat!(env!("OUT_DIR"), "/shaders.rs"));

/// Папка с исходниками шейдеров на машине, где собрана программа.
const SOURCE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/shaders");

/// SPIR-V вершинного и пиксельного шейдера одного файла.
#[derive(Clone)]
pub(crate) struct ShaderPair {
    pub vs: Vec<u8>,
    pub fs: Vec<u8>,
}

struct HotReload {
    _watcher: notify::RecommendedWatcher,
    events: Receiver<PathBuf>,
    slangc: PathBuf,
    dir: PathBuf,
}

pub(crate) struct ShaderLibrary {
    shaders: HashMap<String, ShaderPair>,
    hot: Option<HotReload>,
    /// Ошибки компиляции по именам шейдеров. Пусто: всё собралось.
    errors: HashMap<String, String>,
}

impl ShaderLibrary {
    /// Шейдеры из программы. `hot_reload`: следить ли за исходниками.
    pub fn new(hot_reload: bool) -> Self {
        let shaders = BUILTIN
            .iter()
            .map(|(name, vs, fs)| {
                (
                    (*name).to_string(),
                    ShaderPair {
                        vs: vs.to_vec(),
                        fs: fs.to_vec(),
                    },
                )
            })
            .collect();
        let hot = if hot_reload {
            start_watching(Path::new(SOURCE_DIR))
        } else {
            None
        };
        Self {
            shaders,
            hot,
            errors: HashMap::new(),
        }
    }

    pub fn get(&self, name: &str) -> &ShaderPair {
        self.shaders
            .get(name)
            .unwrap_or_else(|| panic!("шейдер «{name}» не собран: нет файла shaders/{name}.slang"))
    }

    /// Следит ли библиотека за файлами шейдеров.
    pub fn is_watching(&self) -> bool {
        self.hot.is_some()
    }

    /// Текст ошибки шейдера, если сейчас хоть один шейдер не собирается.
    pub fn error(&self) -> Option<String> {
        let mut names: Vec<&String> = self.errors.keys().collect();
        names.sort();
        let first = names.first()?;
        Some(format!(
            "{first}.slang: {}",
            first_line(&self.errors[*first])
        ))
    }

    /// Проверяет, менялись ли файлы шейдеров, и пересобирает изменённые.
    /// Возвращает имена шейдеров, у которых теперь новый рабочий SPIR-V.
    pub fn poll(&mut self) -> Vec<String> {
        let Some(hot) = &self.hot else {
            return Vec::new();
        };
        let mut changed = BTreeSet::new();
        while let Ok(path) = hot.events.try_recv() {
            if let Some(name) = shader_name(&path) {
                changed.insert(name);
            }
        }
        let mut updated = Vec::new();
        for name in changed {
            let source = hot.dir.join(format!("{name}.slang"));
            if !source.is_file() {
                continue;
            }
            match compile_pair(&hot.slangc, &source) {
                Ok(pair) => {
                    if self.errors.remove(&name).is_some() {
                        tracing::info!("Шейдер {name}.slang снова собирается");
                    }
                    tracing::info!("Шейдер {name}.slang перезагружен");
                    self.shaders.insert(name.clone(), pair);
                    updated.push(name);
                }
                Err(e) => {
                    tracing::error!("Ошибка в шейдере {name}.slang, работает прошлая версия:\n{e}");
                    self.errors.insert(name, e);
                }
            }
        }
        updated
    }

    /// Отмечает, что новый SPIR-V шейдера не принял драйвер (конвейер не собрался).
    pub fn set_error(&mut self, name: &str, error: String) {
        tracing::error!(
            "Шейдер {name}.slang не принят видеокартой, работает прошлая версия: {error}"
        );
        self.errors.insert(name.to_string(), error);
    }
}

fn compile_pair(slangc_path: &Path, source: &Path) -> Result<ShaderPair, String> {
    let [vs, fs] =
        slangc::ENTRIES.map(|(entry, stage, _)| slangc::compile(slangc_path, source, entry, stage));
    Ok(ShaderPair { vs: vs?, fs: fs? })
}

fn start_watching(dir: &Path) -> Option<HotReload> {
    use notify::Watcher;

    if !dir.is_dir() {
        tracing::info!("Исходников шейдеров нет рядом, перезагрузка шейдеров выключена");
        return None;
    }
    let Some(slangc) = slangc::find_slangc() else {
        tracing::warn!("slangc не найден (нужен Vulkan SDK), перезагрузка шейдеров выключена");
        return None;
    };
    let (tx, rx) = channel();
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && (event.kind.is_modify() || event.kind.is_create())
        {
            for path in event.paths {
                let _ = tx.send(path);
            }
        }
    });
    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!("Не удалось следить за шейдерами: {e}");
            return None;
        }
    };
    if let Err(e) = watcher.watch(dir, notify::RecursiveMode::NonRecursive) {
        tracing::warn!("Не удалось следить за папкой {}: {e}", dir.display());
        return None;
    }
    tracing::info!(
        "Слежу за шейдерами в {}: после сохранения файла картинка обновится сама",
        dir.display()
    );
    Some(HotReload {
        _watcher: watcher,
        events: rx,
        slangc,
        dir: dir.to_path_buf(),
    })
}

fn shader_name(path: &Path) -> Option<String> {
    if path.extension()? != "slang" {
        return None;
    }
    Some(path.file_stem()?.to_str()?.to_string())
}

/// Первая строка с текстом ошибки: её показываем в заголовке окна.
fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| l.contains("error"))
        .or_else(|| text.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("неизвестная ошибка")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_shaders_are_spirv() {
        let lib = ShaderLibrary::new(false);
        for name in ["triangle", "sprite"] {
            let pair = lib.get(name);
            for spv in [&pair.vs, &pair.fs] {
                assert!(spv.len() > 20 && spv.len() % 4 == 0, "{name}");
                assert_eq!(&spv[..4], &0x0723_0203u32.to_le_bytes(), "{name}");
            }
        }
    }

    #[test]
    fn names_from_paths() {
        assert_eq!(
            shader_name(Path::new("/a/sprite.slang")),
            Some("sprite".into())
        );
        assert_eq!(shader_name(Path::new("/a/sprite.slang~")), None);
        assert_eq!(shader_name(Path::new("/a/notes.txt")), None);
    }

    #[test]
    fn error_line_prefers_error() {
        let text = "sprite.slang(5): warning: x\nsprite.slang(7): error 30015: undefined identifier 'foo'\n  foo;";
        assert_eq!(
            first_line(text),
            "sprite.slang(7): error 30015: undefined identifier 'foo'"
        );
    }

    /// Нужен slangc: правка файла шейдера на диске пересобирает его, ошибка не ломает прошлую версию.
    #[test]
    #[ignore = "нужен slangc"]
    fn hot_reload_compiles_and_reports_errors() {
        let dir = std::env::temp_dir().join(format!("reyvec-hot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = std::fs::read_to_string(Path::new(SOURCE_DIR).join("triangle.slang")).unwrap();
        let file = dir.join("triangle.slang");
        std::fs::write(&file, &src).unwrap();

        let mut lib = ShaderLibrary::new(false);
        lib.hot = start_watching(&dir);
        assert!(lib.is_watching());
        let old = lib.get("triangle").vs.clone();

        let wait = |lib: &mut ShaderLibrary| {
            for _ in 0..100 {
                let changed = lib.poll();
                if !changed.is_empty() || lib.error().is_some() {
                    return changed;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Vec::new()
        };

        std::fs::write(&file, src.replace("pc.time * 0.5", "pc.time * 2.0")).unwrap();
        assert_eq!(wait(&mut lib), vec!["triangle".to_string()]);
        assert_ne!(lib.get("triangle").vs, old);
        let good = lib.get("triangle").vs.clone();

        std::fs::write(&file, src.replace("pc.time * 0.5", "pc.tiem * 0.5")).unwrap();
        assert!(wait(&mut lib).is_empty());
        let err = lib.error().expect("ошибка видна");
        assert!(err.starts_with("triangle.slang:"), "{err}");
        assert_eq!(lib.get("triangle").vs, good, "работает прошлая версия");

        std::fs::write(&file, &src).unwrap();
        assert_eq!(wait(&mut lib), vec!["triangle".to_string()]);
        assert!(lib.error().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
