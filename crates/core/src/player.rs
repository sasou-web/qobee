//! Player orchestration: ties the audio engine to the library and
//! exposes a unified surface to the application layer.
//!
//! The player owns a [`CpalSharedEngine`] (always) plus a lazily-built
//! [`WasapiExclusiveEngine`] on Windows. The active backend is held
//! behind a `Mutex<Arc<dyn AudioEngine>>` and swapped when the user
//! changes the output mode setting. It pumps engine events on a
//! worker thread, enriches them with the current track id, and
//! re-publishes them as [`crate::PlayerEvent`].
//!
//! ## Smoothness improvements
//!
//! - **Pre-fetch**: when a track gets close to the end the next track
//!   is opened in advance (Symphonia probe + decoder build) so the
//!   transition is immediate. Symphonia's open path is the slowest
//!   step on a slow disk; doing it during the previous track's tail
//!   keeps the gap below human perception.
//! - **Auto-advance**: the engine's `EndOfTrack` event triggers the
//!   next queued track without waiting for a user click.
//! - **Recently-played**: every new track start is recorded in the
//!   library DB so the home page reflects what the user is listening
//!   to right now.
//! - **ReplayGain**: per-track or per-album normalization applied
//!   pre-EQ at track start, mode persisted in the settings table.
//! - **Auto-fallback to Shared** if WASAPI Exclusive can't open the
//!   device (busy, unsupported format, etc.). The user gets a soft
//!   warning toast instead of a stuck red banner, and the persisted
//!   mode is rewritten so the next launch doesn't fail the same way.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::broadcast;

#[cfg(target_os = "windows")]
use qobee_engine::backend_wasapi_exclusive::WasapiExclusiveEngine;
use qobee_engine::{
    backend_cpal_shared::{list_output_devices, CpalSharedEngine},
    AudioEngine, EngineError, EngineEvent, OutputDevice, OutputMode, PlaybackStatus, PlayerState,
    PreGainContext,
};
use qobee_library::{Library, LibraryError};

use crate::queue::{Queue, QueueSnapshot, RepeatMode, TrackId};
use crate::{PlayerErrorKind, PlayerEvent, ReplayGainMode, TrackMeta};

/// Map a library-side `DsdRate` to the engine's identical-but-
/// independent enum. Both layers carry the same four-variant set
/// (DSD64–DSD512); we keep them as separate types so neither crate
/// has to depend on the other.
fn engine_dsd_rate_from_library(r: qobee_library::DsdRate) -> qobee_engine::DsdRate {
    match r {
        qobee_library::DsdRate::Dsd64 => qobee_engine::DsdRate::Dsd64,
        qobee_library::DsdRate::Dsd128 => qobee_engine::DsdRate::Dsd128,
        qobee_library::DsdRate::Dsd256 => qobee_engine::DsdRate::Dsd256,
        qobee_library::DsdRate::Dsd512 => qobee_engine::DsdRate::Dsd512,
    }
}

#[derive(Debug, Error)]
pub enum PlayerError {
    #[error("engine error: {0}")]
    Engine(#[from] EngineError),
    #[error("library error: {0}")]
    Library(#[from] LibraryError),
    #[error("track not found: {0}")]
    TrackNotFound(TrackId),
    #[error("queue is empty")]
    QueueEmpty,
}

pub type PlayerResult<T> = Result<T, PlayerError>;

/// Capacity of the [`broadcast::Sender`] sitting on the R8
/// transport bus. 256 is enough to absorb a brief lag of one of
/// the OS bridges (SMTC / MPNowPlayingInfoCenter) without dropping
/// transitions; the pump's worst-case rate is ~4 events/s
/// (250 ms `PositionTick` throttle).
const BROADCAST_CAPACITY: usize = 256;

/// `PositionTick` throttle window. The decoder emits position
/// updates at chunk cadence (typically every ~10–20 ms); we
/// resample to one tick every 250 ms before broadcasting so the
/// UI / OS bridges aren't flooded.
const POSITION_TICK_THROTTLE: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct PlayerHandle {
    inner: Arc<PlayerInner>,
}

struct PlayerInner {
    /// Shared output (CPAL → WASAPI Shared on Windows). Always built;
    /// it's the default and the fallback when Exclusive can't open
    /// the device.
    shared_engine: Arc<CpalSharedEngine>,
    /// WASAPI Exclusive engine (Windows-only). Built lazily the
    /// first time the user enables Exclusive mode so app startup
    /// stays fast for the 99 % of users who never touch it.
    #[cfg(target_os = "windows")]
    exclusive_engine: Mutex<Option<Arc<WasapiExclusiveEngine>>>,
    /// The currently active backend, behind a lock so we can swap it
    /// from a setting change without racing with calls in flight.
    /// Reads use `engine()` to grab a clone of the Arc cheaply.
    active_engine: Mutex<Arc<dyn AudioEngine>>,
    /// Currently active backend: 0 = Shared, 1 = Exclusive. Mirrors
    /// `active_engine` but is cheap to read for snapshots.
    active_backend: AtomicU8,
    library: Library,
    queue: Queue,
    current_track_id: Mutex<Option<TrackId>>,
    /// Track id we have already "warmed" (probe + first-bytes touch).
    /// Reset on each new playback start.
    prefetched_track: Mutex<Option<TrackId>>,
    /// "Endless" mode: when reaching the end of the queue (either via
    /// auto-advance or a manual `next()`), the player picks a random
    /// album and starts it instead of stopping. Toggled from the UI's
    /// "Up next" panel.
    endless: AtomicBool,
    /// Cached volume slider position requested by the user while a
    /// DSD track is playing (R7.8). Applied automatically on the
    /// next PCM track via `start_track`. `None` when no override is
    /// pending.
    cached_volume: Mutex<Option<f32>>,
    /// Cached 10-band EQ gains requested while DSD is playing. Same
    /// semantics as `cached_volume`.
    cached_eq_gains_db: Mutex<Option<Vec<f32>>>,
    /// ReplayGain mode encoded as `u8` (0=Off, 1=Track, 2=Album) so
    /// every read is a relaxed atomic. Mirrors the Tauri setting.
    replaygain_mode: AtomicU8,
    /// Crossfade window in milliseconds (`0` = off). Mirrors the
    /// `playback.crossfade_ms` setting; read by `prefetch_next_if_any`
    /// to widen the pre-fetch lead time so the incoming track is
    /// ready before the fade window opens.
    crossfade_ms: AtomicU32,
    event_tx: Sender<PlayerEvent>,
    event_rx: Receiver<PlayerEvent>,
    /// R8 transport bus: every player-side transition (`Started`,
    /// `Paused`, `Resumed`, `Stopped`, `TrackChanged`,
    /// `PositionTick`, `Errored`) is fan-out via this
    /// `tokio::sync::broadcast` channel. Sits next to the legacy
    /// crossbeam `event_tx` so existing consumers keep working.
    broadcast_tx: broadcast::Sender<PlayerEvent>,
    /// Sleep timer: wall-clock instant at which playback should be
    /// paused, or `None` when no timer is armed. A background
    /// monitor thread polls this and pauses the engine when the
    /// deadline passes. Set via [`PlayerHandle::set_sleep_timer`].
    sleep_deadline: Mutex<Option<Instant>>,
    /// When `true`, the armed sleep timer fires at the *end of the
    /// current track* rather than at a wall-clock deadline. The
    /// `EndOfTrack` handler checks this latch and stops instead of
    /// advancing.
    sleep_stop_after_track: AtomicBool,
}

impl PlayerInner {
    /// Returns the engine currently routing audio. Cheap: clones an
    /// `Arc<dyn AudioEngine>` (refcount bump only).
    fn engine(&self) -> Arc<dyn AudioEngine> {
        Arc::clone(&self.active_engine.lock())
    }

    /// Broadcast a transport bus event, dropping silently when no
    /// receiver is currently subscribed (the broadcast sender
    /// surfaces this case via `Err(SendError)`; semantically a
    /// no-op for us — UI / OS bridges may simply not be wired
    /// yet during early init).
    fn broadcast(&self, ev: PlayerEvent) {
        let _ = self.broadcast_tx.send(ev);
    }

    /// Build a [`TrackMeta`] from a library row. Reads the cover
    /// blob from the cover cache directory when a `cover_key` is
    /// available so the OS bridges can render a thumbnail; the
    /// frontend itself never receives the bytes (cf. `#[serde(skip)]`
    /// on `TrackMeta::cover_bytes`).
    fn track_meta_for(&self, track_id: TrackId) -> Option<TrackMeta> {
        let track = self.library.get_track(track_id).ok().flatten()?;
        let duration_ms = (track.duration_seconds * 1000.0).max(0.0) as u64;
        let cover_bytes = track.cover_key.as_deref().and_then(|key| {
            // Cover keys use forward-slashes (`aa/abcd….jpg`); the
            // cache directory uses native separators.
            let normalized = key.replace('/', std::path::MAIN_SEPARATOR_STR);
            let path = self.library.cover_cache_dir().join(normalized);
            std::fs::read(&path).ok()
        });
        Some(TrackMeta {
            id: track.id,
            title: track.title,
            artist: track.artist,
            album: track.album,
            duration_ms,
            cover_bytes,
        })
    }

    /// Direct handle to the always-present `CpalSharedEngine`. Used
    /// by `crate::diagnostic::run_null_test` to invoke the pre-render
    /// methods that are inherent on the concrete type and not part
    /// of the `AudioEngine` trait.
    pub(crate) fn shared_engine(&self) -> Arc<CpalSharedEngine> {
        Arc::clone(&self.shared_engine)
    }

    /// Subscribe to engine events. Cheap: clones a crossbeam-channel
    /// receiver. Used by `run_null_test` to wait for `EndOfTrack`.
    pub(crate) fn subscribe_engine_events(&self) -> Receiver<EngineEvent> {
        self.shared_engine.subscribe_events()
    }

    fn resolve_path(&self, track_id: TrackId) -> PlayerResult<PathBuf> {
        let track = self
            .library
            .get_track(track_id)?
            .ok_or(PlayerError::TrackNotFound(track_id))?;
        Ok(PathBuf::from(track.path))
    }

    /// User-facing output mode mirrored from the active backend
    /// flag. `start_track` consults this before loading a DSD track
    /// so it can refuse the load in Shared mode (R7.6) without
    /// forcing the orchestrator to expose `current_output_mode` as
    /// a public method on the inner type.
    fn current_output_mode_inner(&self) -> OutputMode {
        match self.active_backend.load(Ordering::Acquire) {
            1 => OutputMode::Exclusive,
            _ => OutputMode::Shared,
        }
    }

    fn start_track(&self, track_id: TrackId, path: PathBuf) -> PlayerResult<()> {
        // Apply ReplayGain before loading the track so the new pre-gain
        // is in effect from the very first sample. Falls back to 0 dB
        // when the file has no RG tag or the mode is "off".
        let track = self
            .library
            .get_track(track_id)?
            .ok_or(PlayerError::TrackNotFound(track_id))?;

        // R7.6 — DSD playback requires WASAPI Exclusive. Refuse
        // any DSD load early when the engine is currently routed
        // through Shared and surface a localised error so the UI
        // can guide the user to Settings → Audio.
        let dsd_rate = track.dsd_rate();
        if dsd_rate.is_some() && self.current_output_mode_inner() != OutputMode::Exclusive {
            let _ = self.event_tx.send(PlayerEvent::Error {
                message: "La lecture DSD requiert le mode Exclusive (Settings → Audio)".to_string(),
            });
            return Ok(());
        }
        // R7.8 — When transitioning back to PCM playback after a
        // DSD track, replay any volume / EQ slider moves the user
        // made while DSD was active. The cache is one-shot so the
        // user can keep tweaking on the new PCM track without us
        // overwriting their fresh changes.
        if dsd_rate.is_none() {
            if let Some(v) = self.cached_volume.lock().take() {
                let _ = self.engine().set_volume(v);
            }
            if let Some(g) = self.cached_eq_gains_db.lock().take() {
                self.engine().set_eq_gains_db(&g);
            }
        }
        let mode = match self.replaygain_mode.load(Ordering::Relaxed) {
            1 => ReplayGainMode::Track,
            2 => ReplayGainMode::Album,
            _ => ReplayGainMode::Off,
        };
        let rg_db = match mode {
            ReplayGainMode::Off => None,
            ReplayGainMode::Track => track.replaygain_track_db.or(track.replaygain_album_db),
            ReplayGainMode::Album => track.replaygain_album_db.or(track.replaygain_track_db),
        };
        let pre_gain_linear = match rg_db {
            Some(db) => 10f32.powf(db.clamp(-24.0, 12.0) / 20.0),
            None => 1.0,
        };
        self.engine().set_pre_gain(pre_gain_linear);

        // Phase B (task 8): also publish the per-load context to the
        // new `Pre_Gain_Stage`. The stage uses `rg_peak` to clamp the
        // demanded RG gain against the configured true-peak ceiling
        // (R1.2/R1.3); the legacy `set_pre_gain` above stays in place
        // until task 18 wires `PcmChain` into `run_decoder_thread`.
        let rg_peak = match mode {
            ReplayGainMode::Off => None,
            ReplayGainMode::Track => track.replaygain_track_peak.or(track.replaygain_album_peak),
            ReplayGainMode::Album => track.replaygain_album_peak.or(track.replaygain_track_peak),
        };
        let slider = self.engine().state().volume;
        self.engine().set_pre_gain_context(PreGainContext {
            rg_db,
            rg_peak,
            slider,
        });
        if let Some(db) = rg_db {
            tracing::debug!(
                target: "qobee::core",
                track_id,
                rg_db = db,
                pre_gain = pre_gain_linear,
                "ReplayGain pre-gain applied"
            );
        }

        *self.current_track_id.lock() = Some(track_id);
        *self.prefetched_track.lock() = None;
        // The engine may still be holding a prepared next track from
        // before this manual jump (e.g. user pressed Next during the
        // last 3 seconds of a song). Drop it so it doesn't get
        // gapless-swapped after the explicit Load below.
        let _ = self.engine().clear_pending_next();
        self.engine()
            .set_current_track_id(Some(track_id.to_string()));
        // R7.3 — DSD tracks take a dedicated load path that opens
        // the device in 24-in-32 at the DoP carrier rate and
        // bypasses every PCM stage. Falls back to a regular load
        // when the engine returns BackendUnavailable (e.g. a non-
        // Windows build).
        if let Some(rate) = dsd_rate {
            let dsd_rate = engine_dsd_rate_from_library(rate);
            match self.engine().load_dsd(&path, dsd_rate) {
                Ok(()) => {}
                Err(EngineError::BackendUnavailable(_)) => {
                    let _ = self.event_tx.send(PlayerEvent::Error {
                        message: "DSD playback unavailable on this platform".to_string(),
                    });
                    return Ok(());
                }
                Err(e) => return Err(PlayerError::Engine(e)),
            }
        } else {
            self.engine().load(&path)?;
        }
        self.engine().play()?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Err(e) = self.library.record_play(track_id, now) {
            tracing::warn!(target: "qobee::core", error = %e, "record_play failed");
        }

        // Broadcast a `Started` transition on the R8 transport
        // bus so the UI store and OS bridges can refresh their
        // Now Playing surface in lock-step. The fan-out task in
        // `src-tauri::setup` (cf. design §Player_Sync) translates
        // a `TrackChanged` for in-queue advances; here we always
        // emit `Started` because `start_track` is the single
        // entry point used by both the initial play and manual
        // jumps within a queue, and the spec models both cases
        // as a freshly-started playback session.
        if let Some(meta) = self.track_meta_for(track_id) {
            let duration_ms = meta.duration_ms;
            self.broadcast(PlayerEvent::Started {
                track: meta,
                position_ms: 0,
                duration_ms,
            });
        }
        Ok(())
    }

    /// Try to advance to the next track in the queue.
    fn advance(&self) -> PlayerResult<bool> {
        match self.queue.advance() {
            Some(id) => {
                let path = self.resolve_path(id)?;
                self.start_track(id, path)?;
                Ok(true)
            }
            None => {
                self.engine().stop()?;
                *self.current_track_id.lock() = None;
                Ok(false)
            }
        }
    }

    /// Prepare the next queued track for a gapless transition.
    /// Calls the engine's `prepare_next` so the file is opened and
    /// (when the format matches) the active decoder thread can swap
    /// in place at end-of-stream — no audible gap. Idempotent: only
    /// prepares once per "current" track. Errors are best-effort.
    fn prefetch_next_if_any(&self) {
        let next_id = self.queue.peek_next();
        let already = *self.prefetched_track.lock();
        match (next_id, already) {
            (Some(id), Some(prev)) if prev == id => (),
            (Some(id), _) => {
                if let Ok(path) = self.resolve_path(id) {
                    if let Err(e) = self.engine().prepare_next(&path, Some(id.to_string())) {
                        tracing::debug!(
                            target: "qobee::core",
                            error = %e,
                            "prepare_next failed; will fall back to non-gapless transition"
                        );
                    }
                    *self.prefetched_track.lock() = Some(id);
                }
            }
            (None, _) => {}
        }
    }
}

pub struct Player {
    inner: Arc<PlayerInner>,
    _pump: JoinHandle<()>,
}

impl Player {
    pub fn new(library: Library) -> PlayerResult<Self> {
        let shared_engine = Arc::new(CpalSharedEngine::new()?);
        let (event_tx, event_rx) = bounded::<PlayerEvent>(256);
        let (broadcast_tx, _) = broadcast::channel::<PlayerEvent>(BROADCAST_CAPACITY);

        let active_engine: Arc<dyn AudioEngine> =
            Arc::clone(&shared_engine) as Arc<dyn AudioEngine>;

        let inner = Arc::new(PlayerInner {
            shared_engine: Arc::clone(&shared_engine),
            #[cfg(target_os = "windows")]
            exclusive_engine: Mutex::new(None),
            active_engine: Mutex::new(active_engine),
            active_backend: AtomicU8::new(0),
            library,
            queue: Queue::new(),
            current_track_id: Mutex::new(None),
            prefetched_track: Mutex::new(None),
            endless: AtomicBool::new(true),
            replaygain_mode: AtomicU8::new(0),
            crossfade_ms: AtomicU32::new(0),
            cached_volume: Mutex::new(None),
            cached_eq_gains_db: Mutex::new(None),
            event_tx,
            event_rx,
            broadcast_tx,
            sleep_deadline: Mutex::new(None),
            sleep_stop_after_track: AtomicBool::new(false),
        });

        // Pump events from the Shared engine. When the user toggles
        // Exclusive, we spawn a second pump thread for that engine —
        // both write to the same player event channel so the UI sees
        // a unified stream of state updates.
        let pump_inner = Arc::clone(&inner);
        let engine_events = shared_engine.subscribe_events();
        let pump = thread::Builder::new()
            .name("qobee-core-pump-shared".into())
            .spawn(move || pump_engine_events(pump_inner, engine_events))
            .map_err(|e| PlayerError::Engine(EngineError::Internal(e.to_string())))?;

        // Sleep-timer monitor. A cheap 1 s poll that pauses the
        // engine once the armed wall-clock deadline passes. Kept as
        // its own detached thread so it never blocks the event pump;
        // it holds a clone of the inner handle and exits when the
        // last `Player` is dropped (the `Arc` upgrade fails).
        let sleep_inner = Arc::downgrade(&inner);
        thread::Builder::new()
            .name("qobee-core-sleep-timer".into())
            .spawn(move || loop {
                thread::sleep(Duration::from_millis(1000));
                let Some(inner) = sleep_inner.upgrade() else {
                    break; // player dropped
                };
                let fire = {
                    let mut slot = inner.sleep_deadline.lock();
                    match *slot {
                        Some(deadline) if Instant::now() >= deadline => {
                            *slot = None; // one-shot
                            true
                        }
                        _ => false,
                    }
                };
                if fire {
                    tracing::info!(target: "qobee::core", "sleep timer elapsed; pausing playback");
                    if let Err(e) = inner.engine().pause() {
                        tracing::warn!(target: "qobee::core", error = %e, "sleep-timer pause failed");
                    }
                }
            })
            .map_err(|e| PlayerError::Engine(EngineError::Internal(e.to_string())))?;

        Ok(Player { inner, _pump: pump })
    }

    pub fn handle(&self) -> PlayerHandle {
        PlayerHandle {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl PlayerHandle {
    pub fn subscribe(&self) -> Receiver<PlayerEvent> {
        self.inner.event_rx.clone()
    }

    /// Subscribe to the R8 transport bus. Returns a fresh
    /// `tokio::sync::broadcast::Receiver` that yields every
    /// transition emitted by the player after the call. Cheap to
    /// create — capacity is shared, the receiver only stores its
    /// own read cursor. Safe to call any number of times (UI store,
    /// SMTC bridge, MPNowPlayingInfoCenter bridge each get their
    /// own receiver).
    ///
    /// Slow consumers receive `RecvError::Lagged(n)` when they fall
    /// more than [`BROADCAST_CAPACITY`] events behind; the design
    /// asks them to resnapshot via [`Self::state`] in that case.
    pub fn subscribe_broadcast(&self) -> broadcast::Receiver<PlayerEvent> {
        self.inner.broadcast_tx.subscribe()
    }

    pub fn state(&self) -> PlayerState {
        let mut state = self.inner.engine().state();
        state.current_track_id = self.inner.current_track_id.lock().map(|id| id.to_string());
        state
    }

    /// Build a [`TrackMeta`] for the currently active track, when
    /// any. Used by the R8 fan-out task in `src-tauri::setup` to
    /// re-publish a synthetic `Started` / `Paused` event after a
    /// `broadcast::RecvError::Lagged`, so the OS bridges and the UI
    /// store can resnapshot without losing track info.
    pub fn current_track_meta(&self) -> Option<TrackMeta> {
        let track_id = (*self.inner.current_track_id.lock())?;
        self.inner.track_meta_for(track_id)
    }

    pub fn output_mode(&self) -> qobee_engine::types::EffectiveOutputMode {
        self.state().output_mode
    }

    /// User-facing output mode (Auto / Shared / Exclusive). Mirrors
    /// the persisted setting; only the *effective* mode is reported by
    /// `state().output_mode`.
    pub fn current_output_mode(&self) -> OutputMode {
        match self.inner.active_backend.load(Ordering::Acquire) {
            1 => OutputMode::Exclusive,
            _ => OutputMode::Shared,
        }
    }

    /// Direct accessor to the always-present CPAL Shared engine.
    /// Used by `crate::diagnostic::run_null_test` to drive the
    /// pre-render path (inherent methods on `CpalSharedEngine`).
    pub(crate) fn shared_engine(&self) -> Arc<CpalSharedEngine> {
        self.inner.shared_engine()
    }

    /// Subscribe to raw engine events. Used by `run_null_test` to
    /// wait for `EndOfTrack` without going through the player event
    /// re-publisher.
    pub(crate) fn subscribe_engine_events(&self) -> Receiver<EngineEvent> {
        self.inner.subscribe_engine_events()
    }

    pub fn play_track(&self, track_id: TrackId) -> PlayerResult<()> {
        // Click-on-row plays the surrounding album so the listener
        // gets album-flow by default. We fall back to single-track
        // playback if the album lookup fails.
        if let Ok(Some(detail)) = self.inner.library.get_album_for_track(track_id) {
            if !detail.tracks.is_empty() {
                let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
                let start = ids.iter().position(|id| *id == track_id);
                self.inner.queue.replace(ids, start);
                let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
                let path = self.inner.resolve_path(cur)?;
                return self.inner.start_track(cur, path);
            }
        }
        let path = self.inner.resolve_path(track_id)?;
        self.inner.queue.replace_with_track(track_id);
        self.inner.start_track(track_id, path)
    }

    pub fn play_album_from_track(
        &self,
        album_id: i64,
        start_track_id: TrackId,
    ) -> PlayerResult<()> {
        let detail = self
            .inner
            .library
            .get_album(album_id)?
            .ok_or(PlayerError::TrackNotFound(start_track_id))?;
        if detail.tracks.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
        let start = ids.iter().position(|id| *id == start_track_id);
        self.inner.queue.replace(ids, start);
        let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
        let path = self.inner.resolve_path(cur)?;
        self.inner.start_track(cur, path)
    }

    pub fn play_playlist_from_track(
        &self,
        playlist_id: i64,
        start_track_id: TrackId,
    ) -> PlayerResult<()> {
        let detail = self
            .inner
            .library
            .get_playlist(playlist_id)?
            .ok_or(PlayerError::TrackNotFound(start_track_id))?;
        if detail.tracks.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
        let start = ids.iter().position(|id| *id == start_track_id);
        self.inner.queue.replace(ids, start);
        let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
        let path = self.inner.resolve_path(cur)?;
        self.inner.start_track(cur, path)
    }

    /// Start playing an album from its first track. Used by the R3
    /// `Play_Button` when the user clicks Play on an album card or
    /// on the album detail header — the UI carries an album id
    /// without knowing the first track id, so we look it up here.
    pub fn play_album(&self, album_id: i64) -> PlayerResult<()> {
        let detail = self
            .inner
            .library
            .get_album(album_id)?
            .ok_or(PlayerError::QueueEmpty)?;
        if detail.tracks.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
        self.inner.queue.replace(ids, Some(0));
        let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
        let path = self.inner.resolve_path(cur)?;
        self.inner.start_track(cur, path)
    }

    /// Start playing a playlist from its first track. Companion to
    /// [`play_album`] for the R3 `Play_Button` on playlist cards.
    pub fn play_playlist(&self, playlist_id: i64) -> PlayerResult<()> {
        let detail = self
            .inner
            .library
            .get_playlist(playlist_id)?
            .ok_or(PlayerError::QueueEmpty)?;
        if detail.tracks.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
        self.inner.queue.replace(ids, Some(0));
        let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
        let path = self.inner.resolve_path(cur)?;
        self.inner.start_track(cur, path)
    }

    pub fn play_tracks(&self, track_ids: Vec<TrackId>, start: Option<usize>) -> PlayerResult<()> {
        if track_ids.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        self.inner.queue.replace(track_ids, start);
        let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
        let path = self.inner.resolve_path(cur)?;
        self.inner.start_track(cur, path)
    }

    /// Insert `track_ids` right after the currently-playing track. If
    /// nothing is currently playing, fall back to starting playback.
    pub fn play_next(&self, track_ids: Vec<TrackId>) -> PlayerResult<()> {
        if track_ids.is_empty() {
            return Ok(());
        }
        if self.inner.current_track_id.lock().is_none() || self.inner.queue.is_empty() {
            // Nothing is currently playing; behave like a normal play.
            return self.play_tracks(track_ids, Some(0));
        }
        self.inner.queue.play_next(track_ids);
        Ok(())
    }

    /// Append `track_ids` at the end of the current queue.
    pub fn add_to_queue(&self, track_ids: Vec<TrackId>) -> PlayerResult<()> {
        if track_ids.is_empty() {
            return Ok(());
        }
        if self.inner.current_track_id.lock().is_none() || self.inner.queue.is_empty() {
            return self.play_tracks(track_ids, Some(0));
        }
        self.inner.queue.append(track_ids);
        Ok(())
    }

    /// Snapshot of the queue (live order + cursor). Used by the UI to
    /// render the "Now Playing / Queue" screen.
    pub fn queue_snapshot(&self) -> QueueSnapshot {
        self.inner.queue.snapshot()
    }

    /// Restore a previously persisted session: rebuild the queue from
    /// `track_ids`, position the cursor at `cursor`, load the current
    /// track and seek to `position_seconds`, but leave it **paused**
    /// so playback doesn't start unprompted on launch. Best-effort:
    /// any track id no longer in the library is dropped, and if the
    /// resulting queue is empty the call is a no-op. Returns `true`
    /// when a track was restored.
    pub fn restore_session(
        &self,
        track_ids: Vec<TrackId>,
        cursor: usize,
        position_seconds: f64,
    ) -> PlayerResult<bool> {
        // Filter out ids that no longer resolve to a library row so a
        // stale session (after a rescan / file move) restores cleanly.
        let valid: Vec<TrackId> = track_ids
            .into_iter()
            .filter(|id| matches!(self.inner.library.get_track(*id), Ok(Some(_))))
            .collect();
        if valid.is_empty() {
            return Ok(false);
        }
        let start = cursor.min(valid.len() - 1);
        self.inner.queue.replace(valid, Some(start));
        let cur = match self.inner.queue.current() {
            Some(id) => id,
            None => return Ok(false),
        };
        let path = self.inner.resolve_path(cur)?;
        // Load the track but do NOT play: `start_track` always calls
        // `play()`, so we replicate the bookkeeping here and leave the
        // engine paused. The engine's `Load` lands in the `Paused`
        // state by default, which is exactly what we want.
        *self.inner.current_track_id.lock() = Some(cur);
        self.inner
            .engine()
            .set_current_track_id(Some(cur.to_string()));
        self.inner.engine().load(&path)?;
        if position_seconds > 1.0 {
            // Seek to the saved offset. The engine handles a seek on a
            // freshly-loaded paused stream by draining + reseeking the
            // decoder; the first play() resumes from there.
            let _ = self.inner.engine().seek(position_seconds);
        }
        Ok(true)
    }

    /// Remove the queue entry at `idx`. If the entry is the currently
    /// playing track, playback advances to the next item; if the
    /// queue empties out, the engine is stopped.
    pub fn queue_remove_at(&self, idx: usize) -> PlayerResult<()> {
        let snap = self.inner.queue.snapshot();
        let was_current = snap.cursor == Some(idx);
        let removed = self.inner.queue.remove_at(idx);
        if !removed {
            return Ok(());
        }
        if was_current {
            // The cursor now points to whatever sat *after* the
            // removed track (or stayed at the same numeric index,
            // which becomes the next track). Restart playback there.
            match self.inner.queue.current() {
                Some(id) => {
                    let path = self.inner.resolve_path(id)?;
                    self.inner.start_track(id, path)?;
                }
                None => {
                    self.inner.engine().stop()?;
                    *self.inner.current_track_id.lock() = None;
                }
            }
        }
        Ok(())
    }

    /// Reorder the queue by moving the entry at `from` to `to`. The
    /// currently playing track is preserved (its index is updated to
    /// follow the move).
    pub fn queue_move(&self, from: usize, to: usize) -> PlayerResult<()> {
        self.inner.queue.move_item(from, to);
        Ok(())
    }

    /// Jump straight to the queue entry at `idx`, restarting playback
    /// there. Equivalent to clicking a row in the "up next" panel.
    pub fn queue_jump_to(&self, idx: usize) -> PlayerResult<()> {
        let snap = self.inner.queue.snapshot();
        if idx >= snap.items.len() {
            return Err(PlayerError::QueueEmpty);
        }
        let id = snap.items[idx];
        // `replace` rebuilds the queue identically and moves the cursor
        // — cheap and avoids exposing internal cursor mutation on
        // `Queue`. The order is preserved because we feed back the
        // same list.
        self.inner.queue.replace(snap.items, Some(idx));
        let path = self.inner.resolve_path(id)?;
        self.inner.start_track(id, path)
    }

    /// Resume / start playback **idempotently** (R8.5).
    ///
    /// - When the engine is already in `Playing`, the call is a
    ///   silent no-op: no engine call, no `PlayerEvent` broadcast.
    /// - When the engine is in `Paused`, this delegates to
    ///   [`Self::resume`] and emits a `Resumed` transport event.
    /// - When the engine is in `Idle | Stopped | Loading | Errored`
    ///   without a current track, this is a no-op (use
    ///   `play_track`, `play_album_from_track`, ... to actually
    ///   start playback).
    pub fn play(&self) -> PlayerResult<()> {
        let status = self.inner.engine().state().status;
        match status {
            PlaybackStatus::Playing => Ok(()),
            PlaybackStatus::Paused => self.resume(),
            // Nothing currently loaded → caller must pick a target
            // via the dedicated `play_*` methods.
            _ => Ok(()),
        }
    }

    pub fn pause(&self) -> PlayerResult<()> {
        // R8.4 — Pausing while already Paused is a silent no-op:
        // no engine call, no broadcast event. Same for any state
        // where a Pause makes no semantic sense (Idle / Stopped /
        // Errored): we don't want to surface a spurious `Paused`
        // transition on the bus.
        let status = self.inner.engine().state().status;
        if !matches!(status, PlaybackStatus::Playing) {
            return Ok(());
        }
        self.inner.engine().pause()?;
        let position_ms = (self.inner.engine().state().position_seconds * 1000.0).max(0.0) as u64;
        self.inner.broadcast(PlayerEvent::Paused { position_ms });
        Ok(())
    }

    pub fn resume(&self) -> PlayerResult<()> {
        // R8.5 — Resuming while already Playing is a silent no-op.
        // Resuming from a non-Paused state (Idle / Stopped /
        // Errored) is also a no-op: there is nothing to resume; the
        // caller should pick a target via the dedicated `play_*`
        // methods.
        let status = self.inner.engine().state().status;
        if !matches!(status, PlaybackStatus::Paused) {
            return Ok(());
        }
        self.inner.engine().resume()?;
        let position_ms = (self.inner.engine().state().position_seconds * 1000.0).max(0.0) as u64;
        self.inner.broadcast(PlayerEvent::Resumed { position_ms });
        Ok(())
    }

    pub fn stop(&self) -> PlayerResult<()> {
        self.inner.engine().stop()?;
        *self.inner.current_track_id.lock() = None;
        self.inner.broadcast(PlayerEvent::Stopped);
        Ok(())
    }

    pub fn seek(&self, position_seconds: f64) -> PlayerResult<()> {
        self.inner.engine().seek(position_seconds)?;
        Ok(())
    }

    pub fn set_volume(&self, volume: f32) -> PlayerResult<()> {
        // R7.8 — While DSD is active the engine bypasses every PCM
        // stage, so volume / EQ / pre-gain mutations have no audible
        // effect on the current track. Cache the requested slider
        // position locally so it is applied automatically on the
        // next PCM track via `start_track`'s pre-gain plumbing,
        // and surface a soft error so the UI can toast the user.
        if self.inner.engine().is_dsd_active() {
            *self.inner.cached_volume.lock() = Some(volume);
            let _ = self.inner.event_tx.try_send(PlayerEvent::Error {
                message: "DSD playback: DSP read-only".to_string(),
            });
            return Ok(());
        }
        self.inner.engine().set_volume(volume)?;
        Ok(())
    }

    pub fn set_output_mode(&self, mode: OutputMode) -> PlayerResult<()> {
        // Map user-facing OutputMode to active backend index.
        // Auto + Shared -> backend 0 (Shared). Exclusive -> backend 1.
        let want_exclusive = matches!(mode, OutputMode::Exclusive);

        #[cfg(target_os = "windows")]
        {
            if want_exclusive {
                self.activate_exclusive_backend()?;
            } else {
                self.activate_shared_backend();
            }
            // Inform the active engine of the mode for any
            // backend-internal book-keeping.
            self.inner.engine().set_output_mode(mode)?;
            Ok(())
        }

        // Non-Windows: only Shared is meaningful. We accept the call
        // silently so the UI doesn't have to special-case the platform.
        #[cfg(not(target_os = "windows"))]
        {
            let _ = want_exclusive;
            self.inner.engine().set_output_mode(mode)?;
            Ok(())
        }
    }

    #[cfg(target_os = "windows")]
    fn activate_shared_backend(&self) {
        if self.inner.active_backend.load(Ordering::Acquire) == 0 {
            return;
        }
        // Stop whatever is currently playing on the active engine.
        let _ = self.inner.engine().stop();
        // Swap: Shared becomes active.
        let shared: Arc<dyn AudioEngine> =
            Arc::clone(&self.inner.shared_engine) as Arc<dyn AudioEngine>;
        *self.inner.active_engine.lock() = shared;
        self.inner.active_backend.store(0, Ordering::Release);
        *self.inner.current_track_id.lock() = None;
        tracing::info!(target: "qobee::core", "switched to Shared backend");
    }

    #[cfg(target_os = "windows")]
    fn activate_exclusive_backend(&self) -> PlayerResult<()> {
        if self.inner.active_backend.load(Ordering::Acquire) == 1 {
            return Ok(());
        }
        // Lazy-init the Exclusive engine.
        let mut slot = self.inner.exclusive_engine.lock();
        if slot.is_none() {
            let eng = Arc::new(WasapiExclusiveEngine::new()?);
            // Pump its events into the same player channel.
            let pump_inner = Arc::clone(&self.inner);
            let events = eng.subscribe_events();
            std::thread::Builder::new()
                .name("qobee-core-pump-exclusive".into())
                .spawn(move || pump_engine_events(pump_inner, events))
                .map_err(|e| PlayerError::Engine(EngineError::Internal(e.to_string())))?;
            // Mirror persistent device + EQ + pre-gain settings.
            let dev = self.inner.shared_engine.selected_device();
            eng.set_output_device(dev);
            let gains = self.inner.shared_engine.eq_gains_db();
            eng.set_eq_gains_db(&gains);
            *slot = Some(eng);
        }
        let eng = Arc::clone(slot.as_ref().unwrap());
        drop(slot);

        // Stop what's playing on Shared.
        let _ = self.inner.shared_engine.stop();

        let dyn_eng: Arc<dyn AudioEngine> = eng as Arc<dyn AudioEngine>;
        *self.inner.active_engine.lock() = dyn_eng;
        self.inner.active_backend.store(1, Ordering::Release);
        *self.inner.current_track_id.lock() = None;
        tracing::info!(target: "qobee::core", "switched to WASAPI Exclusive backend");
        Ok(())
    }

    pub fn next(&self) -> PlayerResult<()> {
        // Same auto-fill semantics as the engine's EndOfTrack handler:
        // when there's nothing left in the queue we pick a random
        // album and start it, instead of stopping. Keeps the listening
        // session going whether the user skips or the track ends on
        // its own. Honors the "endless" toggle.
        if self.inner.advance()? {
            return Ok(());
        }
        if !self.inner.endless.load(Ordering::Relaxed) {
            return Ok(());
        }
        match self.play_random_album() {
            Ok(()) => Ok(()),
            Err(PlayerError::QueueEmpty) => {
                // Nothing in the library to fall back to; just stop.
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Whether endless mode is currently on. UI reads this to mirror
    /// the toggle state.
    pub fn endless(&self) -> bool {
        self.inner.endless.load(Ordering::Relaxed)
    }

    pub fn set_endless(&self, on: bool) {
        self.inner.endless.store(on, Ordering::Relaxed);
    }

    // ---- Crossfade ----

    /// Set the crossfade window in milliseconds (`0` = off). Updates
    /// both the orchestrator's pre-fetch lead time and the engine's
    /// fade window. Takes effect on the next track transition.
    pub fn set_crossfade_ms(&self, ms: u32) {
        self.inner.crossfade_ms.store(ms, Ordering::Relaxed);
        self.inner.engine().set_crossfade_ms(ms);
    }

    /// Current crossfade window in milliseconds (`0` = off).
    pub fn crossfade_ms(&self) -> u32 {
        self.inner.crossfade_ms.load(Ordering::Relaxed)
    }

    // ---- Sleep timer ----

    /// Arm a sleep timer that pauses playback after `minutes`. A
    /// value of `0` disarms any pending timer. The timer is a
    /// wall-clock deadline checked by a 1 s monitor thread, so it
    /// fires within ~1 s of the requested instant.
    pub fn set_sleep_timer(&self, minutes: u32) {
        self.inner
            .sleep_stop_after_track
            .store(false, Ordering::Relaxed);
        let mut slot = self.inner.sleep_deadline.lock();
        if minutes == 0 {
            *slot = None;
        } else {
            *slot = Some(Instant::now() + Duration::from_secs(minutes as u64 * 60));
        }
    }

    /// Arm a sleep timer that stops playback at the end of the
    /// current track instead of after a fixed delay.
    pub fn set_sleep_after_track(&self, on: bool) {
        if on {
            *self.inner.sleep_deadline.lock() = None;
        }
        self.inner
            .sleep_stop_after_track
            .store(on, Ordering::Relaxed);
    }

    /// Remaining whole seconds before the wall-clock sleep timer
    /// fires, or `None` when no wall-clock timer is armed. The
    /// "stop after this track" mode reports `None` (there is no
    /// countdown to show).
    pub fn sleep_timer_remaining_secs(&self) -> Option<u64> {
        let slot = self.inner.sleep_deadline.lock();
        slot.map(|deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .as_secs()
        })
    }

    /// Whether the "stop at end of current track" sleep mode is
    /// armed.
    pub fn sleep_after_track(&self) -> bool {
        self.inner.sleep_stop_after_track.load(Ordering::Relaxed)
    }

    pub fn previous(&self) -> PlayerResult<()> {
        match self.inner.queue.previous() {
            Some(id) => {
                let path = self.inner.resolve_path(id)?;
                self.inner.start_track(id, path)
            }
            None => Ok(()),
        }
    }

    // ---- Repeat / shuffle ----

    pub fn repeat_mode(&self) -> RepeatMode {
        self.inner.queue.repeat()
    }

    pub fn set_repeat_mode(&self, mode: RepeatMode) {
        self.inner.queue.set_repeat(mode);
    }

    pub fn shuffle(&self) -> bool {
        self.inner.queue.shuffle()
    }

    pub fn set_shuffle(&self, on: bool) {
        self.inner.queue.set_shuffle(on);
    }

    /// Pick a random album from the library and start playing it from
    /// track 1. Used by the home page "shuffle library" CTA and by the
    /// auto-fill behavior at end of queue.
    pub fn play_random_album(&self) -> PlayerResult<()> {
        let albums = self.inner.library.list_albums()?;
        if albums.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        // Lehmer-style RNG (no extra crate) seeded on time.
        let now_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E3779B97F4A7C15);
        let idx = (now_nanos as usize) % albums.len();
        let album = &albums[idx];
        let detail = self
            .inner
            .library
            .get_album(album.id)?
            .ok_or(PlayerError::QueueEmpty)?;
        if detail.tracks.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
        let first = ids[0];
        self.inner.queue.replace(ids, Some(0));
        let path = self.inner.resolve_path(first)?;
        self.inner.start_track(first, path)
    }

    // ---- EQ ----

    pub fn set_eq_gains_db(&self, gains: Vec<f32>) {
        // R7.8 — see `set_volume` for the rationale. We cache the
        // requested EQ band gains so they are reapplied on the
        // next PCM track without the user having to redo the
        // sliders.
        if self.inner.engine().is_dsd_active() {
            *self.inner.cached_eq_gains_db.lock() = Some(gains);
            let _ = self.inner.event_tx.try_send(PlayerEvent::Error {
                message: "DSD playback: DSP read-only".to_string(),
            });
            return;
        }
        self.inner.engine().set_eq_gains_db(&gains);
    }

    pub fn eq_gains_db(&self) -> Vec<f32> {
        self.inner.engine().eq_gains_db()
    }

    // ---- Output device picking ----

    pub fn list_output_devices(&self) -> PlayerResult<Vec<OutputDevice>> {
        Ok(list_output_devices()?)
    }

    pub fn set_output_device(&self, device_id: Option<String>) -> PlayerResult<()> {
        self.inner.engine().set_output_device(device_id);
        Ok(())
    }

    pub fn selected_output_device(&self) -> Option<String> {
        self.inner.engine().selected_device()
    }

    // ---- ReplayGain ----

    pub fn replaygain_mode(&self) -> ReplayGainMode {
        match self.inner.replaygain_mode.load(Ordering::Relaxed) {
            1 => ReplayGainMode::Track,
            2 => ReplayGainMode::Album,
            _ => ReplayGainMode::Off,
        }
    }

    pub fn set_replaygain_mode(&self, mode: ReplayGainMode) {
        let v = match mode {
            ReplayGainMode::Off => 0,
            ReplayGainMode::Track => 1,
            ReplayGainMode::Album => 2,
        };
        self.inner.replaygain_mode.store(v, Ordering::Relaxed);

        // R7.8 — DSD playback bypasses the pre-gain stage. Toggling
        // ReplayGain mid-DSD has no audible effect; we still update
        // the persisted setting (above) so the next PCM track picks
        // it up, but we do not call `set_pre_gain` on the engine.
        if self.inner.engine().is_dsd_active() {
            let _ = self.inner.event_tx.try_send(PlayerEvent::Error {
                message: "DSD playback: DSP read-only".to_string(),
            });
            return;
        }

        // Re-apply gain to the currently playing track immediately, so
        // toggling the setting takes effect without waiting for the
        // next track.
        if let Some(track_id) = *self.inner.current_track_id.lock() {
            if let Ok(Some(track)) = self.inner.library.get_track(track_id) {
                let rg_db = match mode {
                    ReplayGainMode::Off => None,
                    ReplayGainMode::Track => {
                        track.replaygain_track_db.or(track.replaygain_album_db)
                    }
                    ReplayGainMode::Album => {
                        track.replaygain_album_db.or(track.replaygain_track_db)
                    }
                };
                let pre_gain = match rg_db {
                    Some(db) => 10f32.powf(db.clamp(-24.0, 12.0) / 20.0),
                    None => 1.0,
                };
                self.inner.engine().set_pre_gain(pre_gain);

                // Mirror the change on the new `Pre_Gain_Stage` so
                // its true-peak protection sees the same RG values
                // even on a mid-track mode change.
                let rg_peak = match mode {
                    ReplayGainMode::Off => None,
                    ReplayGainMode::Track => {
                        track.replaygain_track_peak.or(track.replaygain_album_peak)
                    }
                    ReplayGainMode::Album => {
                        track.replaygain_album_peak.or(track.replaygain_track_peak)
                    }
                };
                let slider = self.inner.engine().state().volume;
                self.inner.engine().set_pre_gain_context(PreGainContext {
                    rg_db,
                    rg_peak,
                    slider,
                });
            }
        }
    }

    // ---- Convolver IR (R9.2 / R9.3 / R9.8) ----

    /// Load a stereo (or mono, duplicated) impulse response WAV from
    /// `path`, validate it, resample it to the device sample rate,
    /// apply the persisted `audio.convolver_gain_db` compensation,
    /// and push it to the engine's `Convolver_Stage`. Runs on the
    /// caller's thread (typically the Tauri command worker), so it
    /// is safe to allocate, decode, and resample without disturbing
    /// the audio hot path.
    ///
    /// On failure the previously-loaded IR is left untouched and
    /// the engine emits an `EngineEvent::IrLoadError` for the UI to
    /// surface (republished as `PlayerEvent::Error` upstream of this
    /// method's return value, which itself surfaces the matching
    /// [`EngineError`] for direct callers).
    pub fn load_convolver_ir(&self, path: &Path) -> Result<(), EngineError> {
        // 1. Decode the WAV via Symphonia (reusing the existing
        //    decoder path).
        let mut decoder = qobee_engine::backend_symphonia::SymphoniaDecoder::open(path)
            .map_err(|e| EngineError::ConvolverIrLoadFailed(e.to_string()))?;
        let format = decoder.format();
        let in_sr = format.sample_rate;
        let in_ch = format.channels.max(1) as usize;

        // Pull every packet into per-channel f32 vectors. The IR is
        // bounded (R9.8 caps the post-resample length at 100_000
        // taps), but we let the loop run to completion before
        // checking — Symphonia's chunk size is small enough that
        // even a 30 s IR at 48 kHz fits in well under the limit.
        let mut planar: Vec<Vec<f32>> = vec![Vec::new(); in_ch];
        loop {
            let pkt = decoder
                .next_packet()
                .map_err(|e| EngineError::ConvolverIrLoadFailed(e.to_string()))?;
            match pkt {
                None => break,
                Some(p) => {
                    if let qobee_engine::PcmBuffer::F32Interleaved(samples) = p.buffer {
                        let frames = samples.len() / in_ch;
                        for f in 0..frames {
                            for c in 0..in_ch {
                                planar[c].push(samples[f * in_ch + c]);
                            }
                        }
                    }
                }
            }
        }

        // 2. Validate: non-empty, all finite. The post-resample
        //    length is checked after step 3.
        if planar.is_empty() || planar.iter().any(|c| c.is_empty()) {
            return Err(EngineError::ConvolverIrInvalid("IR has zero length".into()));
        }
        for (i, ch) in planar.iter().enumerate() {
            if ch.iter().any(|s| !s.is_finite()) {
                return Err(EngineError::ConvolverIrInvalid(format!(
                    "channel {i} contains NaN or Inf"
                )));
            }
        }

        // Mono → duplicated L/R; >stereo → take channels 0/1; stereo
        // → as-is.
        let (mut left, mut right) = match planar.len() {
            1 => (planar[0].clone(), planar[0].clone()),
            _ => (planar.remove(0), planar.remove(0)),
        };

        // 3. Resample to the current device sample rate. We use the
        //    `Best` sinc preset for transparency (the IR is loaded
        //    once, off the hot path, so the extra cost is fine).
        //    `SincFixedIn` consumes a fixed input chunk per call, so
        //    we feed the IR in one chunk plus a trailing
        //    `process_partial` flush to drain the look-ahead tail.
        let device_sr = self.inner.engine().state().sample_rate.unwrap_or(48_000);
        if device_sr != in_sr {
            use rubato::{Resampler, SincFixedIn};
            let params = qobee_engine::backend_cpal_shared::sinc_params_for(
                qobee_engine::ResamplerQuality::Best,
            );
            let ratio = device_sr as f64 / in_sr as f64;
            let chunk = left.len();
            if chunk == 0 {
                return Err(EngineError::ConvolverIrInvalid("empty IR".into()));
            }
            let mut resampler = SincFixedIn::<f32>::new(ratio, 1.1, params, chunk, 2)
                .map_err(|e| EngineError::ConvolverIrLoadFailed(e.to_string()))?;
            let inputs_ref: Vec<&[f32]> = vec![left.as_slice(), right.as_slice()];
            let mut out = resampler
                .process(&inputs_ref, None)
                .map_err(|e| EngineError::ConvolverIrLoadFailed(e.to_string()))?;
            drop(inputs_ref);
            // Drain the resampler's look-ahead tail so the output
            // includes the full convolution.
            if let Ok(tail) = resampler.process_partial::<&[f32]>(None, None) {
                if tail.len() >= 2 {
                    out[0].extend_from_slice(&tail[0]);
                    out[1].extend_from_slice(&tail[1]);
                }
            }
            if out.len() < 2 {
                return Err(EngineError::ConvolverIrInvalid(
                    "resampler produced less than 2 channels".into(),
                ));
            }
            left = std::mem::take(&mut out[0]);
            right = std::mem::take(&mut out[1]);
        }

        // R9.8 — length budget after resample.
        if left.is_empty() || right.is_empty() {
            return Err(EngineError::ConvolverIrInvalid(
                "IR collapsed to zero length after resample".into(),
            ));
        }
        if left.len() > 100_000 || right.len() > 100_000 {
            return Err(EngineError::ConvolverIrTooLong);
        }

        // 4. Apply gain compensation from `audio.convolver_gain_db`
        //    (R9.10). The setting lives in the SQLite settings
        //    table; clamp into the design's [-24, 0] dB range as a
        //    defence against stale rows.
        let gain_db = self
            .inner
            .library
            .get_setting("audio.convolver_gain_db")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<f32>(&s).ok())
            .unwrap_or(-6.0)
            .clamp(-24.0, 0.0);
        let gain = 10f32.powf(gain_db / 20.0);
        for s in left.iter_mut() {
            *s *= gain;
        }
        for s in right.iter_mut() {
            *s *= gain;
        }

        // 5. Hand off to the engine. The decoder thread picks the
        //    new IR up at the next chunk boundary.
        self.inner.engine().set_convolver_ir(left, right);
        Ok(())
    }

    /// Drop the active convolver IR. The engine's `Convolver_Stage`
    /// falls back to bypass on the next chunk boundary (R9.7).
    pub fn unload_convolver_ir(&self) {
        self.inner.engine().set_convolver_ir(Vec::new(), Vec::new());
    }

    /// Length of the currently loaded convolver IR in taps (per
    /// channel); zero when none loaded. Surfaced by the
    /// `get_convolver_status` Tauri command to compute reported
    /// latency.
    pub fn convolver_ir_len(&self) -> usize {
        self.inner.engine().convolver_ir_len()
    }
}

/// Shorten a raw engine error message into something nicer for a
/// toast: drop the parenthesised HRESULT if any, keep the human part.
#[cfg(target_os = "windows")]
fn shorten_error(msg: &str) -> String {
    // Strip an optional " HRESULT 0x..." or "(0x...)" tail.
    let cut = msg
        .find(" HRESULT")
        .or_else(|| msg.find(" (0x"))
        .unwrap_or(msg.len());
    msg[..cut].trim_end_matches([':', ' ']).to_string()
}

/// Pump engine events into the player event channel.
fn pump_engine_events(inner: Arc<PlayerInner>, engine_events: Receiver<EngineEvent>) {
    // Local state for throttling `PositionTick` on the R8 transport
    // bus to one event every 250 ms. The decoder pushes positions
    // at chunk cadence (~10 ms); the UI / OS bridges only need the
    // throttled stream. We keep both `last_position_ms` and the
    // wall-clock instant: the bus carries milliseconds while the
    // decoder reports seconds.
    let mut last_tick_at: Option<Instant> = None;
    let mut last_tick_position_ms: u64 = u64::MAX;

    while let Ok(ev) = engine_events.recv() {
        let player_ev = match ev {
            EngineEvent::StateChanged { mut state } => {
                state.current_track_id = inner.current_track_id.lock().map(|id| id.to_string());
                PlayerEvent::StateChanged { state }
            }
            EngineEvent::Position { position_seconds } => {
                // Pre-fetch the next track when we're within 5s of the
                // end of the current one, so the gap on EndOfTrack is
                // dominated by the engine's load() and not by disk I/O.
                // When crossfade is on, widen the lead time so the
                // incoming track is decoded and ready *before* the
                // fade window opens (crossfade + 2 s margin).
                let dur = inner.engine().state().duration_seconds;
                let xf_secs = inner.crossfade_ms.load(Ordering::Relaxed) as f64 / 1000.0;
                let lead = 5.0_f64.max(xf_secs + 2.0);
                if dur > 0.0 && position_seconds >= dur - lead {
                    inner.prefetch_next_if_any();
                }

                // R8.3 — throttle `PositionTick` to one broadcast
                // every 250 ms. The first non-zero position after
                // a (re)start is always allowed through so the UI
                // gets an immediate confirmation.
                let position_ms = (position_seconds * 1000.0).max(0.0) as u64;
                let now = Instant::now();
                let should_emit = match last_tick_at {
                    None => true,
                    Some(prev) if now.duration_since(prev) >= POSITION_TICK_THROTTLE => true,
                    // Position rewound (seek backward, new track) →
                    // resync immediately, otherwise the bus would
                    // appear stuck for up to 250 ms.
                    _ if position_ms < last_tick_position_ms => true,
                    _ => false,
                };
                if should_emit {
                    last_tick_at = Some(now);
                    last_tick_position_ms = position_ms;
                    inner.broadcast(PlayerEvent::PositionTick { position_ms });
                }

                PlayerEvent::Position { position_seconds }
            }
            EngineEvent::EndOfTrack => {
                // Sleep timer: "stop at end of current track" mode.
                // When armed, halt instead of advancing and disarm
                // the latch so the next manual play works normally.
                if inner.sleep_stop_after_track.swap(false, Ordering::Relaxed) {
                    tracing::info!(
                        target: "qobee::core",
                        "sleep timer (end of track) elapsed; stopping playback"
                    );
                    let _ = inner.engine().stop();
                    *inner.current_track_id.lock() = None;
                    last_tick_at = None;
                    last_tick_position_ms = u64::MAX;
                    inner.broadcast(PlayerEvent::Stopped);
                    let _ = inner.event_tx.try_send(PlayerEvent::EndOfTrack);
                    continue;
                }
                match inner.advance() {
                    Ok(true) => {}
                    Ok(false) => {
                        if inner.endless.load(Ordering::Relaxed) {
                            let me = PlayerHandle {
                                inner: Arc::clone(&inner),
                            };
                            if let Err(e) = me.play_random_album() {
                                tracing::info!(
                                    target: "qobee::core",
                                    error = %e,
                                    "no random album available; staying idle"
                                );
                            }
                        } else {
                            // Endless off + nothing left: surface a
                            // clean `Stopped` on the transport bus so
                            // the UI / OS bridges freeze their
                            // timeline rather than the user seeing
                            // the last `PositionTick` linger.
                            inner.broadcast(PlayerEvent::Stopped);
                        }
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        tracing::warn!(
                            target: "qobee::core",
                            error = %msg,
                            "auto-advance failed"
                        );
                        let _ = inner.event_tx.try_send(PlayerEvent::Error {
                            message: msg.clone(),
                        });
                        inner.broadcast(PlayerEvent::Errored {
                            kind: PlayerErrorKind::Other,
                            message: msg,
                        });
                    }
                }
                // Reset the throttle so the next track starts at
                // tick 0 instead of inheriting the previous track's
                // last instant.
                last_tick_at = None;
                last_tick_position_ms = u64::MAX;
                PlayerEvent::EndOfTrack
            }
            EngineEvent::GaplessTransition => {
                // The engine already swapped the active decoder. We
                // only update bookkeeping: advance the queue cursor
                // (without calling start_track), record the play in
                // history, and clear the prefetch flag so we can
                // prepare the *next* track.
                let mut new_track_meta: Option<TrackMeta> = None;
                if let Some(new_id) = inner.queue.advance() {
                    *inner.current_track_id.lock() = Some(new_id);
                    inner
                        .engine()
                        .set_current_track_id(Some(new_id.to_string()));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if let Err(e) = inner.library.record_play(new_id, now) {
                        tracing::warn!(
                            target: "qobee::core",
                            error = %e,
                            "record_play failed (gapless)"
                        );
                    }
                    new_track_meta = inner.track_meta_for(new_id);
                }
                *inner.prefetched_track.lock() = None;

                // Tell the UI we transitioned. Re-fetch the engine
                // state so the new track's metadata reaches the UI
                // even if it raced with the StateChanged emitted
                // by the engine.
                let mut state = inner.engine().state();
                state.current_track_id = inner.current_track_id.lock().map(|id| id.to_string());
                let _ = inner.event_tx.try_send(PlayerEvent::StateChanged { state });

                // R8 transport bus: gapless = `TrackChanged` (not
                // `Started`). The session is the same; only the
                // active track flips.
                if let Some(meta) = new_track_meta {
                    inner.broadcast(PlayerEvent::TrackChanged { track: meta });
                }
                last_tick_at = None;
                last_tick_position_ms = u64::MAX;

                PlayerEvent::EndOfTrack
            }
            EngineEvent::BitPerfectChanged { health } => {
                // Republish under the dedicated `player:bit-perfect`
                // topic in `src-tauri/src/lib.rs` (R5.5). No further
                // bookkeeping is needed: the engine already owns the
                // 200 ms debounce.
                PlayerEvent::BitPerfectChanged { health }
            }
            EngineEvent::Error { message } => {
                // If WASAPI Exclusive can't open the device, drop back
                // to Shared automatically and retry the current track.
                // This keeps the user's session going even when their
                // device refuses every Exclusive format we try.
                #[cfg(target_os = "windows")]
                {
                    let active = inner.active_backend.load(Ordering::Acquire);
                    let looks_like_exclusive_fail = active == 1
                        && (message.contains("Exclusive") || message.contains("WASAPI"));
                    if looks_like_exclusive_fail {
                        tracing::warn!(
                            target: "qobee::core",
                            error = %message,
                            "WASAPI Exclusive failed; falling back to Shared automatically"
                        );
                        // Swap to Shared on the active engine slot.
                        let shared: Arc<dyn AudioEngine> =
                            Arc::clone(&inner.shared_engine) as Arc<dyn AudioEngine>;
                        *inner.active_engine.lock() = shared;
                        inner.active_backend.store(0, Ordering::Release);

                        // Persist the fallback so the next launch
                        // doesn't try Exclusive again and fail in the
                        // same way. The user can re-enable it from
                        // Settings if they fix their device config.
                        if let Err(e) = inner.library.set_setting("audio.output_mode", "shared") {
                            tracing::warn!(
                                target: "qobee::core",
                                error = %e,
                                "failed to persist fallback output_mode"
                            );
                        }

                        // Restart the same track on Shared.
                        let cur_id = *inner.current_track_id.lock();
                        if let Some(track_id) = cur_id {
                            if let Ok(path) = inner.resolve_path(track_id) {
                                let _ = inner.start_track(track_id, path);
                            }
                        }

                        // Tell the UI we fell back so the dropdown can
                        // sync. We surface a soft warning rather than
                        // the raw HRESULT.
                        let fallback_msg = format!(
                            "Exclusive output unavailable: {}. Switched to Shared.",
                            shorten_error(&message)
                        );
                        let _ = inner.event_tx.try_send(PlayerEvent::Error {
                            message: fallback_msg.clone(),
                        });
                        inner.broadcast(PlayerEvent::Errored {
                            kind: PlayerErrorKind::DeviceUnavailable,
                            message: fallback_msg,
                        });
                        continue;
                    }
                }

                // Surface every other engine error on the transport
                // bus too, with a coarse `kind`. The orchestrator
                // does not have enough context to discriminate
                // FileNotFound vs DecodeFailed at this point — the
                // engine carries that signal in the message text and
                // future revisions can refine the mapping.
                inner.broadcast(PlayerEvent::Errored {
                    kind: classify_engine_error(&message),
                    message: message.clone(),
                });
                PlayerEvent::Error { message }
            }
            EngineEvent::IrLoadError { message } => {
                // Convolver IR load failed (R9.8). The previous IR
                // (if any) stayed in place, so we surface the
                // message as a regular player error and let the UI
                // toast it.
                let payload = format!("Convolver IR: {message}");
                inner.broadcast(PlayerEvent::Errored {
                    kind: PlayerErrorKind::Other,
                    message: payload.clone(),
                });
                PlayerEvent::Error { message: payload }
            }
            EngineEvent::DopUnsupported => {
                // R7.5 — surfaced alongside an `Error` event by
                // the engine. The error event already carries the
                // FR-localised message; this dedicated variant is
                // the topic the UI listens on for the DoP-failure
                // toast.
                PlayerEvent::Error {
                    message: "DoP non supporté par le périphérique".to_string(),
                }
            }
            EngineEvent::DsdReadOnlyDsp => {
                // R7.8 — engine emits this when a volume / EQ /
                // pre-gain mutation arrives mid-DSD. We
                // republish as a soft Error toast so the user
                // understands their slider move was queued for
                // the next PCM track rather than dropped.
                PlayerEvent::Error {
                    message: "DSD playback: DSP read-only".to_string(),
                }
            }
        };
        if inner.event_tx.try_send(player_ev).is_err() {
            tracing::warn!(
                target: "qobee::core",
                "player event channel saturated; dropping engine event"
            );
        }
    }
}

/// Coarse classifier mapping a free-form engine error message onto a
/// [`PlayerErrorKind`] for the R8 transport bus. The mapping is
/// intentionally lossy: the kind is only used by the UI to pick a
/// FR-localised toast / aria-label, while the original message is
/// always preserved alongside.
fn classify_engine_error(message: &str) -> PlayerErrorKind {
    let lower = message.to_ascii_lowercase();
    if lower.contains("not found")
        || lower.contains("no such file")
        || lower.contains("introuvable")
    {
        PlayerErrorKind::FileNotFound
    } else if lower.contains("decode")
        || lower.contains("symphonia")
        || lower.contains("codec")
    {
        PlayerErrorKind::DecodeFailed
    } else if lower.contains("device")
        || lower.contains("wasapi")
        || lower.contains("exclusive")
        || lower.contains("cpal")
        || lower.contains("backend")
    {
        PlayerErrorKind::DeviceUnavailable
    } else {
        PlayerErrorKind::Other
    }
}

// ---------------------------------------------------------------------------
// AudioSettingsApply bridge
// ---------------------------------------------------------------------------
//
// The settings store (see `crates/core/src/audio_settings.rs`) is
// engine-agnostic; it only knows how to push a fresh `AudioSettings`
// snapshot through this trait. Implementing it on `PlayerHandle` (and
// re-exporting on `Player`) lets the application layer wire the store
// to whichever backend is currently active without leaking the
// `Arc<dyn AudioEngine>` lock outside the player.

impl crate::audio_settings::AudioSettingsApply for PlayerHandle {
    fn apply(&self, settings: qobee_engine::AudioSettings) {
        self.inner.engine().set_audio_settings(settings);
    }
}

impl crate::audio_settings::AudioSettingsApply for Player {
    fn apply(&self, settings: qobee_engine::AudioSettings) {
        self.inner.engine().set_audio_settings(settings);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use qobee_engine::{BitPerfectHealth, BitPerfectStatus, EffectiveOutputMode};
    use qobee_library::Library;

    fn fresh_library() -> Library {
        let mut base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        base.push(format!("qobee-player-test-{nanos}-{}", std::process::id()));
        std::fs::create_dir_all(&base).expect("create temp dir");
        let db = base.join("library.db");
        let cover = base.join("covers");
        Library::open(&db, &cover).expect("open library")
    }

    /// Synthetic [`BitPerfectHealth`] snapshot with every field
    /// populated, enough to round-trip through the player event
    /// channel without losing information.
    fn synthetic_health() -> BitPerfectHealth {
        BitPerfectHealth {
            status: BitPerfectStatus::Green,
            source_sample_rate: Some(44_100),
            device_sample_rate: Some(44_100),
            source_bit_depth: Some(24),
            effective_output_mode: EffectiveOutputMode::Exclusive,
            is_native_rate: true,
            unity_volume: true,
            unity_pregain: true,
            eq_bypass: true,
            crossfeed_off: true,
            convolver_off: true,
            limiter_off: true,
            dither_bypass: true,
            balance_off: true,
            upmix_active: false,
            messages: Vec::new(),
        }
    }

    /// Feature: audio-quality-improvements, R5.5 — a
    /// `BitPerfectChanged` engine event must surface on the player
    /// event channel as a [`PlayerEvent::BitPerfectChanged`] with
    /// the same health payload, ready for the Tauri layer to
    /// publish under the `player:bit-perfect` topic.
    #[test]
    fn pump_engine_events_republishes_bit_perfect_changed() {
        let library = fresh_library();
        let player = Player::new(library).expect("Player::new");
        let handle = player.handle();
        let player_rx = handle.subscribe();

        // Build a minimal `PlayerInner` proxy is overkill — instead
        // we inject directly through the engine event pipeline by
        // setting up an isolated `pump_engine_events` instance fed
        // by our own channel.
        let (engine_tx, engine_rx) = crossbeam_channel::bounded::<EngineEvent>(8);
        let inner = Arc::clone(&player.inner);
        let pump_handle = std::thread::Builder::new()
            .name("qobee-test-pump".into())
            .spawn(move || pump_engine_events(inner, engine_rx))
            .expect("spawn test pump");

        let health = synthetic_health();
        engine_tx
            .send(EngineEvent::BitPerfectChanged {
                health: health.clone(),
            })
            .expect("send into engine channel");

        let received = player_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("player event within timeout");
        match received {
            PlayerEvent::BitPerfectChanged { health: got } => {
                assert_eq!(got, health, "health payload must round-trip exactly");
            }
            other => panic!(
                "expected BitPerfectChanged, got {other:?} (variant must \
                 be republished as PlayerEvent::BitPerfectChanged for the \
                 `player:bit-perfect` topic)"
            ),
        }

        // Closing the test channel terminates the pump thread.
        drop(engine_tx);
        let _ = pump_handle.join();
    }

    /// The new `PlayerEvent::BitPerfectChanged` variant must
    /// serialise with the snake_case `type` discriminator that the
    /// UI deserialiser expects (`"type":"bit_perfect_changed"`).
    /// This is the wire contract Tauri publishes under
    /// `player:bit-perfect`.
    #[test]
    fn bit_perfect_changed_serde_tag_is_snake_case() {
        let event = PlayerEvent::BitPerfectChanged {
            health: synthetic_health(),
        };
        let json = serde_json::to_string(&event).expect("serialise");
        assert!(
            json.contains("\"type\":\"bit_perfect_changed\""),
            "expected snake_case tag, got {json}"
        );

        let parsed: PlayerEvent = serde_json::from_str(&json).expect("round-trip");
        match parsed {
            PlayerEvent::BitPerfectChanged { health } => {
                assert_eq!(health.status, BitPerfectStatus::Green);
            }
            other => panic!("round-trip changed variant: {other:?}"),
        }
    }
}
