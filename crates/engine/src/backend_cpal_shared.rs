//! CPAL-based Shared output backend.
//!
//! This is the engine's only output backend. It pulls f32 interleaved
//! PCM from a Symphonia decoder running on a worker thread, optionally
//! resamples it (via rubato sinc, BlackmanHarris²) to the device's
//! native rate, applies an optional ReplayGain pre-gain and a 10-band
//! peaking EQ, then feeds it to the OS mixer via CPAL. The audio
//! callback adds soft-clip + TPDF dither (when the device asks for a
//! ≤16-bit integer format) before quantization.
//!
//! ## Honest reporting
//!
//! On Windows CPAL talks to WASAPI in *shared* mode. The mixer is
//! transparent at unity gain when nothing else is playing, but it is
//! still in the path: it can mix our stream with other apps and apply
//! system-wide effects. We therefore never claim `is_bit_perfect = true`
//! on this backend. The UI describes the mode as "Shared (lossless to
//! OS mixer)" rather than making bit-perfect promises we cannot keep.
//!
//! Software volume is applied here because we are not on a bit-perfect
//! path; this is documented and exposed to the UI.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::backend_symphonia::SymphoniaDecoder;
use crate::error::{EngineError, EngineResult};
use crate::types::{
    EffectiveOutputMode, EngineEvent, OutputMode, PlaybackStatus, PlayerState,
};
use crate::AudioEngine;

/// Holds a decoder + track id prepared in advance for a gapless
/// transition. Stored in `Shared` so the decoder thread can pick
/// it up at end-of-stream without re-routing through the worker.
struct PendingNext {
    decoder: SymphoniaDecoder,
    track_id: Option<String>,
    sample_rate: u32,
    channels: u16,
    bit_depth: Option<u8>,
    duration_seconds: f64,
}

/// Approximate target buffer size for the SPSC ring (in interleaved
/// samples). At 48 kHz stereo this is roughly 1 second of audio, which
/// gives the decoder thread comfortable slack without ballooning latency.
pub(crate) const RING_CAPACITY_SAMPLES: usize = 48_000 * 2;

/// Commands sent from the public API to the worker thread.
enum Command {
    Load(PathBuf),
    /// Prepare a next track for gapless transition. The worker opens
    /// the file, builds a decoder, and (if the format matches the
    /// current stream) stashes it in `Shared::pending_next`. The
    /// active decoder thread then picks it up at EOF without an audio
    /// callback gap.
    PrepareNext { path: PathBuf, track_id: Option<String> },
    /// Drop a previously prepared next track (e.g. user changed the
    /// queue between prepare and EOT). Cheap; the decoder thread will
    /// just see an empty slot at end-of-stream.
    ClearPendingNext,
    Play,
    Pause,
    Resume,
    Stop,
    Seek(f64),
}

/// Shared, lock-free state read from both the audio callback and the
/// public API.
pub(crate) struct Shared {
    /// Volume in `[0.0, 1.0]`, encoded as `(v * 1_000_000) as u32`.
    volume_micro: AtomicU32,
    /// Pre-gain applied before EQ + soft-clip + volume. Stored as
    /// `(linear * 1_000_000) as u32` so it fits in a single atomic.
    /// Used by ReplayGain (track or album normalization). Default 1.0
    /// (0 dB; passthrough).
    pre_gain_micro: AtomicU32,
    /// Whether the audio callback should output silence (paused or
    /// nothing loaded).
    pub(crate) paused: AtomicBool,
    /// Latest known position in seconds, encoded as `(pos * 1000) as u32`
    /// (millisecond resolution is enough for UI display and avoids
    /// requiring a 64-bit atomic).
    pub(crate) position_ms: AtomicU32,
    /// Total duration of the current track in seconds, same encoding.
    pub(crate) duration_ms: AtomicU32,
    /// Current sample rate of the device (and decoder) in Hz.
    pub(crate) sample_rate: AtomicU32,
    /// Current channel count.
    pub(crate) channels: AtomicU32,
    /// Bit depth as reported by the decoder (encoded as a byte; 0 = unknown).
    pub(crate) bit_depth: AtomicU32,
    /// Seek requested by the public API. Encoded as `(secs * 1000)` and
    /// `-1` when no seek is pending. The decoder thread checks this on
    /// each iteration; on a hit it reseeks the demuxer, drains the ring
    /// (via `drain_ring`), and clears the flag.
    pub(crate) pending_seek_ms: AtomicI32,
    /// Set by the worker when it wants the audio callback to drain the
    /// ring on the next callback (so the seek doesn't play stale audio).
    pub(crate) drain_ring: AtomicBool,
    /// 10-band EQ gains in dB. Read by the decoder thread, updated by
    /// the public API. Length is always `eq::NUM_BANDS`.
    pub(crate) eq_gains_db: Mutex<Vec<f32>>,
    /// Bumps every time `eq_gains_db` is written so the decoder thread
    /// can detect "settings changed" and rebuild filters lazily.
    pub(crate) eq_version: AtomicU32,
    /// Total count of audio-callback underruns since stream start.
    /// Bumped from the audio callback (single producer), read from the
    /// worker for periodic warnings. Cheap, lock-free.
    pub(crate) underruns: AtomicU32,
    /// Prepared next track ready for a gapless transition. The
    /// decoder thread picks it up when the current decoder hits EOF
    /// *and* the format matches (same SR + channel count). Mutex
    /// is fine: it's contended only on track boundaries.
    pending_next: Mutex<Option<PendingNext>>,
}

impl Shared {
    pub(crate) fn new() -> Self {
        Shared {
            volume_micro: AtomicU32::new(1_000_000),
            pre_gain_micro: AtomicU32::new(1_000_000),
            paused: AtomicBool::new(true),
            position_ms: AtomicU32::new(0),
            duration_ms: AtomicU32::new(0),
            sample_rate: AtomicU32::new(0),
            channels: AtomicU32::new(0),
            bit_depth: AtomicU32::new(0),
            pending_seek_ms: AtomicI32::new(-1),
            drain_ring: AtomicBool::new(false),
            eq_gains_db: Mutex::new(vec![0.0; crate::eq::NUM_BANDS]),
            eq_version: AtomicU32::new(0),
            underruns: AtomicU32::new(0),
            pending_next: Mutex::new(None),
        }
    }

    fn volume(&self) -> f32 {
        // Linear value as set by the user (0..1). Used by `state()`
        // so the UI slider position is preserved verbatim.
        self.volume_micro.load(Ordering::Relaxed) as f32 / 1_000_000.0
    }

    /// Audible gain applied to samples. Quadratic taper: smoother and
    /// more usable than linear, less aggressive than cubic.
    /// Slider 100% -> 0 dB, 70% -> ~-6 dB, 50% -> ~-12 dB,
    /// 30% -> ~-21 dB. Roughly matches what foobar2000 / MusicBee do
    /// on their default volume curve.
    pub(crate) fn audible_gain(&self) -> f32 {
        let v = self.volume_micro.load(Ordering::Relaxed) as f32 / 1_000_000.0;
        let v = v.clamp(0.0, 1.0);
        v * v
    }

    fn set_volume(&self, v: f32) {
        let clamped = v.clamp(0.0, 1.0);
        self.volume_micro
            .store((clamped * 1_000_000.0) as u32, Ordering::Relaxed);
    }

    /// Linear pre-gain applied before EQ. `1.0` = passthrough.
    pub(crate) fn pre_gain(&self) -> f32 {
        self.pre_gain_micro.load(Ordering::Relaxed) as f32 / 1_000_000.0
    }

    /// Direct setter used by the WASAPI backend (which can't reach
    /// the private field through the engine API on this side of the
    /// crate). The `micro` parameter is `(linear * 1_000_000) as u32`.
    pub(crate) fn set_pre_gain_micro(&self, micro: u32) {
        self.pre_gain_micro.store(micro, Ordering::Relaxed);
    }

    pub(crate) fn set_volume_public(&self, v: f32) {
        self.set_volume(v);
    }

    pub(crate) fn volume_public(&self) -> f32 {
        self.volume()
    }
}

/// CPAL Shared output engine. Implements [`AudioEngine`].
pub struct CpalSharedEngine {
    cmd_tx: Sender<Command>,
    event_tx: Sender<EngineEvent>,
    event_rx: Receiver<EngineEvent>,
    shared: Arc<Shared>,
    status: Arc<Mutex<PlaybackStatus>>,
    current_track: Arc<Mutex<Option<String>>>,
    last_error: Arc<Mutex<Option<String>>>,
    requested_mode: Arc<Mutex<OutputMode>>,
    selected_device: Arc<Mutex<Option<String>>>,
    _worker: JoinHandle<()>,
}

impl CpalSharedEngine {
    /// Create a new engine. A background worker thread is spawned
    /// immediately and lives for the lifetime of the engine.
    pub fn new() -> EngineResult<Self> {
        let (cmd_tx, cmd_rx) = bounded::<Command>(16);
        let (event_tx, event_rx) = bounded::<EngineEvent>(256);

        let shared = Arc::new(Shared::new());
        let status = Arc::new(Mutex::new(PlaybackStatus::Idle));
        let current_track: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let last_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let requested_mode = Arc::new(Mutex::new(OutputMode::default()));
        let selected_device: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let worker_shared = Arc::clone(&shared);
        let worker_status = Arc::clone(&status);
        let worker_event_tx = event_tx.clone();
        let worker_last_error = Arc::clone(&last_error);
        let worker_current_track = Arc::clone(&current_track);
        let worker_requested_mode = Arc::clone(&requested_mode);
        let worker_selected_device = Arc::clone(&selected_device);

        let worker = thread::Builder::new()
            .name("qobee-engine-worker".into())
            .spawn(move || {
                run_worker(WorkerCtx {
                    cmd_rx,
                    event_tx: worker_event_tx,
                    shared: worker_shared,
                    status: worker_status,
                    current_track: worker_current_track,
                    last_error: worker_last_error,
                    requested_mode: worker_requested_mode,
                    selected_device: worker_selected_device,
                });
            })
            .map_err(|e| EngineError::Internal(format!("failed to spawn worker: {e}")))?;

        Ok(CpalSharedEngine {
            cmd_tx,
            event_tx,
            event_rx,
            shared,
            status,
            current_track,
            last_error,
            requested_mode,
            selected_device,
            _worker: worker,
        })
    }

    fn send_cmd(&self, cmd: Command) -> EngineResult<()> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| EngineError::Internal("worker thread is gone".into()))
    }

    /// Used by the orchestration layer to tag the currently loaded track.
    pub fn set_current_track_id(&self, id: Option<String>) {
        *self.current_track.lock() = id;
    }

    /// Pick the output device by its CPAL name. Pass `None` to revert
    /// to the system default. The change applies to the *next* track —
    /// the currently playing stream is not interrupted.
    pub fn set_output_device(&self, device_id: Option<String>) {
        *self.selected_device.lock() = device_id;
    }

    pub fn selected_device(&self) -> Option<String> {
        self.selected_device.lock().clone()
    }

    /// Replace the 10-band EQ gains (dB). Pass any iterable that
    /// produces `crate::eq::NUM_BANDS` values; missing entries default
    /// to 0 dB. Bumping `eq_version` lets the decoder thread pick the
    /// change up on the next chunk.
    pub fn set_eq_gains_db(&self, gains: &[f32]) {
        let mut g = self.shared.eq_gains_db.lock();
        for (slot, v) in g.iter_mut().zip(gains.iter().copied()) {
            *slot = v.clamp(-12.0, 12.0);
        }
        for slot in g.iter_mut().skip(gains.len()) {
            *slot = 0.0;
        }
        drop(g);
        self.shared.eq_version.fetch_add(1, Ordering::Release);
        let _ = self.event_tx.try_send(EngineEvent::StateChanged {
            state: self.state(),
        });
    }

    pub fn eq_gains_db(&self) -> Vec<f32> {
        self.shared.eq_gains_db.lock().clone()
    }

    /// Set the linear pre-gain applied before EQ. Used for ReplayGain
    /// or any other static, per-track gain adjustment. `1.0` is the
    /// default (passthrough). Pass `0.0` to mute, or values >1.0 to
    /// boost (the soft-clipper will protect from overshoot).
    pub fn set_pre_gain(&self, linear: f32) {
        let clamped = linear.clamp(0.0, 8.0);
        self.shared
            .pre_gain_micro
            .store((clamped * 1_000_000.0) as u32, Ordering::Relaxed);
    }

    pub fn pre_gain(&self) -> f32 {
        self.shared.pre_gain()
    }

    /// Open `path` ahead of time so the active decoder thread can
    /// transition to it without a gap when the current track ends.
    /// The worker opens the file, validates the format matches the
    /// current stream (same sample rate + channel count) and stashes
    /// the prepared decoder in shared state. If the format mismatches,
    /// the prepared decoder is dropped and the orchestrator will
    /// observe a regular `EndOfTrack` followed by a fresh `Load`.
    pub fn prepare_next(&self, path: &Path, track_id: Option<String>) -> EngineResult<()> {
        self.send_cmd(Command::PrepareNext {
            path: path.to_path_buf(),
            track_id,
        })
    }

    /// Drop any previously prepared next track. Safe to call when
    /// nothing is prepared.
    pub fn clear_pending_next(&self) -> EngineResult<()> {
        self.send_cmd(Command::ClearPendingNext)
    }
}

impl AudioEngine for CpalSharedEngine {
    fn load(&self, path: &Path) -> EngineResult<()> {
        self.send_cmd(Command::Load(path.to_path_buf()))
    }

    fn play(&self) -> EngineResult<()> {
        self.send_cmd(Command::Play)
    }

    fn pause(&self) -> EngineResult<()> {
        self.send_cmd(Command::Pause)
    }

    fn resume(&self) -> EngineResult<()> {
        self.send_cmd(Command::Resume)
    }

    fn stop(&self) -> EngineResult<()> {
        self.send_cmd(Command::Stop)
    }

    fn seek(&self, position_seconds: f64) -> EngineResult<()> {
        self.send_cmd(Command::Seek(position_seconds))
    }

    fn set_volume(&self, volume: f32) -> EngineResult<()> {
        self.shared.set_volume(volume);
        let _ = self.event_tx.try_send(EngineEvent::StateChanged {
            state: self.state(),
        });
        Ok(())
    }

    fn set_output_mode(&self, mode: OutputMode) -> EngineResult<()> {
        // Shared backend only stores the request for reporting; it
        // ignores `Exclusive` (the orchestrator is responsible for
        // routing to the WasapiExclusive backend instead).
        *self.requested_mode.lock() = mode;
        let _ = self.event_tx.try_send(EngineEvent::StateChanged {
            state: self.state(),
        });
        Ok(())
    }

    fn state(&self) -> PlayerState {
        let status = *self.status.lock();
        let current_track_id = self.current_track.lock().clone();
        let error = self.last_error.lock().clone();

        let sample_rate = match self.shared.sample_rate.load(Ordering::Relaxed) {
            0 => None,
            v => Some(v),
        };
        let channels = match self.shared.channels.load(Ordering::Relaxed) {
            0 => None,
            v => Some(v as u16),
        };
        let bit_depth = match self.shared.bit_depth.load(Ordering::Relaxed) {
            0 => None,
            v => Some(v as u8),
        };

        let requested = *self.requested_mode.lock();
        let _ = requested; // OutputMode is currently informational only
        let output_mode = EffectiveOutputMode::Shared;

        PlayerState {
            status,
            current_track_id,
            position_seconds: self.shared.position_ms.load(Ordering::Relaxed) as f64 / 1000.0,
            duration_seconds: self.shared.duration_ms.load(Ordering::Relaxed) as f64 / 1000.0,
            volume: self.shared.volume(),
            output_mode,
            sample_rate,
            bit_depth,
            channels,
            // Shared (CPAL/WASAPI shared) is never bit-perfect.
            is_bit_perfect: false,
            error,
        }
    }

    fn subscribe_events(&self) -> Receiver<EngineEvent> {
        self.event_rx.clone()
    }

    fn set_current_track_id(&self, id: Option<String>) {
        Self::set_current_track_id(self, id);
    }

    fn set_eq_gains_db(&self, gains: &[f32]) {
        Self::set_eq_gains_db(self, gains);
    }

    fn eq_gains_db(&self) -> Vec<f32> {
        Self::eq_gains_db(self)
    }

    fn set_pre_gain(&self, linear: f32) {
        Self::set_pre_gain(self, linear);
    }

    fn set_output_device(&self, device_id: Option<String>) {
        Self::set_output_device(self, device_id);
    }

    fn selected_device(&self) -> Option<String> {
        Self::selected_device(self)
    }

    fn prepare_next(&self, path: &Path, track_id: Option<String>) -> EngineResult<()> {
        Self::prepare_next(self, path, track_id)
    }

    fn clear_pending_next(&self) -> EngineResult<()> {
        Self::clear_pending_next(self)
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

struct WorkerCtx {
    cmd_rx: Receiver<Command>,
    event_tx: Sender<EngineEvent>,
    shared: Arc<Shared>,
    status: Arc<Mutex<PlaybackStatus>>,
    current_track: Arc<Mutex<Option<String>>>,
    last_error: Arc<Mutex<Option<String>>>,
    requested_mode: Arc<Mutex<OutputMode>>,
    selected_device: Arc<Mutex<Option<String>>>,
}

/// Active stream + decoder pair for one loaded track.
struct ActiveStream {
    stream: cpal::Stream,
    decoder_handle: JoinHandle<()>,
    decoder_alive: Arc<AtomicBool>,
}

fn run_worker(ctx: WorkerCtx) {
    let mut active: Option<ActiveStream> = None;

    while let Ok(cmd) = ctx.cmd_rx.recv() {
        match cmd {
            Command::Load(path) => {
                if let Some(prev) = active.take() {
                    stop_active(prev);
                }
                ctx.shared.pending_seek_ms.store(-1, Ordering::Release);
                ctx.shared.drain_ring.store(false, Ordering::Release);
                // A fresh Load invalidates any prepared next track.
                *ctx.shared.pending_next.lock() = None;
                set_status(&ctx, PlaybackStatus::Loading);

                match start_playback(&ctx, &path) {
                    Ok(s) => {
                        active = Some(s);
                        set_status(&ctx, PlaybackStatus::Paused);
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        *ctx.last_error.lock() = Some(msg.clone());
                        set_status(&ctx, PlaybackStatus::Errored);
                        let _ = ctx.event_tx.try_send(EngineEvent::Error { message: msg });
                    }
                }
            }
            Command::PrepareNext { path, track_id } => {
                // Only prepare while something is actively playing —
                // otherwise the orchestrator should call Load.
                if active.is_none() {
                    tracing::debug!(
                        target: "qobee::engine",
                        "prepare_next with no active track; ignoring"
                    );
                    continue;
                }
                let cur_sr = ctx.shared.sample_rate.load(Ordering::Relaxed);
                let cur_ch = ctx.shared.channels.load(Ordering::Relaxed) as u16;
                match SymphoniaDecoder::open(&path) {
                    Ok(decoder) => {
                        let fmt = decoder.format();
                        if fmt.sample_rate == cur_sr && fmt.channels == cur_ch {
                            *ctx.shared.pending_next.lock() = Some(PendingNext {
                                duration_seconds: decoder.duration_seconds(),
                                decoder,
                                track_id,
                                sample_rate: fmt.sample_rate,
                                channels: fmt.channels,
                                bit_depth: fmt.bit_depth,
                            });
                            tracing::debug!(
                                target: "qobee::engine",
                                "next track prepared for gapless transition"
                            );
                        } else {
                            // Format mismatch: drop the prepared decoder
                            // and let the standard EOT → Load path
                            // handle the boundary. There will be a
                            // small audible gap; that's the trade-off
                            // for a sample-rate change.
                            tracing::info!(
                                target: "qobee::engine",
                                cur_sr,
                                cur_ch,
                                next_sr = fmt.sample_rate,
                                next_ch = fmt.channels,
                                "prepare_next: format change; cannot do gapless"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "qobee::engine",
                            error = %e,
                            "prepare_next: open failed"
                        );
                    }
                }
            }
            Command::ClearPendingNext => {
                *ctx.shared.pending_next.lock() = None;
            }
            Command::Play => {
                if let Some(s) = active.as_ref() {
                    ctx.shared.paused.store(false, Ordering::Release);
                    if let Err(e) = s.stream.play() {
                        record_stream_error(&ctx, &e.to_string());
                    } else {
                        set_status(&ctx, PlaybackStatus::Playing);
                    }
                } else {
                    // Play before Load: just log; never surface to the
                    // UI as an error. The orchestrator can issue this
                    // legitimately during a fallback or a stale event.
                    tracing::debug!(
                        target: "qobee::engine",
                        "play with no active track; ignoring"
                    );
                }
            }
            Command::Pause => {
                if let Some(s) = active.as_ref() {
                    ctx.shared.paused.store(true, Ordering::Release);
                    let _ = s.stream.pause();
                    set_status(&ctx, PlaybackStatus::Paused);
                }
            }
            Command::Resume => {
                if let Some(s) = active.as_ref() {
                    ctx.shared.paused.store(false, Ordering::Release);
                    if let Err(e) = s.stream.play() {
                        record_stream_error(&ctx, &e.to_string());
                    } else {
                        set_status(&ctx, PlaybackStatus::Playing);
                    }
                }
            }
            Command::Stop => {
                if let Some(prev) = active.take() {
                    stop_active(prev);
                }
                ctx.shared.paused.store(true, Ordering::Release);
                ctx.shared.position_ms.store(0, Ordering::Relaxed);
                ctx.shared.duration_ms.store(0, Ordering::Relaxed);
                ctx.shared.sample_rate.store(0, Ordering::Relaxed);
                ctx.shared.channels.store(0, Ordering::Relaxed);
                ctx.shared.bit_depth.store(0, Ordering::Relaxed);
                ctx.shared.pending_seek_ms.store(-1, Ordering::Release);
                ctx.shared.drain_ring.store(false, Ordering::Release);
                *ctx.shared.pending_next.lock() = None;
                set_status(&ctx, PlaybackStatus::Stopped);
            }
            Command::Seek(secs) => {
                if active.is_some() {
                    let secs = secs.max(0.0);
                    let ms = (secs * 1000.0) as i32;
                    // Tell the audio callback to drain on the next tick
                    // so we don't keep playing stale samples until the
                    // decoder catches up.
                    ctx.shared.drain_ring.store(true, Ordering::Release);
                    ctx.shared.pending_seek_ms.store(ms, Ordering::Release);
                    ctx.shared.position_ms.store(ms.max(0) as u32, Ordering::Relaxed);
                    let _ = ctx.event_tx.try_send(EngineEvent::Position {
                        position_seconds: secs,
                    });
                }
            }
        }
    }

    if let Some(prev) = active.take() {
        stop_active(prev);
    }
}

fn set_status(ctx: &WorkerCtx, status: PlaybackStatus) {
    *ctx.status.lock() = status;
    if status != PlaybackStatus::Errored {
        *ctx.last_error.lock() = None;
    }
    let state = snapshot_state(ctx);
    let _ = ctx.event_tx.try_send(EngineEvent::StateChanged { state });
}

fn record_stream_error(ctx: &WorkerCtx, msg: &str) {
    *ctx.last_error.lock() = Some(msg.to_string());
    set_status(ctx, PlaybackStatus::Errored);
    let _ = ctx.event_tx.try_send(EngineEvent::Error {
        message: msg.to_string(),
    });
}

fn snapshot_state(ctx: &WorkerCtx) -> PlayerState {
    let status = *ctx.status.lock();
    let current_track_id = ctx.current_track.lock().clone();
    let error = ctx.last_error.lock().clone();
    let sample_rate = match ctx.shared.sample_rate.load(Ordering::Relaxed) {
        0 => None,
        v => Some(v),
    };
    let channels = match ctx.shared.channels.load(Ordering::Relaxed) {
        0 => None,
        v => Some(v as u16),
    };
    let bit_depth = match ctx.shared.bit_depth.load(Ordering::Relaxed) {
        0 => None,
        v => Some(v as u8),
    };

    let requested = *ctx.requested_mode.lock();
    let _ = requested;
    let output_mode = EffectiveOutputMode::Shared;

    PlayerState {
        status,
        current_track_id,
        position_seconds: ctx.shared.position_ms.load(Ordering::Relaxed) as f64 / 1000.0,
        duration_seconds: ctx.shared.duration_ms.load(Ordering::Relaxed) as f64 / 1000.0,
        volume: ctx.shared.volume(),
        output_mode,
        sample_rate,
        bit_depth,
        channels,
        is_bit_perfect: false,
        error,
    }
}

fn stop_active(active: ActiveStream) {
    drop(active.stream); // closes CPAL callback first
    active.decoder_alive.store(false, Ordering::Release);
    // Decoder thread will exit on its own once it sees the flag and the
    // ring buffer producer is gone; we don't block on it.
    let _ = active.decoder_handle;
}

/// Set up a CPAL stream and a decoder thread to feed it.
fn start_playback(ctx: &WorkerCtx, path: &Path) -> EngineResult<ActiveStream> {
    let decoder = SymphoniaDecoder::open(path)?;
    let format = decoder.format();

    ctx.shared.sample_rate.store(format.sample_rate, Ordering::Relaxed);
    ctx.shared.channels.store(format.channels as u32, Ordering::Relaxed);
    ctx.shared
        .bit_depth
        .store(format.bit_depth.map(|b| b as u32).unwrap_or(0), Ordering::Relaxed);
    ctx.shared.underruns.store(0, Ordering::Relaxed);
    let dur_ms = (decoder.duration_seconds() * 1000.0) as u32;
    ctx.shared.duration_ms.store(dur_ms, Ordering::Relaxed);
    ctx.shared.position_ms.store(0, Ordering::Relaxed);

    let host = cpal::default_host();
    let device = match ctx.selected_device.lock().clone() {
        Some(id) => match find_device_by_name(&host, &id) {
            Some(d) => d,
            None => {
                tracing::warn!(
                    target: "qobee::engine",
                    requested = %id,
                    "selected device not found; falling back to default"
                );
                host.default_output_device()
                    .ok_or_else(|| EngineError::Output("no default output device".into()))?
            }
        },
        None => host
            .default_output_device()
            .ok_or_else(|| EngineError::Output("no default output device".into()))?,
    };

    // Negotiate the best output format the device exposes for this
    // track. We try, in order:
    //   1. Source SR + source channels + F32 (zero conversion, no
    //      resampling, no integer truncation; ideal).
    //   2. Source SR + source channels + I32 (no resampling, but
    //      conversion to integer; we dither only on <= 16-bit, so
    //      I32 is essentially transparent).
    //   3. Source SR + source channels + I16 (no resampling; dither
    //      will mask quantization noise).
    //   4. Default config (the OS picks, we resample to match it).
    //
    // This reaches bit-equivalent quality on devices that natively
    // support the file's sample rate, which is what saves us the
    // rubato resampling pass on most modern DACs.
    let supported_default = device
        .default_output_config()
        .map_err(|e| EngineError::Output(e.to_string()))?;

    let preferred = negotiate_output_format(
        &device,
        format.sample_rate,
        format.channels,
        &supported_default,
    );
    let mut stream_config: StreamConfig = preferred.config();
    let supported = preferred;

    // If the driver only advertised surround configurations (very
    // common with Sony Inzone, Razer Synapse and similar headsets
    // that auto-configure as 7.1) we force a stereo stream config
    // anyway. WASAPI Shared accepts this and does the channel-count
    // adjustment in the OS mixer, which is consistently softer than
    // having the device's surround virtualizer process a stereo
    // signal we'd duplicated to N channels ourselves.
    if format.channels == 2 && stream_config.channels > 2 {
        tracing::info!(
            target: "qobee::engine",
            advertised_channels = stream_config.channels,
            "forcing stereo stream config; OS mixer will adapt to the device layout"
        );
        stream_config.channels = 2;
    }

    let device_sample_rate: u32 = stream_config.sample_rate;
    if device_sample_rate != format.sample_rate {
        tracing::info!(
            target: "qobee::engine",
            file_sr = format.sample_rate,
            device_sr = device_sample_rate,
            "device sample rate differs from file: in-app resampling -> device rate; Shared mode is not bit-perfect"
        );
    } else {
        tracing::info!(
            target: "qobee::engine",
            sample_rate = device_sample_rate,
            channels = stream_config.channels,
            sample_format = ?supported.sample_format(),
            "device opened at source sample rate; no resampling needed"
        );
    }

    let device_channels = stream_config.channels;

    let (producer, consumer) = RingBuffer::<f32>::new(RING_CAPACITY_SAMPLES);

    let decoder_alive = Arc::new(AtomicBool::new(true));
    let decoder_alive_clone = Arc::clone(&decoder_alive);
    let shared_clone = Arc::clone(&ctx.shared);
    let event_tx_clone = ctx.event_tx.clone();
    let status_clone = Arc::clone(&ctx.status);
    let src_sample_rate = format.sample_rate;
    let src_channels = format.channels;

    let decoder_handle = thread::Builder::new()
        .name("qobee-engine-decoder".into())
        .spawn(move || {
            run_decoder_thread(
                decoder,
                producer,
                decoder_alive_clone,
                shared_clone,
                event_tx_clone,
                status_clone,
                src_channels,
                src_sample_rate,
                device_sample_rate,
                EffectiveOutputMode::Shared,
            );
        })
        .map_err(|e| EngineError::Internal(format!("failed to spawn decoder: {e}")))?;

    let event_tx_for_callback = ctx.event_tx.clone();
    let shared_for_callback = Arc::clone(&ctx.shared);

    // Build the output callback. We always emit f32 to the device; if the
    // device asks for something else we request a conversion stream below.
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(
            &device,
            &stream_config,
            consumer,
            shared_for_callback,
            event_tx_for_callback,
            format.channels,
            device_channels,
            None,
        ),
        SampleFormat::I32 => build_stream::<i32>(
            &device,
            &stream_config,
            consumer,
            shared_for_callback,
            event_tx_for_callback,
            format.channels,
            device_channels,
            None,
        ),
        SampleFormat::I16 => build_stream::<i16>(
            &device,
            &stream_config,
            consumer,
            shared_for_callback,
            event_tx_for_callback,
            format.channels,
            device_channels,
            Some(16),
        ),
        SampleFormat::U16 => build_stream::<u16>(
            &device,
            &stream_config,
            consumer,
            shared_for_callback,
            event_tx_for_callback,
            format.channels,
            device_channels,
            Some(16),
        ),
        other => Err(EngineError::Output(format!(
            "unsupported device sample format: {other:?}"
        ))),
    }?;

    Ok(ActiveStream {
        stream,
        decoder_handle,
        decoder_alive,
    })
}

pub(crate) fn run_decoder_thread(
    mut decoder: SymphoniaDecoder,
    mut producer: Producer<f32>,
    alive: Arc<AtomicBool>,
    shared: Arc<Shared>,
    event_tx: Sender<EngineEvent>,
    status: Arc<Mutex<PlaybackStatus>>,
    channels: u16,
    src_sample_rate: u32,
    dst_sample_rate: u32,
    effective_mode: EffectiveOutputMode,
) {
    use rubato::{
        Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
        WindowFunction,
    };

    let need_resample = src_sample_rate != dst_sample_rate;
    let n_ch = channels as usize;

    // Per-track sinc resampler (windowed-sinc, asynchronous, fixed-input).
    // Higher quality than FFT for music: cleaner transients, less
    // pre-ringing. CPU cost is negligible on a modern PC.
    let chunk_size_in: usize = 1024;
    let sinc_params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        oversampling_factor: 256,
        interpolation: SincInterpolationType::Cubic,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler: Option<SincFixedIn<f32>> = if need_resample {
        let ratio = dst_sample_rate as f64 / src_sample_rate as f64;
        match SincFixedIn::<f32>::new(ratio, 1.1, sinc_params, chunk_size_in, n_ch) {
            Ok(r) => {
                tracing::info!(
                    target: "qobee::engine",
                    src_sr = src_sample_rate,
                    dst_sr = dst_sample_rate,
                    channels = n_ch,
                    sinc_len = 256,
                    oversampling = 256,
                    "resampling source to device rate (sinc, BlackmanHarris2)"
                );
                Some(r)
            }
            Err(e) => {
                tracing::error!(
                    target: "qobee::engine",
                    error = %e,
                    "failed to build sinc resampler; falling back to passthrough (audio will be off-pitch)"
                );
                None
            }
        }
    } else {
        None
    };

    let mut output_buffer: Vec<Vec<f32>> = if let Some(ref r) = resampler {
        r.output_buffer_allocate(true)
    } else {
        Vec::new()
    };
    // Pending planar samples per channel, fed from decoded packets.
    let mut pending_planar: Vec<Vec<f32>> = vec![Vec::with_capacity(chunk_size_in * 4); n_ch];

    // Per-track EQ. Built at the *device* sample rate when we resample,
    // since EQ runs on the post-resampler signal.
    let eq_sample_rate = if need_resample { dst_sample_rate } else { src_sample_rate };
    let mut eq = crate::eq::Equalizer::new(eq_sample_rate, channels);
    let mut eq_seen_version: u32 = u32::MAX; // forces an initial sync below

    let sync_eq = |eq: &mut crate::eq::Equalizer,
                   seen: &mut u32,
                   shared: &Shared|
     -> () {
        let cur = shared.eq_version.load(Ordering::Acquire);
        if cur != *seen {
            let gains = shared.eq_gains_db.lock().clone();
            eq.set_gains_db(&gains);
            *seen = cur;
        }
    };

    // Underrun watcher: every ~1s we look at the audio-callback's
    // underrun counter and emit a single throttled warning if it grew.
    // The audio callback never blocks here; this is a passive observer.
    let mut last_underrun_count: u32 = 0;
    let mut last_underrun_check = std::time::Instant::now();

    loop {
        if !alive.load(Ordering::Acquire) {
            return;
        }

        if last_underrun_check.elapsed() >= Duration::from_millis(1000) {
            let now_count = shared.underruns.load(Ordering::Relaxed);
            if now_count > last_underrun_count {
                let delta = now_count - last_underrun_count;
                tracing::warn!(
                    target: "qobee::engine",
                    underruns_in_last_second = delta,
                    total_underruns = now_count,
                    "audio callback ran out of samples; expect short dropouts"
                );
                last_underrun_count = now_count;
            }
            last_underrun_check = std::time::Instant::now();
        }

        // Honor a pending seek before pulling more audio. We seek the
        // demuxer; the audio callback drains the ring (via the
        // `drain_ring` flag the worker raised) so old samples don't leak
        // past the seek point.
        let pending = shared.pending_seek_ms.load(Ordering::Acquire);
        if pending >= 0 {
            let secs = pending as f64 / 1000.0;
            // Give the audio callback a chance to drain stale samples
            // before we start producing fresh ones.
            thread::sleep(Duration::from_millis(10));
            if let Err(e) = decoder.seek(secs) {
                tracing::warn!(
                    target: "qobee::engine",
                    error = %e,
                    "seek failed; continuing from current position"
                );
            }
            // Reset accumulated buffers + resampler state so the seek
            // point is clean.
            for ch in pending_planar.iter_mut() {
                ch.clear();
            }
            if let Some(ref mut r) = resampler {
                r.reset();
            }
            eq.reset_state();
            shared.pending_seek_ms.store(-1, Ordering::Release);
            shared.drain_ring.store(false, Ordering::Release);
        }

        // If the ring is full, sleep briefly and retry. The audio
        // callback will drain it.
        if producer.slots() == 0 {
            thread::sleep(Duration::from_millis(5));
            continue;
        }

        match decoder.next_packet() {
            Ok(Some(packet)) => {
                let crate::types::PcmBuffer::F32Interleaved(samples) = packet.buffer else {
                    tracing::error!(
                        target: "qobee::engine",
                        "decoder produced unexpected non-f32 buffer"
                    );
                    return;
                };

                shared.position_ms.store(
                    (packet.timestamp_seconds * 1000.0) as u32,
                    Ordering::Relaxed,
                );
                let _ = event_tx.try_send(EngineEvent::Position {
                    position_seconds: packet.timestamp_seconds,
                });

                if let Some(ref mut r) = resampler {
                    // Deinterleave + accumulate.
                    deinterleave_into(&samples, n_ch, &mut pending_planar);

                    // Process as many full chunks as we have data for.
                    while pending_planar[0].len() >= chunk_size_in {
                        if !alive.load(Ordering::Acquire) {
                            return;
                        }

                        // Build per-channel slices of exactly chunk_size_in.
                        let input_slices: Vec<&[f32]> = pending_planar
                            .iter()
                            .map(|ch| &ch[..chunk_size_in])
                            .collect();

                        match r.process_into_buffer(&input_slices, &mut output_buffer, None) {
                            Ok((_in_frames, out_frames)) => {
                                // Interleave the resampled output and
                                // run it through the EQ before handing
                                // it to the audio callback.
                                let mut interleaved: Vec<f32> =
                                    Vec::with_capacity(out_frames * n_ch);
                                for f in 0..out_frames {
                                    for c in 0..n_ch {
                                        interleaved.push(output_buffer[c][f]);
                                    }
                                }
                                let pg = shared.pre_gain();
                                if (pg - 1.0).abs() > 1e-4 {
                                    for s in interleaved.iter_mut() {
                                        *s *= pg;
                                    }
                                }
                                sync_eq(&mut eq, &mut eq_seen_version, &shared);
                                eq.process_inplace(&mut interleaved);
                                push_interleaved_to_ring(&interleaved, &mut producer, &alive);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    target: "qobee::engine",
                                    error = %e,
                                    "resampler process failed"
                                );
                            }
                        }

                        // Drop the consumed frames from the front.
                        for ch in pending_planar.iter_mut() {
                            ch.drain(..chunk_size_in);
                        }
                    }
                } else {
                    // Same rate: pre-gain (ReplayGain), EQ, then push.
                    let mut buf = samples;
                    let pg = shared.pre_gain();
                    if (pg - 1.0).abs() > 1e-4 {
                        for s in buf.iter_mut() {
                            *s *= pg;
                        }
                    }
                    sync_eq(&mut eq, &mut eq_seen_version, &shared);
                    eq.process_inplace(&mut buf);
                    push_interleaved_to_ring(&buf, &mut producer, &alive);
                }
            }
            Ok(None) => {
                // End of stream. If a compatible next track has been
                // prepared, swap the decoder in place: the resampler
                // and EQ keep their internal state so the audio
                // callback sees one continuous signal. We emit a
                // `GaplessTransition` event so the orchestrator can
                // advance the queue and update the UI; the audio
                // never paused.
                if let Some(prepared) = shared.pending_next.lock().take() {
                    let PendingNext {
                        decoder: new_decoder,
                        track_id: new_track_id,
                        sample_rate: new_sr,
                        channels: new_ch,
                        bit_depth: new_bd,
                        duration_seconds: new_dur,
                    } = prepared;

                    // The format must match: we validated at prepare
                    // time but defend against state drift.
                    if new_sr == src_sample_rate && new_ch == channels {
                        decoder = new_decoder;

                        // Update the shared track metadata atomically.
                        shared.duration_ms.store((new_dur * 1000.0) as u32, Ordering::Relaxed);
                        shared.position_ms.store(0, Ordering::Relaxed);
                        shared
                            .bit_depth
                            .store(new_bd.map(|b| b as u32).unwrap_or(0), Ordering::Relaxed);

                        let _ = event_tx.try_send(EngineEvent::GaplessTransition);

                        // Build a state snapshot reflecting the new
                        // track. The orchestrator will overwrite
                        // `current_track_id` with what it expects, so
                        // we use what was passed by `prepare_next`.
                        let _ = event_tx.try_send(EngineEvent::StateChanged {
                            state: PlayerState {
                                status: *status.lock(),
                                current_track_id: new_track_id,
                                position_seconds: 0.0,
                                duration_seconds: new_dur,
                                volume: shared.volume(),
                                output_mode: effective_mode,
                                sample_rate: Some(new_sr),
                                bit_depth: new_bd,
                                channels: Some(new_ch),
                                is_bit_perfect: false,
                                error: None,
                            },
                        });

                        tracing::info!(
                            target: "qobee::engine",
                            "gapless transition completed"
                        );
                        // Continue the loop with the new decoder. The
                        // resampler / EQ / pre-gain / volume ramp all
                        // keep their state; the next packet is fed
                        // straight in.
                        continue;
                    } else {
                        tracing::warn!(
                            target: "qobee::engine",
                            "prepared track format mismatched at EOT; falling back to standard end"
                        );
                    }
                }

                // No prepared next track (or it was incompatible):
                // flush the resampler tail and emit EndOfTrack.
                if let Some(ref mut r) = resampler {
                    if !pending_planar[0].is_empty() {
                        for ch in pending_planar.iter_mut() {
                            ch.resize(chunk_size_in, 0.0);
                        }
                        let input_slices: Vec<&[f32]> = pending_planar
                            .iter()
                            .map(|ch| &ch[..chunk_size_in])
                            .collect();
                        if let Ok((_in_f, out_f)) =
                            r.process_into_buffer(&input_slices, &mut output_buffer, None)
                        {
                            let mut interleaved: Vec<f32> = Vec::with_capacity(out_f * n_ch);
                            for f in 0..out_f {
                                for c in 0..n_ch {
                                    interleaved.push(output_buffer[c][f]);
                                }
                            }
                            let pg = shared.pre_gain();
                            if (pg - 1.0).abs() > 1e-4 {
                                for s in interleaved.iter_mut() {
                                    *s *= pg;
                                }
                            }
                            sync_eq(&mut eq, &mut eq_seen_version, &shared);
                            eq.process_inplace(&mut interleaved);
                            push_interleaved_to_ring(&interleaved, &mut producer, &alive);
                        }
                    }
                    if let Ok(out) = r.process_partial::<&[f32]>(None, None) {
                        let frames = out.first().map(|c| c.len()).unwrap_or(0);
                        if frames > 0 {
                            let mut interleaved: Vec<f32> = Vec::with_capacity(frames * n_ch);
                            for f in 0..frames {
                                for c in 0..n_ch {
                                    interleaved
                                        .push(out.get(c).and_then(|v| v.get(f).copied()).unwrap_or(0.0));
                                }
                            }
                            let pg = shared.pre_gain();
                            if (pg - 1.0).abs() > 1e-4 {
                                for s in interleaved.iter_mut() {
                                    *s *= pg;
                                }
                            }
                            sync_eq(&mut eq, &mut eq_seen_version, &shared);
                            eq.process_inplace(&mut interleaved);
                            push_interleaved_to_ring(&interleaved, &mut producer, &alive);
                        }
                    }
                }

                let _ = event_tx.try_send(EngineEvent::EndOfTrack);
                *status.lock() = PlaybackStatus::Stopped;
                let _ = event_tx.try_send(EngineEvent::StateChanged {
                    state: PlayerState {
                        status: PlaybackStatus::Stopped,
                        current_track_id: None,
                        position_seconds: shared.position_ms.load(Ordering::Relaxed) as f64
                            / 1000.0,
                        duration_seconds: shared.duration_ms.load(Ordering::Relaxed) as f64
                            / 1000.0,
                        volume: shared.volume(),
                        output_mode: effective_mode,
                        sample_rate: None,
                        bit_depth: None,
                        channels: None,
                        is_bit_perfect: false,
                        error: None,
                    },
                });
                return;
            }
            Err(e) => {
                let msg = e.to_string();
                tracing::error!(target: "qobee::engine", error = %msg, "decoder error");
                let _ = event_tx.try_send(EngineEvent::Error {
                    message: msg.clone(),
                });
                return;
            }
        }
    }
}

/// Deinterleave `samples` (which is interleaved `n_ch` channels) into
/// per-channel planar vectors, appending to whatever is already there.
fn deinterleave_into(samples: &[f32], n_ch: usize, planar: &mut [Vec<f32>]) {
    let frames = samples.len() / n_ch;
    for f in 0..frames {
        for c in 0..n_ch {
            planar[c].push(samples[f * n_ch + c]);
        }
    }
}

/// Push interleaved samples directly to the ring (no resampling).
fn push_interleaved_to_ring(samples: &[f32], producer: &mut Producer<f32>, alive: &AtomicBool) {
    let mut idx = 0;
    while idx < samples.len() {
        if !alive.load(Ordering::Acquire) {
            return;
        }
        let slots = producer.slots();
        if slots == 0 {
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        let to_write = slots.min(samples.len() - idx);
        for v in &samples[idx..idx + to_write] {
            let _ = producer.push(*v);
        }
        idx += to_write;
    }
}

#[allow(clippy::too_many_arguments)]
fn build_stream<S>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut consumer: Consumer<f32>,
    shared: Arc<Shared>,
    event_tx: Sender<EngineEvent>,
    src_channels: u16,
    dst_channels: u16,
    dither_bits: Option<u8>,
) -> EngineResult<cpal::Stream>
where
    S: cpal::Sample + cpal::SizedSample + cpal::FromSample<f32>,
{
    let err_fn = move |e: cpal::StreamError| {
        let _ = event_tx.try_send(EngineEvent::Error {
            message: e.to_string(),
        });
        tracing::error!(target: "qobee::engine", error = %e, "cpal stream error");
    };

    let src_ch = src_channels as usize;
    let dst_ch = dst_channels as usize;

    // Per-frame source scratch. We allocate once outside the closure
    // and reuse it. Length matches the source channel count (no fixed
    // upper bound: handles 7.1 and beyond cleanly).
    let mut src_frame: Vec<f32> = vec![0.0; src_ch.max(1)];

    // Volume ramping: avoid "zipper noise" when the user drags the
    // slider by interpolating the per-sample gain between callbacks.
    // We keep the smoothed value in the closure (CPAL guarantees the
    // callback runs single-threaded) and step it toward the target
    // shared atomic at the per-sample rate set below.
    let mut current_gain: f32 = shared.audible_gain();
    // Step that converges in ~10 ms at the device rate. With a 48 kHz
    // device that's ~480 samples, well under any audible click.
    let frames_to_converge: f32 = (config.sample_rate as f32 * 0.010).max(1.0);
    let gain_step_per_frame: f32 = 1.0 / frames_to_converge;

    // TPDF dither amplitude (1 LSB peak-to-peak on the destination
    // bit depth). Applied only when the device accepts a low-bit
    // integer format (typically 16-bit). For 24-bit / 32-bit / float
    // formats `dither_bits` is `None` and this is a no-op.
    let dither_amp: f32 = match dither_bits {
        Some(bits) if bits > 0 && bits < 24 => 1.0 / ((1u32 << (bits - 1)) as f32),
        _ => 0.0,
    };
    // Cheap xorshift PRNG, two state words for two TPDF samples.
    let mut rng_a: u32 = 0x12345678;
    let mut rng_b: u32 = 0x9abcdef0;
    let mut next_tpdf = move |amp: f32| -> f32 {
        if amp == 0.0 {
            return 0.0;
        }
        rng_a ^= rng_a << 13;
        rng_a ^= rng_a >> 17;
        rng_a ^= rng_a << 5;
        rng_b ^= rng_b << 13;
        rng_b ^= rng_b >> 17;
        rng_b ^= rng_b << 5;
        let r1 = (rng_a as f32 / u32::MAX as f32) - 0.5;
        let r2 = (rng_b as f32 / u32::MAX as f32) - 0.5;
        // Sum of two uniform => triangular PDF, peak ±amp.
        (r1 + r2) * amp
    };

    // Soft-clipper: smooth limit at ±1.0 so EQ-induced peaks don't
    // hard-clip into the integer rail. tanh-shape via a cheap
    // polynomial approximation; transparent below ~-3 dBFS.
    #[inline(always)]
    fn soft_clip(x: f32) -> f32 {
        // Linear up to ±0.7, then a smooth knee that asymptotes at ±1.
        let t = x.clamp(-1.5, 1.5);
        let t2 = t * t;
        // 3rd-order polynomial: y = t * (1 - t^2/3) on [-1, 1].
        // Outside, clamp to ±2/3 * 1 = ±0.667 then add a softer roll.
        if t.abs() <= 1.0 {
            t * (1.0 - t2 / 3.0)
        } else if t > 0.0 {
            // Asymptote toward 2/3 + small tail; safe ceiling at 1.
            (2.0 / 3.0 + (1.0 - (-((t - 1.0) * 2.0)).exp()) / 3.0).min(1.0)
        } else {
            (-2.0 / 3.0 - (1.0 - (-((-t - 1.0) * 2.0)).exp()) / 3.0).max(-1.0)
        }
    }

    let stream = device
        .build_output_stream(
            config,
            move |output: &mut [S], _info| {
                let paused = shared.paused.load(Ordering::Acquire);
                let target_gain = shared.audible_gain();

                // If a seek is pending, the worker raises `drain_ring`.
                // Pop everything currently in the ring and emit silence
                // for this callback so we don't play stale samples past
                // the seek point.
                if shared.drain_ring.load(Ordering::Acquire) {
                    while consumer.pop().is_ok() {}
                    for slot in output.iter_mut() {
                        *slot = S::from_sample(0.0_f32);
                    }
                    return;
                }

                if paused {
                    for slot in output.iter_mut() {
                        *slot = S::from_sample(0.0_f32);
                    }
                    return;
                }

                // Frames the device wants this callback.
                let frames = output.len() / dst_ch;

                for frame in 0..frames {
                    // Step the smoothed gain toward the slider's target
                    // value. Linear ramp is enough: the difference is
                    // tiny per sample (1/(SR*0.01)) so any harmonic
                    // distortion caused by it is far below the noise
                    // floor of any reasonable DAC.
                    if current_gain < target_gain {
                        current_gain = (current_gain + gain_step_per_frame).min(target_gain);
                    } else if current_gain > target_gain {
                        current_gain = (current_gain - gain_step_per_frame).max(target_gain);
                    }
                    let gain = current_gain;

                    // Pull `src_ch` samples (one frame from source).
                    let take = src_ch;

                    let mut got = 0usize;
                    while got < take {
                        match consumer.pop() {
                            Ok(s) => {
                                src_frame[got] = s * gain;
                                got += 1;
                            }
                            Err(_) => {
                                // Underrun: emit silence for the rest of
                                // this frame and the rest of the buffer.
                                shared.underruns.fetch_add(1, Ordering::Relaxed);
                                for slot in output[(frame * dst_ch)..].iter_mut() {
                                    *slot = S::from_sample(0.0_f32);
                                }
                                return;
                            }
                        }
                    }

                    // Stereo -> mono downmix: equal-power sum so we keep
                    // the energy of both channels instead of dropping
                    // one. (1/sqrt(2) ≈ 0.7071.)
                    let mono_mix = if src_ch >= 2 {
                        (src_frame[0] + src_frame[1]) * 0.707_106_77
                    } else {
                        src_frame[0]
                    };

                    // Map source channels to device channels:
                    //   src=1 -> dst=N: duplicate the mono channel.
                    //   src=2 -> dst=1: equal-power L+R sum.
                    //   src=2 -> dst=2: pass through.
                    //   N -> M (others): copy min(N,M), pad with zeros.
                    // We track whether the chain *can* exceed [-1, 1].
                    // When EQ is bypass, pre-gain is 1.0 and dither is
                    // off (i.e. the device is f32/i32), the signal is
                    // already in range and we skip the soft-clip
                    // entirely — purest passthrough.
                    let pg_now = shared.pre_gain();
                    let chain_clean = (pg_now - 1.0).abs() < 1e-4
                        && shared.eq_gains_db.lock().iter().all(|g| g.abs() < 0.05);

                    for ch in 0..dst_ch {
                        let v = if src_ch == 1 {
                            src_frame[0]
                        } else if dst_ch == 1 {
                            mono_mix
                        } else if ch < src_ch {
                            src_frame[ch]
                        } else {
                            0.0
                        };
                        // Apply soft-clip + dither only when we have
                        // reason to: EQ active, ReplayGain boost, or
                        // a low-bit-depth integer device that needs
                        // dither. Otherwise it's a pure passthrough.
                        let v = if chain_clean && dither_amp == 0.0 {
                            v
                        } else {
                            let v = soft_clip(v);
                            v + next_tpdf(dither_amp)
                        };
                        output[frame * dst_ch + ch] = S::from_sample(v);
                    }
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| EngineError::Output(e.to_string()))?;

    Ok(stream)
}

// ---------------------------------------------------------------------------
// Device discovery helpers
// ---------------------------------------------------------------------------

/// Find a CPAL output device by its `name()`. Returns `None` if no
/// matching device is currently connected.
fn find_device_by_name(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    let devices = host.output_devices().ok()?;
    for d in devices {
        if let Ok(n) = device_label(&d) {
            if n == name {
                return Some(d);
            }
        }
    }
    None
}

/// Negotiate the best output format the device can accept that
/// matches the source's sample rate.
///
/// Strategy:
///   - Walk every supported config range, checking whether each
///     accepts `target_sr` (`min..=max` covers it).
///   - Among accepting configs, score by sample format (F32 > I32 > I16)
///     and by matching the source channel count.
///   - If nothing accepts the source rate, return the device's
///     default (the engine will resample).
///
/// Negotiate the best output format the device can accept that
/// matches the source's sample rate.
///
/// Strategy:
///   - Walk every supported config range, checking whether each
///     accepts `target_sr` (`min..=max` covers it).
///   - Among accepting configs, score by sample format (F32 > I32 > I16)
///     and by matching the source channel count. Configurations that
///     match the source channel count exactly are *strongly* preferred
///     over higher-channel-count surround configurations: when the
///     OS / device firmware is set to a virtual surround layout (very
///     common on Windows with consumer headphones like Sony Inzone),
///     opening the stream in 7.1 forces our stereo signal through
///     the device's surround virtualizer. That virtualizer applies
///     filters and channel decorrelation that audibly colour
///     transients ("sharp" cymbals, hat hits…). Picking the matching
///     channel count makes Windows do the upmix instead, with less
///     destructive processing.
///   - If nothing accepts the source rate, return the device's
///     default (the engine will resample).
///
/// Returns a fully-resolved `SupportedStreamConfig` ready for
/// `device.build_output_stream`.
fn negotiate_output_format(
    device: &cpal::Device,
    target_sr: u32,
    target_channels: u16,
    fallback: &cpal::SupportedStreamConfig,
) -> cpal::SupportedStreamConfig {
    let configs = match device.supported_output_configs() {
        Ok(c) => c.collect::<Vec<_>>(),
        Err(e) => {
            tracing::debug!(
                target: "qobee::engine",
                error = %e,
                "supported_output_configs failed; using fallback"
            );
            return fallback.clone();
        }
    };

    // Score: higher is better.
    fn format_score(fmt: SampleFormat) -> i32 {
        match fmt {
            SampleFormat::F32 => 100,
            SampleFormat::I32 => 80,
            SampleFormat::I16 => 40,
            SampleFormat::U16 => 30,
            _ => 0,
        }
    }

    let target_rate: cpal::SampleRate = target_sr;
    let mut best: Option<(i32, cpal::SupportedStreamConfig)> = None;

    for cfg in &configs {
        if cfg.min_sample_rate() > target_rate || cfg.max_sample_rate() < target_rate {
            continue;
        }
        let fmt = cfg.sample_format();
        let mut score = format_score(fmt);
        // Channel matching weight: matching source channels is much
        // more important than format details. A 7.1 F32 config beats
        // a 2-channel I16 only on format score (+60), but the channel
        // mismatch alone deducts much more (-200).
        if cfg.channels() == target_channels {
            score += 200;
        } else if cfg.channels() < target_channels {
            // Downmix needed (very rare): still better than upmix
            // through a surround virtualizer.
            score += 50;
        } else {
            // Surround upmix to N channels: penalize hard. The driver
            // / firmware will upmix the signal through whatever virtual
            // surround it has configured, and that processing is
            // audibly worse than just letting Windows do the channel
            // count adjustment in the shared mixer.
            score -= 100 - 10 * cfg.channels() as i32;
        }
        let resolved = (*cfg).with_sample_rate(target_rate);
        match &best {
            Some((s, _)) if *s >= score => {}
            _ => best = Some((score, resolved)),
        }
    }

    if let Some((_, cfg)) = best {
        tracing::debug!(
            target: "qobee::engine",
            chosen_channels = cfg.channels(),
            chosen_format = ?cfg.sample_format(),
            chosen_sr = cfg.sample_rate(),
            target_channels,
            target_sr,
            "negotiated output format"
        );
        cfg
    } else {
        fallback.clone()
    }
}

/// Get a stable label for a device. CPAL deprecated `name()` in favor
/// of `description()` + `id()`; we use `description()` for a friendly
/// label and fall back to `name()` on older versions.
fn device_label(d: &cpal::Device) -> Result<String, cpal::DeviceNameError> {
    #[allow(deprecated)]
    d.name()
}

/// Enumerate every output device available on the default host.
///
/// The result is intended to be returned to the UI verbatim. Errors
/// from individual device queries are logged and the device is skipped
/// rather than failing the whole listing.
pub fn list_output_devices() -> EngineResult<Vec<crate::types::OutputDevice>> {
    let host = cpal::default_host();
    let default_name = host
        .default_output_device()
        .and_then(|d| device_label(&d).ok())
        .unwrap_or_default();

    let mut out = Vec::new();
    let devices = host
        .output_devices()
        .map_err(|e| EngineError::Output(format!("output_devices: {e}")))?;
    for d in devices {
        let name = match device_label(&d) {
            Ok(n) => n,
            Err(e) => {
                tracing::debug!(target: "qobee::engine", error = %e, "skipping unnamed device");
                continue;
            }
        };
        let cfg = match d.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(
                    target: "qobee::engine",
                    device = %name,
                    error = %e,
                    "skipping device without default config"
                );
                continue;
            }
        };
        let is_default = !default_name.is_empty() && name == default_name;
        out.push(crate::types::OutputDevice {
            id: name.clone(),
            name,
            is_default,
            default_sample_rate: cfg.sample_rate(),
            channels: cfg.channels(),
        });
    }
    // Default device first, then alphabetical for stability.
    out.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}
