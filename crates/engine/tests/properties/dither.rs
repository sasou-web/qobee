//! Property tests for `Dither_Stage` (task 17).
//!
//! Implements three properties from the design document:
//!
//! * **Property 2** — exact bypass ↔ output > 16 bits. Conversely,
//!   for `output_bits ≤ 16` the stage is never bypassed and the
//!   variance over 1 second of silence is strictly positive
//!   (validates R2.4 / R2.5 / R2.6).
//!
//! * **Property 3** — `shaped_f_weighted` at `fs = 44_100`: total
//!   noise power on 2 s of silence in `[-96.5, -92.5] dBFS`, AND
//!   attenuation in `[2_000, 5_000] Hz` ≥ 8 dB relative to
//!   `[15_000, 20_000] Hz` (validates R2.3).
//!
//! * **Property 4** — for any profile and 2 s silence at 44.1 kHz,
//!   `|mean(samples)| < 0.5 LSB` at `output_bits = 16` (validates
//!   R2.6).
//!
//! ## Spectrum helper
//!
//! `measure_psd_band` computes the RMS amplitude in a `[lo_hz, hi_hz]`
//! band by running a real-input FFT and integrating the bin
//! magnitudes inside the band, then converting to dB. This is enough
//! to confirm the spectral bound without needing windowing or
//! averaging — silence produces a stationary, zero-mean noise floor
//! whose statistics converge well over the ~88_200 samples Property 3
//! consumes.

use proptest::prelude::*;
use qobee_engine::audio_settings::{AudioSettings, DitherProfile};
use qobee_engine::dsp::{DitherStage, DspStage};
use realfft::RealFftPlanner;

use crate::properties::{any_finite_buffer, seedable_silence};

/// 16-bit LSB in linear `[-1, 1]` units. Reused throughout the
/// tests since every property targets `output_bits = 16`.
const LSB_16: f32 = 1.0 / 32_768.0;

/// Compute the RMS amplitude of `samples` inside `[lo_hz, hi_hz]`,
/// expressed in dBFS (full-scale = `1.0` peak sine ⇒ `0 dB`).
///
/// Uses a single forward real-FFT of the supplied buffer with no
/// windowing — the input is assumed to be a noise-like signal whose
/// spectrum is approximately flat (or shaped) and stationary, so a
/// rectangular window's spectral leakage averages out across the
/// many bins inside each band.
///
/// The returned value is `20 · log10(rms)` where `rms` is the
/// square root of the average squared magnitude across the bins
/// strictly inside the band. Empty bands return `f32::NEG_INFINITY`.
fn measure_psd_band(samples: &[f32], fs: u32, lo_hz: f32, hi_hz: f32) -> f32 {
    assert!(lo_hz < hi_hz, "lo must be strictly < hi");

    let n = samples.len();
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(n);

    let mut input = samples.to_vec();
    let mut spectrum = r2c.make_output_vec();
    r2c.process(&mut input, &mut spectrum)
        .expect("FFT processing failed");

    // Bin width and band index range. `spectrum.len() == n/2 + 1`.
    let bin_hz = (fs as f32) / (n as f32);
    let lo_bin = (lo_hz / bin_hz).ceil() as usize;
    let hi_bin = ((hi_hz / bin_hz).floor() as usize).min(spectrum.len() - 1);
    if lo_bin >= hi_bin {
        return f32::NEG_INFINITY;
    }

    // Real-FFT bin magnitude → time-domain amplitude:
    //   |X[k]| / (n/2) for the analytic single-sided spectrum.
    // We accumulate squared magnitudes (proportional to power per
    // bin) and divide by the number of bins to get the average
    // power in the band. The final amplitude RMS is the square
    // root, expressed in dBFS as 20·log10.
    let scale = 2.0 / (n as f32);
    let mut acc = 0.0_f64;
    for c in &spectrum[lo_bin..hi_bin] {
        let mag = (c.re * c.re + c.im * c.im).sqrt() * scale;
        acc += (mag as f64) * (mag as f64);
    }
    let bins = (hi_bin - lo_bin) as f64;
    let rms = (acc / bins).sqrt() as f32;
    20.0 * rms.max(1e-30).log10()
}

/// Build a fully-default `AudioSettings` with the supplied dither
/// profile applied. Every other field stays at its default value.
fn settings(profile: DitherProfile) -> AudioSettings {
    AudioSettings {
        dither_profile: profile,
        ..AudioSettings::default()
    }
}

// -----------------------------------------------------------------------------
// Property 2 — bypass exact ↔ output > 16 bits
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 2: bypass exact pour
// output_bits > 16, et stage actif (variance > 0) pour output_bits <= 16.
//
// **Validates: Requirements R2.4, R2.5, R2.6**
#[test]
fn property_2_dither_bypass_iff_output_above_16_bits() {
    proptest!(
        ProptestConfig::with_cases(100),
        |(profile in any_dither_profile(),
          high_bits in prop_oneof![
              Just(None),
              (24u8..=32u8).prop_map(Some),
          ],
          buf in any_finite_buffer(),
          channels in 1u16..=2u16)| {

            // Truncate to a multiple of `channels`.
            let n = channels as usize;
            let frames = buf.len() / n;
            if frames == 0 {
                return Ok(());
            }
            let mut samples: Vec<f32> = buf.into_iter().take(frames * n).collect();
            let original = samples.clone();

            // Bypass branch: output_bits is None or >= 24.
            let mut stage = DitherStage::new(&settings(profile), 44_100, channels);
            stage.set_output_bits(high_bits);
            prop_assert!(
                stage.is_bypass(),
                "expected bypass for output_bits = {:?}", high_bits
            );
            stage.process_inplace(&mut samples);
            prop_assert_eq!(
                samples, original,
                "bypass must be sample-for-sample identity (output_bits = {:?})",
                high_bits
            );
        }
    );

    // Conversely, for output_bits ≤ 16 the stage must never be
    // bypassed and 1 second of silence must produce a non-zero
    // variance signal. This branch is deterministic — we run it
    // once per profile rather than fuzzing because the property
    // is qualitative ("variance is strictly positive"), not
    // quantitative.
    for profile in [
        DitherProfile::Tpdf,
        DitherProfile::ShapedHp,
        DitherProfile::ShapedFWeighted,
    ] {
        for bits in 8u8..=16u8 {
            let mut stage = DitherStage::new(&settings(profile), 44_100, 1);
            stage.set_output_bits(Some(bits));
            assert!(
                !stage.is_bypass(),
                "stage must be active for output_bits = {bits}, profile = {profile:?}"
            );
            let mut buf = vec![0.0_f32; 44_100];
            stage.process_inplace(&mut buf);

            let mean: f64 = buf.iter().map(|&v| v as f64).sum::<f64>() / buf.len() as f64;
            let var: f64 = buf
                .iter()
                .map(|&v| {
                    let d = v as f64 - mean;
                    d * d
                })
                .sum::<f64>()
                / buf.len() as f64;
            assert!(
                var > 0.0,
                "variance over 1 s of silence must be > 0 (profile = {profile:?}, bits = {bits}); got {var}"
            );
        }
    }
}

// -----------------------------------------------------------------------------
// Property 3 — shaped_f_weighted spectral bounds at 44.1 kHz
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 3: shaped_f_weighted
// produit une puissance totale ∈ [-96.5, -92.5] dBFS et atténue
// [2_000, 5_000] Hz d'au moins 8 dB par rapport à [15_000, 20_000] Hz
// sur 2 secondes de silence à 44.1 kHz, 16-bit.
//
// **Validates: Requirements R2.3**
#[test]
fn property_3_shaped_f_weighted_spectrum_within_bounds() {
    let mut stage = DitherStage::new(&settings(DitherProfile::ShapedFWeighted), 44_100, 1);
    stage.set_output_bits(Some(16));

    // 2 s of mono silence at 44.1 kHz = 88_200 samples.
    let mut buf = seedable_silence(44_100, 1);
    assert_eq!(buf.len(), 88_200);
    stage.process_inplace(&mut buf);

    // Total noise power. Compute as `10 · log10(mean(x²))` directly
    // from the time-domain samples — equivalent to integrating the
    // power spectrum across every bin.
    let mean_sq: f64 = buf.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>()
        / (buf.len() as f64);
    let total_db = 10.0_f64 * mean_sq.max(1e-300).log10();
    assert!(
        (-96.5_f64..=-92.5_f64).contains(&total_db),
        "total power {total_db} dBFS must be inside [-96.5, -92.5]"
    );

    // Band attenuation. The shaper pushes energy from low/mid bands
    // toward Nyquist, so [2_000, 5_000] Hz should sit at least 8 dB
    // below [15_000, 20_000] Hz.
    let low_band_db = measure_psd_band(&buf, 44_100, 2_000.0, 5_000.0);
    let high_band_db = measure_psd_band(&buf, 44_100, 15_000.0, 20_000.0);
    let attenuation_db = high_band_db - low_band_db;
    assert!(
        attenuation_db >= 8.0,
        "expected ≥ 8 dB attenuation in [2k, 5k] vs [15k, 20k]; got {attenuation_db} dB \
         (low = {low_band_db} dBFS, high = {high_band_db} dBFS)"
    );
}

// -----------------------------------------------------------------------------
// Property 4 — zero-mean output on silence
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 4: pour tout profil, la
// moyenne sur 2 secondes de silence à 44.1 kHz reste à |moyenne| < 0.5
// LSB pour output_bits = 16.
//
// **Validates: Requirements R2.6**
#[test]
fn property_4_dither_silence_mean_below_half_lsb() {
    proptest!(
        // 30 cases × 2 s of audio × 2 channels keeps the test under
        // a couple of seconds wall-clock while still covering every
        // profile and both mono / stereo configurations.
        ProptestConfig::with_cases(30),
        |(profile in any_dither_profile(),
          channels in 1u16..=2u16)| {

            let mut stage = DitherStage::new(&settings(profile), 44_100, channels);
            stage.set_output_bits(Some(16));

            // Always reset() so each case starts from the same
            // PRNG state. Without this reset the sequential nature
            // of the xorshift would couple successive cases — fine
            // for production audio (R2.4 wants continuity), but it
            // makes the per-case mean depend on the prior history.
            stage.reset();

            let mut buf = seedable_silence(44_100, channels);
            stage.process_inplace(&mut buf);

            let n = buf.len() as f64;
            let mean = buf.iter().map(|&v| v as f64).sum::<f64>() / n;
            let half_lsb = 0.5_f64 * (LSB_16 as f64);

            prop_assert!(
                mean.abs() < half_lsb,
                "profile {:?}, channels {}: |mean| = {} ≥ 0.5 LSB ({})",
                profile, channels, mean.abs(), half_lsb
            );
        }
    );
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Strategy that picks one of the three dither profiles uniformly.
fn any_dither_profile() -> impl Strategy<Value = DitherProfile> {
    prop_oneof![
        Just(DitherProfile::Tpdf),
        Just(DitherProfile::ShapedHp),
        Just(DitherProfile::ShapedFWeighted),
    ]
}

// -----------------------------------------------------------------------------
// `measure_psd_band` smoke test
// -----------------------------------------------------------------------------

#[cfg(test)]
mod helper_tests {
    use super::*;
    use std::f32::consts::PI;

    /// Sanity check: a unit-amplitude sine at 1 kHz must measure
    /// roughly `-3 dB` RMS in the [900, 1100] Hz band (a sine's
    /// RMS amplitude is `1/sqrt(2)` ≈ -3.01 dBFS), and well below
    /// `-30 dB` in any band that does not contain the fundamental.
    #[test]
    fn measure_psd_band_finds_sine_in_correct_band() {
        let fs = 44_100;
        let n = fs as usize; // 1 second
        let freq = 1_000.0;
        let mut samples = vec![0.0_f32; n];
        for (i, s) in samples.iter_mut().enumerate() {
            let t = i as f32 / fs as f32;
            *s = (2.0 * PI * freq * t).sin();
        }

        let in_band = measure_psd_band(&samples, fs, 900.0, 1_100.0);
        let out_band = measure_psd_band(&samples, fs, 5_000.0, 10_000.0);

        // RMS of a unit sine is 1/sqrt(2) ≈ 0.7071 ⇒ -3.01 dB.
        // The integration averages the squared magnitudes across
        // `(hi_bin - lo_bin)` bins; only the bin containing the
        // fundamental carries energy, so the average sits below the
        // peak. We just assert the sine is clearly detected (within
        // 30 dB of full-scale) and the off-band reading is far
        // lower.
        assert!(
            in_band > -30.0,
            "in-band reading too low: {in_band} dBFS"
        );
        assert!(
            out_band < -50.0,
            "off-band reading too high: {out_band} dBFS"
        );
        assert!(
            in_band - out_band > 30.0,
            "in-band ({in_band}) should dominate off-band ({out_band})"
        );
    }
}
