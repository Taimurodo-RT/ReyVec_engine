//! Редактор ReyVec: окно на весь экран без рамки системы, живая сцена движка посередине,
//! панели инструментов вокруг (Р.1 — каркас).
//!
//! Флаги:
//! - `--project <папка>`: какой проект открыть (по умолчанию последний открытый, а без
//!   него — пример движка рядом со сборкой); без флагов при запуске видно окно «Проекты»;
//! - `--scene-file <файл.ron>`: какую сцену открыть (по умолчанию первую сцену проекта);
//! - `--shape <файл.shape.ron>`: сразу открыть модель во «Формах» (для проверок);
//! - `--assets`: сразу открыть «Ассеты» с проводником файлов (для проверок);
//! - `--window-frame`: обычная рамка окна Windows вместо своей верхней полосы;
//! - `--gpu <номер или часть названия>`: выбрать видеокарту;
//! - `--no-vsync`: без вертикальной синхронизации;
//! - `--validation`: слои валидации Vulkan;
//! - `--seconds <S>`, `--frames <N>`: закрыть окно через S секунд или N кадров;
//! - `--snapshot <файл.png>`: снять окно редактора в файл и закрыть
//!   (`--snapshot-after <S>`: через сколько секунд, по умолчанию 3);
//! - `--version`, `--help`.

mod editor;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const GIT_COMMIT: &str = env!("REYVEC_GIT_COMMIT");

const HELP: &str = "\
Редактор ReyVec

Запуск: reyvec-editor [флаги]

  --project <папка>      какой проект открыть (по умолчанию последний)
  --scene-file <файл.ron>  какую сцену открыть (по умолчанию первую сцену проекта)
  --shape <файл>         сразу открыть модель во «Формах»
  --assets               сразу открыть «Ассеты» (проводник файлов проекта)
  --window-frame         обычная рамка окна Windows вместо своей верхней полосы
  --gpu <номер|имя>      выбрать видеокарту, например --gpu radeon
  --no-vsync             без вертикальной синхронизации
  --validation           слои валидации Vulkan (нужен Vulkan SDK)
  --seconds <S>          закрыть окно через S секунд
  --frames <N>           закрыть окно после N кадров
  --snapshot <файл.png>  снять окно редактора в файл и закрыть
  --snapshot-after <S>   для --snapshot: через сколько секунд снимать (по умолчанию 3)
  --version, -V          версия
  --help, -h             эта справка

В окне: F5 — играть или остановить, Esc во время игры — остановить,
колесо над сценой — приблизить, перетаскивание сцены — сдвинуть камеру.";

#[derive(Debug, Default, PartialEq)]
struct Args {
    help: bool,
    version: bool,
    window_frame: bool,
    no_vsync: bool,
    validation: bool,
    gpu: Option<String>,
    scene_file: Option<String>,
    project: Option<String>,
    shape: Option<String>,
    assets: bool,
    seconds: Option<f64>,
    frames: Option<u64>,
    snapshot: Option<String>,
    snapshot_after: Option<f64>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    let seconds = |flag: &str, n: Option<String>| -> Result<f64, String> {
        let n = n.ok_or(format!("после {flag} нужно число"))?;
        n.parse()
            .ok()
            .filter(|s: &f64| s.is_finite() && *s >= 0.0)
            .ok_or_else(|| format!("{flag}: «{n}» не число"))
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => out.help = true,
            "--version" | "-V" => out.version = true,
            "--window-frame" => out.window_frame = true,
            "--no-vsync" => out.no_vsync = true,
            "--validation" => out.validation = true,
            "--assets" => out.assets = true,
            "--project" => {
                out.project = Some(it.next().ok_or("после --project нужна папка проекта")?);
            }
            "--gpu" => out.gpu = Some(it.next().ok_or("после --gpu нужен номер или название")?),
            "--scene-file" => {
                out.scene_file = Some(
                    it.next()
                        .ok_or("после --scene-file нужен путь к файлу сцены .ron")?,
                );
            }
            "--shape" => {
                out.shape = Some(
                    it.next()
                        .ok_or("после --shape нужен файл модели .shape.ron или сцены 3D .ron")?,
                );
            }
            "--seconds" => out.seconds = Some(seconds("--seconds", it.next())?),
            "--snapshot-after" => {
                out.snapshot_after = Some(seconds("--snapshot-after", it.next())?);
            }
            "--frames" => {
                let n = it.next().ok_or("после --frames нужно число")?;
                out.frames = Some(n.parse().map_err(|_| format!("--frames: «{n}» не число"))?);
            }
            "--snapshot" => {
                out.snapshot = Some(it.next().ok_or("после --snapshot нужно имя файла")?);
            }
            other => return Err(format!("неизвестный флаг «{other}». Справка: --help")),
        }
    }
    Ok(out)
}

/// Папка проекта: в ней `scenes` и `assets`. Рядом с `dist` (где лежит собранный
/// редактор) или там, где проект собирали.
fn project_dir() -> Option<PathBuf> {
    let near_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.to_path_buf()));
    let built_here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    near_exe
        .into_iter()
        .chain([built_here])
        .find(|p| p.join("scenes").is_dir())
        .map(|p| std::path::absolute(&p).unwrap_or(p))
}

/// Флаги, с которыми перезапустить редактор на другом проекте.
struct Relaunch {
    window_frame: bool,
    no_vsync: bool,
    validation: bool,
    gpu: Option<String>,
}

impl Relaunch {
    fn args(&self, project: &Path) -> Vec<std::ffi::OsString> {
        let mut out: Vec<std::ffi::OsString> = vec!["--project".into(), project.into()];
        for (on, flag) in [
            (self.window_frame, "--window-frame"),
            (self.no_vsync, "--no-vsync"),
            (self.validation, "--validation"),
        ] {
            if on {
                out.push(flag.into());
            }
        }
        if let Some(g) = &self.gpu {
            out.push("--gpu".into());
            out.push(g.into());
        }
        out
    }

    /// Запустить редактор заново на проекте `project` (этот процесс сейчас закончится).
    fn spawn(&self, project: &Path) {
        let started = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .args(self.args(project))
                .spawn()
        });
        match started {
            Ok(_) => tracing::info!("Редактор перезапущен на «{}»", project.display()),
            Err(e) => tracing::error!("Не перезапустить редактор: {e}"),
        }
    }
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            return ExitCode::from(2);
        }
    };
    if args.help {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.version {
        println!(
            "Редактор {} (коммит {GIT_COMMIT})",
            reyvec_app::engine_title()
        );
        return ExitCode::SUCCESS;
    }
    let log = reyvec_app::init_logging_to("reyvec-editor.log");
    tracing::info!(
        "Редактор {} запущен, коммит {GIT_COMMIT}",
        reyvec_app::engine_title()
    );
    if let Some(path) = &log.path {
        tracing::info!("Лог пишется в {}", path.display());
    }
    // Проект: из --project, иначе последний открытый, иначе пример движка рядом со сборкой.
    let recent_path = editor::project::recent_path();
    let mut recent = editor::project::Recent::load(&recent_path);
    let project = match &args.project {
        Some(p) => {
            let p = std::path::absolute(p).unwrap_or_else(|_| PathBuf::from(p));
            if !editor::project::is_project(&p) {
                tracing::error!(
                    "В «{}» нет проекта ReyVec: нужна папка scenes или файл project.ron",
                    p.display()
                );
                return ExitCode::FAILURE;
            }
            Some(p)
        }
        None if args.scene_file.is_none() => {
            recent.last().map(Path::to_path_buf).or_else(project_dir)
        }
        None => project_dir(),
    };
    // Окно «Проекты» — при обычном запуске, не в проверках и не с явным проектом.
    let start_screen = args.project.is_none()
        && args.scene_file.is_none()
        && args.shape.is_none()
        && !args.assets
        && args.snapshot.is_none()
        && args.seconds.is_none()
        && args.frames.is_none()
        && !recent.skip_start;
    if let Some(dir) = &project {
        recent.touch(dir);
        recent.save(&recent_path);
    }
    let project_name = project.as_deref().map(|d| editor::project::read(d).name);
    let scene_file = match (&args.scene_file, &project) {
        (Some(p), _) => Some(PathBuf::from(p)),
        (None, Some(dir)) => editor::project::first_scene(dir),
        (None, None) => {
            tracing::warn!("Папка проекта не найдена, открываю встроенный пример сцены");
            None
        }
    };
    if let Some(p) = &scene_file
        && !p.is_file()
    {
        tracing::error!("Нет файла сцены «{}»", p.display());
        return ExitCode::FAILURE;
    }
    // В отладочной сборке валидация включена всегда: так ошибки ловятся сразу.
    let validation = args.validation || cfg!(debug_assertions);
    let config = editor::EditorConfig {
        title: match &project_name {
            Some(name) => format!("{name} — {} — редактор", reyvec_app::engine_title()),
            None => format!("{} — редактор", reyvec_app::engine_title()),
        },
        gpu: reyvec_app::GpuOptions {
            validation,
            gpu: args.gpu.clone(),
        },
        vsync: !args.no_vsync,
        frameless: !args.window_frame,
        scene_file,
        shape: args.shape.map(|p| {
            let p = PathBuf::from(p);
            std::path::absolute(&p).unwrap_or(p)
        }),
        assets: args.assets,
        project,
        start_screen,
        max_seconds: args.seconds,
        max_frames: args.frames,
        snapshot: args.snapshot.map(|p| {
            let p = PathBuf::from(p);
            std::path::absolute(&p).unwrap_or(p)
        }),
        snapshot_after: args.snapshot_after.unwrap_or(3.0),
    };
    let relaunch = Relaunch {
        window_frame: args.window_frame,
        no_vsync: args.no_vsync,
        validation: args.validation,
        gpu: args.gpu.clone(),
    };
    let result = editor::run_editor(config);
    let errors = reyvec_app::validation_error_count();
    match result {
        Ok(exit) => {
            let summary = exit.summary;
            if let Some(dir) = exit.reopen {
                relaunch.spawn(&dir);
            }
            tracing::info!(
                "Редактор закрыт: {} кадров на «{}», в среднем {:.0} к/с",
                summary.frames,
                summary.adapter,
                summary.average_fps
            );
            if errors > 0 {
                tracing::error!(
                    "Слои валидации нашли ошибок: {errors}. Пришлите reyvec-editor.log"
                );
                return ExitCode::FAILURE;
            }
            if validation {
                tracing::info!("Ошибок валидации Vulkan: 0");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!("Редактор не запустился: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Args, String> {
        parse_args(s.split_whitespace().map(String::from))
    }

    #[test]
    fn flags() {
        assert_eq!(parse("").unwrap(), Args::default());
        let a = parse("--window-frame --seconds 2.5 --snapshot a.png --snapshot-after 4").unwrap();
        assert!(a.window_frame);
        assert_eq!(a.seconds, Some(2.5));
        assert_eq!(a.snapshot.as_deref(), Some("a.png"));
        assert_eq!(a.snapshot_after, Some(4.0));
        assert!(parse("--seconds").is_err());
        assert!(parse("--seconds -1").is_err());
        assert!(parse("--frames x").is_err());
        assert!(parse("--что-то").is_err());
    }

    #[test]
    fn relaunch_keeps_flags() {
        let r = Relaunch {
            window_frame: true,
            no_vsync: false,
            validation: false,
            gpu: Some("radeon".into()),
        };
        let a: Vec<String> = r
            .args(Path::new("D:/игры/Моя игра"))
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            a,
            [
                "--project",
                "D:/игры/Моя игра",
                "--window-frame",
                "--gpu",
                "radeon"
            ]
        );
        assert_eq!(parse("--project x").unwrap().project.as_deref(), Some("x"));
        assert!(parse("--project").is_err());
    }

    #[test]
    fn project_is_found() {
        let dir = project_dir().expect("папка проекта рядом со сборкой");
        assert!(dir.join("scenes/first-room.ron").is_file());
    }
}
