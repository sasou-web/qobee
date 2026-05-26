//! File-based logging for Qobee.
//!
//! Writes a daily-rotated log to `%APPDATA%\Qobee\logs\qobee.log`
//! while still printing to stdout in dev. Older files are kept on
//! disk so a user reporting a bug can attach the logs from a
//! previous session.
//!
//! The returned [`tracing_appender::non_blocking::WorkerGuard`] must
//! be kept alive for the lifetime of the app: dropping it flushes
//! and closes the writer. We hand it back to `run()` in `lib.rs`,
//! which keeps it on the stack until the Tauri loop exits.

use std::path::PathBuf;

use tracing_appender::{non_blocking::WorkerGuard, rolling};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Resolve `%APPDATA%\Qobee\logs` (or the platform equivalent),
/// creating the folder if it doesn't exist yet.
pub fn log_dir() -> Option<PathBuf> {
    let base = dirs::data_dir()?.join("Qobee").join("logs");
    if let Err(e) = std::fs::create_dir_all(&base) {
        eprintln!("qobee: could not create log directory {base:?}: {e}");
        return None;
    }
    Some(base)
}

/// Initialize tracing. Logs go to:
/// - the daily-rotated file `<data_dir>\Qobee\logs\qobee.log`,
/// - stdout when running under `cargo run` / a console build.
///
/// Filter respects `RUST_LOG` if set, falling back to `qobee=info,warn`.
pub fn init() -> Option<WorkerGuard> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("qobee=info,warn"));

    let stdout_layer = fmt::layer()
        .with_target(true)
        .with_ansi(true)
        .with_writer(std::io::stdout);

    let (file_layer, guard) = match log_dir() {
        Some(dir) => {
            // Daily rotation keeps each file small. tracing-appender
            // names them `qobee.log.YYYY-MM-DD`.
            let appender = rolling::daily(dir, "qobee.log");
            let (nb, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer()
                .with_target(true)
                .with_ansi(false)
                .with_writer(nb);
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer);
    if let Some(file_layer) = file_layer {
        let _ = registry.with(file_layer).try_init();
    } else {
        let _ = registry.try_init();
    }

    guard
}
