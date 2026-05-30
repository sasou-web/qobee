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

/// `EnvFilter` directive string that suppresses symphonia's MP3
/// layer-3 frame-repair warnings (`invalid main_data_begin`) so they
/// don't flood the logs (R11.1, R11.4).
///
/// `EnvFilter` level directives set a *minimum severity floor* for a
/// target: `target=<level>` enables `<level>` **and everything more
/// severe**. symphonia emits these per-frame repair messages at
/// `warn`, so the floor has to sit *above* `warn` to drop them — we
/// use `error`, which keeps any genuine symphonia error while
/// silencing the per-frame `warn` spam. (A `=debug` floor would do
/// the opposite, *lowering* the bar and letting the `warn`s through.)
///
/// Nothing meaningful is lost: Qobee already logs each repaired frame
/// at `debug` under its own `qobee::engine` target (see
/// `backend_symphonia::SymphoniaDecoder::next_packet`), and emits a
/// single aggregate `info` summary at end of file (R11.2).
const MP3_REPAIR_DIRECTIVE: &str = "symphonia_bundle_mp3::layer3=error";

/// Base filter: honor `RUST_LOG` when present, otherwise fall back to
/// `qobee=info,warn` (Qobee at `info`, everything else at `warn`).
fn base_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("qobee=info,warn"))
}

/// Add the MP3 layer-3 frame-repair directive to `filter`, suppressing
/// symphonia's per-frame repair `warn`s so they don't flood the logs
/// (R11.1, R11.4). Unrelated `warn`s on other targets are untouched.
///
/// Split out from [`build_env_filter`] so the smoke test can exercise
/// the exact directive logic against a fixed base, independent of the
/// ambient `RUST_LOG`.
fn add_mp3_repair_directive(filter: EnvFilter) -> EnvFilter {
    filter.add_directive(
        MP3_REPAIR_DIRECTIVE
            .parse()
            .expect("static EnvFilter directive must parse"),
    )
}

/// Build the `EnvFilter` used by the tracing subscriber: the
/// [`base_env_filter`] plus the MP3 frame-repair directive (R11.1,
/// R11.4).
pub fn build_env_filter() -> EnvFilter {
    add_mp3_repair_directive(base_env_filter())
}

/// Initialize tracing. Logs go to:
/// - the daily-rotated file `<data_dir>\Qobee\logs\qobee.log`,
/// - stdout when running under `cargo run` / a console build.
///
/// Filter respects `RUST_LOG` if set, falling back to `qobee=info,warn`.
pub fn init() -> Option<WorkerGuard> {
    let filter = build_env_filter();

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

// ---------------------------------------------------------------------------
// Tests — task 15.3 (log-level smoke test, R11.1/R11.4)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::Level;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::Layer;

    /// A tiny capturing layer that records `(target, level)` for every
    /// event the filter lets through. That is exactly what reaches the
    /// log file in production, so asserting on it verifies the *effect*
    /// of the `EnvFilter` directive rather than just its spelling.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<(String, Level)>>>);

    impl<S: tracing::Subscriber> Layer<S> for Captured {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let meta = event.metadata();
            self.0
                .lock()
                .unwrap()
                .push((meta.target().to_string(), *meta.level()));
        }
    }

    /// Build the filter against a *fixed* base (not the ambient
    /// `RUST_LOG`, which would make the assertions environment
    /// dependent) but through the same `add_mp3_repair_directive` used
    /// in production, so the smoke test pins the real directive logic.
    fn smoke_filter() -> EnvFilter {
        add_mp3_repair_directive(EnvFilter::new("qobee=info,warn"))
    }

    // Feature: qobee-beta-feedback-improvements, task 15.3 — symphonia
    // MP3 layer-3 frame-repair `warn`s are suppressed, so they never
    // reach the logs at `warn` or above (R11.1, R11.4), while unrelated
    // `warn`s on other targets still pass.
    #[test]
    fn symphonia_layer3_warnings_are_filtered_out() {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry()
            .with(smoke_filter())
            .with(captured.clone());

        tracing::subscriber::with_default(subscriber, || {
            // The exact line symphonia emits per repaired frame
            // (constat 2026-05-26). At `warn` it must be dropped.
            tracing::warn!(
                target: "symphonia_bundle_mp3::layer3",
                "mpa: invalid main_data_begin, underflow by 281 bytes"
            );
            // A second one, to make the "flood" concrete.
            tracing::warn!(
                target: "symphonia_bundle_mp3::layer3",
                "mpa: invalid main_data_begin, underflow by 4 bytes"
            );
            // A generic warning from another target must still pass
            // (the directive is scoped to symphonia, R11.4).
            tracing::warn!(target: "some_other_crate", "real problem worth surfacing");
            // Qobee's own per-frame detail lives under `qobee::engine`
            // at `debug` and is unaffected by the symphonia directive.
            tracing::debug!(target: "qobee::engine", "trame MP3 réparée/skippée");
            // Qobee's aggregate `info` summary must pass.
            tracing::info!(target: "qobee::engine", "12 trames MP3 réparées sur ce fichier");
        });

        let events = captured.0.lock().unwrap();

        // R11.1/R11.4: no `symphonia_bundle_mp3::layer3` event at
        // `warn` (or higher) reaches the subscriber — the per-frame
        // repair spam is gone.
        assert!(
            !events
                .iter()
                .any(|(t, l)| t == "symphonia_bundle_mp3::layer3" && *l <= Level::WARN),
            "symphonia layer3 must not log at warn or above, got {events:?}"
        );

        // In fact no symphonia layer3 event survives at all (the floor
        // sits at `error`), so the flood is fully suppressed.
        assert!(
            !events
                .iter()
                .any(|(t, _)| t == "symphonia_bundle_mp3::layer3"),
            "symphonia layer3 repair noise must be fully suppressed, got {events:?}"
        );

        // A generic `warn` from a different target is unaffected.
        assert!(
            events
                .iter()
                .any(|(t, l)| t == "some_other_crate" && *l == Level::WARN),
            "unrelated warnings must still surface, got {events:?}"
        );

        // Qobee keeps the per-frame detail at `debug` (R11.1) and the
        // aggregate `info` summary (R11.2) under its own target.
        assert!(
            events
                .iter()
                .any(|(t, l)| t == "qobee::engine" && *l == Level::INFO),
            "qobee aggregate info summary must pass, got {events:?}"
        );
    }

    // Feature: qobee-beta-feedback-improvements, task 15.3 — the
    // production builder always carries the MP3 frame-repair directive
    // scoped to the symphonia layer-3 target (R11.1, R11.4),
    // independent of the base filter.
    #[test]
    fn build_env_filter_includes_mp3_repair_directive() {
        // `EnvFilter`'s Display renders its active directives; the
        // symphonia layer-3 directive must be present and pin the floor
        // above `warn` (we use `error`) so the per-frame `warn` spam is
        // dropped rather than enabled.
        let rendered = build_env_filter().to_string();
        assert!(
            rendered.contains("symphonia_bundle_mp3::layer3"),
            "filter must scope the directive to the symphonia MP3 layer-3 target, got {rendered:?}"
        );
        assert!(
            rendered.contains("error"),
            "the symphonia MP3 layer-3 directive must sit at `error` so warn-level \
             repair spam is suppressed, got {rendered:?}"
        );
    }
}
