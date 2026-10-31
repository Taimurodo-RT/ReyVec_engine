//! Песочница ReyVec.
//!
//! На этапе 0 она только сообщает, что сборка удалась, и показывает версию.
//! С этапа 1 здесь откроется окно с первым кадром.

use std::process::ExitCode;

const GIT_COMMIT: &str = env!("REYVEC_GIT_COMMIT");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{} ({GIT_COMMIT})", reyvec_app::engine_title());
        return ExitCode::SUCCESS;
    }

    reyvec_app::init_logging();
    tracing::info!(
        "{} запущен, коммит {GIT_COMMIT}",
        reyvec_app::engine_title()
    );
    tracing::info!("Этап 0: сборка работает. Окно появится на этапе 1.");
    ExitCode::SUCCESS
}
