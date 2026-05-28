//! `Player_Sync` shared types — trait consumed by OS-side bridges
//! (SMTC on Windows, `MPNowPlayingInfoCenter` on macOS) to project
//! the R8 transport bus onto an OS Now Playing surface.
//!
//! The fan-out task lives in `src-tauri::lib::run::setup`: it owns
//! a single `tokio::sync::broadcast::Receiver<PlayerEvent>` returned
//! by [`crate::PlayerHandle::subscribe_broadcast`] and dispatches
//! every received event in order to:
//!
//! 1. the `cfg`-gated OS bridges (synchronous calls in the same loop
//!    iteration so `Started` / `Paused` / `TrackChanged` reach the
//!    OS surface in lock-step with the UI — see R8.1 / R8.2),
//! 2. the Tauri webview through `app_handle.emit("player:event", _)`,
//!    consumed by `ui/src/lib/playerStore.svelte.ts`,
//! 3. an opt-in native notification on `TrackChanged` (R7.7).
//!
//! This module only defines the trait surface. The Windows / macOS
//! implementations land in `src-tauri/src/media/{smtc,mac}.rs` under
//! their respective `#[cfg(target_os = ...)]` blocks (tasks 3.1 and
//! 4.1); none exists yet.

use crate::PlayerEvent;

/// Bridge contract exposed by an OS-side projector of the R8
/// transport bus.
///
/// A bridge does not own playback state — it is a one-way sink from
/// the broadcast bus to the OS Now Playing API. The fan-out task
/// holds an `Arc<dyn MediaBridge>` and calls [`handle_event`] in
/// the broadcast receive loop, so implementations must be
/// `Send + Sync`.
///
/// Implementations should swallow their own internal errors (log
/// + continue) so a transient OS failure never poisons the bus for
/// the other sinks.
///
/// [`handle_event`]: MediaBridge::handle_event
pub trait MediaBridge: Send + Sync {
    /// Project a single `PlayerEvent` onto the underlying OS
    /// surface. Called synchronously from the fan-out task; should
    /// return promptly (no blocking I/O on the broadcast thread).
    fn handle_event(&self, ev: &PlayerEvent);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PlayerErrorKind, TrackMeta};
    use parking_lot::Mutex;
    use std::sync::Arc;

    /// In-memory mock bridge that records the events it receives.
    /// Used by the property-based test in task 2.2 to assert the
    /// fan-out converges all bridges to the same projected state.
    /// Kept here so the Rust orphan rule does not bite when the
    /// test crate (in task 2.2) tries to implement `MediaBridge`
    /// for an external newtype.
    #[derive(Default)]
    struct RecordingBridge {
        events: Mutex<Vec<PlayerEvent>>,
    }

    impl MediaBridge for RecordingBridge {
        fn handle_event(&self, ev: &PlayerEvent) {
            self.events.lock().push(ev.clone());
        }
    }

    /// `MediaBridge` is object-safe (`dyn MediaBridge`) so the
    /// fan-out task can hold an `Arc<dyn MediaBridge>` without
    /// monomorphising per backend. This compile-time test catches
    /// any accidental change (e.g. adding a generic method) that
    /// would break the broadcast wiring.
    #[test]
    fn media_bridge_is_object_safe() {
        let bridge: Arc<dyn MediaBridge> = Arc::new(RecordingBridge::default());
        let track = TrackMeta {
            id: 1,
            title: "t".into(),
            artist: "a".into(),
            album: "al".into(),
            duration_ms: 1000,
            cover_bytes: None,
        };
        bridge.handle_event(&PlayerEvent::Started {
            track: track.clone(),
            position_ms: 0,
            duration_ms: 1000,
        });
        bridge.handle_event(&PlayerEvent::Errored {
            kind: PlayerErrorKind::Other,
            message: "oops".into(),
        });
    }
}
