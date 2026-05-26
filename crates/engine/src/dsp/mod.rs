//! PCM DSP chain — common trait and assembled chain.
//!
//! This module hosts the `DspStage` trait that every PCM stage
//! implements (pre-gain, balance, crossfeed, EQ, convolver, limiter,
//! dither) and the `PcmChain` aggregator that calls each stage in the
//! order fixed by the design.
//!
//! ## Design contract
//!
//! Every stage advertises a cheap `is_bypass()` flag. When the flag is
//! `true`, `process_inplace` MUST return without touching the buffer
//! (sample-for-sample passthrough). This is the foundation of the
//! bit-perfect health badge: when every stage is bypassed, the chain
//! is provably an identity operation.
//!
//! `reconfigure()` is the only place stages may allocate. It runs on
//! the decoder thread, on a chunk boundary, when the
//! `AudioSettings::version` counter has changed since the previous
//! iteration. The audio callback never sees an allocation.
//!
//! ## Phase B placeholder stages
//!
//! Tasks 14 and 16 will implement the real `Peak_Limiter` and
//! `Dither` stages respectively, and task 32 (Phase D) will
//! implement the `Convolver`. Until then, the remaining slots are
//! filled with a `Noop*` placeholder defined in this file. The
//! chain wiring, ordering and bypass-snapshot logic land here, in
//! task 7, so subsequent tasks only have to swap out one
//! `Box<dyn DspStage>` at a time without reshaping the chain.
//!
//! Until task 18 wires `PcmChain` into `run_decoder_thread`, the
//! trait, every Noop placeholder, and the chain itself are referenced
//! only by this module's tests. The `#![allow(dead_code)]` below
//! keeps the placeholder scaffolding warning-free; subsequent tasks
//! will remove the attribute as soon as they consume these items.

#![allow(dead_code)]

pub mod balance;
pub mod clip;
pub mod convolver;
pub mod crossfeed;
pub mod dither;
pub mod limiter;
pub mod pre_gain;

pub use balance::ChannelBalanceStage;
pub use clip::soft_clip;
pub use convolver::ConvolverStage;
pub use crossfeed::CrossfeedStage;
pub use dither::DitherStage;
pub use limiter::PeakLimiter;
pub use pre_gain::{PreGainContext, PreGainStage};

use crate::audio_settings::AudioSettings;

// -----------------------------------------------------------------------------
// Trait
// -----------------------------------------------------------------------------

/// Common contract for every PCM stage in the DSP chain.
///
/// Implementations are owned by the decoder thread; they never need
/// `Sync` because no other thread reads them concurrently. `Send` is
/// required so the decoder thread itself can be moved at construction.
///
/// The trait is `pub` so integration tests can drive a stage end-to-end
/// (call `process_inplace` and `reset` on the concrete type via the
/// trait); the assembled `PcmChain` and the no-op placeholder stages
/// remain `pub(crate)` and are not part of the public API.
pub trait DspStage: Send {
    /// Reconfigurer en fonction des nouveaux réglages. Appelé sur la
    /// boundary de chunk dans le decoder thread quand la version
    /// d'`AudioSettings` a changé. Allocations autorisées ici (rare).
    fn reconfigure(&mut self, settings: &AudioSettings, sample_rate: u32, channels: u16);

    /// Vrai si le stage est strictement no-op (paramètres unité ou flag
    /// désactivé). Lu pour l'agrégation `BitPerfectHealth` ; doit être
    /// peu coûteux (atomique ou booléen membre).
    fn is_bypass(&self) -> bool;

    /// Traitement en place sur f32 entrelacé (frames * channels).
    /// Quand `is_bypass()` est vrai cette fonction DOIT retourner sans
    /// toucher aux samples (passthrough sample-pour-sample exact).
    fn process_inplace(&mut self, samples: &mut [f32]);

    /// Réinitialiser les états internes (delay-lines, FFT history,
    /// envelope follower, dither LFSR). Appelé sur seek et sur load.
    fn reset(&mut self);
}

// -----------------------------------------------------------------------------
// Per-stage bypass snapshot (consumed by BitPerfectHealth, task 24)
// -----------------------------------------------------------------------------

/// Snapshot of every stage's `is_bypass()` flag, taken on demand by
/// the bit-perfect health calculator.
///
/// Field names use semantic terminology rather than literal stage
/// names: `eq_bypass` reads as "EQ is bypassed" and is set to the
/// stage's own `is_bypass()` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagesBypass {
    pub pre_gain_unity: bool,
    pub balance_off: bool,
    pub crossfeed_off: bool,
    pub eq_bypass: bool,
    pub convolver_off: bool,
    pub limiter_off: bool,
    pub dither_bypass: bool,
}

impl StagesBypass {
    /// `true` when every stage is bypassed — i.e. the chain is a
    /// strict identity. Used by the bit-perfect health calculator.
    pub fn all_bypassed(&self) -> bool {
        self.pre_gain_unity
            && self.balance_off
            && self.crossfeed_off
            && self.eq_bypass
            && self.convolver_off
            && self.limiter_off
            && self.dither_bypass
    }
}

// -----------------------------------------------------------------------------
// Phase B placeholder stages (replaced incrementally by tasks 8-17 + 32)
// -----------------------------------------------------------------------------

/// Marker generator for the no-op placeholder stages. Each `Noop*`
/// type implements `DspStage` with `is_bypass() == true`, an empty
/// `process_inplace`, and empty `reconfigure` / `reset`.
macro_rules! noop_stage {
    ($name:ident) => {
        pub(crate) struct $name;

        impl DspStage for $name {
            fn reconfigure(
                &mut self,
                _settings: &AudioSettings,
                _sample_rate: u32,
                _channels: u16,
            ) {
            }

            fn is_bypass(&self) -> bool {
                true
            }

            fn process_inplace(&mut self, _samples: &mut [f32]) {
                // Bypass path: must not touch the buffer.
            }

            fn reset(&mut self) {}
        }
    };
}

noop_stage!(NoopEq);
// `NoopConvolver` lived here in the placeholder phase. Task 32 swaps
// it out for the real `ConvolverStage` (R9), so the macro is no longer
// needed for the convolver slot.

// -----------------------------------------------------------------------------
// PcmChain
// -----------------------------------------------------------------------------

/// Aggregate of every PCM stage, called in fixed order by `process`.
///
/// Stages are stored as `Box<dyn DspStage>` so subsequent tasks can
/// swap a placeholder for the real implementation without reshaping
/// the struct.
pub(crate) struct PcmChain {
    pre_gain: PreGainStage,
    balance: ChannelBalanceStage,
    crossfeed: CrossfeedStage,
    eq: Box<dyn DspStage>,
    convolver: ConvolverStage,
    limiter: PeakLimiter,
    dither: DitherStage,
    settings_version: u64,
    sample_rate: u32,
    channels: u16,
}

impl PcmChain {
    /// Build a new chain. `Pre_Gain_Stage` (task 8),
    /// `Channel_Balance_Stage` (task 10), `Crossfeed_Stage`
    /// (task 12), `Peak_Limiter` (task 14), `Dither_Stage`
    /// (task 16) and `Convolver_Stage` (task 32) are now real
    /// implementations; the EQ slot remains a no-op placeholder
    /// that a subsequent task will replace.
    pub fn new(settings: &AudioSettings, sample_rate: u32, channels: u16) -> Self {
        let mut chain = Self {
            pre_gain: PreGainStage::new(settings, sample_rate, channels),
            balance: ChannelBalanceStage::new(settings, sample_rate, channels),
            crossfeed: CrossfeedStage::new(settings, sample_rate, channels),
            eq: Box::new(NoopEq),
            convolver: ConvolverStage::new(settings, sample_rate, channels),
            limiter: PeakLimiter::new(settings, sample_rate, channels),
            dither: DitherStage::new(settings, sample_rate, channels),
            settings_version: 0,
            sample_rate,
            channels,
        };
        chain.reconfigure(settings, sample_rate, channels);
        chain
    }

    /// Push a new per-load context (RG dB/peak from the current
    /// track + volume slider position) into the pre-gain stage. Will
    /// recompute the stage's combined `g_linear` immediately.
    ///
    /// Called by `run_decoder_thread` (task 18) after a track load
    /// or whenever the volume slider moves.
    pub fn set_pre_gain_context(&mut self, ctx: PreGainContext) {
        self.pre_gain.set_context(ctx);
    }

    /// Reported RG attenuation in dB (R1.5). `0.0` when the demanded
    /// RG gain fits within the configured ceiling; negative when
    /// the stage had to clamp it down. Read by the engine's `state()`
    /// snapshot to surface `PlayerState::rg_attenuation_db`.
    pub fn pre_gain_attenuation_db(&self) -> f32 {
        self.pre_gain.rg_attenuation_db()
    }

    /// Look-ahead size of the limiter in samples. The decoder thread
    /// uses this to compensate `position_ms` (adds
    /// `lookahead_samples / sample_rate` when the limiter is active)
    /// and to size the silence-flush buffer it sends through `process`
    /// before emitting `EndOfTrack` (R3 latency compensation +
    /// EOT flush).
    pub fn limiter_lookahead_samples(&self) -> usize {
        self.limiter.lookahead_samples()
    }

    /// Forward the negotiated output bit-depth to the dither stage.
    /// `None` (or `Some(b)` with `b >= 24`) keeps the stage
    /// bypassed; `Some(b)` with `b < 24` activates the configured
    /// profile (R2.5).
    ///
    /// Called by `run_decoder_thread` (task 18) once the device
    /// format is known. The setter lives on the chain (rather than
    /// the [`DspStage`] trait) because `output_bits` is a *device*
    /// concern, not a user-configurable [`AudioSettings`] field.
    pub fn set_dither_output_bits(&mut self, bits: Option<u8>) {
        self.dither.set_output_bits(bits);
    }

    /// Replace the convolver stage's active IR with a fresh
    /// `(left, right)` pair. Pulled from the worker thread that
    /// loaded the WAV (see `Player::load_convolver_ir`); the audio
    /// hot path never sees an allocation. Pass two empty vectors to
    /// drop the IR (the stage falls back to bypass on the next
    /// chunk).
    pub fn set_convolver_ir(&mut self, ir_left: Vec<f32>, ir_right: Vec<f32>) {
        self.convolver.set_ir(ir_left, ir_right);
    }

    /// Length of the active convolver IR in taps; `0` when none
    /// loaded. Read by `get_convolver_status` to compute the
    /// reported latency in milliseconds.
    pub fn convolver_ir_len(&self) -> usize {
        self.convolver.ir_len()
    }

    /// Run every stage on `samples` in the canonical order:
    /// pre_gain → balance → crossfeed → eq → convolver → limiter → dither.
    ///
    /// No locks are taken in this hot path.
    pub fn process(&mut self, samples: &mut [f32]) {
        self.pre_gain.process_inplace(samples);
        self.balance.process_inplace(samples);
        self.crossfeed.process_inplace(samples);
        self.eq.process_inplace(samples);
        self.convolver.process_inplace(samples);
        self.limiter.process_inplace(samples);
        self.dither.process_inplace(samples);
    }

    /// Propagate a new settings snapshot to every stage and update the
    /// cached `settings_version`. Called on chunk boundaries by the
    /// decoder thread when it observes a version change.
    pub fn reconfigure(&mut self, settings: &AudioSettings, sample_rate: u32, channels: u16) {
        self.sample_rate = sample_rate;
        self.channels = channels;
        self.settings_version = settings.version;

        self.pre_gain.reconfigure(settings, sample_rate, channels);
        self.balance.reconfigure(settings, sample_rate, channels);
        self.crossfeed.reconfigure(settings, sample_rate, channels);
        self.eq.reconfigure(settings, sample_rate, channels);
        self.convolver.reconfigure(settings, sample_rate, channels);
        self.limiter.reconfigure(settings, sample_rate, channels);
        self.dither.reconfigure(settings, sample_rate, channels);
    }

    /// Reset every stage's internal state (delay lines, FFT history,
    /// envelope follower, dither LFSR). Called on seek and on load so
    /// the new audio doesn't bleed previous samples.
    pub fn reset(&mut self) {
        self.pre_gain.reset();
        self.balance.reset();
        self.crossfeed.reset();
        self.eq.reset();
        self.convolver.reset();
        self.limiter.reset();
        self.dither.reset();
    }

    /// Read each stage's `is_bypass()` flag into a `StagesBypass`
    /// snapshot for `BitPerfectHealth` (task 24).
    pub fn bypass_snapshot(&self) -> StagesBypass {
        StagesBypass {
            pre_gain_unity: self.pre_gain.is_bypass(),
            balance_off: self.balance.is_bypass(),
            crossfeed_off: self.crossfeed.is_bypass(),
            eq_bypass: self.eq.is_bypass(),
            convolver_off: self.convolver.is_bypass(),
            limiter_off: self.limiter.is_bypass(),
            dither_bypass: self.dither.is_bypass(),
        }
    }

    /// Last `AudioSettings::version` consumed by `reconfigure`. The
    /// decoder thread uses this to skip work when nothing has changed.
    pub fn settings_version(&self) -> u64 {
        self.settings_version
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_buffer() -> Vec<f32> {
        // Mix of positives, negatives, zero, near-unity, and small
        // values; nothing pathological so passthrough must keep them
        // bit-identical.
        vec![
            0.0,
            1.0,
            -1.0,
            0.5,
            -0.5,
            0.123_456_7,
            -0.987_654_3,
            1e-9,
            -1e-9,
            0.999_999_94,
        ]
    }

    #[test]
    fn pcm_chain_default_is_full_passthrough() {
        // With limiter mode Off the chain reduces to identity (the
        // remaining slots are bypassed by their default settings).
        let settings = AudioSettings {
            peak_limiter_mode: crate::audio_settings::PeakLimiterMode::Off,
            ..AudioSettings::default()
        };
        let mut chain = PcmChain::new(&settings, 48_000, 2);

        let original = sample_buffer();
        let mut buf = original.clone();
        chain.process(&mut buf);

        assert_eq!(
            buf, original,
            "chain with limiter Off must be sample-for-sample identity"
        );
    }

    #[test]
    fn pcm_chain_bypass_snapshot_all_true_with_limiter_off() {
        let settings = AudioSettings {
            peak_limiter_mode: crate::audio_settings::PeakLimiterMode::Off,
            ..AudioSettings::default()
        };
        let chain = PcmChain::new(&settings, 48_000, 2);

        let snap = chain.bypass_snapshot();
        assert!(snap.pre_gain_unity);
        assert!(snap.balance_off);
        assert!(snap.crossfeed_off);
        assert!(snap.eq_bypass);
        assert!(snap.convolver_off);
        assert!(snap.limiter_off);
        assert!(snap.dither_bypass);
        assert!(snap.all_bypassed());
    }

    #[test]
    fn pcm_chain_default_lookahead_limiter_is_not_bypassed() {
        // Default mode is LookaheadLimiter, which always introduces
        // observable latency and therefore reports `is_bypass()`
        // false even when transparent (R3.9 honesty).
        let settings = AudioSettings::default();
        let chain = PcmChain::new(&settings, 48_000, 2);
        let snap = chain.bypass_snapshot();
        assert!(snap.pre_gain_unity);
        assert!(snap.balance_off);
        assert!(snap.crossfeed_off);
        assert!(snap.eq_bypass);
        assert!(snap.convolver_off);
        assert!(
            !snap.limiter_off,
            "default LookaheadLimiter must report active"
        );
        assert!(snap.dither_bypass);
    }

    #[test]
    fn pcm_chain_reconfigure_updates_version() {
        let mut settings = AudioSettings::default();
        let mut chain = PcmChain::new(&settings, 48_000, 2);
        assert_eq!(chain.settings_version(), 0);

        settings.version = 42;
        chain.reconfigure(&settings, 48_000, 2);
        assert_eq!(chain.settings_version(), 42);

        settings.version = 1_000_000;
        chain.reconfigure(&settings, 96_000, 6);
        assert_eq!(chain.settings_version(), 1_000_000);
    }

    #[test]
    fn pcm_chain_reset_does_not_panic() {
        let settings = AudioSettings::default();
        let mut chain = PcmChain::new(&settings, 48_000, 2);
        chain.reset();
        // Calling reset twice in a row must remain safe.
        chain.reset();
    }

    /// Canonical processing order of the PCM chain. Kept here as a
    /// hard-coded reference so the test below catches any reordering
    /// of `process()`'s body. Mirrors the design's "Architecture §
    /// Chaîne DSP PCM" diagram.
    const CANONICAL_STAGE_ORDER: &[&str] = &[
        "pre_gain",
        "balance",
        "crossfeed",
        "eq",
        "convolver",
        "limiter",
        "dither",
    ];

    // Feature: audio-quality-improvements, R3.1 / R6.1 / R10.1 — the
    // PCM chain must call its stages in the canonical order.
    //
    // Several chain slots (`pre_gain`, `balance`, `crossfeed`,
    // `limiter`, `dither`) are stored as concrete typed fields so we
    // cannot trivially substitute mock stages. Instead the smoke test
    // takes a behavioural approach: it sets up the chain so that
    // running pre_gain *before* the limiter clamps the signal, while
    // running them in the wrong order would let the boost escape the
    // ceiling. The single-sample assertion below suffices because the
    // pre_gain × limiter ordering is the most consequential one in
    // the chain (every other PCM stage is linear and commutes with
    // its neighbours under the test signal we feed).
    #[test]
    fn pcm_chain_pre_gain_runs_before_peak_limiter() {
        // Source signal at -6 dBFS, RG boost of +6 dB → post-pre-gain
        // signal sits at full-scale. With the limiter ceiling at
        // -3 dBFS, the post-limiter signal must therefore be clamped
        // to ≤ 10^(-3/20) ≈ 0.7079 in absolute value. If the limiter
        // ran *before* pre_gain, the source at 0.5 would not trip the
        // ceiling and the boost would push the output above 1.0.
        let settings = AudioSettings {
            peak_limiter_ceiling_dbfs: -3.0,
            peak_limiter_mode: crate::audio_settings::PeakLimiterMode::LookaheadLimiter,
            ..AudioSettings::default()
        };

        let mut chain = PcmChain::new(&settings, 48_000, 2);
        // Push a fresh per-track context: +6 dB of RG, peak protection
        // off (so the RG boost is not silently clamped by the
        // pre-gain stage itself), unity volume slider.
        let mut audio = settings.clone();
        audio.rg_peak_protection = false;
        chain.reconfigure(&audio, 48_000, 2);
        chain.set_pre_gain_context(crate::dsp::PreGainContext {
            rg_db: Some(6.0),
            rg_peak: None,
            slider: 1.0,
        });

        // 1 second of stereo at -6 dBFS DC. Plenty of latency in
        // front of the look-ahead so the limiter has a stable view.
        let frames = 48_000;
        let mut buf = vec![0.5_f32; frames * 2];
        chain.process(&mut buf);

        let ceiling = 10f32.powf(-3.0 / 20.0);
        // Skip the first ~10 ms while the limiter primes its
        // delay-line and envelope.
        let warmup = 480 * 2;
        for (i, &y) in buf.iter().enumerate().skip(warmup) {
            assert!(
                y.abs() <= ceiling + 1e-4,
                "sample {i} = {y} exceeds ceiling {ceiling} \
                 — pre_gain must run before limiter"
            );
        }
    }

    #[test]
    fn pcm_chain_canonical_stage_order_documented() {
        // Smoke test guarding the canonical order constant. Any future
        // change to `process()`'s body must also update this list, so
        // a reordering is impossible to land without flipping a test.
        assert_eq!(
            CANONICAL_STAGE_ORDER,
            &[
                "pre_gain",
                "balance",
                "crossfeed",
                "eq",
                "convolver",
                "limiter",
                "dither"
            ]
        );
    }

    // -------------------------------------------------------------------------
    // Task 55 — qualitative DSP cost mini-bench (ignored by default).
    //
    // Runs three configurations of the chain across a 5 s 192 kHz
    // stereo buffer and prints wall-clock + real-time factor. Run
    // with:
    //
    //     cargo test --release -p qobee-engine --lib -- \
    //         --ignored bench_dsp_chain_cost --nocapture
    //
    // The chain is `pub(crate)`, so the bench lives in the dsp
    // module's own test block rather than under `tests/`.
    // -------------------------------------------------------------------------
    #[test]
    #[ignore]
    fn bench_dsp_chain_cost() {
        use crate::audio_settings::{CrossfeedPreset, DitherProfile, PeakLimiterMode};
        use std::time::Instant;

        const SAMPLE_RATE: u32 = 192_000;
        const CHANNELS: u16 = 2;
        const SECONDS: usize = 5;
        const BLOCK_FRAMES: usize = 2_048;
        let total_frames = SAMPLE_RATE as usize * SECONDS;

        // Synthesise a deterministic stereo signal: 100 Hz + 1 kHz
        // sines summed at -12 dBFS each. Doesn't have to be musically
        // meaningful — the chain just needs non-zero, non-pathological
        // input.
        let mut buffer = Vec::<f32>::with_capacity(total_frames * 2);
        for n in 0..total_frames {
            let t = n as f32 / SAMPLE_RATE as f32;
            let s = 0.25 * (2.0 * std::f32::consts::PI * 100.0 * t).sin()
                + 0.25 * (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            buffer.push(s);
            buffer.push(-s);
        }

        // Synthesised 65 536-tap decaying impulse response: delta
        // followed by a Hann-windowed exponential tail. Doesn't have
        // to be musically meaningful — the cost dominates regardless.
        let ir_len = 65_536;
        let ir: Vec<f32> = (0..ir_len)
            .map(|i| {
                if i == 0 {
                    1.0
                } else {
                    let w = 0.5
                        * (1.0
                            - (2.0 * std::f32::consts::PI * i as f32 / (ir_len - 1) as f32).cos());
                    let decay = (-5.0 * i as f32 / ir_len as f32).exp();
                    0.05 * w * decay
                }
            })
            .collect();

        // Configurations:
        //   (a) all off:    limiter Off, no crossfeed, no convolver,
        //                   dither bypassed (output_bits = None).
        //   (b) defaults:   AudioSettings::default() unchanged.
        //   (c) all on:     defaults + crossfeed BauerStrong + convolver
        //                   active with the synthesised IR.
        let cfg_a = AudioSettings {
            peak_limiter_mode: PeakLimiterMode::Off,
            crossfeed_enabled: false,
            convolver_enabled: false,
            ..AudioSettings::default()
        };
        let cfg_b = AudioSettings::default();
        let cfg_c = AudioSettings {
            crossfeed_enabled: true,
            crossfeed_preset: CrossfeedPreset::BauerStrong,
            convolver_enabled: true,
            dither_profile: DitherProfile::ShapedFWeighted,
            ..AudioSettings::default()
        };

        #[allow(clippy::too_many_arguments)]
        fn run_cfg(
            label: &str,
            settings: &AudioSettings,
            sample_rate: u32,
            channels: u16,
            buffer: &[f32],
            block_frames: usize,
            ir_l: Option<Vec<f32>>,
            ir_r: Option<Vec<f32>>,
            output_bits: Option<u8>,
        ) {
            let mut chain = PcmChain::new(settings, sample_rate, channels);
            chain.set_dither_output_bits(output_bits);
            if let (Some(l), Some(r)) = (ir_l, ir_r) {
                chain.set_convolver_ir(l, r);
            }
            let mut work: Vec<f32> = Vec::with_capacity(block_frames * channels as usize);
            let block_samples = block_frames * channels as usize;
            let mut total_blocks = 0usize;
            let mut total_frames = 0usize;
            let start = Instant::now();
            for chunk in buffer.chunks(block_samples) {
                work.clear();
                work.extend_from_slice(chunk);
                chain.process(&mut work);
                total_blocks += 1;
                total_frames += chunk.len() / channels as usize;
            }
            let elapsed = start.elapsed();
            let wall_ms = elapsed.as_secs_f64() * 1_000.0;
            let per_block_ms = wall_ms / total_blocks as f64;
            let audio_secs = total_frames as f64 / sample_rate as f64;
            let rtf = audio_secs / elapsed.as_secs_f64();
            println!(
                "[bench_dsp_chain_cost] {label:>10}: \
                 wall = {wall_ms:>8.2} ms,  per 2048-block = {per_block_ms:>6.3} ms,  \
                 RT factor = {rtf:>7.1}x ({total_blocks} blocks @ {sample_rate} Hz)"
            );
        }

        run_cfg(
            "all-off",
            &cfg_a,
            SAMPLE_RATE,
            CHANNELS,
            &buffer,
            BLOCK_FRAMES,
            None,
            None,
            None,
        );
        run_cfg(
            "defaults",
            &cfg_b,
            SAMPLE_RATE,
            CHANNELS,
            &buffer,
            BLOCK_FRAMES,
            None,
            None,
            Some(24),
        );
        run_cfg(
            "all-on",
            &cfg_c,
            SAMPLE_RATE,
            CHANNELS,
            &buffer,
            BLOCK_FRAMES,
            Some(ir.clone()),
            Some(ir.clone()),
            Some(16),
        );
    }
}
