//! Встраивает в программу короткий номер коммита git, чтобы по выводу песочницы
//! было видно, какая именно версия запущена. Без git пишем `unknown`.

use std::process::Command;

fn main() {
    let commit = Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());

    println!("cargo:rustc-env=REYVEC_GIT_COMMIT={commit}");
    // Пересобирать номер при смене коммита или ветки.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
}
