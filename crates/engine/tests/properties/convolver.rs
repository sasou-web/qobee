//! Property tests for `Convolver_Stage` (task 35).
//!
//! Covers four design-document properties:
//!
//! * **Property 17** (R9.4) — channel-by-channel processing: feeding
//!   `(left, 0)` and `(0, right)` separately and summing the outputs
//!   equals processing `(left, right)` in one pass (linearity per
//!   channel, no cross-channel coupling).
//!
//! * **Property 18** (R9.5) — exact bypass: when `enabled = false`
//!   OR `ir` is absent, output is sample-for-sample identical to
//!   input.
//!
//! * **Property 19** (R9.7) — `ir = [1.0]` (delta) is passthrough
//!   (within FFT block delay).
//!
//! * **Property 20** (R9.7) — `ir = [0.0; N]` produces silence on
//!   output.

use proptest::prelude::*;
use qobee_engine::audio_settings::AudioSettings;
use qobee_engine::dsp::{ConvolverStage, DspStage};

use crate::properties::{any_finite_buffer, any_ir};

/// Block size used by `Convolver_Stage`. Hardcoded in
/// `crates/engine/src/dsp/convolver.rs::BLOCK_SIZE`; mirrored here so
/// the tests can budget enough warmup frames for the partitioned
/// FFT to fill its first block.
const CONVOLVER_BLOCK: usize = 256;

fn enabled_settings() -> AudioSettings {
    AudioSettings {
        convolver_enabled: true,
        // Unity gain compensation so set_ir does not scale the IR
        // (the gain math lives in the loader; the engine stage
        // currently does no scaling — this just nails down the
        // expectation).
        convolver_gain_db: 0.0,
        ..AudioSettings::default()
    }
}

/// Build a fresh stage with the supplied IR loaded.
fn stage_with_ir(ir_l: Vec<f32>, ir_r: Vec<f32>) -> ConvolverStage {
    let mut s = ConvolverStage::new(&enabled_settings(), 48_000, 2);
    s.set_ir(ir_l, ir_r);
    s
}

/// Convolve `(in_l, in_r)` through a stage initialised with
/// `(ir_l, ir_r)` and return the interleaved stereo output. Length
/// of the input matches `frames`; both channels carry independent
/// signals.
fn process_stereo(ir_l: &[f32], ir_r: &[f32], in_l: &[f32], in_r: &[f32]) -> Vec<f32> {
    let frames = in_l.len();
    assert_eq!(frames, in_r.len());
    let mut s = stage_with_ir(ir_l.to_vec(), ir_r.to_vec());
    let mut buf = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        buf.push(in_l[f]);
        buf.push(in_r[f]);
    }
    s.process_inplace(&mut buf);
    buf
}

// -----------------------------------------------------------------------------
// Property 17 — channel-by-channel linearity
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 17: convolveur traite
// chaque canal indépendamment (pas de couplage cross-channel).
//
// **Validates: Requirements 9.4**
#[test]
fn property_17_convolver_per_channel_processing_no_cross_coupling() {
    proptest!(
        ProptestConfig::with_cases(40),
        |((ir_l, ir_r) in any_ir(64),
          input in proptest::collection::vec(-1.0f32..=1.0f32, 64..=256))| {

            // Build per-channel inputs from `input`: even indices
            // form the left channel, odd indices form the right.
            // Truncating to even length keeps frame counts aligned.
            let len = input.len() & !1;
            if len < 4 {
                return Ok(());
            }
            let mut in_l = Vec::with_capacity(len / 2);
            let mut in_r = Vec::with_capacity(len / 2);
            for (i, v) in input.iter().take(len).enumerate() {
                if i % 2 == 0 {
                    in_l.push(*v);
                } else {
                    in_r.push(*v);
                }
            }
            let frames = in_l.len();
            // Pad inputs so the FFT block delay does not bite the
            // sum-of-isolated check: we drop the first `BLOCK_SIZE`
            // samples of every output before comparing, so we need
            // at least `2 × BLOCK_SIZE` total frames.
            if frames < 2 * CONVOLVER_BLOCK {
                let pad = 2 * CONVOLVER_BLOCK - frames;
                in_l.extend(std::iter::repeat_n(0.0, pad));
                in_r.extend(std::iter::repeat_n(0.0, pad));
            }
            let frames = in_l.len();
            let zero = vec![0.0_f32; frames];

            let combined = process_stereo(&ir_l, &ir_r, &in_l, &in_r);
            let only_l = process_stereo(&ir_l, &ir_r, &in_l, &zero);
            let only_r = process_stereo(&ir_l, &ir_r, &zero, &in_r);

            // Assert combined ≈ only_l + only_r, channel by channel,
            // skipping the first BLOCK_SIZE frames where the FFT
            // partitioned tail is still warming up.
            for f in CONVOLVER_BLOCK..frames {
                let lhs_l = combined[f * 2];
                let lhs_r = combined[f * 2 + 1];
                let rhs_l = only_l[f * 2] + only_r[f * 2];
                let rhs_r = only_l[f * 2 + 1] + only_r[f * 2 + 1];
                prop_assert!(
                    (lhs_l - rhs_l).abs() < 1e-3,
                    "left channel mismatch at frame {f}: combined {}, isolated sum {}",
                    lhs_l, rhs_l
                );
                prop_assert!(
                    (lhs_r - rhs_r).abs() < 1e-3,
                    "right channel mismatch at frame {f}: combined {}, isolated sum {}",
                    lhs_r, rhs_r
                );
            }
        }
    );
}

// -----------------------------------------------------------------------------
// Property 18 — exact bypass
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 18: bypass exact
// quand enabled=false OU ir absente — sortie identique sample par
// sample à l'entrée.
//
// **Validates: Requirements 9.5**
#[test]
fn property_18_convolver_exact_bypass_passthrough() {
    proptest!(
        ProptestConfig::with_cases(100),
        |(buf in any_finite_buffer(),
          channels in 1u16..=8u16,
          enabled in any::<bool>(),
          load_ir in any::<bool>())| {

            // Truncate to a multiple of `channels` (the stage
            // assumes interleaved frames).
            let n = channels as usize;
            let frames = buf.len() / n;
            if frames == 0 {
                return Ok(());
            }
            let mut samples: Vec<f32> = buf.into_iter().take(frames * n).collect();
            let original = samples.clone();

            let settings = AudioSettings {
                convolver_enabled: enabled,
                ..AudioSettings::default()
            };
            let mut stage = ConvolverStage::new(&settings, 48_000, channels);

            // The Property 18 contract is: when disabled OR the IR
            // is missing, the stage is bypass-exact. We sample
            // every (enabled, load_ir) combination but only assert
            // bypass on the disabled-or-no-IR cells:
            //   * enabled = false, load_ir = false → bypass
            //   * enabled = false, load_ir = true  → bypass
            //   * enabled = true,  load_ir = false → bypass
            //   * enabled = true,  load_ir = true,
            //                       channels != 2  → bypass (mono /
            //                                       surround clause)
            // The "active" cell (enabled + IR + stereo) is exercised
            // by Properties 17, 19, and 20 instead.
            if load_ir {
                stage.set_ir(vec![0.5, 0.25], vec![0.5, 0.25]);
            }
            let active = enabled && load_ir && channels == 2;
            if active {
                return Ok(());
            }

            prop_assert!(
                stage.is_bypass(),
                "stage must report bypass (enabled={enabled}, load_ir={load_ir}, channels={channels})"
            );
            stage.process_inplace(&mut samples);
            prop_assert_eq!(
                samples, original,
                "bypass must be sample-for-sample identity \
                 (enabled={}, load_ir={}, channels={})",
                enabled, load_ir, channels
            );
        }
    );
}

// -----------------------------------------------------------------------------
// Property 19 — delta IR is passthrough
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 19: ir = [1.0] est
// passthrough (à la latence du bloc FFT près).
//
// **Validates: Requirements 9.7**
#[test]
fn property_19_convolver_delta_ir_is_passthrough() {
    proptest!(
        ProptestConfig::with_cases(40),
        |(left in proptest::collection::vec(-1.0f32..=1.0f32,
                                            CONVOLVER_BLOCK * 2..=CONVOLVER_BLOCK * 4),
          right in proptest::collection::vec(-1.0f32..=1.0f32,
                                             CONVOLVER_BLOCK * 2..=CONVOLVER_BLOCK * 4))| {

            // Match channel lengths by truncating to the smaller.
            let frames = left.len().min(right.len());
            let in_l = &left[..frames];
            let in_r = &right[..frames];

            // Delta IRs on both channels.
            let out = process_stereo(&[1.0], &[1.0], in_l, in_r);

            // After the BLOCK_SIZE warmup, the output equals the
            // input on every channel.
            for f in CONVOLVER_BLOCK..frames {
                let yl = out[f * 2];
                let yr = out[f * 2 + 1];
                prop_assert!(
                    (yl - in_l[f]).abs() < 1e-4,
                    "L channel mismatch at frame {f}: got {}, expected {}",
                    yl, in_l[f]
                );
                prop_assert!(
                    (yr - in_r[f]).abs() < 1e-4,
                    "R channel mismatch at frame {f}: got {}, expected {}",
                    yr, in_r[f]
                );
            }
        }
    );
}

// -----------------------------------------------------------------------------
// Property 20 — zero IR produces silence
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 20: ir = [0.0; N]
// produit du silence sur les deux canaux.
//
// **Validates: Requirements 9.7**
#[test]
fn property_20_convolver_zero_ir_produces_silence() {
    proptest!(
        ProptestConfig::with_cases(40),
        |(left in proptest::collection::vec(-1.0f32..=1.0f32,
                                            CONVOLVER_BLOCK..=CONVOLVER_BLOCK * 4),
          right in proptest::collection::vec(-1.0f32..=1.0f32,
                                             CONVOLVER_BLOCK..=CONVOLVER_BLOCK * 4),
          ir_len in 1usize..=64usize)| {

            let frames = left.len().min(right.len());
            let in_l = &left[..frames];
            let in_r = &right[..frames];

            let zero_ir_l = vec![0.0_f32; ir_len];
            let zero_ir_r = vec![0.0_f32; ir_len];
            let out = process_stereo(&zero_ir_l, &zero_ir_r, in_l, in_r);

            for f in 0..frames {
                let yl = out[f * 2];
                let yr = out[f * 2 + 1];
                prop_assert!(
                    yl.abs() < 1e-6,
                    "L channel non-silent at frame {f}: {yl}"
                );
                prop_assert!(
                    yr.abs() < 1e-6,
                    "R channel non-silent at frame {f}: {yr}"
                );
            }
        }
    );
}
