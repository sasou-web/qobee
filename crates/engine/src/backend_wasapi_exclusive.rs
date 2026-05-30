//! WASAPI Exclusive output backend (Windows-only).
//!
//! Bypasses the OS mixer entirely: when this backend is active no other
//! application can play to the same device. The DAC receives exactly
//! the samples we send. When the device natively supports the source's
//! sample rate, no resampling happens and we report
//! `is_bit_perfect = true`.
//!
//! ## Threading model
//!
//! WASAPI COM interfaces (`IAudioClient`, `IAudioRenderClient`,
//! event handles) are `!Send`. To keep the design simple we run all
//! WASAPI work on a single thread (the *worker* thread): it owns the
//! command channel, opens the device, drives the render loop, and
//! recreates everything on `Load`. A separate *decoder* thread feeds
//! it via an SPSC ring (the same `run_decoder_thread` used by the
//! Shared backend, so the DSP pipeline is shared verbatim).
//!
//! ## Format negotiation
//!
//! For each candidate sample rate we try, in order:
//!   1. S24 valid in 32-bit container (most common HD).
//!   2. Full-range S32.
//!   3. F32 (some DACs prefer it).
//!   4. S16 (CD quality fallback; we add TPDF dither in this case).
//!
//! Sample rate strategy:
//!   - First try the source's own SR (zero resampling, ideal).
//!   - Then try common HD rates â‰¥ source SR (192 â†’ 88.2 kHz), then
//!     common rates < source SR (48, 44.1 kHz). Resampling kicks in
//!     via the same sinc resampler used by the Shared backend.
//!   - As a last resort, ask the device for its `mixformat` and try
//!     that rate (with the source's channel count, then with the
//!     device's preferred channel count when the driver demands it).
//!     When the channel count is upmixed (e.g. stereo â†’ 7.1 surround
//!     because the OS is configured that way), the render path puts
//!     the source channels on the front and silences the rest.
//!     `is_bit_perfect` is reported `false` whenever any of these
//!     fallbacks fire.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use rtrb::{Consumer, RingBuffer};

use wasapi::{initialize_mta, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

use crate::backend_cpal_shared::{
    build_bit_perfect_health, run_decoder_thread, Shared, RING_CAPACITY_SAMPLES,
};
use crate::backend_symphonia::SymphoniaDecoder;
use crate::error::{EngineError, EngineResult};
use crate::format_cache::DeviceConfigFingerprint;
use crate::format_selection::{select_format, CandidateFormat};
use crate::types::{
    EffectiveOutputMode, EngineEvent, OutputMode, PlaybackStatus, PlayerState, UpmixInfo,
};
use crate::AudioEngine;
use crate::AudioSettings;
use crate::PreGainContext;

// ---------------------------------------------------------------------------
// Public engine
// ---------------------------------------------------------------------------

enum Command {
    Load(PathBuf),
    /// DSD load: opens a DSF/DFF file, negotiates 24-in-32 at the
    /// DoP carrier rate, and starts the DSD render thread (R7.3).
    LoadDsd {
        path: PathBuf,
        rate: crate::DsdRate,
    },
    Play,
    Pause,
    Resume,
    Stop,
    Seek(f64),
}

pub struct WasapiExclusiveEngine {
    cmd_tx: Sender<Command>,
    event_tx: Sender<EngineEvent>,
    event_rx: Receiver<EngineEvent>,
    shared: Arc<Shared>,
    status: Arc<Mutex<PlaybackStatus>>,
    current_track: Arc<Mutex<Option<String>>>,
    last_error: Arc<Mutex<Option<String>>>,
    /// Whether the *currently active* stream avoided any resampling.
    is_native_rate: Arc<AtomicBool>,
    selected_device: Arc<Mutex<Option<String>>>,
    _worker: JoinHandle<()>,
}

impl WasapiExclusiveEngine {
    pub fn new() -> EngineResult<Self> {
        let (cmd_tx, cmd_rx) = bounded::<Command>(16);
        let (event_tx, event_rx) = bounded::<EngineEvent>(256);

        let shared = Arc::new(Shared::new());
        let status = Arc::new(Mutex::new(PlaybackStatus::Idle));
        let current_track: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let last_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let is_native_rate = Arc::new(AtomicBool::new(false));
        let selected_device: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let worker_shared = Arc::clone(&shared);
        let worker_status = Arc::clone(&status);
        let worker_event_tx = event_tx.clone();
        let worker_last_error = Arc::clone(&last_error);
        let worker_current_track = Arc::clone(&current_track);
        let worker_native = Arc::clone(&is_native_rate);
        let worker_selected_device = Arc::clone(&selected_device);

        let worker = thread::Builder::new()
            .name("qobee-wasapi-worker".into())
            .spawn(move || {
                // COM must be initialized on the thread that creates
                // and uses the WASAPI interfaces. MTA matches the
                // wasapi-rs examples and is fine for a worker thread.
                if let Err(e) = initialize_mta().ok() {
                    tracing::error!(
                        target: "qobee::engine",
                        error = ?e,
                        "initialize_mta failed; WASAPI exclusive will not work on this thread"
                    );
                }
                run_worker(WorkerCtx {
                    cmd_rx,
                    event_tx: worker_event_tx,
                    shared: worker_shared,
                    status: worker_status,
                    current_track: worker_current_track,
                    last_error: worker_last_error,
                    is_native_rate: worker_native,
                    selected_device: worker_selected_device,
                });
            })
            .map_err(|e| EngineError::Internal(format!("failed to spawn WASAPI worker: {e}")))?;

        Ok(WasapiExclusiveEngine {
            cmd_tx,
            event_tx,
            event_rx,
            shared,
            status,
            current_track,
            last_error,
            is_native_rate,
            selected_device,
            _worker: worker,
        })
    }

    fn send_cmd(&self, cmd: Command) -> EngineResult<()> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| EngineError::Internal("WASAPI worker thread is gone".into()))
    }

    pub fn set_current_track_id(&self, id: Option<String>) {
        *self.current_track.lock() = id;
    }

    pub fn set_output_device(&self, device_id: Option<String>) {
        *self.selected_device.lock() = device_id;
    }

    pub fn selected_device(&self) -> Option<String> {
        self.selected_device.lock().clone()
    }

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

    pub fn set_pre_gain(&self, linear: f32) {
        let clamped = linear.clamp(0.0, 8.0);
        let micro = (clamped * 1_000_000.0) as u32;
        self.shared.set_pre_gain_micro(micro);
    }
}

impl AudioEngine for WasapiExclusiveEngine {
    fn load(&self, path: &Path) -> EngineResult<()> {
        self.send_cmd(Command::Load(path.to_path_buf()))
    }
    fn load_dsd(&self, path: &Path, rate: crate::DsdRate) -> EngineResult<()> {
        self.send_cmd(Command::LoadDsd {
            path: path.to_path_buf(),
            rate,
        })
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
        self.shared.set_volume_public(volume);
        let _ = self.event_tx.try_send(EngineEvent::StateChanged {
            state: self.state(),
        });
        Ok(())
    }

    fn set_output_mode(&self, _mode: OutputMode) -> EngineResult<()> {
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

        let unity_volume = (self.shared.audible_gain() - 1.0).abs() < 1e-4;
        let unity_pregain = (self.shared.pre_gain() - 1.0).abs() < 1e-4;
        let eq_bypass = self
            .shared
            .eq_gains_db
            .lock()
            .iter()
            .all(|g| g.abs() < 0.05);
        let is_bit_perfect = self.is_native_rate.load(Ordering::Relaxed)
            && unity_volume
            && unity_pregain
            && eq_bypass;

        let mut state = PlayerState {
            status,
            current_track_id,
            position_seconds: self.shared.position_ms.load(Ordering::Relaxed) as f64 / 1000.0,
            duration_seconds: self.shared.duration_ms.load(Ordering::Relaxed) as f64 / 1000.0,
            volume: self.shared.volume_public(),
            output_mode: EffectiveOutputMode::Exclusive,
            sample_rate,
            bit_depth,
            channels,
            is_bit_perfect,
            rg_attenuation_db: self.shared.rg_attenuation_db(),
            bit_perfect: None,
            dsd_rate_label: self.shared.dsd_rate_label(),
            is_dsd: self.shared.is_dsd_active(),
            // Active upmix layout (R9.5): set by negotiation when a
            // stereo source had to be widened onto a surround-only
            // device layout; `None` when played natively.
            upmix: self.shared.upmix(),
            // Degraded flag (R11.3) set by the decoder thread at EOF.
            degraded: self.shared.file_degraded(),
            error,
        };
        state.bit_perfect = build_bit_perfect_health(&self.shared, &state);
        state
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

    fn set_audio_settings(&self, settings: AudioSettings) {
        self.shared.set_audio_settings(settings);
        let _ = self.event_tx.try_send(EngineEvent::StateChanged {
            state: self.state(),
        });
    }

    fn set_pre_gain_context(&self, ctx: PreGainContext) {
        self.shared.set_pre_gain_context(ctx);
    }

    fn set_convolver_ir(&self, ir_left: Vec<f32>, ir_right: Vec<f32>) {
        self.shared.set_convolver_ir(ir_left, ir_right);
    }

    fn convolver_ir_len(&self) -> usize {
        self.shared.convolver_ir_len()
    }

    fn is_dsd_active(&self) -> bool {
        self.shared.is_dsd_active()
    }

    fn prepare_next(&self, _path: &Path, _track_id: Option<String>) -> EngineResult<()> {
        // WASAPI Exclusive backend does not support in-place gapless
        // transitions yet (each track opens a fresh device session).
        // The orchestrator will use the standard EOT â†’ Load path,
        // which produces the same small gap as before.
        Ok(())
    }

    fn clear_pending_next(&self) -> EngineResult<()> {
        Ok(())
    }

    fn underrun_count(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed) as u64
    }
}

// ---------------------------------------------------------------------------
// Worker (single-threaded WASAPI driver)
// ---------------------------------------------------------------------------

struct WorkerCtx {
    cmd_rx: Receiver<Command>,
    event_tx: Sender<EngineEvent>,
    shared: Arc<Shared>,
    status: Arc<Mutex<PlaybackStatus>>,
    current_track: Arc<Mutex<Option<String>>>,
    last_error: Arc<Mutex<Option<String>>>,
    is_native_rate: Arc<AtomicBool>,
    selected_device: Arc<Mutex<Option<String>>>,
}

/// What is currently loaded: a decoder thread plus the WASAPI objects
/// that own the device. Living entirely on the worker thread.
struct ActiveTrack {
    audio_client: wasapi::AudioClient,
    render_client: wasapi::AudioRenderClient,
    h_event: wasapi::Handle,
    nego: NegotiatedFormat,
    consumer: Consumer<f32>,
    decoder_alive: Arc<AtomicBool>,
    decoder_handle: Option<JoinHandle<()>>,
    started: bool,
    /// Volume ramp state across render iterations.
    current_gain: f32,
    /// Dither state.
    rng_a: u32,
    rng_b: u32,
}

/// DSD variant of `ActiveTrack`: the WASAPI device is opened in
/// 24-in-32 at the DoP carrier rate and the consumer pulls
/// pre-packed `i32` samples with the DoP marker already in place.
/// No PCM DSP runs.
struct ActiveDsd {
    audio_client: wasapi::AudioClient,
    render_client: wasapi::AudioRenderClient,
    h_event: wasapi::Handle,
    sample_rate: u32,
    channels: u16,
    consumer: Consumer<i32>,
    decoder_alive: Arc<AtomicBool>,
    decoder_handle: Option<JoinHandle<()>>,
    started: bool,
}

impl ActiveDsd {
    fn shutdown(mut self) {
        if self.started {
            let _ = self.audio_client.stop_stream();
        }
        self.decoder_alive.store(false, Ordering::Release);
        if let Some(h) = self.decoder_handle.take() {
            let _ = h.join();
        }
    }
}

/// Either a PCM track (full DSP chain) or a DSD track (no DSP, DoP
/// packed). Both variants share Stop / Pause / Resume semantics.
enum ActiveStream {
    Pcm(ActiveTrack),
    Dsd(ActiveDsd),
}

impl ActiveStream {
    fn is_started(&self) -> bool {
        match self {
            ActiveStream::Pcm(t) => t.started,
            ActiveStream::Dsd(t) => t.started,
        }
    }
    fn audio_client_mut(&mut self) -> &mut wasapi::AudioClient {
        match self {
            ActiveStream::Pcm(t) => &mut t.audio_client,
            ActiveStream::Dsd(t) => &mut t.audio_client,
        }
    }
    fn started_set(&mut self, v: bool) {
        match self {
            ActiveStream::Pcm(t) => t.started = v,
            ActiveStream::Dsd(t) => t.started = v,
        }
    }
    fn shutdown(self) {
        match self {
            ActiveStream::Pcm(t) => t.shutdown(),
            ActiveStream::Dsd(t) => t.shutdown(),
        }
    }
}

impl ActiveTrack {
    fn shutdown(mut self) {
        if self.started {
            let _ = self.audio_client.stop_stream();
        }
        self.decoder_alive.store(false, Ordering::Release);
        if let Some(h) = self.decoder_handle.take() {
            let _ = h.join();
        }
    }
}

fn run_worker(ctx: WorkerCtx) {
    let mut active: Option<ActiveStream> = None;
    // Per-device format cache (R9.1–R9.3). Owned by the worker loop so
    // it persists across `Load`s (per-device reuse) yet stays entirely
    // on the COM-owning thread (never shared, never serialized).
    let mut format_cache: DeviceFormatCache = DeviceFormatCache::new();

    loop {
        // If something is loaded and playing, drain the audio device
        // until we hit a "no available frames" / event-wait state, then
        // poll the command channel non-blocking. If nothing is loaded
        // we block on the command channel.
        let render_active = active.as_ref().is_some_and(|a| a.is_started());

        if render_active {
            // Render one period if there is space.
            if let Some(stream) = active.as_mut() {
                let result = match stream {
                    ActiveStream::Pcm(track) => render_one_period(track, &ctx),
                    ActiveStream::Dsd(track) => render_one_period_dsd(track, &ctx),
                };
                if let Err(e) = result {
                    let msg = e.to_string();
                    tracing::error!(
                        target: "qobee::engine",
                        error = %msg,
                        "WASAPI render error"
                    );
                    *ctx.last_error.lock() = Some(msg.clone());
                    let _ = ctx.event_tx.try_send(EngineEvent::Error { message: msg });
                    if let Some(prev) = active.take() {
                        prev.shutdown();
                    }
                    set_status(&ctx, PlaybackStatus::Errored);
                }
            }

            // Drain commands without blocking.
            while let Ok(cmd) = ctx.cmd_rx.try_recv() {
                handle_cmd(cmd, &ctx, &mut active, &mut format_cache);
            }
        } else {
            // Nothing is rendering: block on the next command.
            match ctx.cmd_rx.recv() {
                Ok(cmd) => handle_cmd(cmd, &ctx, &mut active, &mut format_cache),
                Err(_) => break, // sender dropped
            }
        }
    }

    if let Some(prev) = active.take() {
        prev.shutdown();
    }
}

/// Events emitted by the `Load` worker arm when Exclusive format
/// negotiation fails outright (R9.6). Returned in emission order: an
/// [`EngineEvent::ExclusiveFallback`] carrying the failure `reason` (so
/// the UI can explain the switch to Shared) followed by the
/// [`EngineEvent::Error`] that drives the actual backend swap + track
/// restart in `qobee-core` (behaviour unchanged from before this was
/// factored out). Pulled into its own function so the total-failure →
/// fallback mapping can be unit-tested without a live
/// `wasapi::AudioClient`.
fn exclusive_load_failure_events(reason: String) -> [EngineEvent; 2] {
    [
        EngineEvent::ExclusiveFallback {
            reason: reason.clone(),
        },
        EngineEvent::Error { message: reason },
    ]
}

fn handle_cmd(
    cmd: Command,
    ctx: &WorkerCtx,
    active: &mut Option<ActiveStream>,
    format_cache: &mut DeviceFormatCache,
) {
    match cmd {
        Command::Load(path) => {
            if let Some(prev) = active.take() {
                prev.shutdown();
            }
            ctx.shared.pending_seek_ms.store(-1, Ordering::Release);
            ctx.shared.drain_ring.store(false, Ordering::Release);
            // Leaving DSD: unset the active flag so volume / EQ are
            // re-allowed for the next PCM track.
            ctx.shared.set_dsd_active(false, None);
            // A fresh Load clears any degraded flag from the previous
            // file (R11.3).
            ctx.shared.set_file_degraded(false);
            set_status(ctx, PlaybackStatus::Loading);

            match start_playback(ctx, &path, format_cache) {
                Ok(s) => {
                    *active = Some(ActiveStream::Pcm(s));
                    set_status(ctx, PlaybackStatus::Paused);
                }
                Err(e) => {
                    let msg = e.to_string();
                    *ctx.last_error.lock() = Some(msg.clone());
                    set_status(ctx, PlaybackStatus::Errored);
                    // Exclusive negotiation failed for this device: tell
                    // the orchestrator we are about to fall back to the
                    // Shared backend (R9.6) so it can surface an FR
                    // toast explaining the switch and its reason. The
                    // `Error` event below still drives the actual
                    // backend swap + track restart in `qobee-core`
                    // (behaviour unchanged).
                    for ev in exclusive_load_failure_events(msg) {
                        let _ = ctx.event_tx.try_send(ev);
                    }
                }
            }
        }
        Command::LoadDsd { path, rate } => {
            if let Some(prev) = active.take() {
                prev.shutdown();
            }
            ctx.shared.pending_seek_ms.store(-1, Ordering::Release);
            ctx.shared.drain_ring.store(false, Ordering::Release);
            ctx.shared.set_dsd_active(true, Some(rate.label()));
            // A fresh Load clears any degraded flag from the previous
            // file (R11.3).
            ctx.shared.set_file_degraded(false);
            set_status(ctx, PlaybackStatus::Loading);

            match start_dsd_playback(ctx, &path, rate) {
                Ok(s) => {
                    *active = Some(ActiveStream::Dsd(s));
                    set_status(ctx, PlaybackStatus::Paused);
                }
                Err(e) => {
                    let msg = e.to_string();
                    *ctx.last_error.lock() = Some(msg.clone());
                    ctx.shared.set_dsd_active(false, None);
                    set_status(ctx, PlaybackStatus::Errored);
                    // Surface the DoP-specific event for the UI panel
                    // (R7.5) and an Error toast carrying the FR
                    // localised message so the user sees what went
                    // wrong.
                    let _ = ctx.event_tx.try_send(EngineEvent::DopUnsupported);
                    let _ = ctx.event_tx.try_send(EngineEvent::Error {
                        message: format!("DoP non supporté par le périphérique: {msg}"),
                    });
                }
            }
        }
        Command::Play | Command::Resume => {
            if let Some(stream) = active.as_mut() {
                ctx.shared.paused.store(false, Ordering::Release);
                if !stream.is_started() {
                    let r = stream.audio_client_mut().start_stream();
                    if let Err(e) = r {
                        let msg = format!("start_stream: {e:?}");
                        *ctx.last_error.lock() = Some(msg.clone());
                        set_status(ctx, PlaybackStatus::Errored);
                        let _ = ctx.event_tx.try_send(EngineEvent::Error { message: msg });
                        return;
                    }
                    stream.started_set(true);
                }
                set_status(ctx, PlaybackStatus::Playing);
            }
        }
        Command::Pause => {
            if active.is_some() {
                // Don't stop_stream: keeping the stream running with
                // silence in the buffer means resume is instant. We
                // just flip the `paused` flag and the render path
                // emits zeros.
                ctx.shared.paused.store(true, Ordering::Release);
                set_status(ctx, PlaybackStatus::Paused);
            }
        }
        Command::Stop => {
            if let Some(prev) = active.take() {
                prev.shutdown();
            }
            ctx.shared.paused.store(true, Ordering::Release);
            ctx.shared.position_ms.store(0, Ordering::Relaxed);
            ctx.shared.duration_ms.store(0, Ordering::Relaxed);
            ctx.shared.sample_rate.store(0, Ordering::Relaxed);
            ctx.shared.channels.store(0, Ordering::Relaxed);
            ctx.shared.bit_depth.store(0, Ordering::Relaxed);
            ctx.shared.set_device_format(0, 0);
            ctx.shared.reset_bit_perfect_debounce();
            ctx.shared.pending_seek_ms.store(-1, Ordering::Release);
            ctx.shared.drain_ring.store(false, Ordering::Release);
            ctx.shared.set_dsd_active(false, None);
            ctx.shared.set_upmix(None);
            ctx.is_native_rate.store(false, Ordering::Release);
            set_status(ctx, PlaybackStatus::Stopped);
        }
        Command::Seek(secs) => {
            if active.is_some() {
                let secs = secs.max(0.0);
                let ms = (secs * 1000.0) as i32;
                ctx.shared.drain_ring.store(true, Ordering::Release);
                ctx.shared.pending_seek_ms.store(ms, Ordering::Release);
                ctx.shared
                    .position_ms
                    .store(ms.max(0) as u32, Ordering::Relaxed);
                let _ = ctx.event_tx.try_send(EngineEvent::Position {
                    position_seconds: secs,
                });
            }
        }
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

    let unity_volume = (ctx.shared.audible_gain() - 1.0).abs() < 1e-4;
    let unity_pregain = (ctx.shared.pre_gain() - 1.0).abs() < 1e-4;
    let eq_bypass = ctx.shared.eq_gains_db.lock().iter().all(|g| g.abs() < 0.05);
    let is_bit_perfect =
        ctx.is_native_rate.load(Ordering::Relaxed) && unity_volume && unity_pregain && eq_bypass;

    let mut state = PlayerState {
        status,
        current_track_id,
        position_seconds: ctx.shared.position_ms.load(Ordering::Relaxed) as f64 / 1000.0,
        duration_seconds: ctx.shared.duration_ms.load(Ordering::Relaxed) as f64 / 1000.0,
        volume: ctx.shared.volume_public(),
        output_mode: EffectiveOutputMode::Exclusive,
        sample_rate,
        bit_depth,
        channels,
        is_bit_perfect,
        rg_attenuation_db: ctx.shared.rg_attenuation_db(),
        bit_perfect: None,
        dsd_rate_label: ctx.shared.dsd_rate_label(),
        is_dsd: ctx.shared.is_dsd_active(),
        // Active upmix layout (R9.5): set by negotiation when a stereo
        // source had to be widened onto a surround-only device layout;
        // `None` when played natively.
        upmix: ctx.shared.upmix(),
        // Degraded flag (R11.3) read from the shared atomic.
        degraded: ctx.shared.file_degraded(),
        error,
    };
    state.bit_perfect = build_bit_perfect_health(&ctx.shared, &state);
    state
}

// ---------------------------------------------------------------------------
// Per-track setup
// ---------------------------------------------------------------------------

/// Per-device format cache specialised to this backend's
/// [`NegotiatedFormat`] payload.
///
/// The cache logic lives in the cross-platform [`crate::format_cache`]
/// module (so the property test in
/// `tests/properties/device_format_cache.rs` can reach it on any
/// platform); here we alias it under the names the design uses
/// (`Device_Format_Cache`, `CachedFormat`, `DeviceConfigFingerprint`)
/// bound to the concrete `NegotiatedFormat` the worker thread applies.
///
/// `start_playback` resolves the device id, computes a
/// [`DeviceConfigFingerprint`] from the current mixformat, and reuses a
/// cached [`NegotiatedFormat`] on a fingerprint hit (R9.2) instead of
/// re-probing. Kept module-private (the worker thread is the only
/// consumer) to avoid leaking the private `NegotiatedFormat`.
///
/// [`crate::format_cache::CachedFormat`] holding this backend's
/// [`NegotiatedFormat`].
type CachedFormat = crate::format_cache::CachedFormat<NegotiatedFormat>;

/// [`crate::format_cache::DeviceFormatCache`] of [`NegotiatedFormat`]
/// entries, keyed by stable device id (R9.1–R9.3).
type DeviceFormatCache = crate::format_cache::DeviceFormatCache<NegotiatedFormat>;

#[derive(Clone)]
struct NegotiatedFormat {
    wave_format: WaveFormat,
    sample_rate: u32,
    /// Channel count actually negotiated with the driver (what we
    /// must write to WASAPI). May differ from `src_channels` when the
    /// device is configured for surround and the file is stereo: in
    /// that case the render path duplicates / pads channels.
    channels: u16,
    /// Source's original channel count (what the decoder produces
    /// per frame). Used by the render path to know how many samples
    /// to pop per frame and how to upmix.
    src_channels: u16,
    bytes_per_sample: usize,
    sample_type: SampleType,
    valid_bits: usize,
    /// Active upmix layout (R9.5). `Some(UpmixInfo { src, dst })` when
    /// no native config matching `src_channels` was available and the
    /// engine had to upmix onto a wider device layout (`channels >
    /// src_channels`); `None` otherwise. Set by `negotiate_with_fallback`
    /// via the pure [`select_format`] and propagated to
    /// `PlayerState::upmix` (through `Shared::upmix`) so the UI can
    /// surface an upmix indicator.
    upmix: Option<UpmixInfo>,
}

/// Resolve the device, opening a friendly-name match or the system
/// default. Runs on the worker thread that owns COM.
fn find_device(name: Option<&str>) -> EngineResult<wasapi::Device> {
    let enumerator = DeviceEnumerator::new()
        .map_err(|e| EngineError::Output(format!("device enumerator: {e:?}")))?;

    if let Some(want) = name {
        let collection = enumerator
            .get_device_collection(&Direction::Render)
            .map_err(|e| EngineError::Output(format!("device collection: {e:?}")))?;
        let count = collection
            .get_nbr_devices()
            .map_err(|e| EngineError::Output(format!("count: {e:?}")))?;
        for i in 0..count {
            if let Ok(d) = collection.get_device_at_index(i) {
                if let Ok(n) = d.get_friendlyname() {
                    if n == want {
                        return Ok(d);
                    }
                }
            }
        }
        tracing::warn!(
            target: "qobee::engine",
            requested = %want,
            "WASAPI: requested device not found; falling back to default"
        );
    }

    enumerator
        .get_default_device(&Direction::Render)
        .map_err(|e| EngineError::Output(format!("default device: {e:?}")))
}

fn negotiate_exclusive_format(
    audio_client: &wasapi::AudioClient,
    sr: u32,
    channels: u16,
    src_channels: u16,
) -> EngineResult<NegotiatedFormat> {
    let candidates: &[(usize, usize, SampleType)] = &[
        (32, 24, SampleType::Int),
        (32, 32, SampleType::Int),
        (32, 32, SampleType::Float),
        (16, 16, SampleType::Int),
    ];

    for (storebits, validbits, st) in candidates {
        let wf = WaveFormat::new(
            *storebits,
            *validbits,
            st,
            sr as usize,
            channels as usize,
            None,
        );
        if let Ok(resolved) = audio_client.is_supported_exclusive_with_quirks(&wf) {
            tracing::info!(
                target: "qobee::engine",
                sample_rate = sr,
                channels = channels,
                src_channels = src_channels,
                storebits = *storebits,
                validbits = *validbits,
                sample_type = ?st,
                "WASAPI Exclusive format negotiated"
            );
            return Ok(NegotiatedFormat {
                wave_format: resolved,
                sample_rate: sr,
                channels,
                src_channels,
                bytes_per_sample: storebits / 8,
                sample_type: *st,
                valid_bits: *validbits,
                // Upmix detection / flagging is wired in task 13.1 via
                // the pure `select_format`; this low-level negotiator
                // does not set it.
                upmix: None,
            });
        }
    }

    Err(EngineError::Output(format!(
        "no compatible WASAPI Exclusive format for {sr} Hz / {channels} ch"
    )))
}

/// Find a WASAPI Exclusive format the driver accepts, given the
/// source's preferred sample rate and channel count.
///
/// Strategy:
///   1. Try the source SR (best case: zero resampling).
///   2. Probe a list of common rates that most DACs accept in
///      Exclusive: 192/176.4/96/88.2/48/44.1 kHz. We pick the first
///      that is >= source SR (to avoid downsampling when possible),
///      then fall back to anything that works.
///   3. As a last resort, ask the device for its `mixformat` and try
///      that rate (the driver always accepts its own preferred rate
///      in some integer/float format).
///
/// On failure, returns a friendly error so the orchestrator can show
/// it and revert to Shared.
fn negotiate_with_fallback(
    audio_client: &wasapi::AudioClient,
    device_id: &str,
    src_sr: u32,
    src_channels: u16,
) -> EngineResult<(NegotiatedFormat, bool)> {
    // Accumulate every `(sample_rate, channels)` probe attempted across
    // the whole fallback ladder so we can emit ONE aggregated summary
    // (R9.7) instead of one `warn!` line per probed frequency.
    let mut tried: Vec<(u32, u16)> = Vec::with_capacity(8);

    // Try source rate first. A first-try success is the bit-perfect
    // happy path: no fallback happened, so we return immediately
    // without a summary `warn!` (`negotiate_exclusive_format` already
    // logs the negotiated format at info level).
    if let Ok(n) = negotiate_exclusive_format(audio_client, src_sr, src_channels, src_channels) {
        return Ok((n, true));
    }
    tried.push((src_sr, src_channels));

    // Common alternatives, ordered to pick the closest "up" first
    // (less destructive than downsampling).
    let common: [u32; 6] = [192_000, 176_400, 96_000, 88_200, 48_000, 44_100];
    for &sr in common.iter().filter(|&&sr| sr >= src_sr) {
        if sr == src_sr {
            continue;
        }
        tried.push((sr, src_channels));
        if let Ok(n) = negotiate_exclusive_format(audio_client, sr, src_channels, src_channels) {
            log_negotiation_summary(
                device_id,
                &tried,
                &format!(
                    "source rate refused; using a higher common rate ({sr} Hz, sinc-resampled)"
                ),
            );
            return Ok((n, false));
        }
    }
    for &sr in common.iter().filter(|&&sr| sr < src_sr) {
        tried.push((sr, src_channels));
        if let Ok(n) = negotiate_exclusive_format(audio_client, sr, src_channels, src_channels) {
            log_negotiation_summary(
                device_id,
                &tried,
                &format!(
                    "source rate refused; using a lower common rate ({sr} Hz, sinc-resampled)"
                ),
            );
            return Ok((n, false));
        }
    }

    // Last resort: ask the device for the format it actually wants
    // (its mixformat) and pick a layout with the pure `select_format`
    // (R9.4): prefer a NATIVE STEREO configuration at the device's
    // rate over any surround upmix, and only flag an upmix (R9.5) when
    // the chosen layout is genuinely wider than the source.
    if let Ok(mixfmt) = audio_client.get_mixformat() {
        let mix_sr = mixfmt.get_samplespersec();
        let mix_ch = mixfmt.get_nchannels();

        // Probe the channel layouts the device accepts at its
        // mixformat rate. We consider the source's own channel count
        // (no up/down-mix), a native stereo layout (R9.4), and the
        // device's preferred layout (`mix_ch`). Keep the full
        // negotiated format for each accepted layout so the one
        // `select_format` picks can be applied directly.
        let mut layouts: Vec<u16> = vec![src_channels.max(1), 2, mix_ch.max(1)];
        layouts.sort_unstable();
        layouts.dedup();

        let mut probed: Vec<(CandidateFormat, NegotiatedFormat)> = Vec::new();
        for &ch in &layouts {
            tried.push((mix_sr, ch));
            if let Ok(n) = negotiate_exclusive_format(audio_client, mix_sr, ch, src_channels) {
                probed.push((CandidateFormat::new(mix_sr, ch), n));
            }
        }

        let candidates: Vec<CandidateFormat> = probed.iter().map(|(c, _)| *c).collect();
        if let Some(sel) = select_format(&candidates, src_channels) {
            if let Some((_, mut n)) = probed.into_iter().find(|(c, _)| *c == sel.chosen) {
                // R9.5: upmix is `Some` iff the chosen layout is wider
                // than the source; the pure selector already enforces
                // this invariant.
                n.upmix = sel.upmix;
                let native_rate = mix_sr == src_sr;
                let outcome = match sel.upmix {
                    Some(u) => format!(
                        "no native layout available; upmixing {}→{} ch onto device layout at {mix_sr} Hz",
                        u.src_channels, u.dst_channels
                    ),
                    None => format!(
                        "using device mixformat layout ({} ch at {mix_sr} Hz, native, no upmix)",
                        sel.chosen.channels
                    ),
                };
                log_negotiation_summary(device_id, &tried, &outcome);
                return Ok((n, native_rate));
            }
        }
    }

    log_negotiation_summary(
        device_id,
        &tried,
        "no compatible format found; reverting to Shared",
    );
    Err(EngineError::Output(format!(
        "no compatible WASAPI Exclusive format found for this device (tried {} Hz and {} other rates). \
         The device may be busy with another app, or its driver does not support Exclusive mode for any of: \
         24-in-32 / 32 int / 32 float / 16 int. Try a different output device.",
        src_sr,
        tried.len().saturating_sub(1)
    )))
}

/// Emit the single aggregated WASAPI Exclusive negotiation summary
/// (R9.7). Replaces the previous one-`warn!`-per-probed-frequency
/// noise with a single line carrying the device id, every
/// `(sample_rate, channels)` pair attempted, and the final outcome
/// (the format we settled on, or the total failure that triggers the
/// Shared fallback).
fn log_negotiation_summary(device_id: &str, tried: &[(u32, u16)], outcome: &str) {
    tracing::warn!(
        target: "qobee::engine",
        device_id,
        tried = ?tried,
        outcome,
        "WASAPI Exclusive: format negotiation summary"
    );
}

/// Build a stable device identifier for the format cache key (R9.1):
/// the friendly name combined with the endpoint id. Either query may
/// fail on exotic drivers; we fall back to whatever is available so
/// caching still works (an empty id simply shares one cache slot).
fn device_identity(device: &wasapi::Device) -> String {
    let name = device.get_friendlyname().unwrap_or_default();
    let id = device.get_id().unwrap_or_default();
    format!("{name}::{id}")
}

/// Compute the device-config fingerprint from a client's current
/// mixformat (R9.2 reuse guard / R9.3 staleness guard). Returns `None`
/// when the mixformat can't be read, in which case the caller skips
/// caching and always renegotiates.
fn device_fingerprint(audio_client: &wasapi::AudioClient) -> Option<DeviceConfigFingerprint> {
    audio_client
        .get_mixformat()
        .ok()
        .map(|mix| DeviceConfigFingerprint {
            mix_sample_rate: mix.get_samplespersec(),
            mix_channels: mix.get_nchannels(),
        })
}

/// Open and initialize a WASAPI Exclusive stream for `nego`, handling
/// the `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` retry by recreating the
/// client at the next-highest aligned size (as the wasapi-rs example
/// does). Returns the initialized client plus its event handle and
/// render client, ready to render.
fn initialize_exclusive_stream(
    device: &wasapi::Device,
    nego: &NegotiatedFormat,
) -> EngineResult<(
    wasapi::AudioClient,
    wasapi::Handle,
    wasapi::AudioRenderClient,
)> {
    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|e| EngineError::Output(format!("get_iaudioclient: {e:?}")))?;

    // Aim for ~ 1.5x the device's minimum period (handles Symphonia's
    // FLAC packet cadence comfortably).
    let (_def_period, min_period) = audio_client
        .get_device_period()
        .map_err(|e| EngineError::Output(format!("get_device_period: {e:?}")))?;
    let target_period = (min_period * 3 / 2).max(min_period);
    let period_hns = audio_client
        .calculate_aligned_period_near(target_period, Some(128), &nego.wave_format)
        .map_err(|e| EngineError::Output(format!("calculate_aligned_period: {e:?}")))?;

    let mode = StreamMode::EventsExclusive { period_hns };

    if let Err(e) = audio_client.initialize_client(&nego.wave_format, &Direction::Render, &mode) {
        if let wasapi::WasapiError::Windows(werr) = &e {
            use windows::Win32::Media::Audio::AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED;
            if werr.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
                tracing::warn!(
                    target: "qobee::engine",
                    "WASAPI Exclusive: unaligned period, retrying with aligned size"
                );
                let buffersize = audio_client
                    .get_buffer_size()
                    .map_err(|e| EngineError::Output(format!("get_buffer_size: {e:?}")))?;
                let aligned_period = wasapi::calculate_period_100ns(
                    buffersize as i64,
                    nego.wave_format.get_samplespersec() as i64,
                );
                // Get a fresh client (the failed one is poisoned).
                audio_client = device
                    .get_iaudioclient()
                    .map_err(|e| EngineError::Output(format!("get_iaudioclient (retry): {e:?}")))?;
                let mode_retry = StreamMode::EventsExclusive {
                    period_hns: aligned_period,
                };
                audio_client
                    .initialize_client(&nego.wave_format, &Direction::Render, &mode_retry)
                    .map_err(|e| {
                        EngineError::Output(format!("initialize_client (retry): {e:?}"))
                    })?;
            } else {
                return Err(EngineError::Output(format!(
                    "initialize_client: HRESULT {:#010x}",
                    werr.code().0
                )));
            }
        } else {
            return Err(EngineError::Output(format!("initialize_client: {e:?}")));
        }
    }

    let h_event = audio_client
        .set_get_eventhandle()
        .map_err(|e| EngineError::Output(format!("set_get_eventhandle: {e:?}")))?;
    let render_client = audio_client
        .get_audiorenderclient()
        .map_err(|e| EngineError::Output(format!("get_audiorenderclient: {e:?}")))?;

    Ok((audio_client, h_event, render_client))
}

/// Resolve the negotiated format for `device` and open the exclusive
/// stream, honouring the per-device [`DeviceFormatCache`]:
///   * On a cache **hit** (same device id, unchanged config
///     fingerprint), reuse the stored [`NegotiatedFormat`] without
///     re-probing any candidate rate (R9.2).
///   * On a cache **miss**, run a full [`negotiate_with_fallback`] and
///     store the result (R9.1).
///   * If applying a *cached* format fails (`initialize` errors — the
///     device config changed under us), invalidate the entry and fall
///     through to a full renegotiation (R9.3).
fn resolve_and_open(
    device: &wasapi::Device,
    device_id: &str,
    fingerprint: Option<&DeviceConfigFingerprint>,
    src_sr: u32,
    src_channels: u16,
    format_cache: &mut DeviceFormatCache,
) -> EngineResult<(
    NegotiatedFormat,
    bool,
    wasapi::AudioClient,
    wasapi::Handle,
    wasapi::AudioRenderClient,
)> {
    // 1. Cache hit (R9.2): reuse the negotiated format with no re-probe.
    if let Some(fp) = fingerprint {
        let cached = format_cache
            .get(device_id, fp)
            .map(|c| (c.nego.clone(), c.is_native_rate));
        if let Some((nego, native_rate)) = cached {
            match initialize_exclusive_stream(device, &nego) {
                Ok((ac, ev, rc)) => {
                    tracing::debug!(
                        target: "qobee::engine",
                        device_id,
                        "WASAPI Exclusive: reused cached format (no re-probe)"
                    );
                    return Ok((nego, native_rate, ac, ev, rc));
                }
                Err(e) => {
                    // R9.3: the cached format no longer applies (device
                    // config changed) → drop the stale entry and
                    // renegotiate from scratch below.
                    tracing::warn!(
                        target: "qobee::engine",
                        device_id,
                        error = %e,
                        "WASAPI Exclusive: cached format failed to apply; invalidating and renegotiating"
                    );
                    format_cache.invalidate(device_id);
                }
            }
        }
    }

    // 2. Cache miss (or just-invalidated): full negotiation (R9.1).
    let probe = device
        .get_iaudioclient()
        .map_err(|e| EngineError::Output(format!("get_iaudioclient: {e:?}")))?;
    let (nego, native_rate) = negotiate_with_fallback(&probe, device_id, src_sr, src_channels)?;
    let (ac, ev, rc) = initialize_exclusive_stream(device, &nego)?;

    // Record the freshly negotiated format keyed by device id (R9.1),
    // tagged with the current config fingerprint so a later config
    // change is reported as a miss (R9.3). Skipped when the mixformat
    // was unreadable (no fingerprint to guard against staleness).
    if let Some(fp) = fingerprint {
        format_cache.put(
            device_id.to_string(),
            CachedFormat::new(nego.clone(), native_rate, *fp),
        );
    }

    Ok((nego, native_rate, ac, ev, rc))
}

fn start_playback(
    ctx: &WorkerCtx,
    path: &Path,
    format_cache: &mut DeviceFormatCache,
) -> EngineResult<ActiveTrack> {
    let decoder = SymphoniaDecoder::open_uri(path.to_string_lossy().as_ref())?;
    let format = decoder.format();

    let device_name = ctx.selected_device.lock().clone();
    let device = find_device(device_name.as_deref())?;

    // Stable device id (friendly name + endpoint id) and current-config
    // fingerprint for the per-device format cache (R9.1–R9.3).
    let device_id = device_identity(&device);
    let fingerprint = {
        let probe = device
            .get_iaudioclient()
            .map_err(|e| EngineError::Output(format!("get_iaudioclient: {e:?}")))?;
        device_fingerprint(&probe)
    };

    // Cache-aware format resolution + stream open. Reuses a cached
    // format when the config is unchanged (R9.2), negotiates + caches
    // otherwise (R9.1), and self-heals a stale cache entry (R9.3).
    let (nego, native_rate, audio_client, h_event, render_client) = resolve_and_open(
        &device,
        &device_id,
        fingerprint.as_ref(),
        format.sample_rate,
        format.channels,
        format_cache,
    )?;

    ctx.shared
        .sample_rate
        .store(nego.sample_rate, Ordering::Relaxed);
    // Report the *source* channel count to the UI: what the user
    // expects to see is "stereo file", not the device's surround
    // configuration.
    ctx.shared
        .channels
        .store(nego.src_channels as u32, Ordering::Relaxed);
    ctx.shared
        .bit_depth
        .store(nego.valid_bits as u32, Ordering::Relaxed);
    ctx.shared.underruns.store(0, Ordering::Relaxed);
    // Publish the active upmix layout (R9.5) so `PlayerState::upmix`
    // surfaces an indicator when a stereo source had to be widened onto
    // a surround-only device layout; `None` when played natively.
    ctx.shared.set_upmix(nego.upmix);
    let dur_ms = (decoder.duration_seconds() * 1000.0) as u32;
    ctx.shared.duration_ms.store(dur_ms, Ordering::Relaxed);
    ctx.shared.position_ms.store(0, Ordering::Relaxed);
    // Bit-perfect requires both same SR *and* same channel count
    // (no upmix). When the device demanded surround we route mono/
    // stereo into a few channels and pad the rest with silence â€”
    // that's still cleaner than the OS mixer but no longer strictly
    // bit-perfect.
    let no_upmix = nego.src_channels == nego.channels;
    ctx.is_native_rate
        .store(native_rate && no_upmix, Ordering::Release);

    // Publish the negotiated device format so `state()` can build a
    // [`BitPerfectHealth`] snapshot from a single atomic read. The
    // device channel count is what we actually write to WASAPI
    // (post-upmix when applicable); pairing it with `nego.src_channels`
    // is what lets `BitPerfectHealth::compute` flag an upmix.
    ctx.shared
        .set_device_format(nego.sample_rate, nego.channels);
    ctx.shared.reset_bit_perfect_debounce();

    // R10.1/R10.2 — size the SPSC ring for this transition via the
    // pure `RingPlan` policy (shared with the Shared backend). An
    // upmix (`src_channels < channels`) or a device rate above the
    // source rate widens the capacity so the renderer has enough slack
    // to absorb the renegotiation latency without a size-induced
    // underrun. See the design-deviation note in the Shared backend's
    // `start_playback`: the ring is created here (before the decoder
    // thread / render loop take the producer/consumer), and re-prefilled
    // on every (re)open through the `ring_ready` gate, rather than being
    // re-created mid-thread.
    let ring_plan = crate::RingPlan::compute(
        RING_CAPACITY_SAMPLES,
        format.sample_rate,
        nego.sample_rate,
        nego.src_channels,
        nego.channels,
    );

    // Arm the prefill gate: the PCM render loop emits silence (without
    // counting underruns) until the decoder thread has prefilled the
    // ring to `prefill_threshold` and flips `ring_ready` (R10.1).
    ctx.shared.set_ring_ready(false);

    let (producer, consumer) = RingBuffer::<f32>::new(ring_plan.capacity);

    // Decoder thread: same pipeline as Shared (resampler, EQ, ReplayGain).
    let decoder_alive = Arc::new(AtomicBool::new(true));
    let decoder_alive_clone = Arc::clone(&decoder_alive);
    let shared_for_decoder = Arc::clone(&ctx.shared);
    let event_tx_for_decoder = ctx.event_tx.clone();
    let status_for_decoder = Arc::clone(&ctx.status);
    let src_sr = format.sample_rate;
    let src_ch = format.channels;
    let dst_sr = nego.sample_rate;
    let prefill_threshold = ring_plan.prefill_threshold;

    let decoder_handle = thread::Builder::new()
        .name("qobee-wasapi-decoder".into())
        .spawn(move || {
            run_decoder_thread(
                decoder,
                producer,
                decoder_alive_clone,
                shared_for_decoder,
                event_tx_for_decoder,
                status_for_decoder,
                src_ch,
                src_sr,
                dst_sr,
                EffectiveOutputMode::Exclusive,
                prefill_threshold,
            );
        })
        .map_err(|e| EngineError::Internal(format!("WASAPI decoder spawn: {e}")))?;

    Ok(ActiveTrack {
        audio_client,
        render_client,
        h_event,
        nego,
        consumer,
        decoder_alive,
        decoder_handle: Some(decoder_handle),
        started: false,
        current_gain: ctx.shared.audible_gain(),
        rng_a: 0x12345678,
        rng_b: 0x9abcdef0,
    })
}

// ---------------------------------------------------------------------------
// Render: one period of audio
// ---------------------------------------------------------------------------

fn render_one_period(track: &mut ActiveTrack, ctx: &WorkerCtx) -> EngineResult<()> {
    let dst_ch = track.nego.channels as usize;
    let src_ch = track.nego.src_channels.max(1) as usize;
    let block_align = track.nego.bytes_per_sample * dst_ch;

    // Drain on seek.
    if ctx.shared.drain_ring.load(Ordering::Acquire) {
        while track.consumer.pop().is_ok() {}
    }

    let avail = track
        .audio_client
        .get_available_space_in_frames()
        .map_err(|e| EngineError::Output(format!("get_available_space: {e:?}")))?
        as usize;

    if avail == 0 {
        // Block until WASAPI signals that it wants more samples.
        // 200 ms is plenty even at very low buffer sizes; if we hit
        // the timeout something is wrong upstream and we simply
        // return so the worker can re-enter the loop and process
        // commands in the meantime.
        let _ = track.h_event.wait_for_event(200);
        return Ok(());
    }

    let frames_to_write = avail;
    let mut bytes: Vec<u8> = vec![0u8; frames_to_write * block_align];

    let paused = ctx.shared.paused.load(Ordering::Acquire);
    let target_gain = ctx.shared.audible_gain();
    let frames_to_converge: f32 = (track.nego.sample_rate as f32 * 0.010).max(1.0);
    let gain_step_per_frame: f32 = 1.0 / frames_to_converge;

    let dither_amp: f32 = if track.nego.sample_type == SampleType::Int && track.nego.valid_bits < 24
    {
        1.0 / ((1u32 << (track.nego.valid_bits.saturating_sub(1))) as f32)
    } else {
        0.0
    };

    // R10.1 — post-transition prefill gate. Until the decoder thread
    // has buffered `prefill_threshold` samples it leaves
    // `ring_ready == false`; feed the device silence for this period and
    // do *not* count it as an underrun (the ring is being filled, not
    // starved). Treated exactly like `paused`: the device keeps
    // streaming so resume is glitch-free, and `bytes` is already
    // zero-filled. Once the gate opens a starved ring is a genuine
    // underrun, counted below (R10.3).
    let ring_ready = ctx.shared.ring_ready();

    if paused || !ring_ready {
        // Send silence; the device keeps streaming, resume is instant.
        // bytes is already zero-filled from `vec!`.
    } else {
        let mut underrun = false;
        // Per-frame source scratch, sized for the source channel count.
        let mut src_frame = vec![0.0f32; src_ch];

        for frame in 0..frames_to_write {
            if track.current_gain < target_gain {
                track.current_gain = (track.current_gain + gain_step_per_frame).min(target_gain);
            } else if track.current_gain > target_gain {
                track.current_gain = (track.current_gain - gain_step_per_frame).max(target_gain);
            }
            let gain = track.current_gain;

            // Pull one source frame (src_ch samples).
            for s in src_frame.iter_mut() {
                match track.consumer.pop() {
                    Ok(v) => *s = v * gain,
                    Err(_) => {
                        ctx.shared.underruns.fetch_add(1, Ordering::Relaxed);
                        underrun = true;
                        break;
                    }
                }
            }
            if underrun {
                break;
            }

            // Equal-power stereo mono mix kept around for surround
            // downmix scenarios.
            let mono_mix = if src_ch >= 2 {
                (src_frame[0] + src_frame[1]) * 0.707_106_77
            } else {
                src_frame[0]
            };

            // Map src_ch -> dst_ch:
            //   src=1 -> dst=N: duplicate (mono everywhere).
            //   src=2 -> dst=1: equal-power L+R.
            //   src=N -> dst=N: passthrough.
            //   src=2 -> dst>=2 (surround): L on dst[0], R on dst[1],
            //     remaining channels left silent.
            //   src>2 -> dst=2: take first two as L/R (very rough but
            //     extremely uncommon: nobody plays a 5.1 file through
            //     stereo in Exclusive mode in practice).
            let frame_off = frame * block_align;
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

                let v = soft_clip(v);
                let v = if dither_amp > 0.0 {
                    v + next_tpdf(&mut track.rng_a, &mut track.rng_b, dither_amp)
                } else {
                    v
                };

                let off = frame_off + ch * track.nego.bytes_per_sample;
                write_sample(
                    &mut bytes[off..off + track.nego.bytes_per_sample],
                    v,
                    &track.nego,
                );
            }
        }
    }

    track
        .render_client
        .write_to_device(frames_to_write, &bytes, None)
        .map_err(|e| EngineError::Output(format!("write_to_device: {e:?}")))?;

    // Wait for next event so we don't busy-spin.
    let _ = track.h_event.wait_for_event(200);
    Ok(())
}

#[inline(always)]
fn soft_clip(x: f32) -> f32 {
    crate::dsp::clip::soft_clip(x)
}

#[inline(always)]
fn next_tpdf(rng_a: &mut u32, rng_b: &mut u32, amp: f32) -> f32 {
    *rng_a ^= *rng_a << 13;
    *rng_a ^= *rng_a >> 17;
    *rng_a ^= *rng_a << 5;
    *rng_b ^= *rng_b << 13;
    *rng_b ^= *rng_b >> 17;
    *rng_b ^= *rng_b << 5;
    let r1 = (*rng_a as f32 / u32::MAX as f32) - 0.5;
    let r2 = (*rng_b as f32 / u32::MAX as f32) - 0.5;
    (r1 + r2) * amp
}

#[inline(always)]
fn write_sample(slot: &mut [u8], v: f32, nego: &NegotiatedFormat) {
    match nego.sample_type {
        SampleType::Float => {
            let bytes = v.to_le_bytes();
            slot.copy_from_slice(&bytes);
        }
        SampleType::Int => match (nego.bytes_per_sample, nego.valid_bits) {
            (2, 16) => {
                let scaled = (v.clamp(-1.0, 1.0) * 32_767.0) as i16;
                slot.copy_from_slice(&scaled.to_le_bytes());
            }
            (4, 24) => {
                // 24-bit valid in 32-bit container: high 24 bits hold
                // the sample, low 8 bits are zero.
                let scaled = (v.clamp(-1.0, 1.0) * 8_388_607.0) as i32;
                let shifted = scaled << 8;
                slot.copy_from_slice(&shifted.to_le_bytes());
            }
            (4, 32) => {
                let scaled = (v.clamp(-1.0, 1.0) as f64 * 2_147_483_647.0) as i32;
                slot.copy_from_slice(&scaled.to_le_bytes());
            }
            _ => {
                for b in slot.iter_mut() {
                    *b = 0;
                }
            }
        },
    }
}

// ---------------------------------------------------------------------------
// DSD pipeline (R7.3 / R7.5 / R7.7)
// ---------------------------------------------------------------------------

/// Open a DSF or DFF file (decided by extension), negotiate a
/// 24-in-32 integer format at `rate.dop_carrier_rate()`, spawn the
/// DSD decoder thread, and return an `ActiveDsd` ready for the
/// render loop.
#[allow(clippy::needless_range_loop)]
fn start_dsd_playback(
    ctx: &WorkerCtx,
    path: &Path,
    rate: crate::DsdRate,
) -> EngineResult<ActiveDsd> {
    use crate::dsd::{dff, dop::DopPacker, dsf, DsdGroup16, DsdStream};

    // 1. Parse the DSD file.
    let stream: DsdStream = {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let file = std::fs::File::open(path)?;
        let buf = std::io::BufReader::new(file);
        match ext.as_str() {
            "dsf" => dsf::read_dsf(buf)?,
            "dff" => dff::read_dff(buf)?,
            other => {
                return Err(EngineError::DsdInvalidFile(format!(
                    "unsupported DSD extension: {other}"
                )))
            }
        }
    };
    if stream.rate != rate {
        // Sanity check: orchestrator's metadata should match what
        // the parser reports. A mismatch usually means the library
        // index is stale; refuse rather than render at the wrong
        // carrier rate.
        return Err(EngineError::DsdInvalidFile(format!(
            "DSD rate mismatch: orchestrator={:?} file={:?}",
            rate, stream.rate
        )));
    }

    // 2. Open the device and negotiate (32, 24, Int) at the carrier
    //    rate.
    let device_name = ctx.selected_device.lock().clone();
    let device = find_device(device_name.as_deref())?;
    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|e| EngineError::Output(format!("get_iaudioclient: {e:?}")))?;

    let carrier_sr = rate.dop_carrier_rate();
    let channels = stream.channels;
    let nego = negotiate_dsd_format(&audio_client, carrier_sr, channels)?;

    ctx.shared.sample_rate.store(rate.hz(), Ordering::Relaxed);
    ctx.shared
        .channels
        .store(channels as u32, Ordering::Relaxed);
    ctx.shared.bit_depth.store(1, Ordering::Relaxed);
    ctx.shared.underruns.store(0, Ordering::Relaxed);
    // The DSD render loop (`render_one_period_dsd`) does not use the
    // PCM prefill gate (it pulls pre-packed DoP `i32` straight from its
    // own ring with a spin-fill producer). Leave the gate open so any
    // `false` left over from a previous PCM track can't stall it (R10.1
    // applies to the PCM path only).
    ctx.shared.set_ring_ready(true);
    let dur_ms = ((stream.frames() as f64 * 8.0 / rate.hz() as f64) * 1000.0) as u32;
    ctx.shared.duration_ms.store(dur_ms, Ordering::Relaxed);
    ctx.shared.position_ms.store(0, Ordering::Relaxed);
    ctx.shared
        .set_device_format(nego.sample_rate, nego.channels);
    ctx.shared.reset_bit_perfect_debounce();
    ctx.is_native_rate.store(false, Ordering::Release);

    let (_def_period, min_period) = audio_client
        .get_device_period()
        .map_err(|e| EngineError::Output(format!("get_device_period: {e:?}")))?;
    let target_period = (min_period * 3 / 2).max(min_period);
    let period_hns = audio_client
        .calculate_aligned_period_near(target_period, Some(128), &nego.wave_format)
        .map_err(|e| EngineError::Output(format!("calculate_aligned_period: {e:?}")))?;

    let mode = StreamMode::EventsExclusive { period_hns };
    if let Err(e) = audio_client.initialize_client(&nego.wave_format, &Direction::Render, &mode) {
        return Err(EngineError::Output(format!(
            "initialize_client (DSD): {e:?}"
        )));
    }

    let h_event = audio_client
        .set_get_eventhandle()
        .map_err(|e| EngineError::Output(format!("set_get_eventhandle: {e:?}")))?;
    let render_client = audio_client
        .get_audiorenderclient()
        .map_err(|e| EngineError::Output(format!("get_audiorenderclient: {e:?}")))?;

    // 3. SPSC ring carrying *DoP-packed* i32 samples (interleaved).
    //    Sized like the PCM ring so a brief stall on the WASAPI
    //    side does not starve the renderer.
    let (mut producer, consumer) = RingBuffer::<i32>::new(RING_CAPACITY_SAMPLES);

    let decoder_alive = Arc::new(AtomicBool::new(true));
    let alive_clone = Arc::clone(&decoder_alive);
    let decoder_handle = thread::Builder::new()
        .name("qobee-wasapi-dsd-decoder".into())
        .spawn(move || {
            let mut packer = DopPacker::new(channels);
            let groups = stream.dop_groups();
            let bytes = stream.bytes_per_channel;
            let lsb_first = stream.lsb_first;
            for g in 0..groups {
                if !alive_clone.load(Ordering::Acquire) {
                    return;
                }
                // Build a DsdGroup16 from two consecutive bytes per
                // channel. The packer expects the *oldest* DSD bit
                // in the MSB of the 16-bit slice; convert
                // accordingly based on the source's bit ordering.
                let mut per_channel = Vec::with_capacity(channels as usize);
                for c in 0..channels as usize {
                    let b0 = bytes[c][g * 2];
                    let b1 = bytes[c][g * 2 + 1];
                    let v = if lsb_first {
                        // DSF: bit 0 of each byte is the oldest in
                        // time. Reverse the bits of each byte and
                        // place the older byte in the MSB position.
                        let r0 = b0.reverse_bits();
                        let r1 = b1.reverse_bits();
                        ((r0 as u16) << 8) | (r1 as u16)
                    } else {
                        // DFF: bit 7 of each byte is the oldest in
                        // time. The older byte already lands in the
                        // MSB position with no reordering.
                        ((b0 as u16) << 8) | (b1 as u16)
                    };
                    per_channel.push(v);
                }
                let group = DsdGroup16 { per_channel };
                let samples = packer.pack(&group);
                // Push every i32 onto the ring; spin briefly when
                // the consumer is behind.
                for s in samples {
                    while alive_clone.load(Ordering::Acquire) {
                        match producer.push(s) {
                            Ok(()) => break,
                            Err(_) => std::thread::sleep(std::time::Duration::from_micros(500)),
                        }
                    }
                }
            }
        })
        .map_err(|e| EngineError::Internal(format!("DSD decoder spawn: {e}")))?;

    Ok(ActiveDsd {
        audio_client,
        render_client,
        h_event,
        sample_rate: nego.sample_rate,
        channels,
        consumer,
        decoder_alive,
        decoder_handle: Some(decoder_handle),
        started: false,
    })
}

/// Try the DoP-required `(32, 24, Int)` format only. Anything else
/// would not carry the marker correctly, so we don't fall back to
/// the PCM negotiation table here.
fn negotiate_dsd_format(
    audio_client: &wasapi::AudioClient,
    sr: u32,
    channels: u16,
) -> EngineResult<NegotiatedFormat> {
    let wf = WaveFormat::new(
        32,
        24,
        &SampleType::Int,
        sr as usize,
        channels as usize,
        None,
    );
    if let Ok(resolved) = audio_client.is_supported_exclusive_with_quirks(&wf) {
        return Ok(NegotiatedFormat {
            wave_format: resolved,
            sample_rate: sr,
            channels,
            src_channels: channels,
            bytes_per_sample: 4,
            sample_type: SampleType::Int,
            valid_bits: 24,
            // DoP carries the source channels 1:1 — never an upmix.
            upmix: None,
        });
    }
    Err(EngineError::Output(format!(
        "DoP format (32-bit, 24 valid, Int) at {sr} Hz / {channels} ch refused by device"
    )))
}

/// Render one period of DoP-packed samples to the device. Same
/// shape as `render_one_period` but copies `i32` straight from the
/// ring without volume / dither / soft-clip — the DSD pipeline is
/// strictly bypass.
fn render_one_period_dsd(track: &mut ActiveDsd, ctx: &WorkerCtx) -> EngineResult<()> {
    let block_align = 4 * track.channels as usize;

    if ctx.shared.drain_ring.load(Ordering::Acquire) {
        while track.consumer.pop().is_ok() {}
    }

    let avail = track
        .audio_client
        .get_available_space_in_frames()
        .map_err(|e| EngineError::Output(format!("get_available_space (DSD): {e:?}")))?
        as usize;
    if avail == 0 {
        let _ = track.h_event.wait_for_event(200);
        return Ok(());
    }

    let frames_to_write = avail;
    let mut bytes: Vec<u8> = vec![0u8; frames_to_write * block_align];
    let paused = ctx.shared.paused.load(Ordering::Acquire);

    if !paused {
        let mut wrote = 0usize;
        for frame in 0..frames_to_write {
            let mut frame_ok = true;
            let frame_off = frame * block_align;
            for c in 0..track.channels as usize {
                match track.consumer.pop() {
                    Ok(v) => {
                        let off = frame_off + c * 4;
                        bytes[off..off + 4].copy_from_slice(&v.to_le_bytes());
                    }
                    Err(_) => {
                        ctx.shared.underruns.fetch_add(1, Ordering::Relaxed);
                        frame_ok = false;
                        break;
                    }
                }
            }
            if !frame_ok {
                break;
            }
            wrote += 1;
        }
        // Position is updated coarsely from frames written. The DoP
        // carrier rate is `rate.hz() / 16` so frames-to-time is just
        // `frames / sample_rate`.
        let pos_ms = ctx.shared.position_ms.load(Ordering::Relaxed) as u64
            + (wrote as u64) * 1000 / track.sample_rate.max(1) as u64;
        ctx.shared
            .position_ms
            .store(pos_ms.min(u32::MAX as u64) as u32, Ordering::Relaxed);
    }

    track
        .render_client
        .write_to_device(frames_to_write, &bytes, None)
        .map_err(|e| EngineError::Output(format!("write_to_device (DSD): {e:?}")))?;
    let _ = track.h_event.wait_for_event(200);
    Ok(())
}

/// List WASAPI render endpoints for the UI device picker. Names
/// match what CPAL returns for the same hardware.
pub fn list_render_devices() -> EngineResult<Vec<String>> {
    let _ = initialize_mta();
    let enumerator = DeviceEnumerator::new()
        .map_err(|e| EngineError::Output(format!("device enumerator: {e:?}")))?;
    let collection = enumerator
        .get_device_collection(&Direction::Render)
        .map_err(|e| EngineError::Output(format!("device collection: {e:?}")))?;
    let count = collection
        .get_nbr_devices()
        .map_err(|e| EngineError::Output(format!("count: {e:?}")))?;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        if let Ok(d) = collection.get_device_at_index(i) {
            if let Ok(n) = d.get_friendlyname() {
                out.push(n);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// `WasapiExclusiveEngine::state` must populate the new
    /// `bit_perfect` field as `Some(...)` once a device format has
    /// been published â€” no need to actually open a WASAPI session
    /// (which would require a Windows render endpoint not available
    /// in CI). Mirrors the equivalent test in `backend_cpal_shared`.
    #[test]
    fn wasapi_engine_state_publishes_bit_perfect_after_device_format_set() {
        let engine = match WasapiExclusiveEngine::new() {
            Ok(e) => e,
            Err(e) => {
                // CI runners without a working WASAPI host (rare, but
                // possible on bare images) should not flake this
                // test. Skip with a tracing line so the absence is
                // visible in the run log.
                eprintln!("skipping: WASAPI engine boot failed: {e}");
                return;
            }
        };

        engine.shared.set_device_format(48_000, 2);
        engine.shared.sample_rate.store(48_000, Ordering::Relaxed);
        engine.shared.channels.store(2, Ordering::Relaxed);
        engine.shared.bit_depth.store(24, Ordering::Relaxed);
        // Pretend the negotiation produced the source rate exactly
        // (i.e. a bit-perfect candidate run) so the `is_native_rate`
        // flag carries through the snapshot.
        engine.is_native_rate.store(true, Ordering::Release);
        *engine.status.lock() = PlaybackStatus::Playing;

        let state = AudioEngine::state(&engine);
        assert!(
            state.bit_perfect.is_some(),
            "device format published â‡’ bit_perfect snapshot must be Some"
        );
        let health = state.bit_perfect.unwrap();
        assert_eq!(health.device_sample_rate, Some(48_000));
        assert_eq!(health.source_sample_rate, Some(48_000));
        assert_eq!(health.effective_output_mode, EffectiveOutputMode::Exclusive);
        assert!(health.is_native_rate);
    }

    /// Conversely, a freshly-booted engine with no device open must
    /// return `None` for `bit_perfect`. This guarantees the wire
    /// format stays compact while idle (R5.7).
    #[test]
    fn wasapi_engine_state_carries_bit_perfect_field_default_none() {
        let engine = match WasapiExclusiveEngine::new() {
            Ok(e) => e,
            Err(e) => {
                eprintln!("skipping: WASAPI engine boot failed: {e}");
                return;
            }
        };
        let state = AudioEngine::state(&engine);
        assert!(state.bit_perfect.is_none());

        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("\"bit_perfect\":"),
            "bit_perfect=None must be skipped on the wire, got {json}"
        );
    }

    // -----------------------------------------------------------------
    // Task 13.3 — negotiation example tests (R9.6, R9.7)
    //
    // The pure decision parts of the negotiation are unit-testable
    // without a live `wasapi::AudioClient`:
    //   * stereo-only / surround-only layout choice via the pure
    //     `select_format` (R9.4/R9.5),
    //   * the total-failure → `ExclusiveFallback` event mapping via
    //     `exclusive_load_failure_events` (R9.6),
    //   * the single aggregated negotiation log line via
    //     `log_negotiation_summary` (R9.7).
    //
    // The full `negotiate_with_fallback` / `start_playback` paths need
    // a real WASAPI render endpoint (COM, `IAudioClient`) that a unit
    // test cannot construct, so the probing/`initialize` round-trip
    // itself stays covered by manual/integration testing on hardware.
    // -----------------------------------------------------------------
    use tracing_test::traced_test;

    /// Feature: qobee-beta-feedback-improvements, task 13.3 —
    /// stereo-only device → no upmix (R9.4/R9.5).
    ///
    /// A device that only offers a native stereo layout, fed a stereo
    /// source, must be selected at 2 channels with no upmix flag.
    #[test]
    fn stereo_only_device_selects_native_no_upmix() {
        let candidates = [CandidateFormat::new(48_000, 2)];
        let sel = select_format(&candidates, 2).expect("a stereo candidate must be selectable");
        assert_eq!(sel.chosen.channels, 2, "stereo-only device must stay 2ch");
        assert!(
            sel.upmix.is_none(),
            "a native stereo layout must not flag an upmix (R9.5)"
        );
    }

    /// Feature: qobee-beta-feedback-improvements, task 13.3 —
    /// surround-only device → upmix 2→8 with flag (R9.5).
    ///
    /// A device that only exposes a 7.1 (8ch) layout, fed a stereo
    /// source, must be selected at 8 channels and carry an explicit
    /// `UpmixInfo { src: 2, dst: 8 }`.
    #[test]
    fn surround_only_device_flags_upmix_2_to_8() {
        let candidates = [CandidateFormat::new(48_000, 8)];
        let sel = select_format(&candidates, 2).expect("an 8ch candidate must be selectable");
        assert_eq!(sel.chosen.channels, 8, "surround-only device stays 8ch");
        assert_eq!(
            sel.upmix,
            Some(UpmixInfo {
                src_channels: 2,
                dst_channels: 8,
            }),
            "upmixing a stereo source onto a surround-only layout must flag 2→8 (R9.5)"
        );
    }

    /// Feature: qobee-beta-feedback-improvements, task 13.3 — total
    /// negotiation failure → `ExclusiveFallback` emitted (R9.6).
    ///
    /// The `Load` worker arm maps a negotiation failure to exactly two
    /// events, in order: `ExclusiveFallback { reason }` (so the UI can
    /// explain the switch to Shared) then `Error { message }` (which
    /// drives the backend swap + track restart in `qobee-core`). Both
    /// carry the same failure reason.
    #[test]
    fn total_failure_emits_exclusive_fallback_then_error() {
        let reason = "no compatible WASAPI Exclusive format found for this device".to_string();
        let events = exclusive_load_failure_events(reason.clone());

        assert_eq!(events.len(), 2, "exactly two events on total failure");
        match &events[0] {
            EngineEvent::ExclusiveFallback { reason: r } => {
                assert_eq!(r, &reason, "fallback must carry the failure reason (R9.6)");
            }
            other => panic!("expected ExclusiveFallback first, got {other:?}"),
        }
        match &events[1] {
            EngineEvent::Error { message } => {
                assert_eq!(message, &reason, "Error must carry the same reason");
            }
            other => panic!("expected Error second, got {other:?}"),
        }
    }

    /// Feature: qobee-beta-feedback-improvements, task 13.3 — multiple
    /// probes → exactly one aggregated log line (R9.7).
    ///
    /// `log_negotiation_summary` emits a *single* `warn!` summary that
    /// carries the whole `tried` list, replacing the old
    /// one-line-per-probed-frequency noise. Even when six
    /// `(sample_rate, channels)` pairs were attempted, exactly one
    /// "format negotiation summary" line is produced.
    #[traced_test]
    #[test]
    fn many_probes_produce_a_single_aggregated_log_line() {
        // Simulate the full fallback ladder having probed six rates
        // before settling (or failing).
        let tried: Vec<(u32, u16)> = vec![
            (44_100, 2),
            (48_000, 2),
            (88_200, 2),
            (96_000, 2),
            (176_400, 2),
            (192_000, 2),
        ];

        log_negotiation_summary(
            "Test Device (Render Endpoint)",
            &tried,
            "no compatible format found; reverting to Shared",
        );

        logs_assert(|lines: &[&str]| {
            let summaries = lines
                .iter()
                .filter(|l| l.contains("format negotiation summary"))
                .count();
            if summaries == 1 {
                Ok(())
            } else {
                Err(format!(
                    "expected exactly one aggregated negotiation summary line, got {summaries} (R9.7)"
                ))
            }
        });
    }
}
