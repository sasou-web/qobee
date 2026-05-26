//! Property-test generators shared across the engine integration
//! tests.
//!
//! Keep generators *small and intelligent*: each one constrains the
//! input to the space its consumer actually cares about, so failing
//! cases shrink to something an engineer can inspect quickly. The
//! generators listed here are the Phase A baseline; later phases
//! (DSP, DSD, convolver) will add their own (`any_dsd_stream`,
//! `any_ir`, `any_player_state_paused`, ...).
//!
//! ## Available generators
//!
//! - [`any_finite_buffer`] â€” interleaved `Vec<f32>` without NaN/Inf.
//! - [`any_audio_settings`] â€” fully-randomised `AudioSettings` whose
//!   numeric fields are inside the bounds declared in the design.
//! - [`seedable_silence`] â€” two seconds of zeros, deterministic.
//!
//! Tests using any of the above must include the spec tag in their
//! header comment, e.g. `// Feature: audio-quality-improvements,
//! Property N`.

#![allow(dead_code)] // Phase A scaffolding â€” not every generator has a
                     // consumer yet; later phases will use them all.

use proptest::collection::vec;
use proptest::prelude::*;
use qobee_engine::{
    AudioSettings, CrossfeedPreset, DitherProfile, PeakLimiterMode, ResamplerQuality, VolumeCurve,
};

/// Maximum buffer length explored by [`any_finite_buffer`]. Keeps the
/// per-iteration cost bounded so a 100-case run finishes well under
/// the CI budget.
pub const MAX_BUFFER_LEN: usize = 8_192;

/// Random `Vec<f32>` of length in `[1, MAX_BUFFER_LEN]` with every
/// element guaranteed finite (no NaN / no infinity) and clamped into
/// `[-4.0, 4.0]`. The slight overshoot above unity gain lets DSP
/// stages exercise their headroom without risking overflow during
/// downstream multiplication.
pub fn any_finite_buffer() -> impl Strategy<Value = Vec<f32>> {
    let sample = any::<f32>()
        .prop_filter("finite", |x| x.is_finite())
        .prop_map(|x| x.clamp(-4.0, 4.0));
    vec(sample, 1..MAX_BUFFER_LEN)
}

/// Random `AudioSettings`, every numeric field inside its design
/// range, every enum sampled uniformly. `convolver_ir_path` is left
/// `None` to keep the generator filesystem-free; `version` is fixed
/// at zero (the version counter is the store's responsibility, not
/// the generator's).
pub fn any_audio_settings() -> impl Strategy<Value = AudioSettings> {
    // Tuples have a 12-element max; build the settings in two halves
    // and merge. The bounds mirror the table in design.md (the same
    // ones enforced by `AudioSettings::clamp_in_place`).
    let part_a = (
        any::<bool>(),   // rg_peak_protection
        0.0f32..=3.0f32, // rg_safety_headroom_db
        prop_oneof![
            Just(DitherProfile::Tpdf),
            Just(DitherProfile::ShapedHp),
            Just(DitherProfile::ShapedFWeighted),
        ],
        prop_oneof![
            Just(PeakLimiterMode::Off),
            Just(PeakLimiterMode::SoftClip),
            Just(PeakLimiterMode::LookaheadLimiter),
        ],
        -3.0f32..=0.0f32,   // peak_limiter_ceiling_dbfs
        2.0f32..=10.0f32,   // peak_limiter_lookahead_ms
        20.0f32..=500.0f32, // peak_limiter_release_ms
        prop_oneof![Just(VolumeCurve::Quadratic), Just(VolumeCurve::Logarithmic),],
        -80.0f32..=-30.0f32, // volume_floor_db
    );
    let part_b = (
        any::<bool>(), // crossfeed_enabled
        prop_oneof![
            Just(CrossfeedPreset::Bauer),
            Just(CrossfeedPreset::BauerStrong),
            Just(CrossfeedPreset::Custom),
        ],
        200.0f32..=400.0f32,  // crossfeed_delay_us
        500.0f32..=1500.0f32, // crossfeed_lp_cutoff_hz
        prop_oneof![
            Just(ResamplerQuality::Standard),
            Just(ResamplerQuality::Best),
        ],
        any::<bool>(),     // convolver_enabled
        -24.0f32..=0.0f32, // convolver_gain_db
        -1.0f32..=1.0f32,  // balance
        vec(-12.0f32..=0.0f32, 0..=AudioSettings::MAX_CHANNELS),
    );

    (part_a, part_b).prop_map(|(a, b)| AudioSettings {
        version: 0,
        rg_peak_protection: a.0,
        rg_safety_headroom_db: a.1,
        dither_profile: a.2,
        peak_limiter_mode: a.3,
        peak_limiter_ceiling_dbfs: a.4,
        peak_limiter_lookahead_ms: a.5,
        peak_limiter_release_ms: a.6,
        volume_curve: a.7,
        volume_floor_db: a.8,
        crossfeed_enabled: b.0,
        crossfeed_preset: b.1,
        crossfeed_delay_us: b.2,
        crossfeed_lp_cutoff_hz: b.3,
        resampler_quality: b.4,
        convolver_enabled: b.5,
        convolver_ir_path: None,
        convolver_gain_db: b.6,
        balance: b.7,
        trim_db_per_channel: b.8,
    })
}

/// Two seconds of interleaved silence at `sample_rate` Hz with
/// `channels` channels. Used by the dither tests (Properties 3 and
/// 4) to verify that quiescent input produces the expected
/// statistics on output without introducing any randomness in the
/// generator itself.
pub fn seedable_silence(sample_rate: u32, channels: u16) -> Vec<f32> {
    let frames = (sample_rate as usize) * 2;
    vec![0.0_f32; frames * channels as usize]
}

/// Generator for a `PreGainStage` recompute scenario covering
/// Property 1's full input space. Each component is constrained to
/// the range declared by the design's "Settings keys" table.
///
/// Returns a tuple
/// `(rg_db, rg_peak, ceiling_dbfs, headroom_db,
///   peak_protection_on, slider, curve, floor_db)`.
///
/// `rg_db` and `rg_peak` are independently `Option<_>` to exercise
/// the "missing tag" fallbacks from R1.3 and the no-RG path of the
/// stage's recompute.
pub fn any_pre_gain_inputs() -> impl Strategy<
    Value = (
        Option<f32>,
        Option<f32>,
        f32,
        f32,
        bool,
        f32,
        VolumeCurve,
        f32,
    ),
> {
    (
        prop_oneof![Just(None), (-24.0f32..=12.0f32).prop_map(Some)],
        prop_oneof![Just(None), (1e-6f32..=4.0f32).prop_map(Some)],
        -3.0f32..=0.0f32,
        0.0f32..=3.0f32,
        any::<bool>(),
        0.0f32..=1.0f32,
        prop_oneof![Just(VolumeCurve::Quadratic), Just(VolumeCurve::Logarithmic)],
        -80.0f32..=-30.0f32,
    )
}

/// Generator for a `Channel_Balance_Stage` scenario covering
/// Property 21's input space.
///
/// Returns a tuple `(balance, trim_db_per_channel, channels)`:
///   * `balance âˆˆ [-1.0, 1.0]`,
///   * each entry of `trim_db_per_channel âˆˆ [-12.0, 0.0]`, length in
///     `0..=AudioSettings::MAX_CHANNELS` (independent of `channels`,
///     so the test exercises both the truncate-and-pad fallback when
///     the array is shorter than the channel count and the "extra
///     trims are ignored" path when it is longer),
///   * `channels âˆˆ [2, 8]` â€” mono is skipped because the design
///     force-bypasses mono flows regardless of settings (already
///     covered by an example test).
pub fn any_balance_inputs() -> impl Strategy<Value = (f32, Vec<f32>, u16)> {
    (
        -1.0f32..=1.0f32,
        vec(-12.0f32..=0.0f32, 0..=AudioSettings::MAX_CHANNELS),
        2u16..=8u16,
    )
}

/// Generator for a stereo impulse response pair `(left, right)`
/// used by Properties 17, 19, and 20 (`Convolver_Stage`).
///
/// Each channel has length in `1..=max_taps` and every sample is
/// finite, clamped to `[-1.0, 1.0]` to keep the convolver's output
/// bounded. The two channels are sampled independently so cross-
/// channel coupling tests get distinct shapes per side.
pub fn any_ir(max_taps: usize) -> impl Strategy<Value = (Vec<f32>, Vec<f32>)> {
    let max_taps = max_taps.max(1);
    let sample = any::<f32>()
        .prop_filter("finite", |x| x.is_finite())
        .prop_map(|x| x.clamp(-1.0, 1.0));
    (vec(sample.clone(), 1..=max_taps), vec(sample, 1..=max_taps))
}

// -----------------------------------------------------------------------------
// Phase C â€” BitPerfectHealth generator (task 27)
// -----------------------------------------------------------------------------

use qobee_engine::EffectiveOutputMode;

/// Inputs accepted by [`BitPerfectHealth::compute`] for Properties
/// 9, 10 and 30. The tuple shape matches the function's argument
/// list one-for-one so the property tests stay readable.
///
/// Returns
/// `(state, settings, device_sr, src_ch, device_ch,
///   eq_bypass, convolver_off, dither_bypass)`.
pub type BitPerfectInputs = (
    qobee_engine::PlayerState,
    AudioSettings,
    Option<u32>,
    u16,
    u16,
    bool,
    bool,
    bool,
);

// -----------------------------------------------------------------------------
// Phase E — DSD pipeline generator (task 45)
// -----------------------------------------------------------------------------

use qobee_engine::{DsdRate, DsdStream};

/// Random [`DsdStream`] for the DSD property tests.
///
/// Constraints (per task 45):
///   * `rate ∈ {DSD64, DSD128, DSD256, DSD512}`,
///   * `channels ∈ [1, 8]`,
///   * frames per channel `∈ [1024, 65_536]`,
///   * each byte uniformly random across `0..=255`,
///   * `lsb_first` randomly toggled so both DSF (LSB) and DFF (MSB)
///     bit orderings are exercised.
pub fn any_dsd_stream() -> impl Strategy<Value = DsdStream> {
    let rate = prop_oneof![
        Just(DsdRate::Dsd64),
        Just(DsdRate::Dsd128),
        Just(DsdRate::Dsd256),
        Just(DsdRate::Dsd512),
    ];
    let channels = 1u16..=8u16;
    let frames = 1024usize..=65_536usize;
    let lsb_first = any::<bool>();

    (rate, channels, frames, lsb_first).prop_flat_map(|(rate, channels, frames, lsb_first)| {
        let bytes = vec(any::<u8>(), frames..=frames);
        let per_channel = vec(bytes, channels as usize..=channels as usize);
        per_channel.prop_map(move |bytes_per_channel| DsdStream {
            rate,
            channels,
            bytes_per_channel,
            lsb_first,
        })
    })
}

/// Strategy producing a fully randomised set of inputs for the
/// tri-state `BitPerfectHealth::compute` truth table. The generator
/// is intelligent in two ways:
///
///   * The `PlayerState` always carries a `status` of `Playing` so
///     the source side fields are kept live (idle drops them per
///     R5.7 and that path is covered by example tests).
///   * The `EffectiveOutputMode` is sampled via `prop_oneof!` so
///     both the Exclusive (Green-eligible) and Shared
///     (Amber-eligible) branches of the truth table are visited.
///
/// `device_ch == 0` would mean "no device channels published" which
/// the upmix detector must treat as "unknown"; we keep `device_ch`
/// strictly positive so the generator never hits that special case
/// (covered by a dedicated example).
pub fn any_bit_perfect_inputs() -> impl Strategy<Value = BitPerfectInputs> {
    use qobee_engine::types::PlaybackStatus;

    let mode = prop_oneof![
        Just(EffectiveOutputMode::Exclusive),
        Just(EffectiveOutputMode::Shared),
    ];
    let limiter = prop_oneof![
        Just(qobee_engine::PeakLimiterMode::Off),
        Just(qobee_engine::PeakLimiterMode::SoftClip),
        Just(qobee_engine::PeakLimiterMode::LookaheadLimiter),
    ];

    // proptest tuples max out at 12 elements; group into two halves.
    let head = (
        mode,
        // volume slider
        0.0f32..=1.0f32,
        // sample_rate (source) â€” common audio rates
        prop_oneof![
            Just(44_100u32),
            Just(48_000u32),
            Just(88_200u32),
            Just(96_000u32),
            Just(176_400u32),
            Just(192_000u32),
        ],
        // device sample rate â€” None or one of the same set
        prop_oneof![
            Just(None),
            Just(Some(44_100u32)),
            Just(Some(48_000u32)),
            Just(Some(96_000u32)),
            Just(Some(192_000u32)),
        ],
        // src_ch / device_ch â€” keep both > 0 to leave the
        // upmix detector well-defined.
        1u16..=8u16,
        1u16..=8u16,
        any::<bool>(), // eq_bypass
        any::<bool>(), // convolver_off
        any::<bool>(), // dither_bypass
    );
    let tail = (
        any::<bool>(), // crossfeed_enabled
        limiter,
        -1.0f32..=1.0f32, // balance
        vec(-12.0f32..=0.0f32, 0..=AudioSettings::MAX_CHANNELS),
        // RG attenuation: None (no clamp) or a small negative dB
        prop_oneof![Just(None), (-6.0f32..=0.0f32).prop_map(Some)],
    );

    (head, tail).prop_map(
        |(
            (
                output_mode,
                volume,
                source_sr,
                device_sr,
                src_ch,
                device_ch,
                eq_bypass,
                convolver_off,
                dither_bypass,
            ),
            (crossfeed_enabled, peak_limiter_mode, balance, trim_db, rg_attn),
        )| {
            let state = qobee_engine::PlayerState {
                status: PlaybackStatus::Playing,
                current_track_id: Some("t".into()),
                position_seconds: 1.0,
                duration_seconds: 60.0,
                volume,
                output_mode,
                sample_rate: Some(source_sr),
                bit_depth: Some(24),
                channels: Some(src_ch),
                is_bit_perfect: matches!(output_mode, EffectiveOutputMode::Exclusive),
                rg_attenuation_db: rg_attn,
                bit_perfect: None,
                dsd_rate_label: None,
                is_dsd: false,
                error: None,
            };
            let settings = AudioSettings {
                crossfeed_enabled,
                peak_limiter_mode,
                balance,
                trim_db_per_channel: trim_db,
                ..AudioSettings::default()
            };
            (
                state,
                settings,
                device_sr,
                src_ch,
                device_ch,
                eq_bypass,
                convolver_off,
                dither_bypass,
            )
        },
    )
}
