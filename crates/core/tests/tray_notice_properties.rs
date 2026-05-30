//! Property-based tests for the R7 first-close "tray notice" one-shot
//! latch (cf. spec `qobee-beta-feedback-improvements`, Requirement 7).
//!
//! ## Scope
//!
//! This file covers **Property 3 — Avis de première fermeture tray
//! affiché une seule fois**. It quantifies over arbitrary finite
//! sequences of window-close events, each carrying a `close_behavior`
//! that may or may not mask the app to a notification-area surface,
//! and checks that the `tray:first-close-notice` event is emitted at
//! most once (exactly once when at least one masking close occurs) and
//! that the persisted `windows.tray_notice_shown` flag latches to
//! `true` and is never re-armed.
//!
//! ## Why a model and not the real backend hook
//!
//! The production logic lives in `src-tauri/src/lib.rs` as
//! `emit_first_close_notice_once`, invoked from the
//! `WindowEvent::CloseRequested` handler. That code is in the Tauri
//! **binary** crate: it depends on a live `tauri::AppHandle`, the
//! managed `AppState`, a SQLite-backed `Library` (for the persisted
//! `windows.tray_notice_shown` setting) and an actual OS window event
//! loop. None of that can be constructed in a `crates/core`
//! integration test, let alone driven 100+ times inside a `proptest!`
//! block. As recommended by the task, we model the one-shot latch
//! against a small in-memory state machine that mirrors the backend's
//! exact branch structure (the same approach used by
//! `player_sync_properties.rs`).
//!
//! The model is a literal Rust transcription of the two production
//! decision points (cf. task 9.1 in `src-tauri/src/lib.rs`):
//!
//!   * The `CloseRequested` handler only calls
//!     `emit_first_close_notice_once` when the resolved
//!     `close_behavior ∈ {MinimizeToTray, KeepRunningInBackground}`
//!     (i.e. a behavior that hides the window to a notification-area
//!     surface). For `Quit`, the window just closes and no notice is
//!     ever emitted.
//!   * `emit_first_close_notice_once` reads
//!     `windows.tray_notice_shown`; if it is already `"true"` it
//!     returns immediately (no emit). Otherwise it emits
//!     `tray:first-close-notice` and writes the flag to `"true"`. It
//!     never reads or writes `windows.close_behavior` (R7.4).

use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// What the `WindowEvent::CloseRequested` hook resolves the active
/// close preference to. Mirrors `CloseBehavior` in
/// `src-tauri/src/lib.rs`: `Quit` lets the window close, while
/// `MinimizeToTray` / `KeepRunningInBackground` both hide the window to
/// a notification-area surface (and so trigger the one-shot notice).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CloseBehavior {
    Quit,
    MinimizeToTray,
    KeepRunningInBackground,
}

impl CloseBehavior {
    /// `true` iff this behavior hides the app to a notification-area
    /// surface instead of quitting. Only these behaviors reach
    /// `emit_first_close_notice_once` in the production hook (R7.1).
    fn masks_app(self) -> bool {
        matches!(
            self,
            CloseBehavior::MinimizeToTray | CloseBehavior::KeepRunningInBackground
        )
    }
}

/// In-memory mirror of the persisted `windows.tray_notice_shown`
/// latch. The real flag lives in the SQLite `settings` table; here it
/// is just a `bool` so the property can drive thousands of close
/// sequences without any I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrayNoticeModel {
    /// Mirrors `windows.tray_notice_shown == "true"`.
    tray_notice_shown: bool,
}

impl TrayNoticeModel {
    fn new() -> Self {
        Self {
            tray_notice_shown: false,
        }
    }

    /// Handle a single window-close event under `behavior`. Returns
    /// `true` iff `tray:first-close-notice` is emitted on this call.
    ///
    /// This fuses the two production decision points:
    ///   1. The `CloseRequested` match arm: only masking behaviors
    ///      call into the notice path; `Quit` returns without emitting.
    ///   2. `emit_first_close_notice_once`: emit + latch only when the
    ///      flag is still `false`; otherwise stay silent.
    fn on_close(&mut self, behavior: CloseBehavior) -> bool {
        // Non-masking close (Quit): the window closes, the notice path
        // is never reached, and the flag is left untouched (R7.4 — we
        // never touch close_behavior, and a quit close never arms the
        // notice flag).
        if !behavior.masks_app() {
            return false;
        }
        // Masking close: one-shot guard.
        if self.tray_notice_shown {
            return false;
        }
        self.tray_notice_shown = true;
        true
    }
}

/// Replay a finite sequence of close events from a fresh model.
/// Returns, for each event, whether the notice was emitted, plus the
/// snapshot of the flag *after* that event. The per-step flag history
/// lets the property assert the latch is monotonic (never re-armed).
fn simulate(events: &[CloseBehavior]) -> (Vec<bool>, Vec<bool>) {
    let mut model = TrayNoticeModel::new();
    let mut emissions = Vec::with_capacity(events.len());
    let mut flag_history = Vec::with_capacity(events.len());
    for &behavior in events {
        let emitted = model.on_close(behavior);
        emissions.push(emitted);
        flag_history.push(model.tray_notice_shown);
    }
    (emissions, flag_history)
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Any of the three close behaviors, with masking and non-masking
/// variants equally represented so sequences mix `Quit` closes in
/// between masking ones.
fn any_close_behavior() -> impl Strategy<Value = CloseBehavior> {
    prop_oneof![
        Just(CloseBehavior::Quit),
        Just(CloseBehavior::MinimizeToTray),
        Just(CloseBehavior::KeepRunningInBackground),
    ]
}

/// Finite close sequences up to 32 events long, including the empty
/// sequence (no close ever happened).
fn any_close_seq() -> impl Strategy<Value = Vec<CloseBehavior>> {
    proptest::collection::vec(any_close_behavior(), 0..=32)
}

// ---------------------------------------------------------------------------
// Property 3 — Avis de première fermeture tray affiché une seule fois
// ---------------------------------------------------------------------------

// Feature: qobee-beta-feedback-improvements, Property 3: Avis de
// première fermeture tray affiché une seule fois
//
// For any finite sequence of close events, each carrying a
// `close_behavior` that masks the app or not:
//   * the `tray:first-close-notice` event is emitted at most once;
//   * it is emitted exactly once iff there is at least one close under
//     a masking behavior;
//   * the `windows.tray_notice_shown` flag transitions to `true` (on
//     the first masking close) and is never re-armed afterwards
//     (monotonic: once `true`, stays `true`).
//
// Validates: Requirements 7.3
#[test]
fn property_3_tray_notice_shown_at_most_once() {
    proptest!(
        ProptestConfig::with_cases(100),
        |(events in any_close_seq())| {
            let (emissions, flag_history) = simulate(&events);

            let emit_count = emissions.iter().filter(|&&e| e).count();
            let has_masking_close = events.iter().any(|b| b.masks_app());

            // (1) The notice is emitted at most once across the whole
            //     sequence.
            prop_assert!(
                emit_count <= 1,
                "notice emitted {} times (expected <= 1): events={:?}",
                emit_count, events,
            );

            // (2) Exactly once iff there is >= 1 masking close.
            prop_assert_eq!(
                emit_count == 1, has_masking_close,
                "emit_count==1 ({}) must match has_masking_close ({}): events={:?}",
                emit_count == 1, has_masking_close, events,
            );

            // (3) The single emission, when present, happens precisely
            //     on the FIRST masking close — never on a Quit close,
            //     never on a later masking close.
            if let Some(first_masking_idx) =
                events.iter().position(|b| b.masks_app())
            {
                prop_assert!(
                    emissions[first_masking_idx],
                    "notice must be emitted on the first masking close \
                     (idx {}): events={:?}",
                    first_masking_idx, events,
                );
                for (i, &emitted) in emissions.iter().enumerate() {
                    prop_assert_eq!(
                        emitted, i == first_masking_idx,
                        "only the first masking close may emit; idx {} \
                         emitted={} : events={:?}",
                        i, emitted, events,
                    );
                }
            }

            // (4) The flag latch is monotonic: once it is `true` it
            //     stays `true` for every subsequent event (never
            //     re-armed). It is `false` before the first masking
            //     close and `true` from that point on.
            let mut seen_true = false;
            for (i, &flag) in flag_history.iter().enumerate() {
                if seen_true {
                    prop_assert!(
                        flag,
                        "tray_notice_shown was re-armed to false at idx {}: \
                         events={:?}, flag_history={:?}",
                        i, events, flag_history,
                    );
                }
                seen_true |= flag;
            }

            // (5) The final flag value reflects whether any masking
            //     close occurred at all.
            let final_flag = flag_history.last().copied().unwrap_or(false);
            prop_assert_eq!(
                final_flag, has_masking_close,
                "final tray_notice_shown ({}) must match has_masking_close \
                 ({}): events={:?}",
                final_flag, has_masking_close, events,
            );
        }
    );
}

// ---------------------------------------------------------------------------
// Sanity unit tests on the model itself
// ---------------------------------------------------------------------------
//
// These pin down the one-shot latch by example so a future contributor
// can grok the rules without re-reading `on_close`. They are not a
// substitute for the property above.

#[test]
fn model_quit_close_never_emits_or_arms() {
    let mut m = TrayNoticeModel::new();
    assert!(!m.on_close(CloseBehavior::Quit));
    assert!(!m.tray_notice_shown);
    // Repeated quit closes stay silent and never arm the flag.
    assert!(!m.on_close(CloseBehavior::Quit));
    assert!(!m.tray_notice_shown);
}

#[test]
fn model_first_minimize_to_tray_emits_and_latches() {
    let mut m = TrayNoticeModel::new();
    assert!(m.on_close(CloseBehavior::MinimizeToTray));
    assert!(m.tray_notice_shown);
}

#[test]
fn model_first_keep_running_emits_and_latches() {
    let mut m = TrayNoticeModel::new();
    assert!(m.on_close(CloseBehavior::KeepRunningInBackground));
    assert!(m.tray_notice_shown);
}

#[test]
fn model_second_masking_close_is_silent() {
    let mut m = TrayNoticeModel::new();
    assert!(m.on_close(CloseBehavior::MinimizeToTray));
    // Any subsequent masking close — same or other masking behavior —
    // is a silent no-op once latched.
    assert!(!m.on_close(CloseBehavior::MinimizeToTray));
    assert!(!m.on_close(CloseBehavior::KeepRunningInBackground));
    assert!(m.tray_notice_shown);
}

#[test]
fn model_quit_before_masking_close_does_not_consume_the_notice() {
    let mut m = TrayNoticeModel::new();
    // A quit close first must not arm the flag, so the later masking
    // close still gets the one and only notice.
    assert!(!m.on_close(CloseBehavior::Quit));
    assert!(m.on_close(CloseBehavior::MinimizeToTray));
    assert!(m.tray_notice_shown);
}

#[test]
fn model_masking_behaviors_are_the_masking_set() {
    assert!(!CloseBehavior::Quit.masks_app());
    assert!(CloseBehavior::MinimizeToTray.masks_app());
    assert!(CloseBehavior::KeepRunningInBackground.masks_app());
}
