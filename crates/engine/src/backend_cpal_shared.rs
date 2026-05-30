//! CPAL-based Shared output backend.
//!
//! This is the engine's only output backend. It pulls f32 interleaved
//! PCM from a Symphonia decoder running on a worker thread, optionally
//! resamples it (via rubato sinc, BlackmanHarrisÂ²) to the device's
//! native rate, applies an optional ReplayGain pre-gain and a 10-band
//! peaking EQ, then feeds it to the OS mixer via CPAL. The audio
//! callback adds soft-clip + TPDF dither (when the device asks for a
//! â‰¤16-bit integer format) before quantization.
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

use arc_swap::ArcSwap;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::audio_settings::{AudioSettings, ResamplerQuality};
use crate::backend_symphonia::SymphoniaDecoder;
use crate::error::{EngineError, EngineResult};
use crate::types::{
    BitPerfectHealth, EffectiveOutputMode, EngineEvent, OutputMode, PlaybackStatus, PlayerState,
    UpmixInfo,
};
use crate::AudioEngine;
use crate::PreGainContext;

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
    PrepareNext {
        path: PathBuf,
        track_id: Option<String>,
    },
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
    /// Snapshot of every `audio.*` setting consumed by the DSP
    /// pipeline. The decoder thread loads it lazily at chunk
    /// boundaries (`load_full()` is a single atomic read + refcount)
    /// and rebuilds DSP stages when [`AudioSettings::version`] has
    /// changed since its last sync.
    ///
    /// Kept as a single [`ArcSwap`] (rather than per-field atomics) to
    /// avoid bloating this struct as new audio settings land in later
    /// phases of the spec.
    audio_settings: ArcSwap<AudioSettings>,
    /// Per-load context for `Pre_Gain_Stage`: ReplayGain dB and peak
    /// from the active track's tags + the current volume slider.
    /// Stashed by `Player::start_track` (and on volume changes); the
    /// decoder thread (task 18) will pick it up on chunk boundaries
    /// and forward it to `PcmChain::set_pre_gain_context`.
    pre_gain_context: Mutex<PreGainContext>,
    /// Bumped whenever `pre_gain_context` is replaced. Read by the
    /// decoder thread to detect a fresh context without holding the
    /// mutex on the hot path.
    pub(crate) pre_gain_context_version: AtomicU32,
    /// Last RG attenuation in dB reported by `Pre_Gain_Stage`. Lives
    /// here (rather than on the chain) so `state()` can read it
    /// without crossing the decoder thread. Encoded as `(db_x1000) as
    /// i32` so it fits in a single relaxed atomic.
    pub(crate) rg_attenuation_db_x1000: AtomicI32,
    /// Negotiated sample rate of the output device in Hz, or `0` when
    /// no device is open. Distinct from `sample_rate` (which tracks
    /// the *source* SR after format negotiation): the device may run
    /// at a different rate when the source SR was refused, in which
    /// case the engine resamples on its way to the device.
    pub(crate) device_sample_rate: AtomicU32,
    /// Negotiated channel count of the output device, or `0` when no
    /// device is open. Distinct from `channels` (which always tracks
    /// the source) so the bit-perfect health calculator can spot an
    /// upmix without re-running negotiation.
    pub(crate) device_channels: AtomicU32,
    /// Last [`BitPerfectHealth`] snapshot pushed via
    /// [`EngineEvent::BitPerfectChanged`]. Compared on every chunk
    /// boundary so we only emit a fresh event when the snapshot
    /// meaningfully differs *and* at least 200 ms have elapsed since
    /// the previous emit (R5.5 cadence + flooding guard). The mutex
    /// is contended only at chunk boundaries (~tens of times per
    /// second) so a `parking_lot::Mutex` is fine.
    pub(crate) last_bit_perfect: Mutex<Option<BitPerfectHealth>>,
    /// Wall-clock instant of the last `BitPerfectChanged` emit.
    /// Locked together with `last_bit_perfect` so the pair stays
    /// consistent under concurrent reads.
    pub(crate) last_bit_perfect_emit: Mutex<Option<std::time::Instant>>,
    /// Per-chunk snapshot of the chain's `eq_bypass`,
    /// `convolver_off`, and `dither_bypass` flags, packed into one
    /// atomic byte. Bit 0 = eq_bypass, bit 1 = convolver_off, bit 2
    /// = dither_bypass. Conservative initial value `0b111` (every
    /// stage bypassed) so an early `state()` call before the
    /// decoder thread runs reports a clean chain.
    pub(crate) chain_bypass_bits: AtomicU32,
    /// Pending convolver IR for the decoder thread to apply at the
    /// next chunk boundary (R9.2). Written by the worker thread that
    /// loaded the WAV (`Player::load_convolver_ir`); the decoder
    /// thread takes the value once and forwards it to
    /// `PcmChain::set_convolver_ir`. Empty channels (`Vec::new`)
    /// signal an explicit unload.
    pending_ir: Mutex<Option<(Vec<f32>, Vec<f32>)>>,
    /// Bumped whenever `pending_ir` is replaced. Read by the
    /// decoder thread to detect a fresh IR without holding the
    /// mutex on the hot path.
    pub(crate) pending_ir_version: AtomicU32,
    /// Length (in taps, per channel) of the most recently published
    /// IR; zero when none loaded. Cached at `set_convolver_ir` time
    /// so `state()`-style readers can compute latency without
    /// crossing the decoder thread boundary.
    pub(crate) convolver_ir_len: AtomicU32,
    /// `true` while the engine is actively rendering a DSD stream
    /// (R7.8). Set at the start of a DSD load, cleared on `Stop` /
    /// on a PCM `Load`. Read by `Player` to gate volume / EQ /
    /// pre-gain commands and by the decoder thread to ignore the
    /// PCM chain.
    pub(crate) is_dsd_active: AtomicBool,
    /// Active DSD rate label (`"DSD64"` …). Populated when the DSD
    /// pipeline starts emitting samples and cleared on stop / on a
    /// PCM load. Mutex contention is limited to track boundaries.
    pub(crate) dsd_rate_label: Mutex<Option<String>>,
    /// Pre-render sink installed by [`CpalSharedEngine::start_pre_render`]
    /// for the Exclusive-mode null-test branch (R11.3). When
    /// [`crate::diagnostic::pre_render::PreRenderSink::is_active`]
    /// is true the decoder thread
    /// writes its post-DSP chunks to this file *instead* of the
    /// audio device. Always present (the sink defaults to inactive)
    /// so the decoder thread never has to handle an `Option`.
    pub(crate) pre_render: crate::diagnostic::pre_render::PreRenderSink,
    /// Source URI / path of the track currently feeding the active
    /// stream. Stored so the worker can rebuild the output stream
    /// in place when the OS yanks the device out from under us — the
    /// usual cause being a voice/video call app switching the
    /// default output to a communication device mid-playback. Empty
    /// while nothing is loaded.
    current_source_path: Mutex<Option<PathBuf>>,
    /// Raised by the CPAL stream-error callback when the device
    /// becomes unavailable (call app grabbed it, headphones
    /// unplugged, default device switched). The worker polls this
    /// between commands and, when set, tears the dead stream down
    /// and rebuilds it on the *current* default device, resuming
    /// from the last known position. Idempotent: the callback may
    /// raise it repeatedly; the worker clears it once it starts a
    /// recovery attempt.
    pub(crate) recover_device: AtomicBool,
    /// Crossfade duration in milliseconds (`0` = off). Read by the
    /// decoder thread at track-load time and at the crossfade
    /// trigger point. When non-zero and a compatible next track is
    /// prepared, the decoder mixes the outgoing tail with the
    /// incoming head over this window (equal-power curve) instead of
    /// the instant gapless swap.
    pub(crate) crossfade_ms: AtomicU32,
    /// Anti-click fade target for play/pause, encoded as
    /// `(linear * 1_000_000) as u32` (so `0` or `1_000_000`). The
    /// audio callback keeps a closure-local smoothed `fade_gain`
    /// that ramps toward this value over ~12 ms and multiplies every
    /// output sample by it. Pause sets the target to `0` and waits
    /// for the ramp before stopping the stream, so the device never
    /// sees an abrupt amplitude step (the source of the click).
    /// Resume sets it back to `1`. Defaults to `1.0` so playback that
    /// starts without an explicit fade-in is unaffected; a freshly
    /// built stream's `fade_gain` still starts at `0`, giving every
    /// new track a clean ~12 ms fade-in.
    pub(crate) fade_target_micro: AtomicU32,
    /// Active upmix layout (R9.5) of the *currently open* stream, or
    /// `None` when the source channel count is played natively (no
    /// upmix). Set by the WASAPI Exclusive backend when negotiation
    /// has to widen a stereo source onto a surround-only device
    /// layout; read by `state()` / `snapshot_state` so the indicator
    /// reaches `PlayerState::upmix`. The Shared backend never upmixes,
    /// so it leaves this `None`. A `parking_lot::Mutex` is ample: it
    /// is written once per stream open and read only at snapshot time.
    pub(crate) upmix: Mutex<Option<UpmixInfo>>,
    /// `true` when the current file exceeded the MP3 frame-repair
    /// threshold (R11.3): so many `invalid main_data_begin` frames
    /// were repaired that the file is likely degraded. Set by the
    /// decoder thread at EOF (when `repaired_frames >
    /// DEGRADED_FRAME_THRESHOLD`), read by `state()` / `snapshot_state`
    /// so the indicator reaches `PlayerState::degraded`, and reset to
    /// `false` on the next `Load`.
    pub(crate) file_degraded: AtomicBool,
    /// Gate that tells the audio callback whether the ring has been
    /// prefilled enough to start consuming after a format transition
    /// (R10.1). Set to `false` when a stream opens (in `start_playback`)
    /// and flipped to `true` by the decoder thread once it has pushed at
    /// least `prefill_threshold` interleaved samples into the ring (or
    /// when the source reaches EOF before that threshold, so a short
    /// track still plays out). While `false`, the callback emits silence
    /// **without** incrementing `underruns`; once `true`, a starved ring
    /// counts as a genuine underrun (R10.3). The Shared callback and the
    /// WASAPI PCM render loop both honor this flag.
    pub(crate) ring_ready: AtomicBool,
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
            audio_settings: ArcSwap::from_pointee(AudioSettings::default()),
            pre_gain_context: Mutex::new(PreGainContext::default()),
            pre_gain_context_version: AtomicU32::new(0),
            rg_attenuation_db_x1000: AtomicI32::new(0),
            device_sample_rate: AtomicU32::new(0),
            device_channels: AtomicU32::new(0),
            last_bit_perfect: Mutex::new(None),
            last_bit_perfect_emit: Mutex::new(None),
            chain_bypass_bits: AtomicU32::new(0b111),
            pending_ir: Mutex::new(None),
            pending_ir_version: AtomicU32::new(0),
            convolver_ir_len: AtomicU32::new(0),
            is_dsd_active: AtomicBool::new(false),
            dsd_rate_label: Mutex::new(None),
            pre_render: crate::diagnostic::pre_render::PreRenderSink::new(),
            current_source_path: Mutex::new(None),
            recover_device: AtomicBool::new(false),
            crossfade_ms: AtomicU32::new(0),
            fade_target_micro: AtomicU32::new(1_000_000),
            upmix: Mutex::new(None),
            file_degraded: AtomicBool::new(false),
            // The ring is empty at construction; the decoder thread
            // flips this to `true` after the first prefill (R10.1).
            ring_ready: AtomicBool::new(false),
        }
    }

    fn volume(&self) -> f32 {
        // Linear value as set by the user (0..1). Used by `state()`
        // so the UI slider position is preserved verbatim.
        self.volume_micro.load(Ordering::Relaxed) as f32 / 1_000_000.0
    }

    /// Audible gain applied to samples. Drives the 10 ms volume ramp
    /// in the audio callback (and the WASAPI exclusive render loop)
    /// by translating the user's slider position through the
    /// currently published [`AudioSettings::volume_curve`] (R4.1,
    /// R4.4). Boundary contracts:
    ///
    ///   * `slider == 0.0` â†’ `0.0` (mute-exact, R4.2).
    ///   * `slider == 1.0` â†’ `1.0` (unity-exact, R4.3 â€” the value
    ///     the bit-perfect badge rests on).
    ///
    /// The ramp itself stays linear: it operates on the `target_gain`
    /// returned here, so the curve only sets the sliderâ†’linear
    /// mapping. The conversion is one `ArcSwap::load` + a single
    /// `slider_to_linear` call, both branchless on the hot path.
    pub(crate) fn audible_gain(&self) -> f32 {
        let v = self.volume_micro.load(Ordering::Relaxed) as f32 / 1_000_000.0;
        let v = v.clamp(0.0, 1.0);
        let settings = self.audio_settings.load();
        crate::volume::slider_to_linear(v, settings.volume_curve, settings.volume_floor_db)
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
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn set_pre_gain_micro(&self, micro: u32) {
        self.pre_gain_micro.store(micro, Ordering::Relaxed);
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn set_volume_public(&self, v: f32) {
        self.set_volume(v);
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn volume_public(&self) -> f32 {
        self.volume()
    }

    /// Snapshot of the latest [`AudioSettings`] published to the DSP
    /// pipeline. This is a single atomic load + refcount bump; cheap
    /// enough to call at every chunk boundary in the decoder thread.
    pub(crate) fn audio_settings(&self) -> Arc<AudioSettings> {
        self.audio_settings.load_full()
    }

    /// Remember the source path of the track currently feeding the
    /// active stream, so the worker can rebuild the stream in place
    /// on a device-loss event without involving the orchestrator.
    pub(crate) fn set_current_source_path(&self, path: Option<PathBuf>) {
        *self.current_source_path.lock() = path;
    }

    /// The source path of the track currently feeding the active
    /// stream, if any.
    pub(crate) fn current_source_path(&self) -> Option<PathBuf> {
        self.current_source_path.lock().clone()
    }

    /// Crossfade window in milliseconds; `0` disables crossfade.
    pub(crate) fn crossfade_ms(&self) -> u32 {
        self.crossfade_ms.load(Ordering::Relaxed)
    }

    /// Set the crossfade window (ms). Takes effect on the next
    /// track transition; an in-flight fade is unaffected.
    pub fn set_crossfade_ms(&self, ms: u32) {
        self.crossfade_ms.store(ms, Ordering::Relaxed);
    }

    /// Anti-click fade target (`0.0` or `1.0`) read by the audio
    /// callback's per-sample ramp.
    pub(crate) fn fade_target(&self) -> f32 {
        self.fade_target_micro.load(Ordering::Relaxed) as f32 / 1_000_000.0
    }

    /// Set the anti-click fade target. `1.0` ramps the output up
    /// (resume / fresh start), `0.0` ramps it down (pre-pause).
    pub(crate) fn set_fade_target(&self, v: f32) {
        let clamped = v.clamp(0.0, 1.0);
        self.fade_target_micro
            .store((clamped * 1_000_000.0) as u32, Ordering::Relaxed);
    }

    /// Replace the published [`AudioSettings`] snapshot. Bumps
    /// [`AudioSettings::version`] off the *currently published* value
    /// so the decoder thread observes a strictly increasing version
    /// even if the caller did not update it (the store layer is the
    /// source of truth for clamping; this method only owns the version
    /// counter).
    pub fn set_audio_settings(&self, new: AudioSettings) {
        let mut new = new;
        let current_version = self.audio_settings.load().version;
        new.version = current_version.wrapping_add(1);
        self.audio_settings.store(Arc::new(new));
    }

    /// Replace the per-load `PreGainContext` and bump its version
    /// counter so the decoder thread (task 18) picks it up on the
    /// next chunk boundary.
    pub(crate) fn set_pre_gain_context(&self, ctx: PreGainContext) {
        *self.pre_gain_context.lock() = ctx;
        self.pre_gain_context_version
            .fetch_add(1, Ordering::Release);
    }

    /// Snapshot of the current pre-gain context. Cheap (one mutex
    /// lock; the mutex is contended only on track boundaries).
    #[allow(dead_code)] // Consumed by run_decoder_thread in task 18.
    pub(crate) fn pre_gain_context(&self) -> PreGainContext {
        *self.pre_gain_context.lock()
    }

    /// Last RG attenuation in dB reported by `Pre_Gain_Stage`. Returns
    /// `None` when no track is active or the value has not been
    /// published yet (encoded as exactly zero).
    pub(crate) fn rg_attenuation_db(&self) -> Option<f32> {
        let raw = self.rg_attenuation_db_x1000.load(Ordering::Relaxed);
        if raw == 0 {
            None
        } else {
            Some(raw as f32 / 1000.0)
        }
    }

    /// Setter used by the decoder thread (task 18) after computing a
    /// new pre-gain. `db = 0.0` is encoded as the "no attenuation"
    /// sentinel (it is also the value when the demanded gain fit).
    #[allow(dead_code)] // Wired up by run_decoder_thread in task 18.
    pub(crate) fn set_rg_attenuation_db(&self, db: f32) {
        let raw = (db * 1000.0).round() as i32;
        self.rg_attenuation_db_x1000.store(raw, Ordering::Relaxed);
    }

    /// Negotiated device sample rate in Hz, or `None` if no device is
    /// currently open. Read by [`BitPerfectHealth::compute`] in
    /// `state()` to detect a hidden double-resample (R5.3).
    pub(crate) fn device_sample_rate(&self) -> Option<u32> {
        match self.device_sample_rate.load(Ordering::Relaxed) {
            0 => None,
            v => Some(v),
        }
    }

    /// Negotiated device channel count, or `None` if no device is
    /// open. Used by `BitPerfectHealth` to spot an upmix.
    pub(crate) fn device_channels(&self) -> Option<u16> {
        match self.device_channels.load(Ordering::Relaxed) {
            0 => None,
            v => Some(v as u16),
        }
    }

    /// Setter used when negotiating the output stream. Pass `0` for
    /// `sr` and `ch` on stop / on a failed open so the snapshot
    /// reverts to "no device".
    pub(crate) fn set_device_format(&self, sr: u32, ch: u16) {
        self.device_sample_rate.store(sr, Ordering::Relaxed);
        self.device_channels.store(ch as u32, Ordering::Relaxed);
    }

    /// Publish the active upmix layout for the open stream (R9.5).
    /// Pass `None` when the source is played natively (no upmix) — e.g.
    /// on `Stop` / on a failed open so the snapshot reverts to "no
    /// upmix". Written by the WASAPI Exclusive backend at stream open.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn set_upmix(&self, upmix: Option<UpmixInfo>) {
        *self.upmix.lock() = upmix;
    }

    /// Current active upmix layout (R9.5), or `None` when the source is
    /// played natively. Read by `state()` / `snapshot_state` so the
    /// indicator reaches `PlayerState::upmix`.
    pub(crate) fn upmix(&self) -> Option<UpmixInfo> {
        *self.upmix.lock()
    }

    /// Publish whether the current file is degraded (R11.3): set by the
    /// decoder thread at EOF when the number of repaired MP3 frames
    /// exceeds [`DEGRADED_FRAME_THRESHOLD`](crate::backend_symphonia::DEGRADED_FRAME_THRESHOLD).
    /// Reset to `false` on the next `Load`.
    pub(crate) fn set_file_degraded(&self, degraded: bool) {
        self.file_degraded.store(degraded, Ordering::Relaxed);
    }

    /// Whether the current file is flagged as possibly degraded (R11.3).
    /// Read by `state()` / `snapshot_state` so the indicator reaches
    /// `PlayerState::degraded`.
    pub(crate) fn file_degraded(&self) -> bool {
        self.file_degraded.load(Ordering::Relaxed)
    }

    /// Arm or disarm the post-transition prefill gate (R10.1). Called
    /// with `false` when a stream opens (or at the start of a format
    /// transition) so the callback emits silence without counting
    /// underruns, and with `true` by the decoder thread once the ring
    /// has been prefilled to its `prefill_threshold`.
    pub(crate) fn set_ring_ready(&self, ready: bool) {
        self.ring_ready.store(ready, Ordering::Release);
    }

    /// Whether the ring has been prefilled enough for the callback to
    /// consume after a transition (R10.1). While `false`, a starved
    /// callback emits silence without counting an underrun; once
    /// `true`, an empty ring is a genuine underrun (R10.3).
    pub(crate) fn ring_ready(&self) -> bool {
        self.ring_ready.load(Ordering::Acquire)
    }

    /// Decide whether a new [`BitPerfectHealth`] snapshot should be
    /// published as [`EngineEvent::BitPerfectChanged`]: emit only when
    /// the snapshot meaningfully differs from the last one *and* at
    /// least 200 ms have elapsed since the previous emit (R5 debounce
    /// guard against slider drag flooding).
    ///
    /// Returns `true` when the caller should emit; on `true` the
    /// internal "last" snapshot + timestamp are updated atomically so
    /// the next call sees the new state.
    pub(crate) fn should_emit_bit_perfect(&self, candidate: &BitPerfectHealth) -> bool {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(200);

        let mut last = self.last_bit_perfect.lock();
        let mut last_emit = self.last_bit_perfect_emit.lock();

        // First emit (no prior snapshot): always publish so the UI
        // sees an initial state without waiting for a flag flip.
        let same_as_last = matches!(&*last, Some(prev) if prev == candidate);
        if same_as_last {
            return false;
        }
        if let Some(prev_at) = *last_emit {
            if prev_at.elapsed() < DEBOUNCE {
                return false;
            }
        }

        *last = Some(candidate.clone());
        *last_emit = Some(std::time::Instant::now());
        true
    }

    /// Reset the bit-perfect debounce state. Called on `Stop` so the
    /// next track's first snapshot is always emitted (no leftover
    /// timestamp from the previous session can suppress it).
    pub(crate) fn reset_bit_perfect_debounce(&self) {
        *self.last_bit_perfect.lock() = None;
        *self.last_bit_perfect_emit.lock() = None;
    }

    /// Snapshot the chain bypass flags into the packed atomic byte.
    /// Called by the decoder thread on each chunk boundary so
    /// `state()` (called from any thread) can read a coherent view
    /// without crossing the chain boundary.
    pub(crate) fn set_chain_bypass(
        &self,
        eq_bypass: bool,
        convolver_off: bool,
        dither_bypass: bool,
    ) {
        let bits =
            (eq_bypass as u32) | ((convolver_off as u32) << 1) | ((dither_bypass as u32) << 2);
        self.chain_bypass_bits.store(bits, Ordering::Relaxed);
    }

    /// Snapshot of the chain's `eq_bypass`, `convolver_off`,
    /// `dither_bypass` flags. Returns the conservative `(true, true,
    /// true)` triple before the decoder thread has run.
    pub(crate) fn chain_bypass(&self) -> (bool, bool, bool) {
        let bits = self.chain_bypass_bits.load(Ordering::Relaxed);
        (
            (bits & 0b001) != 0,
            (bits & 0b010) != 0,
            (bits & 0b100) != 0,
        )
    }

    /// Stash a new convolver IR for the decoder thread to apply at
    /// the next chunk boundary (R9.2). Empty channels signal an
    /// explicit unload â€” the convolver stage falls back to bypass.
    /// Caller is the worker thread that decoded + resampled the WAV
    /// (typically `Player::load_convolver_ir`).
    pub(crate) fn set_convolver_ir(&self, ir_l: Vec<f32>, ir_r: Vec<f32>) {
        // Cache the per-channel length so `state()` can report
        // latency without crossing the decoder thread boundary.
        let len = ir_l.len().min(ir_r.len()) as u32;
        self.convolver_ir_len.store(len, Ordering::Relaxed);
        *self.pending_ir.lock() = Some((ir_l, ir_r));
        self.pending_ir_version.fetch_add(1, Ordering::Release);
    }

    /// Take the latest pending IR, if any. Called by the decoder
    /// thread on chunk boundaries when `pending_ir_version` has
    /// changed since the previous iteration.
    pub(crate) fn take_pending_ir(&self) -> Option<(Vec<f32>, Vec<f32>)> {
        self.pending_ir.lock().take()
    }

    /// Length of the active convolver IR in taps; zero when none
    /// loaded. Consumed by `commands::get_convolver_status` to
    /// compute the reported latency.
    pub fn convolver_ir_len(&self) -> usize {
        self.convolver_ir_len.load(Ordering::Relaxed) as usize
    }

    // ---- DSD pipeline state (R7.7 / R7.8) ----

    /// Mark the engine as actively rendering DSD. Read by the
    /// orchestrator (`Player`) to gate volume / EQ / pre-gain
    /// commands while DSD is playing.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn set_dsd_active(&self, active: bool, label: Option<&str>) {
        self.is_dsd_active.store(active, Ordering::Release);
        *self.dsd_rate_label.lock() = label.map(|s| s.to_string());
    }

    /// Whether the DSD pipeline is currently active.
    pub fn is_dsd_active(&self) -> bool {
        self.is_dsd_active.load(Ordering::Acquire)
    }

    /// Active DSD rate label (`"DSD64"`, ...) or `None` for PCM /
    /// idle. Surfaced through `PlayerState::dsd_rate_label`
    /// (R7.7).
    pub(crate) fn dsd_rate_label(&self) -> Option<String> {
        self.dsd_rate_label.lock().clone()
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
    host_kind: HostKind,
    _worker: JoinHandle<()>,
}

/// Which cpal host the engine drives.
///
/// The decode → DSP → render machinery is identical for every host;
/// only the device-enumeration host and the reported
/// [`EffectiveOutputMode`] differ. Keeping this on the engine lets the
/// ASIO backend reuse the entire `CpalSharedEngine` worker instead of
/// duplicating it (see [`backend_asio`](crate::backend_asio)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// The platform default host (WASAPI Shared on Windows,
    /// CoreAudio on macOS, ALSA/Pulse on Linux). Routes through the
    /// OS mixer; reports [`EffectiveOutputMode::Shared`].
    System,
    /// The ASIO host (Windows, pro-audio). Bypasses the OS mixer and
    /// reports [`EffectiveOutputMode::Asio`]. Only reachable when the
    /// engine is compiled with the `engine-asio` feature *and* the
    /// Steinberg ASIO SDK was present at build time; otherwise opening
    /// a stream on this host returns [`EngineError::BackendUnavailable`].
    Asio,
}

impl HostKind {
    /// Effective mode this host reports to the UI.
    pub(crate) fn effective_mode(self) -> EffectiveOutputMode {
        match self {
            HostKind::System => EffectiveOutputMode::Shared,
            HostKind::Asio => EffectiveOutputMode::Asio,
        }
    }
}

impl CpalSharedEngine {
    /// Create a new engine on the platform default host. A background
    /// worker thread is spawned immediately and lives for the lifetime
    /// of the engine.
    pub fn new() -> EngineResult<Self> {
        Self::with_host(HostKind::System)
    }

    /// Create an engine bound to a specific cpal host. Used by the
    /// ASIO backend to reuse the entire Shared worker on the ASIO
    /// host. Public within the crate only; external callers use
    /// [`CpalSharedEngine::new`] or the dedicated backend constructors.
    pub(crate) fn with_host(host_kind: HostKind) -> EngineResult<Self> {
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
                    host_kind,
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
            host_kind,
            _worker: worker,
        })
    }

    fn send_cmd(&self, cmd: Command) -> EngineResult<()> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| EngineError::Internal("worker thread is gone".into()))
    }

    /// Activate pre-render mode for the null-test diagnostic
    /// (R11.3, Exclusive branch). The decoder thread routes its
    /// post-DSP chunks to `output_wav` (RIFF, 24-bit, the source's
    /// native SR) instead of the audio device. Mutually exclusive
    /// with normal device playback: returns
    /// [`EngineError::InvalidState`] when a track is already loaded
    /// and the audio callback is active.
    ///
    /// Caller workflow:
    ///   1. `engine.start_pre_render(&source, &output_wav, sr, ch)?;`
    ///   2. `engine.load(&source)?;`
    ///   3. `engine.play()?;`
    ///   4. wait for [`EngineEvent::EndOfTrack`].
    ///   5. `engine.finalise_pre_render()?;`
    pub fn start_pre_render(
        &self,
        output_wav: &Path,
        sample_rate: u32,
        channels: u16,
    ) -> EngineResult<()> {
        if !matches!(
            *self.status.lock(),
            PlaybackStatus::Idle | PlaybackStatus::Stopped
        ) {
            return Err(EngineError::InvalidState(
                "pre-render requires the engine to be idle (no track loaded)",
            ));
        }
        crate::diagnostic::pre_render::open_pre_render(
            &self.shared.pre_render,
            output_wav,
            sample_rate,
            channels,
        )
    }

    /// Close the active pre-render session: flush, patch the RIFF
    /// header sizes, drop the writer. Idempotent — safe to call when
    /// no session is active.
    pub fn finalise_pre_render(&self) -> EngineResult<()> {
        crate::diagnostic::pre_render::finalise_pre_render(&self.shared.pre_render)
    }

    /// Whether a pre-render session is currently in flight.
    pub fn is_pre_render_active(&self) -> bool {
        self.shared.pre_render.is_active()
    }

    /// Used by the orchestration layer to tag the currently loaded track.
    pub fn set_current_track_id(&self, id: Option<String>) {
        *self.current_track.lock() = id;
    }

    /// Pick the output device by its CPAL name. Pass `None` to revert
    /// to the system default. The change applies to the *next* track â€”
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
        let output_mode = self.host_kind.effective_mode();

        let mut state = PlayerState {
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
            rg_attenuation_db: self.shared.rg_attenuation_db(),
            bit_perfect: None,
            dsd_rate_label: self.shared.dsd_rate_label(),
            is_dsd: self.shared.is_dsd_active(),
            // Shared mode plays the device's own layout; upmix flag
            // (R9.5) is a WASAPI-Exclusive concern, always None here.
            upmix: None,
            // Degraded flag (R11.3) set by the decoder thread at EOF
            // when the MP3 frame-repair threshold is exceeded.
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

    fn prepare_next(&self, path: &Path, track_id: Option<String>) -> EngineResult<()> {
        Self::prepare_next(self, path, track_id)
    }

    fn clear_pending_next(&self) -> EngineResult<()> {
        Self::clear_pending_next(self)
    }

    fn set_crossfade_ms(&self, ms: u32) {
        self.shared.set_crossfade_ms(ms);
    }

    fn underrun_count(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed) as u64
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
    /// Which cpal host this worker drives. `System` (default host) or
    /// `Asio`. Selected once at engine construction and constant for
    /// the worker's lifetime.
    host_kind: HostKind,
}

/// Active stream + decoder pair for one loaded track.
struct ActiveStream {
    stream: cpal::Stream,
    decoder_handle: JoinHandle<()>,
    decoder_alive: Arc<AtomicBool>,
}

fn run_worker(ctx: WorkerCtx) {
    let mut active: Option<ActiveStream> = None;

    loop {
        // Poll for commands with a short timeout so we can also
        // service device-recovery requests raised asynchronously by
        // the CPAL stream-error callback (e.g. a call app switching
        // the OS default output). A blocking `recv()` would leave a
        // dead stream wedged until the next user command.
        match ctx.cmd_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(cmd) => {
                handle_command(&ctx, &mut active, cmd);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // No command pending — check whether the device needs
                // to be recovered.
                maybe_recover_device(&ctx, &mut active);
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    if let Some(prev) = active.take() {
        stop_active(prev);
    }
}

/// Handle a single worker command. The body is the original
/// `run_worker` match arm set, unchanged except for recording the
/// source path on `Load` / clearing it on `Stop` (used by device
/// recovery) and resetting the `recover_device` latch where stale.
fn handle_command(ctx: &WorkerCtx, active: &mut Option<ActiveStream>, cmd: Command) {
    match cmd {
        Command::Load(path) => {
            if let Some(prev) = active.take() {
                stop_active(prev);
            }
            ctx.shared.pending_seek_ms.store(-1, Ordering::Release);
            ctx.shared.drain_ring.store(false, Ordering::Release);
            // A device-loss flag from a previous track is stale now;
            // a fresh Load supersedes any pending recovery.
            ctx.shared.recover_device.store(false, Ordering::Release);
            // A fresh Load invalidates any prepared next track.
            *ctx.shared.pending_next.lock() = None;
            // A fresh Load clears any degraded flag from the previous
            // file (R11.3).
            ctx.shared.set_file_degraded(false);
            set_status(ctx, PlaybackStatus::Loading);

            match start_playback(ctx, &path) {
                Ok(s) => {
                    // Remember the source so a device-loss event can
                    // rebuild the stream in place.
                    ctx.shared.set_current_source_path(Some(path.clone()));
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
        Command::PrepareNext { path, track_id } => {
            // Only prepare while something is actively playing â€”
            // otherwise the orchestrator should call Load.
            if active.is_none() {
                tracing::debug!(
                    target: "qobee::engine",
                    "prepare_next with no active track; ignoring"
                );
                return;
            }
            let cur_sr = ctx.shared.sample_rate.load(Ordering::Relaxed);
            let cur_ch = ctx.shared.channels.load(Ordering::Relaxed) as u16;
            let cur_bd = ctx.shared.bit_depth.load(Ordering::Relaxed) as u8;
            let cur_is_dsd = ctx.shared.is_dsd_active();
            let cur_dsd_rate = crate::DsdRate::from_hz_bits(cur_sr, cur_bd);
            match SymphoniaDecoder::open_uri(path.to_string_lossy().as_ref()) {
                Ok(decoder) => {
                    let fmt = decoder.format();
                    let next_dsd_rate =
                        crate::DsdRate::from_hz_bits(fmt.sample_rate, fmt.bit_depth.unwrap_or(0));
                    let next_is_dsd = next_dsd_rate.is_some();
                    // R7 acceptance #5: gapless is impossible across
                    // DSD/PCM boundaries and across DSD rate changes
                    // (DSD64 → DSD128, …).
                    let dsd_pcm_change = cur_is_dsd != next_is_dsd;
                    let dsd_rate_change =
                        cur_is_dsd && next_is_dsd && cur_dsd_rate != next_dsd_rate;
                    if !dsd_pcm_change
                        && !dsd_rate_change
                        && fmt.sample_rate == cur_sr
                        && fmt.channels == cur_ch
                    {
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
                        tracing::info!(
                            target: "qobee::engine",
                            cur_sr,
                            cur_ch,
                            cur_is_dsd,
                            next_sr = fmt.sample_rate,
                            next_ch = fmt.channels,
                            next_is_dsd,
                            "format change: gapless impossible"
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
                // Anti-click: start the stream first, then ramp the
                // fade gain up from silence. The callback's
                // `fade_gain` starts at 0 for a fresh stream, so the
                // first ~12 ms fade in cleanly.
                ctx.shared.set_fade_target(1.0);
                if let Err(e) = s.stream.play() {
                    record_stream_error(ctx, &e.to_string());
                } else {
                    set_status(ctx, PlaybackStatus::Playing);
                }
            } else {
                // Play before Load: just log; never surface to the UI
                // as an error. The orchestrator can issue this
                // legitimately during a fallback or a stale event.
                tracing::debug!(
                    target: "qobee::engine",
                    "play with no active track; ignoring"
                );
            }
        }
        Command::Pause => {
            if let Some(s) = active.as_ref() {
                // Anti-click: ramp the output down to silence before
                // telling the device to pause, so it never sees an
                // abrupt amplitude step. The callback converges the
                // fade in ~12 ms; we wait a touch longer than that
                // before stopping the stream.
                ctx.shared.set_fade_target(0.0);
                thread::sleep(Duration::from_millis(16));
                ctx.shared.paused.store(true, Ordering::Release);
                let _ = s.stream.pause();
                set_status(ctx, PlaybackStatus::Paused);
            }
        }
        Command::Resume => {
            if let Some(s) = active.as_ref() {
                ctx.shared.paused.store(false, Ordering::Release);
                ctx.shared.set_fade_target(1.0);
                if let Err(e) = s.stream.play() {
                    record_stream_error(ctx, &e.to_string());
                } else {
                    set_status(ctx, PlaybackStatus::Playing);
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
            ctx.shared.set_device_format(0, 0);
            ctx.shared.reset_bit_perfect_debounce();
            ctx.shared.pending_seek_ms.store(-1, Ordering::Release);
            ctx.shared.drain_ring.store(false, Ordering::Release);
            ctx.shared.recover_device.store(false, Ordering::Release);
            ctx.shared.set_current_source_path(None);
            *ctx.shared.pending_next.lock() = None;
            set_status(ctx, PlaybackStatus::Stopped);
        }
        Command::Seek(secs) => {
            if active.is_some() {
                let secs = secs.max(0.0);
                let ms = (secs * 1000.0) as i32;
                // Tell the audio callback to drain on the next tick so
                // we don't keep playing stale samples until the
                // decoder catches up.
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

/// Rebuild the output stream after a device-loss event (the CPAL
/// error callback raised [`Shared::recover_device`]). We reopen the
/// track from its source path, seek back to the last known
/// position, and restore the prior play/pause state — all without
/// involving the orchestrator, so the queue cursor and history are
/// untouched. Best-effort: on failure we surface a soft error and
/// leave the latch cleared so we don't spin.
fn maybe_recover_device(ctx: &WorkerCtx, active: &mut Option<ActiveStream>) {
    if !ctx.shared.recover_device.swap(false, Ordering::AcqRel) {
        return;
    }
    // Nothing to recover if no track is loaded.
    let Some(path) = ctx.shared.current_source_path() else {
        return;
    };

    let was_playing = !ctx.shared.paused.load(Ordering::Acquire);
    let resume_ms = ctx.shared.position_ms.load(Ordering::Relaxed);

    tracing::info!(
        target: "qobee::engine",
        resume_ms,
        was_playing,
        "recovering audio output on the current default device"
    );

    // Drop the dead stream first so the device handle is released
    // before we try to reopen (some drivers refuse a second open
    // while the broken stream is still alive).
    if let Some(prev) = active.take() {
        stop_active(prev);
    }

    // Give the OS a beat to finish switching the default device
    // (call apps flip the default output then back; reopening too
    // eagerly can land on the transient comm device).
    thread::sleep(Duration::from_millis(150));

    match start_playback(ctx, &path) {
        Ok(s) => {
            *active = Some(s);
            // Seek back to where we were. `start_playback` resets
            // position to 0, so push the saved offset through the
            // normal seek path (drains the ring + reseeks decoder).
            if resume_ms > 0 {
                let secs = resume_ms as f64 / 1000.0;
                ctx.shared.drain_ring.store(true, Ordering::Release);
                ctx.shared
                    .pending_seek_ms
                    .store(resume_ms as i32, Ordering::Release);
                ctx.shared.position_ms.store(resume_ms, Ordering::Relaxed);
                let _ = ctx.event_tx.try_send(EngineEvent::Position {
                    position_seconds: secs,
                });
            }
            // Restore transport state.
            if was_playing {
                ctx.shared.paused.store(false, Ordering::Release);
                // Fresh stream: its callback `fade_gain` starts at 0,
                // so aim the fade at unity for a clean fade-in on the
                // recovered device.
                ctx.shared.set_fade_target(1.0);
                if let Some(s) = active.as_ref() {
                    if let Err(e) = s.stream.play() {
                        record_stream_error(ctx, &e.to_string());
                    } else {
                        set_status(ctx, PlaybackStatus::Playing);
                    }
                }
            } else {
                ctx.shared.paused.store(true, Ordering::Release);
                set_status(ctx, PlaybackStatus::Paused);
            }
            tracing::info!(target: "qobee::engine", "audio output recovered");
        }
        Err(e) => {
            // Could not reopen (no device at all, or the new default
            // refuses every format). Surface a soft error; the next
            // device-change event or a user action can retry.
            let msg = format!("audio output recovery failed: {e}");
            tracing::warn!(target: "qobee::engine", error = %e, "device recovery failed");
            *ctx.last_error.lock() = Some(msg.clone());
            set_status(ctx, PlaybackStatus::Errored);
            let _ = ctx.event_tx.try_send(EngineEvent::Error { message: msg });
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
    let output_mode = ctx.host_kind.effective_mode();

    let mut state = PlayerState {
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
        rg_attenuation_db: ctx.shared.rg_attenuation_db(),
        bit_perfect: None,
        dsd_rate_label: ctx.shared.dsd_rate_label(),
        is_dsd: ctx.shared.is_dsd_active(),
        // Shared mode: no upmix flag (R9.5 is WASAPI-Exclusive only).
        upmix: None,
        // Degraded flag (R11.3) read from the shared atomic.
        degraded: ctx.shared.file_degraded(),
        error,
    };
    state.bit_perfect = build_bit_perfect_health(&ctx.shared, &state);
    state
}

fn stop_active(active: ActiveStream) {
    drop(active.stream); // closes CPAL callback first
    active.decoder_alive.store(false, Ordering::Release);
    // Decoder thread will exit on its own once it sees the flag and the
    // ring buffer producer is gone; we don't block on it.
    let _ = active.decoder_handle;
}

/// Resolve the cpal host for a [`HostKind`].
///
/// `System` always succeeds (it is `cpal::default_host()`). `Asio`
/// only resolves when the engine was compiled with the `engine-asio`
/// feature, which in turn requires the Steinberg ASIO SDK to have been
/// present at build time (cpal pulls it through its own `asio`
/// feature). When the feature is off, the function returns
/// [`EngineError::BackendUnavailable`] with a localised, actionable
/// message so the orchestrator can fall back to Shared and the UI can
/// explain why.
fn resolve_host(kind: HostKind) -> EngineResult<cpal::Host> {
    match kind {
        HostKind::System => Ok(cpal::default_host()),
        HostKind::Asio => {
            #[cfg(all(target_os = "windows", feature = "engine-asio"))]
            {
                cpal::host_from_id(cpal::HostId::Asio).map_err(|e| {
                    EngineError::BackendUnavailable(format!(
                        "ASIO host indisponible : {e}. Vérifiez qu'un pilote ASIO est installé."
                    ))
                })
            }
            #[cfg(not(all(target_os = "windows", feature = "engine-asio")))]
            {
                Err(EngineError::BackendUnavailable(
                    "Le mode ASIO n'est pas inclus dans cette build \
                     (fonctionnalité « engine-asio » désactivée)."
                        .to_string(),
                ))
            }
        }
    }
}

/// Set up a CPAL stream and a decoder thread to feed it.
fn start_playback(ctx: &WorkerCtx, path: &Path) -> EngineResult<ActiveStream> {
    let decoder = SymphoniaDecoder::open_uri(path.to_string_lossy().as_ref())?;
    let format = decoder.format();

    ctx.shared
        .sample_rate
        .store(format.sample_rate, Ordering::Relaxed);
    ctx.shared
        .channels
        .store(format.channels as u32, Ordering::Relaxed);
    ctx.shared.bit_depth.store(
        format.bit_depth.map(|b| b as u32).unwrap_or(0),
        Ordering::Relaxed,
    );
    ctx.shared.underruns.store(0, Ordering::Relaxed);
    let dur_ms = (decoder.duration_seconds() * 1000.0) as u32;
    ctx.shared.duration_ms.store(dur_ms, Ordering::Relaxed);
    ctx.shared.position_ms.store(0, Ordering::Relaxed);

    let host = resolve_host(ctx.host_kind)?;
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

    // Publish the negotiated device format so `state()` can build a
    // [`BitPerfectHealth`] snapshot from a single atomic read.
    ctx.shared
        .set_device_format(device_sample_rate, device_channels);
    ctx.shared.reset_bit_perfect_debounce();

    // R10.1/R10.2 — size the SPSC ring for this format transition. The
    // pure `RingPlan` policy widens the capacity proportionally to the
    // throughput ratio (`device_sr / src_sr * dst_ch / src_ch`, clamped
    // to `[1, 4]`) so a higher device rate or an upmix gets enough slack
    // to absorb renegotiation latency without underrunning on size.
    //
    // Design deviation (documented): the design text says to recreate
    // the `rtrb` ring *inside* `run_decoder_thread` when the target
    // capacity grows. The ring is created here, before the decoder
    // thread is spawned and before the callback takes ownership of the
    // `Consumer`. Re-creating the ring mid-thread would require tearing
    // down the live stream to re-plumb the `Producer`/`Consumer` pair
    // (the callback already holds the consumer), which is exactly the
    // glitch R10 sets out to avoid. Because `start_playback` runs on
    // every `Load` *and* on every device-recovery / format transition
    // (the worker calls it again with the new device), computing the
    // plan here sizes the ring for the upcoming transition — achieving
    // R10.2 (capacity sized for the new format) without unsafe
    // mid-thread re-plumbing. The `ring_ready` gate below re-prefills on
    // every (re)open, which covers the "resume from Paused" / gapless
    // transition cases too.
    let ring_plan = crate::RingPlan::compute(
        RING_CAPACITY_SAMPLES,
        format.sample_rate,
        device_sample_rate,
        format.channels,
        device_channels,
    );

    // Arm the prefill gate: the callback emits silence (without counting
    // underruns) until the decoder thread has pushed `prefill_threshold`
    // samples and flips `ring_ready` to `true` (R10.1).
    ctx.shared.set_ring_ready(false);

    let (producer, consumer) = RingBuffer::<f32>::new(ring_plan.capacity);

    let decoder_alive = Arc::new(AtomicBool::new(true));
    let decoder_alive_clone = Arc::clone(&decoder_alive);
    let shared_clone = Arc::clone(&ctx.shared);
    let event_tx_clone = ctx.event_tx.clone();
    let status_clone = Arc::clone(&ctx.status);
    let src_sample_rate = format.sample_rate;
    let src_channels = format.channels;
    let effective_mode = ctx.host_kind.effective_mode();
    let prefill_threshold = ring_plan.prefill_threshold;

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
                effective_mode,
                prefill_threshold,
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

/// Build the rubato `SincInterpolationParameters` block for the
/// requested [`ResamplerQuality`]. The `Best` preset preserves the
/// historical configuration (`sinc_len = 256`, `oversampling = 256`,
/// cubic interpolation, BlackmanHarrisÂ²) and is the default that
/// Properties 15â€“16 measure at â‰¥ 140 dB SNR. The `Standard` preset
/// drops both `sinc_len` and `oversampling_factor` to 128 and uses
/// linear interpolation, trading the last few dB of SNR for ~1/4
/// the CPU; Properties 15â€“16 require it to stay â‰¥ 120 dB SNR.
///
/// The preset applies to the *next* track because the resampler is
/// instantiated once per `run_decoder_thread` call (R8.2). Mid-stream
/// changes to `audio.resampler_quality` therefore take effect at the
/// following `Load`.
///
/// Exposed publicly so the integration tests in
/// `tests/properties/resampler.rs` can reuse the exact same params
/// the engine ships with.
pub fn sinc_params_for(quality: ResamplerQuality) -> rubato::SincInterpolationParameters {
    use rubato::{SincInterpolationParameters, SincInterpolationType, WindowFunction};
    match quality {
        ResamplerQuality::Standard => SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: 0.95,
            oversampling_factor: 128,
            interpolation: SincInterpolationType::Linear,
            window: WindowFunction::BlackmanHarris2,
        },
        ResamplerQuality::Best => SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            oversampling_factor: 256,
            interpolation: SincInterpolationType::Cubic,
            window: WindowFunction::BlackmanHarris2,
        },
    }
}

#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
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
    prefill_threshold: usize,
) {
    use rubato::{Resampler, SincFixedIn};

    let need_resample = src_sample_rate != dst_sample_rate;
    let n_ch = channels as usize;

    // Per-track sinc resampler (windowed-sinc, asynchronous, fixed-input).
    // Higher quality than FFT for music: cleaner transients, less
    // pre-ringing. CPU cost is negligible on a modern PC.
    //
    // The `audio.resampler_quality` setting is read once at decoder
    // start (R8.2 â€” the preset applies to the *next* track, not
    // mid-stream): instantiating a `SincFixedIn` reallocates the
    // entire FIR table and would tear an in-flight chunk if swapped
    // hot.
    let chunk_size_in: usize = 1024;
    let quality = shared.audio_settings().resampler_quality;
    let sinc_params = sinc_params_for(quality);
    let sinc_len_log = sinc_params.sinc_len;
    let oversampling_log = sinc_params.oversampling_factor;
    let mut resampler: Option<SincFixedIn<f32>> = if need_resample {
        let ratio = dst_sample_rate as f64 / src_sample_rate as f64;
        match SincFixedIn::<f32>::new(ratio, 1.1, sinc_params, chunk_size_in, n_ch) {
            Ok(r) => {
                tracing::info!(
                    target: "qobee::engine",
                    quality = ?quality,
                    sinc_len = sinc_len_log,
                    oversampling = oversampling_log,
                    "resampler params"
                );
                tracing::info!(
                    target: "qobee::engine",
                    src_sr = src_sample_rate,
                    dst_sr = dst_sample_rate,
                    channels = n_ch,
                    sinc_len = sinc_len_log,
                    oversampling = oversampling_log,
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

    // Crossfade engine. Off by default; when `audio` crossfade is
    // enabled (`Shared::crossfade_ms() > 0`) and a compatible next
    // track is prepared, the loop below fades the outgoing tail into
    // the incoming head over the window. See `maybe_crossfade`.
    let mut output_buffer: Vec<Vec<f32>> = if let Some(ref r) = resampler {
        r.output_buffer_allocate(true)
    } else {
        Vec::new()
    };
    // Pending planar samples per channel, fed from decoded packets.
    let mut pending_planar: Vec<Vec<f32>> = vec![Vec::with_capacity(chunk_size_in * 4); n_ch];

    // The 10-band EQ now lives inside `PcmChain` in its canonical
    // slot (pre_gain → balance → crossfeed → **eq** → convolver →
    // limiter → dither). EQ gains are not part of `AudioSettings`;
    // they have their own versioned slot on `Shared`, so we forward
    // them into the chain whenever `eq_version` moves. `u32::MAX`
    // forces an initial sync on the first loop iteration below.
    let mut eq_seen_version: u32 = u32::MAX;

    let sync_eq = |chain: &mut crate::dsp::PcmChain, seen: &mut u32, shared: &Shared| {
        let cur = shared.eq_version.load(Ordering::Acquire);
        if cur != *seen {
            let gains = shared.eq_gains_db.lock().clone();
            chain.set_eq_gains_db(&gains);
            *seen = cur;
        }
    };

    // Assemble the PCM DSP chain. Every stage — pre-gain, balance,
    // crossfeed, EQ, convolver, limiter, dither — now lives in the
    // chain in its canonical order, so `chain.process` is the single
    // DSP entry point. EQ gains are forwarded into the chain's EQ
    // slot via `sync_eq` (see above) whenever the user moves a slider.
    let chain_sample_rate = if need_resample {
        dst_sample_rate
    } else {
        src_sample_rate
    };
    let mut pcm_chain = {
        let initial_settings = shared.audio_settings();
        let mut chain = crate::dsp::PcmChain::new(&initial_settings, chain_sample_rate, channels);
        let initial_bit_depth = match shared.bit_depth.load(Ordering::Relaxed) {
            0 => None,
            v => Some(v as u8),
        };
        chain.set_dither_output_bits(initial_bit_depth);
        // Volume is applied by the audio callback's gain ramp via
        // `Shared::audible_gain`, which now reads the configured
        // `volume_curve` + `volume_floor_db` (task 22). Force the
        // pre-gain stage's slider to unity so the user's slider
        // position is never multiplied twice. The chain still applies
        // ReplayGain + true-peak protection + mute-exact at slider = 0
        // through `set_pre_gain_context`.
        let mut ctx = shared.pre_gain_context();
        ctx.slider = 1.0;
        chain.set_pre_gain_context(ctx);
        shared.set_rg_attenuation_db(chain.pre_gain_attenuation_db());
        chain
    };
    let mut last_pre_gain_ctx_version = shared.pre_gain_context_version.load(Ordering::Acquire);
    let mut last_seen_bit_depth: u32 = shared.bit_depth.load(Ordering::Relaxed);
    let mut last_pending_ir_version = shared.pending_ir_version.load(Ordering::Acquire);

    // Underrun watcher: every ~1s we look at the audio-callback's
    // underrun counter and emit a single throttled warning if it grew.
    // The audio callback never blocks here; this is a passive observer.
    let mut last_underrun_count: u32 = 0;
    let mut last_underrun_check = std::time::Instant::now();

    // R10.1 — post-transition prefill gate. `start_playback` armed
    // `Shared::ring_ready` to `false`; while it stays `false` the audio
    // callback emits silence *without* counting underruns. We flip it to
    // `true` once the ring holds at least `prefill_threshold` interleaved
    // samples (computed by `RingPlan` for this transition), or at EOF for
    // a track shorter than the threshold so it still plays out. The ring
    // is empty when the decoder thread starts, so `producer.slots()` here
    // is the usable capacity.
    let ring_capacity = producer.slots();
    let mut ring_prefilled = prefill_threshold == 0;
    if ring_prefilled {
        // Degenerate threshold (tiny ring): nothing to wait for.
        shared.set_ring_ready(true);
    }

    loop {
        if !alive.load(Ordering::Acquire) {
            return;
        }

        // Flip the prefill gate once the ring has buffered enough to
        // absorb renegotiation latency (R10.1). `filled = capacity -
        // free slots`. Cheap: a single relaxed atomic read on the SPSC
        // producer; checked once per chunk boundary.
        if !ring_prefilled {
            let filled = ring_capacity.saturating_sub(producer.slots());
            if filled >= prefill_threshold {
                shared.set_ring_ready(true);
                ring_prefilled = true;
            }
        }

        // Re-check the published `AudioSettings` snapshot at every
        // chunk boundary. A single atomic load + version compare; we
        // only observe a change at chunk boundaries so DSP rebuilds
        // never tear an in-flight chunk.
        let cur_settings = shared.audio_settings();
        if cur_settings.version != pcm_chain.settings_version() {
            pcm_chain.reconfigure(&cur_settings, chain_sample_rate, channels);
            tracing::trace!(
                target: "qobee::engine",
                version = cur_settings.version,
                "PcmChain reconfigured from new AudioSettings snapshot"
            );
        }

        // Publish the chain's bypass triple so `state()` (called
        // from any thread) can build a coherent BitPerfectHealth
        // snapshot, and emit a `BitPerfectChanged` event when one of
        // the flags has actually moved (debounced 200 ms inside
        // `should_emit_bit_perfect`).
        let bypass = pcm_chain.bypass_snapshot();
        shared.set_chain_bypass(bypass.eq_bypass, bypass.convolver_off, bypass.dither_bypass);

        // Build a transient `PlayerState` carrying just the inputs
        // `BitPerfectHealth::compute` needs. We do not push this
        // through the event channel (the existing `StateChanged`
        // path already covers that); only the dedicated bit-perfect
        // event is debounced separately.
        let bp_state = PlayerState {
            status: *status.lock(),
            current_track_id: None,
            position_seconds: 0.0,
            duration_seconds: 0.0,
            volume: shared.volume(),
            output_mode: effective_mode,
            sample_rate: match shared.sample_rate.load(Ordering::Relaxed) {
                0 => None,
                v => Some(v),
            },
            bit_depth: match shared.bit_depth.load(Ordering::Relaxed) {
                0 => None,
                v => Some(v as u8),
            },
            channels: match shared.channels.load(Ordering::Relaxed) {
                0 => None,
                v => Some(v as u16),
            },
            is_bit_perfect: false,
            rg_attenuation_db: shared.rg_attenuation_db(),
            bit_perfect: None,
            dsd_rate_label: None,
            is_dsd: shared.is_dsd_active(),
            upmix: None,
            degraded: false,
            error: None,
        };
        if let Some(health) = build_bit_perfect_health(&shared, &bp_state) {
            if shared.should_emit_bit_perfect(&health) {
                let _ = event_tx.try_send(EngineEvent::BitPerfectChanged { health });
            }
        }

        // Pick up a new pre-gain context (RG dB/peak from the loaded
        // track + slider) when the orchestrator publishes one. Force
        // the slider to unity â€” see `pcm_chain` construction comment.
        let cur_ctx_v = shared.pre_gain_context_version.load(Ordering::Acquire);
        if cur_ctx_v != last_pre_gain_ctx_version {
            let mut ctx = shared.pre_gain_context();
            ctx.slider = 1.0;
            pcm_chain.set_pre_gain_context(ctx);
            shared.set_rg_attenuation_db(pcm_chain.pre_gain_attenuation_db());
            last_pre_gain_ctx_version = cur_ctx_v;
        }

        // Forward the negotiated output bit-depth to the dither stage
        // when it changes. CPAL Shared does not renegotiate mid-track
        // today, but the read is cheap and keeps the stage in sync if
        // a future backend ever pushes a new value.
        let cur_bit_depth_raw = shared.bit_depth.load(Ordering::Relaxed);
        if cur_bit_depth_raw != last_seen_bit_depth {
            let bits = if cur_bit_depth_raw == 0 {
                None
            } else {
                Some(cur_bit_depth_raw as u8)
            };
            pcm_chain.set_dither_output_bits(bits);
            last_seen_bit_depth = cur_bit_depth_raw;
        }

        // Pick up a freshly-loaded convolver IR (R9.2). The worker
        // thread that decoded the WAV stashed it on `Shared` and
        // bumped `pending_ir_version`; we take it once and forward it
        // to the chain. Empty channels signal an explicit unload.
        let cur_ir_v = shared.pending_ir_version.load(Ordering::Acquire);
        if cur_ir_v != last_pending_ir_version {
            if let Some((ir_l, ir_r)) = shared.take_pending_ir() {
                pcm_chain.set_convolver_ir(ir_l, ir_r);
            }
            last_pending_ir_version = cur_ir_v;
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
            // `pcm_chain.reset()` clears every stage's state including
            // the EQ delay lines (the EQ is a chain stage now).
            pcm_chain.reset();
            shared.pending_seek_ms.store(-1, Ordering::Release);
            shared.drain_ring.store(false, Ordering::Release);
        }

        // If the ring is full, sleep briefly and retry. The audio
        // callback will drain it.
        if producer.slots() == 0 {
            thread::sleep(Duration::from_millis(5));
            continue;
        }

        // ----------------------------------------------------------
        // Crossfade trigger (opt-in; default off).
        //
        // When `Shared::crossfade_ms() > 0`, the current track is
        // within the fade window of its end, and a format-compatible
        // next track is prepared, blend the outgoing tail into the
        // incoming head over the window (equal-power curve) instead
        // of the instant gapless swap. The steady-state decode path
        // below is left untouched for the common no-crossfade case.
        //
        // Skipped while a null-test pre-render or a DSD stream is
        // active (both use dedicated paths).
        let xf_ms = shared.crossfade_ms();
        if xf_ms > 0 && !shared.pre_render.is_active() && !shared.is_dsd_active() {
            let dur = decoder.duration_seconds();
            let pos = shared.position_ms.load(Ordering::Relaxed) as f64 / 1000.0;
            // Cap the window so very short tracks still play most of
            // their length before fading.
            let xf_secs = (xf_ms as f64 / 1000.0).min(dur * 0.45);
            if dur > 1.0 && xf_secs > 0.05 && pos > 0.0 && pos >= dur - xf_secs {
                // Peek (don't take) so an incompatible prepared track
                // is left in place for the gapless / EOT fallback.
                let compatible = {
                    let guard = shared.pending_next.lock();
                    guard
                        .as_ref()
                        .map(|p| p.sample_rate == src_sample_rate && p.channels == channels)
                        .unwrap_or(false)
                };
                if compatible {
                    // Build the incoming track's own resampler (same
                    // params as the outgoing one). If it can't be
                    // built we skip crossfade and let the normal EOT
                    // path handle the transition.
                    let in_resampler: Option<SincFixedIn<f32>> = if need_resample {
                        let ratio = dst_sample_rate as f64 / src_sample_rate as f64;
                        SincFixedIn::<f32>::new(
                            ratio,
                            1.1,
                            sinc_params_for(quality),
                            chunk_size_in,
                            n_ch,
                        )
                        .ok()
                    } else {
                        None
                    };

                    if !need_resample || in_resampler.is_some() {
                        let prepared = shared.pending_next.lock().take();
                        if let Some(prepared) = prepared {
                            let PendingNext {
                                decoder: in_decoder,
                                track_id: in_track_id,
                                sample_rate: in_sr,
                                channels: in_ch,
                                bit_depth: in_bd,
                                duration_seconds: in_dur,
                            } = prepared;

                            // Wrap the outgoing pipeline.
                            let mut out_stream = crate::crossfade::DecodeStream::from_parts(
                                decoder,
                                resampler.take(),
                                std::mem::take(&mut output_buffer),
                                std::mem::take(&mut pending_planar),
                                chunk_size_in,
                                n_ch,
                            );
                            // Build the incoming pipeline.
                            let in_output_buffer = in_resampler
                                .as_ref()
                                .map(|r| r.output_buffer_allocate(true))
                                .unwrap_or_default();
                            let in_pending: Vec<Vec<f32>> =
                                vec![Vec::with_capacity(chunk_size_in * 4); n_ch];
                            let mut in_stream = crate::crossfade::DecodeStream::from_parts(
                                in_decoder,
                                in_resampler,
                                in_output_buffer,
                                in_pending,
                                chunk_size_in,
                                n_ch,
                            );

                            let xf_frames = ((xf_secs * chain_sample_rate as f64) as usize).max(1);
                            let block = 1024usize;
                            let mut done = 0usize;
                            let mut out_buf: Vec<f32> = Vec::with_capacity(block * n_ch);
                            let mut in_buf: Vec<f32> = Vec::with_capacity(block * n_ch);

                            tracing::info!(
                                target: "qobee::engine",
                                xf_ms,
                                xf_frames,
                                "crossfade started"
                            );

                            while done < xf_frames {
                                if !alive.load(Ordering::Acquire) {
                                    return;
                                }
                                let this = block.min(xf_frames - done);
                                out_buf.clear();
                                in_buf.clear();
                                out_stream.pull(this, &mut out_buf);
                                in_stream.pull(this, &mut in_buf);

                                let mut mixed: Vec<f32> = Vec::with_capacity(this * n_ch);
                                for f in 0..this {
                                    let t = (done + f) as f32 / xf_frames as f32;
                                    let (og, ig) = crate::crossfade::equal_power(t);
                                    for c in 0..n_ch {
                                        let idx = f * n_ch + c;
                                        mixed.push(out_buf[idx] * og + in_buf[idx] * ig);
                                    }
                                }
                                sync_eq(&mut pcm_chain, &mut eq_seen_version, &shared);
                                pcm_chain.process(&mut mixed);
                                dispatch_post_dsp(&mixed, &mut producer, &alive, &shared);
                                done += this;
                            }

                            // The fade is over. Hand the incoming
                            // pipeline back to the main loop locals and
                            // flush whatever it already decoded past
                            // the fade window.
                            let (in_dec, in_rs, in_ob, in_pp, leftover) = in_stream.into_parts();
                            decoder = in_dec;
                            resampler = in_rs;
                            output_buffer = in_ob;
                            pending_planar = in_pp;
                            drop(out_stream); // releases the outgoing decoder

                            // Advance the queue + update Now Playing.
                            shared
                                .duration_ms
                                .store((in_dur * 1000.0) as u32, Ordering::Relaxed);
                            // The incoming track has already played the
                            // fade window.
                            shared
                                .position_ms
                                .store((xf_secs * 1000.0) as u32, Ordering::Relaxed);
                            shared
                                .bit_depth
                                .store(in_bd.map(|b| b as u32).unwrap_or(0), Ordering::Relaxed);
                            let _ = event_tx.try_send(EngineEvent::GaplessTransition);
                            let mut new_state = PlayerState {
                                status: *status.lock(),
                                current_track_id: in_track_id,
                                position_seconds: xf_secs,
                                duration_seconds: in_dur,
                                volume: shared.volume(),
                                output_mode: effective_mode,
                                sample_rate: Some(in_sr),
                                bit_depth: in_bd,
                                channels: Some(in_ch),
                                is_bit_perfect: false,
                                rg_attenuation_db: shared.rg_attenuation_db(),
                                bit_perfect: None,
                                dsd_rate_label: None,
                                is_dsd: shared.is_dsd_active(),
                                upmix: None,
                                degraded: false,
                                error: None,
                            };
                            new_state.bit_perfect = build_bit_perfect_health(&shared, &new_state);
                            let _ =
                                event_tx.try_send(EngineEvent::StateChanged { state: new_state });

                            if !leftover.is_empty() {
                                let mut lo = leftover;
                                sync_eq(&mut pcm_chain, &mut eq_seen_version, &shared);
                                pcm_chain.process(&mut lo);
                                dispatch_post_dsp(&lo, &mut producer, &alive, &shared);
                            }

                            tracing::info!(target: "qobee::engine", "crossfade completed");
                            continue;
                        }
                    }
                }
            }
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
                    position_ms_with_latency(
                        packet.timestamp_seconds,
                        pcm_chain.limiter_lookahead_samples(),
                        chain_sample_rate,
                    ),
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
                                // run it through the PcmChain in its
                                // canonical order (pre-gain → balance →
                                // crossfeed → EQ → convolver → limiter
                                // → dither). EQ gains are synced into
                                // the chain's EQ slot just before.
                                let mut interleaved: Vec<f32> =
                                    Vec::with_capacity(out_frames * n_ch);
                                for f in 0..out_frames {
                                    for c in 0..n_ch {
                                        interleaved.push(output_buffer[c][f]);
                                    }
                                }
                                sync_eq(&mut pcm_chain, &mut eq_seen_version, &shared);
                                pcm_chain.process(&mut interleaved);
                                dispatch_post_dsp(&interleaved, &mut producer, &alive, &shared);
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
                    // Same rate: legacy EQ + PcmChain (pre-gain â†’
                    // balance â†’ crossfeed â†’ noop EQ â†’ noop convolver
                    // â†’ limiter â†’ dither). The chain's pre-gain stage
                    // owns the ReplayGain multiply that used to live
                    // inline here.
                    let mut buf = samples;
                    sync_eq(&mut pcm_chain, &mut eq_seen_version, &shared);
                    pcm_chain.process(&mut buf);
                    dispatch_post_dsp(&buf, &mut producer, &alive, &shared);
                }
            }
            Ok(None) => {
                // End of stream. If we never reached the prefill
                // threshold (a track shorter than ~half the ring), make
                // sure the callback is allowed to consume what we did
                // buffer instead of emitting silence forever (R10.1).
                if !ring_prefilled {
                    shared.set_ring_ready(true);
                    ring_prefilled = true;
                }
                // Determine whether the file that just
                // finished exceeded the MP3 frame-repair threshold
                // (R11.3) before we potentially swap in the next
                // decoder.
                let was_degraded =
                    decoder.repaired_frames() > crate::backend_symphonia::DEGRADED_FRAME_THRESHOLD;
                // Publish the degraded status of the file that just
                // finished (R11.3); read by `state()` / `snapshot_state`.
                shared.set_file_degraded(was_degraded);
                // If a compatible next track has been
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
                        // The incoming track starts fresh: clear the
                        // degraded flag from the file that just ended
                        // (R11.3). The new decoder will set it again at
                        // its own EOF if it exceeds the threshold.
                        shared.set_file_degraded(false);

                        // Update the shared track metadata atomically.
                        shared
                            .duration_ms
                            .store((new_dur * 1000.0) as u32, Ordering::Relaxed);
                        shared.position_ms.store(0, Ordering::Relaxed);
                        shared
                            .bit_depth
                            .store(new_bd.map(|b| b as u32).unwrap_or(0), Ordering::Relaxed);

                        let _ = event_tx.try_send(EngineEvent::GaplessTransition);

                        // Build a state snapshot reflecting the new
                        // track. The orchestrator will overwrite
                        // `current_track_id` with what it expects, so
                        // we use what was passed by `prepare_next`.
                        let mut new_state = PlayerState {
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
                            rg_attenuation_db: shared.rg_attenuation_db(),
                            bit_perfect: None,
                            dsd_rate_label: None,
                            is_dsd: shared.is_dsd_active(),
                            upmix: None,
                            degraded: false,
                            error: None,
                        };
                        new_state.bit_perfect = build_bit_perfect_health(&shared, &new_state);
                        let _ = event_tx.try_send(EngineEvent::StateChanged { state: new_state });

                        tracing::info!(
                            target: "qobee::engine",
                            "gapless transition completed"
                        );
                        // Continue the loop with the new decoder. The
                        // resampler / legacy EQ / PcmChain (pre-gain,
                        // balance, crossfeed, limiter delay-line,
                        // dither PRNG + shaping history) all keep
                        // their state so the listener hears one
                        // continuous signal across the boundary.
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
                            sync_eq(&mut pcm_chain, &mut eq_seen_version, &shared);
                            pcm_chain.process(&mut interleaved);
                            dispatch_post_dsp(&interleaved, &mut producer, &alive, &shared);
                        }
                    }
                    if let Ok(out) = r.process_partial::<&[f32]>(None, None) {
                        let frames = out.first().map(|c| c.len()).unwrap_or(0);
                        if frames > 0 {
                            let mut interleaved: Vec<f32> = Vec::with_capacity(frames * n_ch);
                            for f in 0..frames {
                                for c in 0..n_ch {
                                    interleaved.push(
                                        out.get(c).and_then(|v| v.get(f).copied()).unwrap_or(0.0),
                                    );
                                }
                            }
                            sync_eq(&mut pcm_chain, &mut eq_seen_version, &shared);
                            pcm_chain.process(&mut interleaved);
                            dispatch_post_dsp(&interleaved, &mut producer, &alive, &shared);
                        }
                    }
                }

                // Drain the limiter look-ahead buffer with silence so
                // the last `lookahead_samples` of real audio still
                // reach the device before we emit `EndOfTrack` (R3
                // EOT-flush requirement). When the limiter is `Off`
                // or in `SoftClip` mode this loop is a no-op because
                // `limiter_lookahead_samples()` returns zero.
                let look = pcm_chain.limiter_lookahead_samples();
                if look > 0 {
                    let mut tail = vec![0.0_f32; look * n_ch];
                    pcm_chain.process(&mut tail);
                    dispatch_post_dsp(&tail, &mut producer, &alive, &shared);
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
                        rg_attenuation_db: None,
                        bit_perfect: None,
                        dsd_rate_label: None,
                        is_dsd: false,
                        upmix: None,
                        degraded: false,
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

/// Reduce `timestamp_seconds` (the demuxer's pre-DSP timestamp) by
/// the limiter's look-ahead so the position the UI displays matches
/// what is *audible* at this moment. Returns milliseconds, never
/// underflows below zero.
fn position_ms_with_latency(
    timestamp_seconds: f64,
    lookahead_samples: usize,
    sample_rate: u32,
) -> u32 {
    if lookahead_samples == 0 || sample_rate == 0 {
        return (timestamp_seconds * 1000.0).max(0.0) as u32;
    }
    let lookahead_seconds = lookahead_samples as f64 / sample_rate as f64;
    let adjusted = (timestamp_seconds - lookahead_seconds).max(0.0);
    (adjusted * 1000.0) as u32
}

/// Aggregate the chain state into a [`BitPerfectHealth`] snapshot
/// for the supplied [`PlayerState`]. Returns `None` when no device
/// is currently open (`device_sample_rate` and `device_channels`
/// both zero), so the wire format stays compact while idle.
///
/// Pulls from:
///   * `state` for source SR/bit depth/channels and effective mode,
///   * [`Shared::audio_settings`] for the per-stage settings flags
///     (crossfeed / limiter / balance â€” read directly by
///     [`BitPerfectHealth::compute`]),
///   * [`Shared::chain_bypass`] for the per-stage bypass triple
///     `(eq_bypass, convolver_off, dither_bypass)` published by the
///     decoder thread on each chunk boundary,
///   * [`Shared::device_sample_rate`] / [`Shared::device_channels`]
///     for the negotiated output format.
pub(crate) fn build_bit_perfect_health(
    shared: &Shared,
    state: &PlayerState,
) -> Option<BitPerfectHealth> {
    let device_sr = shared.device_sample_rate();
    let device_ch = shared.device_channels();
    // No device open â†’ no health to report. The UI hides the
    // device-side block in that case anyway (R5.7).
    if device_sr.is_none() && device_ch.is_none() {
        return None;
    }
    let settings = shared.audio_settings();
    let (eq_bypass, convolver_off, dither_bypass) = shared.chain_bypass();
    let src_ch = state.channels.unwrap_or(0);
    let device_ch = device_ch.unwrap_or(0);
    Some(BitPerfectHealth::compute(
        state,
        &settings,
        device_sr,
        src_ch,
        device_ch,
        eq_bypass,
        convolver_off,
        dither_bypass,
    ))
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

/// Forward post-DSP samples to either the device ring or the
/// pre-render sink, depending on whether a null-test pre-render
/// session is active (R11.3, Exclusive branch). When the sink is
/// active the device ring is *not* fed: pre-render is mutually
/// exclusive with normal device playback by construction.
fn dispatch_post_dsp(
    samples: &[f32],
    producer: &mut Producer<f32>,
    alive: &AtomicBool,
    shared: &Shared,
) {
    if shared.pre_render.is_active() {
        shared.pre_render.write_chunk(samples);
    } else {
        push_interleaved_to_ring(samples, producer, alive);
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
    let err_fn = {
        let shared = Arc::clone(&shared);
        move |e: cpal::StreamError| {
            match e {
                // The device went away under us. The dominant
                // real-world trigger is a voice/video call app
                // (Discord, FaceTime, Teams, Zoom…) switching the
                // OS default output to a communication device, or
                // a Bluetooth headset disconnecting. Rather than
                // killing the session with a hard error, flag a
                // recovery so the worker rebuilds the stream on the
                // current default device and resumes from the last
                // known position.
                cpal::StreamError::DeviceNotAvailable => {
                    shared.recover_device.store(true, Ordering::Release);
                    tracing::warn!(
                        target: "qobee::engine",
                        "output device became unavailable; scheduling stream recovery"
                    );
                }
                // Any other backend error is genuinely unexpected;
                // surface it so the orchestrator / UI can react.
                other => {
                    let _ = event_tx.try_send(EngineEvent::Error {
                        message: other.to_string(),
                    });
                    tracing::error!(target: "qobee::engine", error = %other, "cpal stream error");
                }
            }
        }
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

    // Anti-click play/pause fade. A separate smoothed gain that ramps
    // toward `shared.fade_target()` (0.0 or 1.0) over ~12 ms and
    // multiplies every output sample. Starting at 0.0 gives every
    // freshly built stream a clean fade-in; pause ramps it down to 0
    // *before* the worker stops the stream, so the device never sees
    // an abrupt amplitude step (the source of the click/pop).
    let mut fade_gain: f32 = 0.0;
    let fade_frames_to_converge: f32 = (config.sample_rate as f32 * 0.012).max(1.0);
    let fade_step_per_frame: f32 = 1.0 / fade_frames_to_converge;

    // Phase B (task 19) note. Inline soft-clip and TPDF dither used
    // to live here as the *only* clipping/dither path. With the new
    // `PcmChain` (`Peak_Limiter` + `Dither_Stage`) those operations
    // run before the samples reach this callback. The callback is
    // now a transparent multiplier (volume ramp) + ring drain +
    // channel mapper. The `dither_bits` parameter is kept on the
    // signature so subsequent tasks (e.g. a future fallback path)
    // can re-introduce a callback dither without changing every
    // call site.
    let _ = dither_bits;

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

                // R10.1 — post-transition prefill gate. Until the
                // decoder thread has buffered `prefill_threshold`
                // samples it leaves `ring_ready == false`; emit silence
                // and, crucially, do *not* count this as an underrun
                // (the ring is intentionally being filled, not starved).
                // Once the gate opens, a starved ring is a genuine
                // underrun and is counted below (R10.3).
                if !shared.ring_ready() {
                    for slot in output.iter_mut() {
                        *slot = S::from_sample(0.0_f32);
                    }
                    return;
                }

                // Frames the device wants this callback.
                let frames = output.len() / dst_ch;

                // Anti-click fade target for this callback (0.0 or
                // 1.0). Read once; the per-frame ramp below converges
                // toward it over ~12 ms.
                let fade_target = shared.fade_target();

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

                    // Step the anti-click fade toward its target.
                    if fade_gain < fade_target {
                        fade_gain = (fade_gain + fade_step_per_frame).min(fade_target);
                    } else if fade_gain > fade_target {
                        fade_gain = (fade_gain - fade_step_per_frame).max(fade_target);
                    }

                    let gain = current_gain * fade_gain;

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
                    // one. (1/sqrt(2) â‰ˆ 0.7071.)
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
                    //
                    // The samples already went through `PcmChain`'s
                    // `Peak_Limiter` (soft-clip in `SoftClip` mode,
                    // hard-clamped to the configured ceiling in
                    // `LookaheadLimiter` mode) and `Dither_Stage`
                    // (TPDF or noise-shaped dither when the device
                    // is â‰¤ 16-bit). The callback adds nothing to the
                    // signal beyond the volume ramp applied above
                    // and the channel mapping below.
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
///     transients ("sharp" cymbals, hat hitsâ€¦). Picking the matching
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
    list_output_devices_on(HostKind::System)
}

/// Enumerate output devices on a specific cpal host. Used by the ASIO
/// backend (`HostKind::Asio`) so the UI can offer the ASIO driver list
/// alongside the system devices. Behaves exactly like
/// [`list_output_devices`] for `HostKind::System`; for `HostKind::Asio`
/// it returns [`EngineError::BackendUnavailable`] on builds without the
/// `engine-asio` feature (callers that prefer an empty list should use
/// [`crate::backend_asio::list_asio_devices`]).
pub(crate) fn list_output_devices_on(
    kind: HostKind,
) -> EngineResult<Vec<crate::types::OutputDevice>> {
    let host = resolve_host(kind)?;
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_settings::{AudioSettings, DitherProfile, PeakLimiterMode, VolumeCurve};

    #[test]
    fn shared_new_publishes_default_audio_settings() {
        let shared = Shared::new();
        let snap = shared.audio_settings();

        // Fresh `Shared` should expose `AudioSettings::default()` with
        // version 0 (the store layer bumps it on every mutation).
        assert_eq!(snap.version, 0);
        assert_eq!(*snap, AudioSettings::default());
    }

    // Feature: qobee-beta-feedback-improvements, task 11.2 — smoke test
    // for the underrun counter getter (R10.4).
    //
    // `get_underrun_count` (Tauri) → `Player::underrun_count` →
    // `AudioEngine::underrun_count`. The lowest, most robust place to
    // assert the at-rest value is the engine trait method itself: a
    // freshly-booted `CpalSharedEngine` (the default, cross-platform
    // backend) has opened no stream, so its `Shared::underruns`
    // (`AtomicU32`) is still 0 and the getter must report 0. Building
    // the CPAL engine here also proves the override compiles and links
    // on non-Windows targets (the WASAPI override is Windows-only; the
    // trait still provides the `0` default everywhere).
    #[test]
    fn underrun_count_is_zero_at_rest() {
        let engine = match CpalSharedEngine::new() {
            Ok(e) => e,
            Err(e) => {
                // A CI runner without any usable audio host should not
                // flake this smoke test. The trait default already
                // guarantees `0`; skip loudly if the device-less boot
                // is impossible on this image.
                eprintln!("skipping: CPAL engine boot failed: {e}");
                return;
            }
        };

        // No stream has been started → the underrun counter is at its
        // initial value. Read it through the trait method exactly as
        // the Tauri command path does.
        assert_eq!(
            AudioEngine::underrun_count(&engine),
            0,
            "a freshly-booted engine with no stream must report 0 underruns"
        );
    }

    // Feature: qobee-beta-feedback-improvements, task 15.3 — the
    // degraded flag propagated to `PlayerState` mirrors the decoder
    // thread's EOF decision (`repaired_frames > DEGRADED_FRAME_THRESHOLD`)
    // and is cleared on the next Load (R11.3).
    #[test]
    fn file_degraded_flag_tracks_repair_threshold_and_resets_on_load() {
        use crate::backend_symphonia::DEGRADED_FRAME_THRESHOLD;

        let shared = Shared::new();
        // A fresh `Shared` starts clean.
        assert!(!shared.file_degraded(), "default must not be degraded");

        // A clean / lightly-repaired file (<= threshold) stays clean,
        // exactly as the decoder thread computes `was_degraded` at EOF.
        let repaired_clean = DEGRADED_FRAME_THRESHOLD; // boundary: not degraded
        shared.set_file_degraded(repaired_clean > DEGRADED_FRAME_THRESHOLD);
        assert!(
            !shared.file_degraded(),
            "exactly threshold repairs must not flag degraded"
        );

        // A damaged file (> threshold) flips the flag, which `state()` /
        // `snapshot_state` read into `PlayerState::degraded`.
        let repaired_damaged = DEGRADED_FRAME_THRESHOLD + 1;
        shared.set_file_degraded(repaired_damaged > DEGRADED_FRAME_THRESHOLD);
        assert!(
            shared.file_degraded(),
            "over-threshold repairs must flag the file degraded"
        );

        // The next Load resets the flag (the decoder thread calls
        // `set_file_degraded(false)` before decoding the new file).
        shared.set_file_degraded(false);
        assert!(!shared.file_degraded(), "a fresh Load must clear the flag");
    }

    // Feature: qobee-beta-feedback-improvements, task 14.2 — example
    // test complementing Property 7 (task 4.2). Property 7 proves the
    // *pure* `RingPlan` sizing invariants (`capacity >= base`,
    // `0 < prefill_threshold <= capacity`). This example models the
    // *runtime* consequence the design promises for R10.5: at a stable
    // format (no transition: `src_sr == device_sr`, identical channel
    // counts) a ring sized by `RingPlan` and prefilled to
    // `prefill_threshold` before the consumer starts must never produce
    // an underrun attributable to sizing, as long as the producer keeps
    // pace on average.
    //
    // We exercise the *real* `RingPlan` to size a *real* `rtrb` ring and
    // a *real* `Shared` underrun counter, then mirror the audio
    // callback's actual consume + underrun-counting logic
    // (`backend_cpal_shared.rs` callback): while `ring_ready == true`, a
    // failed `consumer.pop()` bumps `Shared::underruns`. A single-
    // threaded "produce a chunk, consume a callback" interleave models
    // steady state deterministically (no thread-timing flakiness).

    /// Mirror the audio callback's per-frame consume loop for one
    /// callback block of `block` interleaved samples. Honors the
    /// `ring_ready` prefill gate (silence, no underrun while `false`) and
    /// counts a genuine underrun on a starved ring once the gate is open
    /// — exactly as the real callback does (R10.1/R10.3).
    fn drain_one_callback(consumer: &mut Consumer<f32>, shared: &Shared, block: usize) {
        if !shared.ring_ready() {
            // Prefill gate closed: callback emits silence and does *not*
            // count an underrun (the ring is being filled, not starved).
            return;
        }
        for _ in 0..block {
            if consumer.pop().is_err() {
                // Starved while consuming: count one underrun and emit
                // silence for the rest of the block (callback returns).
                shared.underruns.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }

    /// Push up to `n` samples, stopping early if the ring is full.
    /// Returns the number actually pushed.
    fn produce(producer: &mut Producer<f32>, n: usize) -> usize {
        let mut pushed = 0;
        for _ in 0..n {
            if producer.push(0.0_f32).is_err() {
                break;
            }
            pushed += 1;
        }
        pushed
    }

    #[test]
    fn steady_state_stable_format_never_underruns_due_to_sizing() {
        // Stable format: source and device agree (no transition), so the
        // `RingPlan` scale clamps to 1.0 and `capacity == base`.
        const SR: u32 = 48_000;
        const CH: u16 = 2;
        let plan = crate::RingPlan::compute(RING_CAPACITY_SAMPLES, SR, SR, CH, CH);
        assert_eq!(
            plan.capacity, RING_CAPACITY_SAMPLES,
            "stable format must keep the baseline capacity"
        );
        assert!(plan.prefill_threshold > 0 && plan.prefill_threshold <= plan.capacity);

        // Real SPSC ring sized by the real `RingPlan`.
        let (mut producer, mut consumer) = RingBuffer::<f32>::new(plan.capacity);
        let shared = Shared::new();

        // One audio callback pulls 480 frames (10 ms at 48 kHz) × 2 ch.
        let callback_samples = 480 * CH as usize;

        // --- Prefill phase (R10.1) ----------------------------------
        // The callback starts gated (`ring_ready == false`) — draining
        // now must emit silence without counting an underrun. Fill the
        // ring up to `prefill_threshold`, then open the gate.
        assert!(!shared.ring_ready(), "fresh Shared starts gated");
        drain_one_callback(&mut consumer, &shared, callback_samples);
        assert_eq!(
            shared.underruns.load(Ordering::Relaxed),
            0,
            "draining while gated must not count an underrun (R10.1)"
        );
        let prefilled = produce(&mut producer, plan.prefill_threshold);
        assert_eq!(
            prefilled, plan.prefill_threshold,
            "prefill must reach the threshold within capacity"
        );
        shared.set_ring_ready(true);

        // --- Steady-state phase (R10.5) -----------------------------
        // Producer keeps pace *on average* but in bursts: it produces a
        // 4-callback burst every 4th round and nothing in between. The
        // prefilled, `RingPlan`-sized ring must absorb that decode jitter
        // without ever starving the consumer over a long run.
        const ROUNDS: usize = 4_000; // ~40 s of audio at 10 ms/callback
        const BURST_PERIOD: usize = 4;
        let burst = callback_samples * BURST_PERIOD;

        let mut min_fill = plan.prefill_threshold;
        for round in 0..ROUNDS {
            if round % BURST_PERIOD == 0 {
                produce(&mut producer, burst);
            }
            drain_one_callback(&mut consumer, &shared, callback_samples);

            let fill = plan.capacity.saturating_sub(producer.slots());
            min_fill = min_fill.min(fill);
        }

        assert_eq!(
            shared.underruns.load(Ordering::Relaxed),
            0,
            "a stable-format run over {ROUNDS} callbacks must not underrun \
             due to ring sizing (R10.5)"
        );
        // The buffer kept genuine slack: it never drained to a single
        // callback block, confirming the prefill + sizing absorbed the
        // producer jitter rather than merely breaking even.
        assert!(
            min_fill >= callback_samples,
            "ring should retain at least one callback block of slack \
             (min_fill={min_fill}, block={callback_samples})"
        );
    }

    #[test]
    fn set_audio_settings_bumps_version_and_keeps_payload() {
        let shared = Shared::new();
        let initial_version = shared.audio_settings().version;
        assert_eq!(initial_version, 0);

        // Use a snapshot whose `version` field is intentionally bogus
        // to confirm `set_audio_settings` ignores it and bumps off the
        // currently published value.
        let new_settings = AudioSettings {
            version: 999,
            peak_limiter_mode: PeakLimiterMode::Off,
            dither_profile: DitherProfile::Tpdf,
            volume_curve: VolumeCurve::Quadratic,
            volume_floor_db: -45.0,
            ..AudioSettings::default()
        };

        shared.set_audio_settings(new_settings.clone());

        let snap = shared.audio_settings();
        assert_eq!(snap.version, initial_version + 1);
        assert_eq!(snap.peak_limiter_mode, PeakLimiterMode::Off);
        assert_eq!(snap.dither_profile, DitherProfile::Tpdf);
        assert_eq!(snap.volume_curve, VolumeCurve::Quadratic);
        assert_eq!(snap.volume_floor_db, -45.0);
    }

    #[test]
    fn set_audio_settings_twice_bumps_version_twice() {
        let shared = Shared::new();
        assert_eq!(shared.audio_settings().version, 0);

        shared.set_audio_settings(AudioSettings::default());
        assert_eq!(shared.audio_settings().version, 1);

        shared.set_audio_settings(AudioSettings::default());
        assert_eq!(shared.audio_settings().version, 2);
    }

    #[test]
    fn position_ms_with_latency_subtracts_lookahead() {
        // 5 ms look-ahead at 48 kHz = 240 samples. A 10 s timestamp
        // should report as 9_995 ms once the limiter delay is
        // accounted for.
        let ms = position_ms_with_latency(10.0, 240, 48_000);
        assert_eq!(ms, 9_995);
    }

    #[test]
    fn position_ms_with_latency_clamps_to_zero_at_track_start() {
        // The very first frames of a track sit *inside* the
        // look-ahead window and produce a negative wall-clock
        // position. Clamp to zero rather than wrapping `u32`.
        let ms = position_ms_with_latency(0.001, 240, 48_000);
        assert_eq!(ms, 0);
    }

    #[test]
    fn position_ms_with_latency_passthrough_when_lookahead_zero() {
        // SoftClip / Off modes report `lookahead_samples == 0`. In
        // that case the helper must reduce to the historical
        // `(timestamp_seconds * 1000.0) as u32` cast.
        let ms = position_ms_with_latency(2.5, 0, 48_000);
        assert_eq!(ms, 2_500);
    }

    // ---- audible_gain Ã— volume curve coupling (R4.1, R4.4) ------------------

    #[test]
    fn audible_gain_uses_quadratic_curve_when_configured() {
        // Set the published settings to the legacy quadratic curve;
        // any half-slider value should produce vÂ² gain.
        let shared = Shared::new();
        let s = AudioSettings {
            volume_curve: VolumeCurve::Quadratic,
            ..AudioSettings::default()
        };
        shared.set_audio_settings(s);

        shared.set_volume_public(0.5);
        let g = shared.audible_gain();
        assert!(
            (g - 0.25).abs() < 1e-6,
            "quadratic curve at v=0.5 should give 0.25, got {g}"
        );

        shared.set_volume_public(0.1);
        let g = shared.audible_gain();
        assert!(
            (g - 0.01).abs() < 1e-6,
            "quadratic curve at v=0.1 should give 0.01, got {g}"
        );
    }

    #[test]
    fn audible_gain_uses_logarithmic_curve_when_configured() {
        // Logarithmic at v=0.5 with floor=-60 dB lands at -30 dB =
        // 10^(-30/20) â‰ˆ 0.03162.
        let shared = Shared::new();
        let s = AudioSettings {
            volume_curve: VolumeCurve::Logarithmic,
            volume_floor_db: -60.0,
            ..AudioSettings::default()
        };
        shared.set_audio_settings(s);

        shared.set_volume_public(0.5);
        let g = shared.audible_gain();
        let want = 10f32.powf(-30.0 / 20.0);
        assert!(
            (g - want).abs() < 1e-5,
            "logarithmic curve at v=0.5 should give {want}, got {g}"
        );
    }

    #[test]
    fn audible_gain_anchors_match_curve_contract() {
        // Both anchors must hold for both curves: v=0 â†’ 0.0 exact
        // (mute) and v=1 â†’ 1.0 exact (unity, basis for the
        // bit-perfect badge).
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            let shared = Shared::new();
            let s = AudioSettings {
                volume_curve: curve,
                ..AudioSettings::default()
            };
            shared.set_audio_settings(s);

            shared.set_volume_public(0.0);
            assert_eq!(shared.audible_gain(), 0.0, "{curve:?}: v=0 must mute");

            shared.set_volume_public(1.0);
            assert_eq!(
                shared.audible_gain(),
                1.0,
                "{curve:?}: v=1 must produce strict unity"
            );
        }
    }

    #[test]
    fn audible_gain_picks_up_runtime_curve_change() {
        // Switching the curve at runtime (as the UI does on
        // settings change) must immediately propagate to the next
        // `audible_gain` read â€” the audio callback re-reads the
        // target every period, so there's no caching here.
        let shared = Shared::new();
        shared.set_volume_public(0.5);

        let quad = AudioSettings {
            volume_curve: VolumeCurve::Quadratic,
            ..AudioSettings::default()
        };
        shared.set_audio_settings(quad);
        let g_quad = shared.audible_gain();
        assert!((g_quad - 0.25).abs() < 1e-6);

        let log = AudioSettings {
            volume_curve: VolumeCurve::Logarithmic,
            volume_floor_db: -60.0,
            ..AudioSettings::default()
        };
        shared.set_audio_settings(log);
        let g_log = shared.audible_gain();
        let want = 10f32.powf(-30.0 / 20.0);
        assert!((g_log - want).abs() < 1e-5);
    }

    // ---- Bit-perfect health snapshot publishing (task 25, R5.2/R5.5) -------

    /// Build a minimal `PlayerState` shaped like what `state()`
    /// would synthesise for an actively-playing track. Used by the
    /// helpers below to drive `build_bit_perfect_health` without
    /// the rest of the engine.
    fn playing_state(channels: u16, sr: u32) -> PlayerState {
        PlayerState {
            status: PlaybackStatus::Playing,
            current_track_id: Some("t".into()),
            position_seconds: 0.0,
            duration_seconds: 60.0,
            volume: 1.0,
            output_mode: EffectiveOutputMode::Shared,
            sample_rate: Some(sr),
            bit_depth: Some(24),
            channels: Some(channels),
            is_bit_perfect: false,
            rg_attenuation_db: None,
            bit_perfect: None,
            dsd_rate_label: None,
            is_dsd: false,
            upmix: None,
            degraded: false,
            error: None,
        }
    }

    #[test]
    fn build_bit_perfect_health_returns_none_when_no_device_open() {
        let shared = Shared::new();
        // Default `Shared` has `device_sample_rate = 0` (no device).
        let state = playing_state(2, 44_100);
        assert!(build_bit_perfect_health(&shared, &state).is_none());
    }

    #[test]
    fn build_bit_perfect_health_is_some_after_device_format_published() {
        let shared = Shared::new();
        // Simulate what `start_playback` does once it has finished
        // negotiating the output format.
        shared.set_device_format(44_100, 2);

        let state = playing_state(2, 44_100);
        let health = build_bit_perfect_health(&shared, &state)
            .expect("device open â‡’ health snapshot present");

        assert_eq!(health.device_sample_rate, Some(44_100));
        assert_eq!(health.source_sample_rate, Some(44_100));
        assert!(health.is_native_rate);
        assert!(!health.upmix_active);
    }

    #[test]
    fn build_bit_perfect_health_flags_double_resample_in_shared() {
        let shared = Shared::new();
        // Source 44.1 kHz, device running at 48 kHz â†’ hidden double
        // resample (R5.3).
        shared.set_device_format(48_000, 2);
        let state = playing_state(2, 44_100);

        let health = build_bit_perfect_health(&shared, &state).unwrap();
        assert!(!health.is_native_rate);
        assert!(
            health
                .messages
                .iter()
                .any(|m| m.contains("Double rééchantillonnage caché")),
            "expected a double-resample warning, got messages = {:?}",
            health.messages
        );
    }

    #[test]
    fn chain_bypass_round_trip_packs_and_unpacks_bits() {
        let shared = Shared::new();
        // Initial state: every flag conservative `true`.
        assert_eq!(shared.chain_bypass(), (true, true, true));

        shared.set_chain_bypass(false, true, false);
        assert_eq!(shared.chain_bypass(), (false, true, false));

        shared.set_chain_bypass(true, false, true);
        assert_eq!(shared.chain_bypass(), (true, false, true));

        shared.set_chain_bypass(false, false, false);
        assert_eq!(shared.chain_bypass(), (false, false, false));
    }

    #[test]
    fn should_emit_bit_perfect_first_call_publishes_unconditionally() {
        let shared = Shared::new();
        shared.set_device_format(44_100, 2);
        let state = playing_state(2, 44_100);
        let health = build_bit_perfect_health(&shared, &state).unwrap();
        assert!(
            shared.should_emit_bit_perfect(&health),
            "first emit must always go through"
        );
        // Repeating with the *same* health: suppressed by content
        // equality (no flag movement).
        assert!(!shared.should_emit_bit_perfect(&health));
    }

    #[test]
    fn should_emit_bit_perfect_suppresses_within_debounce_window() {
        let shared = Shared::new();
        shared.set_device_format(44_100, 2);
        let state = playing_state(2, 44_100);
        let health = build_bit_perfect_health(&shared, &state).unwrap();
        assert!(shared.should_emit_bit_perfect(&health));

        // Mutate the snapshot so content equality alone would not
        // suppress; only the 200 ms debounce should hold us back.
        let mut other = health.clone();
        other.upmix_active = !other.upmix_active;
        assert!(
            !shared.should_emit_bit_perfect(&other),
            "must be suppressed within 200 ms even when content differs"
        );
    }

    #[test]
    fn should_emit_bit_perfect_emits_after_debounce_window_elapses() {
        let shared = Shared::new();
        shared.set_device_format(44_100, 2);
        let state = playing_state(2, 44_100);
        let mut health = build_bit_perfect_health(&shared, &state).unwrap();
        assert!(shared.should_emit_bit_perfect(&health));

        // Advance past the 200 ms guard.
        std::thread::sleep(std::time::Duration::from_millis(220));

        // Move a flag so content equality does not suppress.
        health.upmix_active = !health.upmix_active;
        assert!(
            shared.should_emit_bit_perfect(&health),
            "post-debounce emit must go through"
        );
    }

    #[test]
    fn reset_bit_perfect_debounce_clears_state() {
        let shared = Shared::new();
        shared.set_device_format(44_100, 2);
        let state = playing_state(2, 44_100);
        let health = build_bit_perfect_health(&shared, &state).unwrap();
        assert!(shared.should_emit_bit_perfect(&health));
        // Same content, would be suppressed.
        assert!(!shared.should_emit_bit_perfect(&health));

        // Reset acts like a brand-new track: next emit goes through.
        shared.reset_bit_perfect_debounce();
        assert!(shared.should_emit_bit_perfect(&health));
    }

    #[test]
    fn cpal_engine_state_carries_bit_perfect_field_default_none() {
        // Engine boot does not open a device â†’ `state.bit_perfect`
        // must be `None` and serde must omit it from the JSON wire
        // format (compactness contract: `skip_serializing_if`).
        let engine = CpalSharedEngine::new().expect("CPAL engine boot");
        let state = AudioEngine::state(&engine);
        assert!(state.bit_perfect.is_none());

        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("\"bit_perfect\":"),
            "bit_perfect=None must be skipped on the wire, got {json}"
        );
    }

    #[test]
    fn cpal_engine_state_publishes_bit_perfect_after_device_format_set() {
        // We can't exercise a real `Load` in unit tests (needs a
        // working CPAL device), but the engine's `state()` reads
        // the device format directly from `Shared`. Publishing it
        // by hand mimics what `start_playback` does once it has
        // finished negotiating. Status is forced to `Playing` so
        // `BitPerfectHealth::compute` exposes the source-side
        // fields (R5.7 hides them while idle).
        let engine = CpalSharedEngine::new().expect("CPAL engine boot");
        engine.shared.set_device_format(44_100, 2);
        engine.shared.sample_rate.store(44_100, Ordering::Relaxed);
        engine.shared.channels.store(2, Ordering::Relaxed);
        engine.shared.bit_depth.store(24, Ordering::Relaxed);
        *engine.status.lock() = PlaybackStatus::Playing;

        let state = AudioEngine::state(&engine);
        assert!(
            state.bit_perfect.is_some(),
            "device format published â‡’ bit_perfect snapshot must be Some"
        );
        let health = state.bit_perfect.unwrap();
        assert_eq!(health.device_sample_rate, Some(44_100));
        assert_eq!(health.source_sample_rate, Some(44_100));
        assert!(health.is_native_rate);
        assert_eq!(health.effective_output_mode, EffectiveOutputMode::Shared);
    }
}
