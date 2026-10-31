// Поиск и запуск компилятора шейдеров Slang (slangc).
// Этот файл подключается и в build.rs (сборка шейдеров при компиляции движка),
// и в сам движок (перекомпиляция на лету при правке шейдера).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Точки входа, которые движок ищет в каждом файле `.slang`: имя функции и стадия.
pub const ENTRIES: [(&str, &str, &str); 2] =
    [("vs_main", "vertex", "vs"), ("fs_main", "fragment", "fs")];

/// Ищет slangc: переменная `REYVEC_SLANGC`, потом Vulkan SDK, потом `PATH`.
pub fn find_slangc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("REYVEC_SLANGC") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = if cfg!(windows) {
        "slangc.exe"
    } else {
        "slangc"
    };
    if let Some(sdk) = std::env::var_os("VULKAN_SDK") {
        let sdk = PathBuf::from(sdk);
        for bin in ["Bin", "bin"] {
            let p = sdk.join(bin).join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
}

/// Компилирует одну точку входа файла `source` в SPIR-V. При ошибке возвращает текст
/// сообщения компилятора (с номером строки), чтобы его можно было показать человеку.
pub fn compile(slangc: &Path, source: &Path, entry: &str, stage: &str) -> Result<Vec<u8>, String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let out = std::env::temp_dir().join(format!(
        "reyvec-{}-{}-{entry}.spv",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let output = Command::new(slangc)
        .arg(source)
        .args(["-target", "spirv", "-profile", "spirv_1_6"])
        // 41012: «профиль расширен автоматически», для Vulkan 1.3 это нормально.
        .args(["-warnings-disable", "41012"])
        .args(["-entry", entry, "-stage", stage])
        .arg("-o")
        .arg(&out)
        .output()
        .map_err(|e| format!("не удалось запустить {}: {e}", slangc.display()))?;
    let result = if output.status.success() {
        std::fs::read(&out).map_err(|e| format!("slangc не создал {}: {e}", out.display()))
    } else {
        let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stdout));
        Err(text.trim().to_string())
    };
    let _ = std::fs::remove_file(&out);
    result
}
