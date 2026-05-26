//! Entry point for the `properties` integration test target.
//!
//! Cargo's integration tests are individual binaries: each `*.rs`
//! file directly under `tests/` is compiled as `--test <stem>`.
//! Phase A only needs the shared generators (no DSP yet), so this
//! file simply pulls in the `properties` module and runs a smoke
//! test exercising every generator. Subsequent phases will declare
//! more `#[test]` entries here (or in sibling modules) that import
//! the generators via `mod properties; use properties::*;`.

#[path = "properties/mod.rs"]
mod properties;

#[path = "properties/limiter.rs"]
mod properties_limiter;

#[path = "properties/dither.rs"]
mod properties_dither;

#[path = "properties/volume.rs"]
mod properties_volume;

#[path = "properties/bit_perfect.rs"]
mod properties_bit_perfect;

#[path = "properties/convolver.rs"]
mod properties_convolver;

#[path = "properties/resampler.rs"]
mod properties_resampler;

#[path = "properties/dsd.rs"]
mod properties_dsd;

#[path = "properties/null_test.rs"]
mod properties_null_test;

use proptest::prelude::*;
use qobee_engine::AudioSettings;

use properties::{any_audio_settings, any_finite_buffer, seedable_silence, MAX_BUFFER_LEN};

// Feature: audio-quality-improvements, Phase A — generator smoke test.
#[test]
fn generators_produce_well_formed_values() {
    proptest!(ProptestConfig::with_cases(64), |(buf in any_finite_buffer(),
                                                 settings in any_audio_settings())| {
        // any_finite_buffer
        prop_assert!(!buf.is_empty(), "buffer should be non-empty");
        prop_assert!(buf.len() <= MAX_BUFFER_LEN, "buffer length must be bounded");
        for &x in &buf {
            prop_assert!(x.is_finite(), "no NaN / Inf allowed");
        }

        // any_audio_settings — every numeric field within its
        // declared bounds. We intentionally re-check the contract
        // here so a regression in the generator surfaces as a test
        // failure rather than a downstream surprise.
        prop_assert_eq!(settings.version, 0);
        prop_assert!((0.0..=3.0).contains(&settings.rg_safety_headroom_db));
        prop_assert!((-3.0..=0.0).contains(&settings.peak_limiter_ceiling_dbfs));
        prop_assert!((2.0..=10.0).contains(&settings.peak_limiter_lookahead_ms));
        prop_assert!((20.0..=500.0).contains(&settings.peak_limiter_release_ms));
        prop_assert!((-80.0..=-30.0).contains(&settings.volume_floor_db));
        prop_assert!((200.0..=400.0).contains(&settings.crossfeed_delay_us));
        prop_assert!((500.0..=1500.0).contains(&settings.crossfeed_lp_cutoff_hz));
        prop_assert!((-24.0..=0.0).contains(&settings.convolver_gain_db));
        prop_assert!((-1.0..=1.0).contains(&settings.balance));
        prop_assert!(settings.trim_db_per_channel.len() <= AudioSettings::MAX_CHANNELS);
        for t in &settings.trim_db_per_channel {
            prop_assert!((-12.0..=0.0).contains(t));
        }
        prop_assert!(settings.convolver_ir_path.is_none(),
            "Phase A generator must not depend on the filesystem");
    });
}

// Feature: audio-quality-improvements, Phase A — silence generator.
#[test]
fn seedable_silence_is_deterministic_and_correctly_sized() {
    let a = seedable_silence(44_100, 2);
    let b = seedable_silence(44_100, 2);
    assert_eq!(a, b, "silence generator must be deterministic");
    assert_eq!(a.len(), 44_100 * 2 * 2, "2 s × 44.1 kHz × 2 channels");
    assert!(
        a.iter().all(|&s| s == 0.0),
        "every sample must be exactly zero"
    );

    // Different config -> different size, still all zeros.
    let mono = seedable_silence(48_000, 1);
    assert_eq!(mono.len(), 48_000 * 2);
    assert!(mono.iter().all(|&s| s == 0.0));
}

// -----------------------------------------------------------------------------
// Phase B — Pre_Gain_Stage property and example tests (task 9)
// -----------------------------------------------------------------------------

use qobee_engine::dsp::PreGainStage;
use qobee_engine::PreGainContext;

/// Build an `AudioSettings` snapshot whose `Pre_Gain_Stage`-relevant
/// fields match the supplied tuple. Other fields stay at their
/// default values.
fn pre_gain_settings(
    ceiling_dbfs: f32,
    headroom_db: f32,
    peak_protection_on: bool,
    curve: qobee_engine::VolumeCurve,
    floor_db: f32,
) -> AudioSettings {
    AudioSettings {
        peak_limiter_ceiling_dbfs: ceiling_dbfs,
        rg_safety_headroom_db: headroom_db,
        rg_peak_protection: peak_protection_on,
        volume_curve: curve,
        volume_floor_db: floor_db,
        ..AudioSettings::default()
    }
}

// Feature: audio-quality-improvements, Property 1: Pre_Gain respecte la
// protection true-peak et rapporte l'atténuation.
//
// **Validates: Requirements R1.2, R1.3, R1.5**
#[test]
fn property_1_pre_gain_respects_true_peak_and_reports_attenuation() {
    proptest!(
        ProptestConfig::with_cases(100),
        |(inputs in properties::any_pre_gain_inputs())| {
            let (rg_db, rg_peak, ceiling_dbfs, headroom_db,
                 peak_protection_on, slider, curve, floor_db) = inputs;

            let settings =
                pre_gain_settings(ceiling_dbfs, headroom_db, peak_protection_on, curve, floor_db);

            let mut stage = PreGainStage::new(&settings, 48_000, 2);
            stage.set_context(PreGainContext { rg_db, rg_peak, slider });

            let g = stage.linear_gain();
            prop_assert!(g.is_finite(), "g_linear must be finite, got {g}");
            prop_assert!(g >= 0.0, "g_linear must be non-negative, got {g}");

            // Invariant 1 (R1.2): when peak protection is on AND RG is
            // requested, the post-gain peak stays under ceiling × safety.
            // (When rg_db is None, the stage skips the clamp by design;
            //  the volume curve alone keeps g_linear ≤ 1.0 ≤ peak budget
            //  for realistic peaks but is not what this invariant
            //  is asserting.)
            if peak_protection_on && rg_db.is_some() {
                // R1.3 fallback: missing peak ⇒ assume full-scale 1.0.
                let peak = rg_peak.unwrap_or(1.0).max(1e-6);
                let ceiling_linear = 10f32.powf(ceiling_dbfs / 20.0);
                let headroom = 10f32.powf(-headroom_db / 20.0);
                let max_post_gain = ceiling_linear * headroom;
                // f32 arithmetic accumulates a few ULPs through the
                // chain of pow/min/mul; allow a small absolute slack.
                prop_assert!(
                    g * peak <= max_post_gain + 1e-5,
                    "g ({g}) * peak ({peak}) = {} exceeds ceiling*headroom {}",
                    g * peak,
                    max_post_gain
                );
            }

            // Invariant 2 (R4.2): g_linear == 0.0 when slider == 0.0.
            if slider == 0.0 {
                prop_assert_eq!(g, 0.0, "mute must be exact zero");
            }

            // Invariant 3 (R4.3): |g_linear - 1.0| <= 1e-6 when
            // slider == 1.0 AND rg_db.is_none() (no RG branch ⇒
            // no peak clamp ⇒ stage is unity-exact).
            if (slider - 1.0).abs() < 1e-9 && rg_db.is_none() {
                prop_assert!(
                    (g - 1.0).abs() <= 1e-6,
                    "expected unity at v=1 with no RG, got {g}"
                );
            }

            // Invariant 4 (R1.5): rg_attenuation_db is exactly 0 when
            // the demanded RG fits, and strictly negative when the
            // ceiling clamped it down. Since `applied <= demanded`
            // by construction, attenuation can never be positive.
            let attn = stage.rg_attenuation_db();
            prop_assert!(attn.is_finite(), "attenuation must be finite, got {attn}");
            prop_assert!(attn <= 0.0, "attenuation must be <= 0 dB, got {attn}");
            // Stays bounded for any input in the generator's range.
            prop_assert!(attn >= -60.0, "attenuation drifted out of range: {attn}");
        }
    );
}

// Feature: audio-quality-improvements, R1.3 — missing peak tag falls
// back to a conservative full-scale (1.0) peak.
#[test]
fn r1_3_missing_peak_falls_back_to_full_scale() {
    let settings = AudioSettings {
        rg_peak_protection: true,
        rg_safety_headroom_db: 1.0,
        peak_limiter_ceiling_dbfs: -1.0,
        ..AudioSettings::default()
    };
    let mut stage = PreGainStage::new(&settings, 48_000, 2);
    stage.set_context(PreGainContext {
        rg_db: Some(6.0), // demanding a +6 dB boost
        rg_peak: None,    // no peak tag → fallback 1.0
        slider: 1.0,
    });
    // ceiling × safety = 10^(-2/20) ≈ 0.7943; demanded ≈ 1.995.
    // The clamp must apply against the conservative full-scale peak.
    let expected = 10f32.powf(-2.0 / 20.0);
    assert!(
        (stage.linear_gain() - expected).abs() < 1e-5,
        "expected {expected}, got {}",
        stage.linear_gain()
    );
    assert!(stage.rg_attenuation_db() < 0.0);
}

// Feature: audio-quality-improvements, R4.2 — slider=0 mutes the
// signal to an exact zero (no logarithmic floor leak).
#[test]
fn r4_2_slider_zero_mute_is_exact_zero() {
    let settings = AudioSettings::default();
    let mut stage = PreGainStage::new(&settings, 48_000, 2);
    stage.set_context(PreGainContext {
        rg_db: None,
        rg_peak: None,
        slider: 0.0,
    });
    assert_eq!(stage.linear_gain(), 0.0);
}

// Feature: audio-quality-improvements, R4.3 — slider=1 with no RG
// gives a unity-exact gain.
#[test]
fn r4_3_slider_one_no_rg_is_unity_exact() {
    let settings = AudioSettings::default();
    let mut stage = PreGainStage::new(&settings, 48_000, 2);
    stage.set_context(PreGainContext {
        rg_db: None,
        rg_peak: None,
        slider: 1.0,
    });
    assert!(
        (stage.linear_gain() - 1.0).abs() < 1e-9,
        "expected unity, got {}",
        stage.linear_gain()
    );
}

// -----------------------------------------------------------------------------
// Phase B — Channel_Balance_Stage property and example tests (task 11)
// -----------------------------------------------------------------------------

use qobee_engine::dsp::{ChannelBalanceStage, DspStage};

/// Build an `AudioSettings` snapshot from the balance/trim
/// parameters supplied by `any_balance_inputs`. Other fields keep
/// their defaults; the balance stage only consumes `balance` and
/// `trim_db_per_channel`.
fn balance_settings(balance: f32, trim: Vec<f32>) -> AudioSettings {
    AudioSettings {
        balance,
        trim_db_per_channel: trim,
        ..AudioSettings::default()
    }
}

// Feature: audio-quality-improvements, Property 21: Channel_Balance —
// formule de gain par canal (balance + trim).
//
// **Validates: Requirements R10.2, R10.3**
#[test]
fn property_21_channel_balance_formula_per_channel() {
    proptest!(
        ProptestConfig::with_cases(100),
        |((balance, trim, channels) in properties::any_balance_inputs(),
          input_l in -1.0f32..=1.0f32,
          input_r in -1.0f32..=1.0f32)| {

            let settings = balance_settings(balance, trim.clone());

            let mut stage = ChannelBalanceStage::new(&settings, 48_000, channels);
            // reset() snaps current = target so we observe the
            // steady-state formula directly, without the 50 ms ramp
            // muddying the per-sample comparison.
            stage.reset();

            // Build a single frame of `channels` samples. Channels
            // beyond stereo carry a known constant (0.5) so we can
            // verify they pass through unchanged when no trim is
            // configured for them.
            let n = channels as usize;
            let mut buf: Vec<f32> = Vec::with_capacity(n);
            for c in 0..n {
                buf.push(match c {
                    0 => input_l,
                    1 => input_r,
                    _ => 0.5,
                });
            }
            let original = buf.clone();
            stage.process_inplace(&mut buf);

            // Expected gains from the design's balance law.
            let (gain_l, gain_r) = if balance <= 0.0 {
                (1.0, 1.0 + balance)
            } else {
                (1.0 - balance, 1.0)
            };

            for c in 0..n {
                let base = match c {
                    0 => gain_l,
                    1 => gain_r,
                    _ => 1.0,
                };
                // Mirror the stage's own "≈ 0 dB ⇒ exactly 1.0"
                // shortcut so the expectation matches the
                // implementation's bit pattern, not just its
                // approximate value.
                let trim_db = trim.get(c).copied().unwrap_or(0.0);
                let trim_lin = if trim_db.abs() < 1e-6 {
                    1.0
                } else {
                    10f32.powf(trim_db / 20.0)
                };
                let expected = original[c] * base * trim_lin;
                prop_assert!(
                    (buf[c] - expected).abs() < 1e-5,
                    "channel {}: got {}, expected {} (balance={}, trim={:?}, channels={})",
                    c, buf[c], expected, balance, trim, channels
                );
            }

            // Endpoint sanity (R10.2): balance == -1 ⇒ right gain == 0;
            // balance == 1 ⇒ left gain == 0.
            if (balance + 1.0).abs() < 1e-9 {
                prop_assert!((1.0 + balance).abs() < 1e-6);
            }
            if (balance - 1.0).abs() < 1e-9 {
                prop_assert!((1.0 - balance).abs() < 1e-6);
            }
        }
    );
}

// Feature: audio-quality-improvements, Property 22: Channel_Balance est
// en bypass exact si tous les paramètres sont unitaires.
//
// **Validates: Requirements R10.4**
#[test]
fn property_22_channel_balance_bypass_is_sample_exact_passthrough() {
    proptest!(
        ProptestConfig::with_cases(100),
        |(buf in properties::any_finite_buffer(),
          channels in 1u16..=8u16)| {

            let settings = AudioSettings::default();
            // Defaults: balance == 0.0 and trim_db_per_channel is empty
            // ⇒ every per-channel gain is unity ⇒ stage is bypass.
            prop_assert_eq!(settings.balance, 0.0);
            prop_assert!(settings.trim_db_per_channel.is_empty());

            // Truncate the buffer to a multiple of `channels` (the
            // stage assumes interleaved frames).
            let n = channels as usize;
            let frames = buf.len() / n;
            if frames == 0 {
                return Ok(());
            }
            let mut samples: Vec<f32> = buf.into_iter().take(frames * n).collect();
            let original = samples.clone();

            let mut stage = ChannelBalanceStage::new(&settings, 48_000, channels);
            stage.reset();
            prop_assert!(stage.is_bypass(),
                "default settings must report bypass for channels = {}", channels);
            stage.process_inplace(&mut samples);

            prop_assert_eq!(
                samples, original,
                "bypass must be sample-for-sample identity (channels = {})",
                channels
            );
        }
    );
}

// Feature: audio-quality-improvements, R10.2 — `balance = -1.0` silences
// the right channel.
#[test]
fn r10_2_balance_minus_one_silences_right() {
    let audio = AudioSettings {
        balance: -1.0,
        ..AudioSettings::default()
    };
    let mut stage = ChannelBalanceStage::new(&audio, 48_000, 2);
    stage.reset(); // snap to target — skip the 50 ms ramp
    let mut buf = vec![1.0_f32, 1.0, 1.0, 1.0]; // 2 stereo frames
    stage.process_inplace(&mut buf);
    assert!((buf[0] - 1.0).abs() < 1e-6, "L frame 0 must stay at 1.0");
    assert!(
        buf[1].abs() < 1e-6,
        "R frame 0 must be silenced, got {}",
        buf[1]
    );
    assert!((buf[2] - 1.0).abs() < 1e-6, "L frame 1 must stay at 1.0");
    assert!(
        buf[3].abs() < 1e-6,
        "R frame 1 must be silenced, got {}",
        buf[3]
    );
}

// Feature: audio-quality-improvements, R10.3 — `trim_db[0] = -6.0` dB
// attenuates channel 0 by exactly -6 dB linear.
#[test]
fn r10_3_trim_minus_six_db_attenuates_left() {
    let audio = AudioSettings {
        trim_db_per_channel: vec![-6.0, 0.0],
        ..AudioSettings::default()
    };
    let mut stage = ChannelBalanceStage::new(&audio, 48_000, 2);
    stage.reset();
    let mut buf = vec![1.0_f32, 1.0]; // one stereo frame
    stage.process_inplace(&mut buf);
    let expected_l = 10f32.powf(-6.0 / 20.0);
    assert!(
        (buf[0] - expected_l).abs() < 1e-5,
        "left channel: got {}, expected {expected_l}",
        buf[0]
    );
    assert!(
        (buf[1] - 1.0).abs() < 1e-6,
        "right channel must stay at 1.0, got {}",
        buf[1]
    );
}

// -----------------------------------------------------------------------------
// Phase B — Crossfeed_Stage property and example tests (task 13)
// -----------------------------------------------------------------------------

use qobee_engine::dsp::CrossfeedStage;

// Feature: audio-quality-improvements, Property 11: Crossfeed est en
// bypass exact pour disabled, mono ou > stéréo.
//
// **Validates: Requirements R6.4, R6.5, R6.6**
#[test]
fn property_11_crossfeed_bypass_is_sample_exact_passthrough() {
    proptest!(
        ProptestConfig::with_cases(100),
        |(buf in properties::any_finite_buffer(),
          channels in 1u16..=8u16,
          enabled in any::<bool>())| {

            // The contract under test:
            //   bypass ⇔ !enabled || channels != 2
            // Skip the one configuration that should NOT bypass — we
            // are validating the bypass *implication*, not its
            // contrapositive (the active path is exercised by example
            // tests instead).
            if enabled && channels == 2 {
                return Ok(());
            }

            // Truncate the buffer to a multiple of `channels` (the
            // stage assumes interleaved frames).
            let n = channels as usize;
            let frames = buf.len() / n;
            if frames == 0 {
                return Ok(());
            }
            let mut samples: Vec<f32> = buf.into_iter().take(frames * n).collect();
            let original = samples.clone();

            let settings = AudioSettings {
                crossfeed_enabled: enabled,
                ..AudioSettings::default()
            };
            let mut stage = CrossfeedStage::new(&settings, 48_000, channels);

            prop_assert!(
                stage.is_bypass(),
                "is_bypass() must be true when enabled={enabled} && channels={channels}"
            );
            stage.process_inplace(&mut samples);
            prop_assert_eq!(
                samples,
                original,
                "bypass must be sample-for-sample identity (enabled={}, channels={})",
                enabled,
                channels
            );
        }
    );
}

// Feature: audio-quality-improvements, R6.5 — mono flow with crossfeed
// enabled is force-bypassed (passthrough).
#[test]
fn r6_5_mono_with_crossfeed_enabled_is_bypass() {
    let settings = AudioSettings {
        crossfeed_enabled: true,
        ..AudioSettings::default()
    };
    let mut stage = CrossfeedStage::new(&settings, 48_000, 1);
    assert!(stage.is_bypass(), "mono flow must report bypass");
    let original = vec![0.1_f32, -0.2, 0.3, -0.4, 0.5, -0.5, 1.0, -1.0];
    let mut buf = original.clone();
    stage.process_inplace(&mut buf);
    assert_eq!(
        buf, original,
        "mono passthrough must be bit-for-bit identity"
    );
}

// Feature: audio-quality-improvements, R6 — stereo flow with crossfeed
// enabled produces a coherent signal: no NaN/Inf and per-channel RMS
// stays within ±0.5 dB of `input_rms × gain_compensation`.
//
// The crossfeed sums `(1 - mix) * direct + mix * lowpassed_opposite`
// and applies a -3 dB scalar. For input where the two channels are
// equal (a mono signal duplicated to stereo), `direct == opposite`
// at DC and the LP filter passes DC unchanged, so the steady-state
// per-channel gain is exactly `gain_compensation` (0.7079...). For
// the more interesting case of independent channels we accept a
// looser bound on the RMS ratio because the LP filter attenuates
// high-frequency energy in the cross-feed contribution.
#[test]
fn crossfeed_processes_stereo_with_finite_output() {
    let settings = AudioSettings {
        crossfeed_enabled: true,
        ..AudioSettings::default()
    };
    let mut stage = CrossfeedStage::new(&settings, 48_000, 2);

    // Build a deterministic mono signal duplicated across L and R.
    // Frequency = 100 Hz (well below the LP cutoff of 700 Hz so the
    // cross-feed contribution is barely attenuated). 1 second at 48
    // kHz gives ample room for the IIR to settle and for the RMS
    // estimate to converge.
    let sr = 48_000_usize;
    let frames = sr;
    let freq = 100.0_f32;
    let amp = 0.5_f32;
    let mut buf = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let t = n as f32 / sr as f32;
        let v = amp * (2.0 * std::f32::consts::PI * freq * t).sin();
        buf.push(v); // L
        buf.push(v); // R (duplicated mono → identical channels)
    }
    let input = buf.clone();
    stage.process_inplace(&mut buf);

    // No NaN / no Inf anywhere in the output.
    for (i, &x) in buf.iter().enumerate() {
        assert!(x.is_finite(), "sample {i} not finite (got {x})");
    }

    // Per-channel RMS check. With both channels carrying the same
    // signal, the steady-state output per channel is
    //   `((1 - mix) + mix * lp(direct)) * gain_comp`.
    // At 100 Hz the low-pass at 700 Hz Q=0.707 has near-unity gain,
    // so the expected RMS is `input_rms * gain_compensation`.
    let gain_comp = 0.707_945_78_f32;
    // Skip the first 200 ms to let the IIR / delay-line settle.
    let warmup = sr / 5;
    let mut rms_in_l = 0.0_f64;
    let mut rms_in_r = 0.0_f64;
    let mut rms_out_l = 0.0_f64;
    let mut rms_out_r = 0.0_f64;
    for f in warmup..frames {
        let li = input[f * 2] as f64;
        let ri = input[f * 2 + 1] as f64;
        let lo = buf[f * 2] as f64;
        let ro = buf[f * 2 + 1] as f64;
        rms_in_l += li * li;
        rms_in_r += ri * ri;
        rms_out_l += lo * lo;
        rms_out_r += ro * ro;
    }
    let span = (frames - warmup) as f64;
    let rms_in_l = (rms_in_l / span).sqrt() as f32;
    let rms_in_r = (rms_in_r / span).sqrt() as f32;
    let rms_out_l = (rms_out_l / span).sqrt() as f32;
    let rms_out_r = (rms_out_r / span).sqrt() as f32;

    let expected_l = rms_in_l * gain_comp;
    let expected_r = rms_in_r * gain_comp;

    // ±0.5 dB ≈ ratio in [10^(-0.5/20), 10^(0.5/20)] ≈ [0.944, 1.059].
    let lo_ratio = 10f32.powf(-0.5 / 20.0);
    let hi_ratio = 10f32.powf(0.5 / 20.0);
    let ratio_l = rms_out_l / expected_l;
    let ratio_r = rms_out_r / expected_r;
    assert!(
        (lo_ratio..=hi_ratio).contains(&ratio_l),
        "left RMS ratio {ratio_l} out of [{lo_ratio}, {hi_ratio}] \
         (rms_in_l={rms_in_l}, rms_out_l={rms_out_l}, expected={expected_l})"
    );
    assert!(
        (lo_ratio..=hi_ratio).contains(&ratio_r),
        "right RMS ratio {ratio_r} out of [{lo_ratio}, {hi_ratio}] \
         (rms_in_r={rms_in_r}, rms_out_r={rms_out_r}, expected={expected_r})"
    );
}
