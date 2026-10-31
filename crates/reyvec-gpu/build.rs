//! Собирает шейдеры Slang из папки `shaders/` в SPIR-V при компиляции движка.
//! Готовый SPIR-V встраивается в программу, поэтому для запуска slangc не нужен.
//! slangc нужен только для сборки: он входит в Vulkan SDK.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[path = "src/slangc.rs"]
#[allow(dead_code)]
mod slangc;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo задаёт"));
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo задаёт"));
    let shader_dir = manifest.join("shaders");
    println!("cargo:rerun-if-changed=shaders");
    println!("cargo:rerun-if-changed=src/slangc.rs");
    println!("cargo:rerun-if-env-changed=REYVEC_SLANGC");
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");

    let Some(slangc) = slangc::find_slangc() else {
        panic!(
            "\n\nНе найден компилятор шейдеров slangc.\n\
             Он входит в Vulkan SDK: установите его (docs/setup-windows.md, шаг 4)\n\
             и перезапустите консоль, чтобы появилась переменная VULKAN_SDK.\n\
             Другой путь можно указать переменной REYVEC_SLANGC.\n\n"
        );
    };

    let mut sources: Vec<PathBuf> = std::fs::read_dir(&shader_dir)
        .expect("папка shaders")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "slang"))
        .collect();
    sources.sort();

    let mut generated = String::from(
        "/// Шейдеры, собранные при компиляции: имя файла, SPIR-V вершинного и пиксельного шейдера.\n\
         pub(crate) const BUILTIN: &[(&str, &[u8], &[u8])] = &[\n",
    );
    let mut errors = Vec::new();
    for source in &sources {
        let name = source
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("имя файла");
        let mut paths = Vec::new();
        for (entry, stage, short) in slangc::ENTRIES {
            let out = out_dir.join(format!("{name}.{short}.spv"));
            match slangc::compile(&slangc, source, entry, stage) {
                Ok(spv) => std::fs::write(&out, spv).expect("запись SPIR-V"),
                Err(e) => errors.push(format!("{}:{entry}\n{e}", source.display())),
            }
            paths.push(out);
        }
        writeln!(
            generated,
            "    ({name:?}, include_bytes!({:?}), include_bytes!({:?})),",
            path_str(&paths[0]),
            path_str(&paths[1])
        )
        .expect("запись в строку");
    }
    generated.push_str("];\n");
    if !errors.is_empty() {
        panic!("\n\nОшибки в шейдерах:\n\n{}\n", errors.join("\n\n"));
    }
    std::fs::write(out_dir.join("shaders.rs"), generated).expect("запись shaders.rs");
}

fn path_str(p: &Path) -> String {
    p.to_str().expect("путь в UTF-8").to_string()
}
