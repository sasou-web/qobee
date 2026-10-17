//! The single, rodio-backed audio backend.
//!
//! [`RodioEngine`] wraps a [`rodio::OutputStream`] (the CPAL stream
//! container) and a [`rodio::Sink`]. rodio owns decoding (Symphonia),
//! output (CPAL), volume, seeking, and source queuing; this type maps
//! the [`AudioEngine`](crate::AudioEngine) trait onto those primitives
//! and runs a small watcher thread that polls the sink to emit
//! `Position` / `EndOfTrack` events (rodio does not push events).

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};

use crate::device::resolve_output_device;
use crate::error::{EngineError, EngineResult};
use crate::types::{EngineEvent, PlaybackStatus, PlayerState};
use crate::AudioEngine;

/// Poll interval for the end-of-track / position watcher thread.
const WATCH_INTERVAL: Duration = Duration::from_millis(150);

/// Clamp a slider position in `[0.0, 1.0]` to the linear gain applied
/// by the sink. The mapping is the identity (clamped), so `0.0 → 0.0`
/// and `1.0 → 1.0` with a monotone, in-range result for everything in
/// between (Property 1). NaN maps to mute.
pub fn map_volume(slider: f32) -> f32 {
    if slider.is_nan() {
        return 0.0;
    }
    slider.clamp(0.0, 1.0)
}

/// Build a rodio `Decoder` from a track path or a `drv://…` Drive URI.
/// Local files use `Decoder::try_from(File)` (which records the byte
/// length for accurate seeking); Drive URIs feed an HTTP byte-range
/// `MediaSource` through the generic decoder builder.
fn open_decoder(path: &Path) -> EngineResult<Box<dyn Source<Item = f32> + Send>> {
    let path_str = path.to_string_lossy();

    // Drive-backed remote track: `drv://<source_id>/<file_id>`.
    if let Some((source_id, file_id)) = qobee_drive::parse_drive_uri(&path_str) {
        let client = qobee_drive::DriveClient::new(source_id)
            .map_err(|e| EngineError::Decode(format!("Drive auth: {e}")))?;
        let source = qobee_drive::DriveMediaSource::open(Arc::new(client), file_id)
            .map_err(|e| EngineError::Decode(format!("Drive open: {e}")))?;
        let decoder = Decoder::new(BufReader::new(source))
            .map_err(|e| EngineError::Decode(e.to_string()))?;
        return Ok(Box::new(decoder));
    }

    // Local file.
    let file = File::open(path)?;
    let decoder = Decoder::try_from(file).map_err(|e| EngineError::Decode(e.to_string()))?;
    Ok(Box::new(decoder))
}

struct EngineInner {
    /// Held to keep the CPAL stream alive; dropping it stops audio.
    /// Rebuilt when the selected output device changes.
    stream: Option<OutputStream>,
    /// Current playback sink. Rebuilt on each successful `load`.
    sink: Option<Sink>,
    state: PlayerState,
    /// Total duration of the loaded source in seconds (`0.0` if unknown).
    duration_seconds: f64,
    /// Path currently loaded, kept for diagnostics / potential reopen.
    current_path: Option<PathBuf>,
}

impl EngineInner {
    fn new() -> Self {
        EngineInner {
            stream: None,
            sink: None,
            state: PlayerState::default(),
            duration_seconds: 0.0,
            current_path: None,
        }
    }
}

/// Single rodio-backed implementation of [`AudioEngine`].
pub struct RodioEngine {
    inner: Mutex<EngineInner>,
    events_tx: Sender<EngineEvent>,
    events_rx: Receiver<EngineEvent>,
    /// Selected output device id (`None` = system default).
    selected_device_id: Mutex<Option<String>>,
    /// Watcher thread liveness flag; set to `false` on drop.
    watcher_alive: Arc<AtomicBool>,
}

impl RodioEngine {
    /// Build a new engine. The output stream is opened lazily on the
    /// first `load` so construction never fails on a headless box.
    ///
    /// Prefer [`new_shared`] in production: it wraps the engine in an
    /// `Arc` and starts the end-of-track / position watcher thread.
    pub fn new() -> EngineResult<Self> {
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        Ok(RodioEngine {
            inner: Mutex::new(EngineInner::new()),
            events_tx,
            events_rx,
            selected_device_id: Mutex::new(None),
            watcher_alive: Arc::new(AtomicBool::new(true)),
        })
    }

    /// Snapshot the current player state.
    fn snapshot(&self, inner: &EngineInner) -> PlayerState {
        let mut state = inner.state.clone();
        if let Some(sink) = inner.sink.as_ref() {
            state.position_seconds = sink.get_pos().as_secs_f64();
        }
        state.duration_seconds = inner.duration_seconds;
        state
    }

    /// Ensure an output stream is open, (re)building it for the
    /// currently selected device. Returns whether a device fallback
    /// occurred. The caller holds the `inner` lock.
    fn ensure_stream(&self, inner: &mut EngineInner) -> EngineResult<bool> {
        if inner.stream.is_some() {
            return Ok(false);
        }
        let requested = self.selected_device_id.lock().clone();
        let (device, fell_back) = resolve_output_device(requested.as_deref())?;
        let stream = OutputStreamBuilder::from_device(device)
            .map_err(|e| EngineError::Output(e.to_string()))?
            .open_stream()
            .map_err(|e| EngineError::Output(e.to_string()))?;
        inner.stream = Some(stream);
        Ok(fell_back)
    }

    fn send_state(&self, inner: &EngineInner) {
        let _ = self
            .events_tx
            .send(EngineEvent::StateChanged {
                state: self.snapshot(inner),
            });
    }
}

impl Drop for RodioEngine {
    fn drop(&mut self) {
        self.watcher_alive.store(false, Ordering::SeqCst);
    }
}

impl AudioEngine for RodioEngine {
    fn load(&self, path: &Path) -> EngineResult<()> {
        // Build the decoder first so a bad file leaves the previous
        // sink untouched (Requirement 1.5 / Property 2).
        let source = open_decoder(path)?;
        let duration = source.total_duration().map(|d| d.as_secs_f64());

        let mut inner = self.inner.lock();
        let fell_back = self.ensure_stream(&mut inner)?;
        if fell_back {
            let _ = self.events_tx.send(EngineEvent::Error {
                message: "selected output device unavailable; using system default".to_string(),
            });
        }
        let stream = inner
            .stream
            .as_ref()
            .ok_or(EngineError::InvalidState("no output stream"))?;

        // Fresh sink on the current stream's mixer; start paused so the
        // orchestrator controls when audio begins.
        let sink = Sink::connect_new(stream.mixer());
        sink.pause();
        sink.set_volume(map_volume(inner.state.volume));
        sink.append(source);

        inner.sink = Some(sink);
        inner.duration_seconds = duration.unwrap_or(0.0);
        inner.current_path = Some(path.to_path_buf());
        inner.state.status = PlaybackStatus::Paused;
        inner.state.position_seconds = 0.0;
        inner.state.duration_seconds = inner.duration_seconds;
        inner.state.error = None;
        self.send_state(&inner);
        Ok(())
    }

    fn play(&self) -> EngineResult<()> {
        let mut inner = self.inner.lock();
        let sink = inner
            .sink
            .as_ref()
            .ok_or(EngineError::InvalidState("play() with no track loaded"))?;
        sink.play();
        inner.state.status = PlaybackStatus::Playing;
        self.send_state(&inner);
        Ok(())
    }

    fn pause(&self) -> EngineResult<()> {
        let mut inner = self.inner.lock();
        if let Some(sink) = inner.sink.as_ref() {
            sink.pause();
        }
        inner.state.status = PlaybackStatus::Paused;
        self.send_state(&inner);
        Ok(())
    }

    fn resume(&self) -> EngineResult<()> {
        let mut inner = self.inner.lock();
        if let Some(sink) = inner.sink.as_ref() {
            sink.play();
        }
        inner.state.status = PlaybackStatus::Playing;
        self.send_state(&inner);
        Ok(())
    }

    fn stop(&self) -> EngineResult<()> {
        let mut inner = self.inner.lock();
        if let Some(sink) = inner.sink.take() {
            sink.stop();
        }
        inner.current_path = None;
        inner.duration_seconds = 0.0;
        inner.state.status = PlaybackStatus::Stopped;
        inner.state.position_seconds = 0.0;
        inner.state.duration_seconds = 0.0;
        self.send_state(&inner);
        Ok(())
    }

    fn seek(&self, position_seconds: f64) -> EngineResult<()> {
        let inner = self.inner.lock();
        let sink = inner
            .sink
            .as_ref()
            .ok_or(EngineError::InvalidState("seek() with no track loaded"))?;
        let pos = Duration::from_secs_f64(position_seconds.max(0.0));
        sink.try_seek(pos)
            .map_err(|e| EngineError::InvalidState(seek_error_message(&e)))?;
        Ok(())
    }

    fn set_volume(&self, volume: f32) -> EngineResult<()> {
        let mut inner = self.inner.lock();
        let gain = map_volume(volume);
        inner.state.volume = volume.clamp(0.0, 1.0);
        if let Some(sink) = inner.sink.as_ref() {
            sink.set_volume(gain);
        }
        Ok(())
    }

    fn state(&self) -> PlayerState {
        let inner = self.inner.lock();
        self.snapshot(&inner)
    }

    fn subscribe_events(&self) -> Receiver<EngineEvent> {
        self.events_rx.clone()
    }

    fn list_output_devices(&self) -> EngineResult<Vec<crate::types::OutputDevice>> {
        crate::device::list_output_devices()
    }

    fn set_output_device(&self, device_id: Option<String>) -> EngineResult<()> {
        *self.selected_device_id.lock() = device_id;
        // Drop the current stream + sink so the next `load`/`play`
        // rebuilds them on the freshly selected device. Playback of the
        // current track stops; the orchestrator restarts it if needed.
        let mut inner = self.inner.lock();
        if let Some(sink) = inner.sink.take() {
            sink.stop();
        }
        inner.stream = None;
        inner.state.status = PlaybackStatus::Stopped;
        Ok(())
    }

    fn selected_output_device(&self) -> Option<String> {
        self.selected_device_id.lock().clone()
    }
}

/// Map a rodio [`SeekError`](rodio::source::SeekError) to a stable,
/// static engine-error message.
fn seek_error_message(err: &rodio::source::SeekError) -> &'static str {
    match err {
        rodio::source::SeekError::NotSupported { .. } => "seek not supported for this source",
        _ => "seek failed",
    }
}

/// Wrap a freshly built engine in an `Arc` and start its end-of-track /
/// position watcher thread.
///
/// The watcher is started here (rather than in [`RodioEngine::new`])
/// because it needs an `Arc<RodioEngine>` to read the sink/state on
/// each poll without a self-referential borrow. The orchestrator stores
/// the returned `Arc` and uses it for every call.
pub fn new_shared() -> EngineResult<Arc<RodioEngine>> {
    let engine = Arc::new(RodioEngine::new()?);
    let weak = Arc::downgrade(&engine);
    let alive = Arc::clone(&engine.watcher_alive);

    thread::Builder::new()
        .name("qobee-engine-watcher".into())
        .spawn(move || {
            // Track whether the current sink was non-empty so we only
            // emit a single `EndOfTrack` on the empty→true transition.
            let mut was_playing_source = false;
            let mut last_pos_emit = f64::NAN;

            while alive.load(Ordering::SeqCst) {
                thread::sleep(WATCH_INTERVAL);
                let Some(engine) = weak.upgrade() else {
                    break;
                };
                let inner = engine.inner.lock();
                let Some(sink) = inner.sink.as_ref() else {
                    was_playing_source = false;
                    continue;
                };
                let status = inner.state.status;
                let empty = sink.empty();
                let pos = sink.get_pos().as_secs_f64();

                if matches!(status, PlaybackStatus::Playing) && !empty {
                    was_playing_source = true;
                    // Emit a position tick when it moved meaningfully.
                    if last_pos_emit.is_nan() || (pos - last_pos_emit).abs() >= 0.2 {
                        last_pos_emit = pos;
                        let _ = engine.events_tx.send(EngineEvent::Position {
                            position_seconds: pos,
                        });
                    }
                } else if was_playing_source
                    && empty
                    && matches!(status, PlaybackStatus::Playing)
                {
                    // The source drained while still in Playing: natural
                    // end of track.
                    was_playing_source = false;
                    last_pos_emit = f64::NAN;
                    let _ = engine.events_tx.send(EngineEvent::EndOfTrack);
                }
                drop(inner);
            }
        })
        .map_err(|e| EngineError::Internal(e.to_string()))?;

    Ok(engine)
}
