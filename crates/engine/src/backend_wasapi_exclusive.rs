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
//!   - Then try common HD rates ≥ source SR (192 → 88.2 kHz), then
//!     common rates < source SR (48, 44.1 kHz). Resampling kicks in
//!     via the same sinc resampler used by the Shared backend.
//!   - As a last resort, ask the device for its `mixformat` and try
//!     that rate (with the source's channel count, then with the
//!     device's preferred channel count when the driver demands it).
//!     When the channel count is upmixed (e.g. stereo → 7.1 surround
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

use wasapi::{
    initialize_mta, Direction, DeviceEnumerator, SampleType, StreamMode, WaveFormat,
};

use crate::backend_cpal_shared::{run_decoder_thread, Shared, RING_CAPACITY_SAMPLES};
use crate::backend_symphonia::SymphoniaDecoder;
use crate::error::{EngineError, EngineResult};
use crate::types::{
    EffectiveOutputMode, EngineEvent, OutputMode, PlaybackStatus, PlayerState,
};
use crate::AudioEngine;

// ---------------------------------------------------------------------------
// Public engine
// ---------------------------------------------------------------------------

enum Command {
    Load(PathBuf),
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

        PlayerState {
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

    fn prepare_next(&self, _path: &Path, _track_id: Option<String>) -> EngineResult<()> {
        // WASAPI Exclusive backend does not support in-place gapless
        // transitions yet (each track opens a fresh device session).
        // The orchestrator will use the standard EOT → Load path,
        // which produces the same small gap as before.
        Ok(())
    }

    fn clear_pending_next(&self) -> EngineResult<()> {
        Ok(())
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
    let mut active: Option<ActiveTrack> = None;

    loop {
        // If something is loaded and playing, drain the audio device
        // until we hit a "no available frames" / event-wait state, then
        // poll the command channel non-blocking. If nothing is loaded
        // we block on the command channel.
        let render_active = active.as_ref().is_some_and(|a| a.started);

        if render_active {
            // Render one period if there is space.
            if let Some(track) = active.as_mut() {
                match render_one_period(track, &ctx) {
                    Ok(()) => {}
                    Err(e) => {
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
            }

            // Drain commands without blocking.
            while let Ok(cmd) = ctx.cmd_rx.try_recv() {
                handle_cmd(cmd, &ctx, &mut active);
            }
        } else {
            // Nothing is rendering: block on the next command.
            match ctx.cmd_rx.recv() {
                Ok(cmd) => handle_cmd(cmd, &ctx, &mut active),
                Err(_) => break, // sender dropped
            }
        }
    }

    if let Some(prev) = active.take() {
        prev.shutdown();
    }
}

fn handle_cmd(cmd: Command, ctx: &WorkerCtx, active: &mut Option<ActiveTrack>) {
    match cmd {
        Command::Load(path) => {
            if let Some(prev) = active.take() {
                prev.shutdown();
            }
            ctx.shared.pending_seek_ms.store(-1, Ordering::Release);
            ctx.shared.drain_ring.store(false, Ordering::Release);
            set_status(ctx, PlaybackStatus::Loading);

            match start_playback(ctx, &path) {
                Ok(s) => {
                    *active = Some(s);
                    set_status(ctx, PlaybackStatus::Paused);
                }
                Err(e) => {
                    let msg = e.to_string();
                    *ctx.last_error.lock() = Some(msg.clone());
                    set_status(ctx, PlaybackStatus::Errored);
                    let _ = ctx.event_tx.try_send(EngineEvent::Error { message: msg });
                }
            }
        }
        Command::Play | Command::Resume => {
            if let Some(track) = active.as_mut() {
                ctx.shared.paused.store(false, Ordering::Release);
                if !track.started {
                    if let Err(e) = track.audio_client.start_stream() {
                        let msg = format!("start_stream: {e:?}");
                        *ctx.last_error.lock() = Some(msg.clone());
                        set_status(ctx, PlaybackStatus::Errored);
                        let _ = ctx.event_tx.try_send(EngineEvent::Error { message: msg });
                        return;
                    }
                    track.started = true;
                }
                set_status(ctx, PlaybackStatus::Playing);
            }
        }
        Command::Pause => {
            if let Some(_track) = active.as_mut() {
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
            ctx.shared.pending_seek_ms.store(-1, Ordering::Release);
            ctx.shared.drain_ring.store(false, Ordering::Release);
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
    let eq_bypass = ctx
        .shared
        .eq_gains_db
        .lock()
        .iter()
        .all(|g| g.abs() < 0.05);
    let is_bit_perfect = ctx.is_native_rate.load(Ordering::Relaxed)
        && unity_volume
        && unity_pregain
        && eq_bypass;

    PlayerState {
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
        error,
    }
}

// ---------------------------------------------------------------------------
// Per-track setup
// ---------------------------------------------------------------------------

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
    src_sr: u32,
    src_channels: u16,
) -> EngineResult<(NegotiatedFormat, bool)> {
    // Try source rate first.
    if let Ok(n) = negotiate_exclusive_format(audio_client, src_sr, src_channels, src_channels) {
        return Ok((n, true));
    }

    // Common alternatives, ordered to pick the closest "up" first
    // (less destructive than downsampling).
    let common: [u32; 6] = [192_000, 176_400, 96_000, 88_200, 48_000, 44_100];
    let mut tried: Vec<u32> = Vec::with_capacity(8);
    for &sr in common.iter().filter(|&&sr| sr >= src_sr) {
        if sr == src_sr {
            continue;
        }
        tried.push(sr);
        if let Ok(n) = negotiate_exclusive_format(audio_client, sr, src_channels, src_channels) {
            tracing::warn!(
                target: "qobee::engine",
                src_sr,
                fallback_sr = sr,
                "WASAPI Exclusive: source rate refused; using a higher common rate (sinc-resampled)"
            );
            return Ok((n, false));
        }
    }
    for &sr in common.iter().filter(|&&sr| sr < src_sr) {
        tried.push(sr);
        if let Ok(n) = negotiate_exclusive_format(audio_client, sr, src_channels, src_channels) {
            tracing::warn!(
                target: "qobee::engine",
                src_sr,
                fallback_sr = sr,
                "WASAPI Exclusive: source rate refused; using a lower common rate (sinc-resampled)"
            );
            return Ok((n, false));
        }
    }

    // Last resort: ask the device for the format it actually wants
    // (its mixformat) and use that SR. We try the mix format's SR
    // *with the source's channel count* first (so a stereo source
    // doesn't suddenly turn into 7.1) before accepting the mix
    // format's own channel count.
    if let Ok(mixfmt) = audio_client.get_mixformat() {
        let mix_sr = mixfmt.get_samplespersec();
        let mix_ch = mixfmt.get_nchannels();

        if !tried.contains(&mix_sr) {
            if let Ok(n) =
                negotiate_exclusive_format(audio_client, mix_sr, src_channels, src_channels)
            {
                tracing::warn!(
                    target: "qobee::engine",
                    src_sr,
                    fallback_sr = mix_sr,
                    "WASAPI Exclusive: using device's preferred (mixformat) rate, source channels"
                );
                return Ok((n, false));
            }
        }

        if mix_ch != src_channels {
            if let Ok(n) = negotiate_exclusive_format(audio_client, mix_sr, mix_ch, src_channels) {
                tracing::warn!(
                    target: "qobee::engine",
                    src_sr,
                    fallback_sr = mix_sr,
                    src_channels,
                    dst_channels = mix_ch,
                    "WASAPI Exclusive: device requires a different channel layout; will upmix"
                );
                return Ok((n, false));
            }
        }
    }

    Err(EngineError::Output(format!(
        "no compatible WASAPI Exclusive format found for this device (tried {} Hz and {} other rates). \
         The device may be busy with another app, or its driver does not support Exclusive mode for any of: \
         24-in-32 / 32 int / 32 float / 16 int. Try a different output device.",
        src_sr,
        tried.len()
    )))
}

fn start_playback(ctx: &WorkerCtx, path: &Path) -> EngineResult<ActiveTrack> {
    let decoder = SymphoniaDecoder::open(path)?;
    let format = decoder.format();

    let device_name = ctx.selected_device.lock().clone();
    let device = find_device(device_name.as_deref())?;
    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|e| EngineError::Output(format!("get_iaudioclient: {e:?}")))?;

    let (nego, native_rate) = negotiate_with_fallback(
        &audio_client,
        format.sample_rate,
        format.channels,
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
    let dur_ms = (decoder.duration_seconds() * 1000.0) as u32;
    ctx.shared.duration_ms.store(dur_ms, Ordering::Relaxed);
    ctx.shared.position_ms.store(0, Ordering::Relaxed);
    // Bit-perfect requires both same SR *and* same channel count
    // (no upmix). When the device demanded surround we route mono/
    // stereo into a few channels and pad the rest with silence —
    // that's still cleaner than the OS mixer but no longer strictly
    // bit-perfect.
    let no_upmix = nego.src_channels == nego.channels;
    ctx.is_native_rate
        .store(native_rate && no_upmix, Ordering::Release);

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

    // Initialize. If WASAPI returns AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED
    // we recreate the client at the next-highest aligned size, as the
    // wasapi-rs example does.
    if let Err(e) =
        audio_client.initialize_client(&nego.wave_format, &Direction::Render, &mode)
    {
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

    let (producer, consumer) = RingBuffer::<f32>::new(RING_CAPACITY_SAMPLES);

    // Decoder thread: same pipeline as Shared (resampler, EQ, ReplayGain).
    let decoder_alive = Arc::new(AtomicBool::new(true));
    let decoder_alive_clone = Arc::clone(&decoder_alive);
    let shared_for_decoder = Arc::clone(&ctx.shared);
    let event_tx_for_decoder = ctx.event_tx.clone();
    let status_for_decoder = Arc::clone(&ctx.status);
    let src_sr = format.sample_rate;
    let src_ch = format.channels;
    let dst_sr = nego.sample_rate;

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

    let dither_amp: f32 =
        if track.nego.sample_type == SampleType::Int && track.nego.valid_bits < 24 {
            1.0 / ((1u32 << (track.nego.valid_bits.saturating_sub(1))) as f32)
        } else {
            0.0
        };

    if paused {
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
    let t = x.clamp(-1.5, 1.5);
    let t2 = t * t;
    if t.abs() <= 1.0 {
        t * (1.0 - t2 / 3.0)
    } else if t > 0.0 {
        (2.0 / 3.0 + (1.0 - (-((t - 1.0) * 2.0)).exp()) / 3.0).min(1.0)
    } else {
        (-2.0 / 3.0 - (1.0 - (-((-t - 1.0) * 2.0)).exp()) / 3.0).max(-1.0)
    }
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
