//! Windows SMTC bridge — projects the R8 [`PlayerEvent`] bus onto
//! `Windows.Media.SystemMediaTransportControls` and wires the
//! `ButtonPressed` / `PlaybackPositionChangeRequested` events back
//! to [`qobee_core::PlayerHandle`].
//!
//! Validates: Requirements R1.1, R1.2, R1.3, R8.6
//!
//! ## Architecture
//!
//! - `SmtcBridge::init(window)` resolves the window's HWND, calls
//!   `ISystemMediaTransportControlsInterop::GetForWindow(hwnd)` to
//!   obtain the per-process `SystemMediaTransportControls`, enables
//!   the Play / Pause / Stop / Next / Previous buttons, and
//!   registers the two TypedEventHandlers that route OS-side input
//!   back into [`PlayerHandle`].
//! - `impl MediaBridge for SmtcBridge` projects each variant of the
//!   transport bus onto the `DisplayUpdater` (Title / Artist / Album
//!   / Type=Music + Thumbnail) and the `Timeline` (start / end /
//!   position) accordingly. `Started` / `TrackChanged` republish
//!   the full Now Playing surface; `Paused` / `Resumed` / `Stopped`
//!   / `PositionTick` keep the `PlaybackStatus` and timeline in
//!   sync. `Errored` is collapsed to `Stopped` so the OS surface
//!   never shows a stale "Playing" indicator after a hard error.
//!
//! ## Threading model
//!
//! `SystemMediaTransportControls` is a WinRT runtime class; both
//! its inherent vtable and its `*EventHandler` callbacks are
//! `Send + Sync`. The fan-out task in `lib::run::setup` calls
//! `handle_event` synchronously from a tokio worker thread; the
//! handler is non-blocking (a few WinRT property setters) so the
//! 200 ms convergence budget (R1.1, R1.3) is comfortably met.
//!
//! ## Cover thumbnail (R8.6)
//!
//! Thumbnails are published as a `RandomAccessStreamReference`
//! built on top of an `InMemoryRandomAccessStream` populated with
//! the raw cover bytes via `DataWriter`. When `cover_bytes` is
//! `None` or stream construction fails, we fall back to an embedded
//! placeholder PNG (`include_bytes!("placeholder_cover.png")`) so
//! the OS surface always renders an artwork tile.

use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager, WebviewWindow};

use windows::core::HSTRING;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs,
    SystemMediaTransportControls, SystemMediaTransportControlsButton,
    SystemMediaTransportControlsButtonPressedEventArgs,
    SystemMediaTransportControlsTimelineProperties,
};
use windows::Storage::Streams::{
    DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;

use qobee_core::{MediaBridge, PlayerEvent, TrackMeta};

use crate::state::AppState;

/// Embedded fallback artwork (R8.6) — used as the SMTC thumbnail
/// when `cover_bytes` is missing or the stream construction fails.
/// Lives next to this file so the byte slice is part of the
/// compiled binary.
const PLACEHOLDER_COVER_BYTES: &[u8] = include_bytes!("placeholder_cover.png");

/// 100-nanosecond ticks per millisecond. WinRT `TimeSpan::Duration`
/// is encoded in 100 ns units (same as a Windows `FILETIME` delta).
const TICKS_PER_MS: i64 = 10_000;

/// Convert a millisecond duration into a WinRT `TimeSpan`.
fn ms_to_timespan(ms: u64) -> TimeSpan {
    TimeSpan {
        // Saturate at i64::MAX to avoid an overflow if the engine
        // reports a pathological position; SMTC clamps any value
        // larger than the configured `EndTime` automatically.
        Duration: (ms as i128).min(i64::MAX as i128) as i64 * TICKS_PER_MS,
    }
}

/// Lazily-resolved `RandomAccessStreamReference` to the embedded
/// placeholder PNG. Computed once per process and reused across
/// every `Started` / `TrackChanged` that lacks a usable
/// `cover_bytes` blob — building the stream on every event would
/// waste cycles for the common "no embedded cover" case.
fn placeholder_thumbnail() -> Option<RandomAccessStreamReference> {
    static SLOT: OnceLock<Option<RandomAccessStreamReference>> = OnceLock::new();
    SLOT.get_or_init(|| match thumbnail_from_bytes(PLACEHOLDER_COVER_BYTES) {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!(
                target: "qobee::smtc",
                error = %e,
                "could not build SMTC placeholder thumbnail"
            );
            None
        }
    })
    .clone()
}

/// Build a `RandomAccessStreamReference` containing `bytes`. Used
/// for both the per-track cover and the embedded placeholder.
fn thumbnail_from_bytes(bytes: &[u8]) -> Result<RandomAccessStreamReference> {
    let stream = InMemoryRandomAccessStream::new()?;
    // `CreateDataWriter` takes the stream's `IOutputStream` view;
    // the cast is implicit because `InMemoryRandomAccessStream`
    // implements `IRandomAccessStream` which exposes
    // `GetOutputStreamAt`.
    let output = stream.GetOutputStreamAt(0)?;
    let writer = DataWriter::CreateDataWriter(&output)?;
    writer.WriteBytes(bytes)?;
    // Flush the writer's buffer into the underlying stream and
    // detach so the stream survives the writer being dropped.
    let _ = writer.StoreAsync()?.get()?;
    let _ = writer.FlushAsync()?.get()?;
    let _ = writer.DetachStream()?;
    Ok(RandomAccessStreamReference::CreateFromStream(&stream)?)
}

/// Project a [`TrackMeta::cover_bytes`] onto a SMTC thumbnail,
/// falling back to the embedded placeholder when bytes are missing
/// or the stream construction fails (R8.6).
fn thumbnail_for(track: &TrackMeta) -> Option<RandomAccessStreamReference> {
    if let Some(bytes) = track.cover_bytes.as_deref() {
        match thumbnail_from_bytes(bytes) {
            Ok(r) => return Some(r),
            Err(e) => {
                tracing::debug!(
                    target: "qobee::smtc",
                    track_id = track.id,
                    error = %e,
                    "cover_bytes failed to encode; falling back to placeholder"
                );
            }
        }
    }
    placeholder_thumbnail()
}

/// Windows SMTC bridge.
///
/// `Send + Sync`: the WinRT runtime classes stored inside are both
/// `Send + Sync` (their `unsafe impl`s live in the `windows`
/// crate). The `Mutex` around the controls handle is purely
/// defensive: WinRT lets us call property setters from any thread,
/// but we serialise our event-driven mutations so two consecutive
/// `handle_event` calls (e.g. fast-skipping tracks) cannot
/// interleave a half-updated `DisplayUpdater::Update()`.
pub struct SmtcBridge {
    controls: Mutex<SystemMediaTransportControls>,
}

impl SmtcBridge {
    /// Initialise the SMTC bridge attached to the main window's
    /// HWND. Returns `Err` when:
    ///
    /// - the window does not yet have a native HWND,
    /// - `RoGetActivationFactory(ISystemMediaTransportControlsInterop)`
    ///   fails (extremely rare; would indicate a busted WinRT
    ///   stack),
    /// - `GetForWindow(hwnd)` fails (e.g. the HWND is not a
    ///   top-level window).
    ///
    /// The fan-out task in `lib::run::setup` is expected to log a
    /// warning and continue without a bridge in any of those cases
    /// — Qobee remains fully functional, just without SMTC.
    pub fn init(window: &WebviewWindow, app: AppHandle) -> Result<Self> {
        let hwnd: HWND = window
            .hwnd()
            .map_err(|e| anyhow!("WebviewWindow::hwnd failed: {e}"))?;
        // Resolve `ISystemMediaTransportControlsInterop` via the
        // standard WinRT activation-factory route, then ask it for
        // the per-window `SystemMediaTransportControls` instance.
        let interop: ISystemMediaTransportControlsInterop =
            windows::core::factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
        // SAFETY: `interop` was just resolved successfully and the
        // HWND was just obtained from a live `tauri::WebviewWindow`,
        // so it points at a top-level window owned by this process
        // — both invariants required by `GetForWindow`.
        let controls: SystemMediaTransportControls =
            unsafe { interop.GetForWindow::<SystemMediaTransportControls>(hwnd)? };

        // Enable the surface and the buttons we route — Stop is
        // optional but exposing it lets the user clear the Now
        // Playing chip from the volume mixer cleanly.
        controls.SetIsEnabled(true)?;
        controls.SetIsPlayEnabled(true)?;
        controls.SetIsPauseEnabled(true)?;
        controls.SetIsStopEnabled(true)?;
        controls.SetIsNextEnabled(true)?;
        controls.SetIsPreviousEnabled(true)?;

        // Initial status — we're idle until the first `Started`
        // event arrives.
        controls.SetPlaybackStatus(MediaPlaybackStatus::Closed)?;

        // Wire ButtonPressed → PlayerHandle.
        let app_buttons = app.clone();
        let button_handler = TypedEventHandler::new(
            move |_sender: windows::core::Ref<SystemMediaTransportControls>,
                  args: windows::core::Ref<SystemMediaTransportControlsButtonPressedEventArgs>| {
                if let Some(args) = args.as_ref() {
                    if let Ok(button) = args.Button() {
                        dispatch_button(&app_buttons, button);
                    }
                }
                Ok(())
            },
        );
        controls.ButtonPressed(&button_handler)?;

        // Wire PlaybackPositionChangeRequested → PlayerHandle::seek.
        let app_seek = app.clone();
        let seek_handler = TypedEventHandler::new(
            move |_sender: windows::core::Ref<SystemMediaTransportControls>,
                  args: windows::core::Ref<PlaybackPositionChangeRequestedEventArgs>| {
                if let Some(args) = args.as_ref() {
                    if let Ok(ts) = args.RequestedPlaybackPosition() {
                        // `Duration` is in 100 ns ticks; convert
                        // to seconds for the engine (which takes
                        // an `f64`).
                        let seconds = (ts.Duration as f64) / 1.0e7;
                        if let Some(state) = app_seek.try_state::<AppState>() {
                            if let Err(e) = state.player().seek(seconds.max(0.0)) {
                                tracing::warn!(
                                    target: "qobee::smtc",
                                    error = %e,
                                    "seek requested by SMTC failed"
                                );
                            }
                        }
                    }
                }
                Ok(())
            },
        );
        controls.PlaybackPositionChangeRequested(&seek_handler)?;

        Ok(SmtcBridge {
            controls: Mutex::new(controls),
        })
    }
}

/// Project the metadata + timeline of a freshly started track onto
/// the SMTC `DisplayUpdater` and `TimelineProperties`. Shared by
/// `Started` and `TrackChanged`.
fn publish_track(
    controls: &SystemMediaTransportControls,
    track: &TrackMeta,
    position_ms: u64,
    duration_ms: u64,
) -> windows::core::Result<()> {
    let updater = controls.DisplayUpdater()?;
    updater.SetType(MediaPlaybackType::Music)?;

    let music = updater.MusicProperties()?;
    music.SetTitle(&HSTRING::from(track.title.as_str()))?;
    music.SetArtist(&HSTRING::from(track.artist.as_str()))?;
    music.SetAlbumTitle(&HSTRING::from(track.album.as_str()))?;

    if let Some(thumb) = thumbnail_for(track) {
        // Errors are non-fatal — if SMTC rejects the thumbnail
        // (extremely rare), we keep the rest of the metadata.
        let _ = updater.SetThumbnail(&thumb);
    }

    updater.Update()?;

    let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
    timeline.SetStartTime(ms_to_timespan(0))?;
    timeline.SetMinSeekTime(ms_to_timespan(0))?;
    timeline.SetEndTime(ms_to_timespan(duration_ms))?;
    timeline.SetMaxSeekTime(ms_to_timespan(duration_ms))?;
    timeline.SetPosition(ms_to_timespan(position_ms.min(duration_ms)))?;
    controls.UpdateTimelineProperties(&timeline)?;

    Ok(())
}

/// Update only the timeline `Position` without rebuilding the
/// metadata block. Shared by `PositionTick`, `Paused`, `Resumed`.
fn publish_position(
    controls: &SystemMediaTransportControls,
    position_ms: u64,
) -> windows::core::Result<()> {
    let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
    timeline.SetStartTime(ms_to_timespan(0))?;
    timeline.SetMinSeekTime(ms_to_timespan(0))?;
    // We don't know the current track's duration here; the OS
    // accepts a 0-end timeline with a non-zero position by clamping
    // the displayed bar. The next `Started` / `TrackChanged` will
    // re-publish a complete timeline.
    timeline.SetEndTime(ms_to_timespan(0))?;
    timeline.SetMaxSeekTime(ms_to_timespan(0))?;
    timeline.SetPosition(ms_to_timespan(position_ms))?;
    controls.UpdateTimelineProperties(&timeline)?;
    Ok(())
}

impl MediaBridge for SmtcBridge {
    fn handle_event(&self, ev: &PlayerEvent) {
        let controls = self.controls.lock();
        let result: windows::core::Result<()> = match ev {
            PlayerEvent::Started {
                track,
                position_ms,
                duration_ms,
            } => controls
                .SetPlaybackStatus(MediaPlaybackStatus::Playing)
                .and_then(|_| publish_track(&controls, track, *position_ms, *duration_ms)),
            PlayerEvent::TrackChanged { track } => {
                // Preserve the current playback status — we only
                // refresh the metadata. Use the duration carried
                // by the track and rewind position to 0 because
                // a track-change always restarts from the head.
                publish_track(&controls, track, 0, track.duration_ms)
            }
            PlayerEvent::Paused { position_ms } => controls
                .SetPlaybackStatus(MediaPlaybackStatus::Paused)
                .and_then(|_| publish_position(&controls, *position_ms)),
            PlayerEvent::Resumed { position_ms } => controls
                .SetPlaybackStatus(MediaPlaybackStatus::Playing)
                .and_then(|_| publish_position(&controls, *position_ms)),
            PlayerEvent::Stopped => controls.SetPlaybackStatus(MediaPlaybackStatus::Stopped),
            PlayerEvent::PositionTick { position_ms } => publish_position(&controls, *position_ms),
            PlayerEvent::Errored { .. } => {
                // R1.3 / R8.7 — collapse a hard error to Stopped
                // on the OS surface so the user does not see a
                // stale "Playing" indicator.
                controls.SetPlaybackStatus(MediaPlaybackStatus::Stopped)
            }
            // Legacy variants — not part of the R8 transport bus,
            // ignored on purpose. They remain on the same enum so
            // the legacy crossbeam pump keeps working without
            // duplicating the type surface.
            PlayerEvent::StateChanged { .. }
            | PlayerEvent::Position { .. }
            | PlayerEvent::EndOfTrack
            | PlayerEvent::BitPerfectChanged { .. }
            | PlayerEvent::Error { .. } => Ok(()),
        };

        if let Err(e) = result {
            tracing::warn!(
                target: "qobee::smtc",
                event = ?event_kind(ev),
                error = %e,
                "SMTC update failed"
            );
        }
    }
}

/// Map a `SystemMediaTransportControlsButton` press onto the
/// matching `PlayerHandle` action. Errors are logged but never
/// surfaced — the OS handler is fire-and-forget.
fn dispatch_button(app: &AppHandle, button: SystemMediaTransportControlsButton) {
    let Some(state) = app.try_state::<AppState>() else {
        tracing::warn!(target: "qobee::smtc", "AppState missing in SMTC button handler");
        return;
    };
    let player = state.player();
    let result = match button {
        // Some senders (e.g. the keyboard media key) emit `Play`
        // when the user means "toggle"; the engine reports the
        // current status so we can branch correctly.
        SystemMediaTransportControlsButton::Play => match player.state().status {
            qobee_engine::PlaybackStatus::Paused => player.resume(),
            qobee_engine::PlaybackStatus::Playing => Ok(()),
            _ => player.play(),
        },
        SystemMediaTransportControlsButton::Pause => player.pause(),
        SystemMediaTransportControlsButton::Stop => player.stop(),
        SystemMediaTransportControlsButton::Next => player.next(),
        SystemMediaTransportControlsButton::Previous => player.previous(),
        // FastForward / Rewind / Record / ChannelUp / ChannelDown
        // are not part of the R1 surface; they remain disabled at
        // the controls level, but a misbehaving sender could still
        // emit one. Treat them as no-ops.
        _ => Ok(()),
    };
    if let Err(e) = result {
        tracing::warn!(
            target: "qobee::smtc",
            button = ?button,
            error = %e,
            "SMTC button dispatch failed"
        );
    }
}

/// Render a short human-readable kind tag for tracing.
fn event_kind(ev: &PlayerEvent) -> &'static str {
    match ev {
        PlayerEvent::Started { .. } => "started",
        PlayerEvent::TrackChanged { .. } => "track_changed",
        PlayerEvent::Paused { .. } => "paused",
        PlayerEvent::Resumed { .. } => "resumed",
        PlayerEvent::Stopped => "stopped",
        PlayerEvent::PositionTick { .. } => "position_tick",
        PlayerEvent::Errored { .. } => "errored",
        PlayerEvent::StateChanged { .. } => "state_changed",
        PlayerEvent::Position { .. } => "position",
        PlayerEvent::EndOfTrack => "end_of_track",
        PlayerEvent::BitPerfectChanged { .. } => "bit_perfect",
        PlayerEvent::Error { .. } => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity check: the embedded placeholder PNG is non-trivial
    /// (more than just the PNG signature). Catches accidental
    /// truncation of the asset during tooling changes.
    #[test]
    fn placeholder_cover_is_non_trivial() {
        assert!(
            PLACEHOLDER_COVER_BYTES.len() > 100,
            "placeholder_cover.png unexpectedly small"
        );
        // PNG signature.
        assert_eq!(
            &PLACEHOLDER_COVER_BYTES[..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );
    }

    #[test]
    fn ms_to_timespan_round_trips() {
        assert_eq!(ms_to_timespan(0).Duration, 0);
        assert_eq!(ms_to_timespan(1).Duration, 10_000);
        assert_eq!(ms_to_timespan(1000).Duration, 10_000_000);
    }
}
