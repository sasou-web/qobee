//! Resampler quality property tests (task 31).
//!
//! Implements **Property 15** and **Property 16** from the design.
//! Both reuse `rubato::SincFixedIn` directly (mirroring how
//! `run_decoder_thread` builds its resampler) so the SNR figure
//! reflects exactly what the engine produces in production. The
//! reference signal is a 1 kHz, −6 dBFS sinusoid resampled from
//! 44_100 Hz to 48_000 Hz; the SNR is measured by FFT-integrating
//! the noise power in `[20, 20_000]` Hz minus the fundamental bin.
//!
//! Both tests are gated behind the `slow-tests` Cargo feature
//! (`#[cfg_attr(not(feature = "slow-tests"), ignore)]`) so the
//! standard CI suite stays under its time budget — each test
//! processes ~65_536 frames through a 256-tap sinc FIR.

use std::f32::consts::PI;

use realfft::RealFftPlanner;
use rubato::{Resampler, SincFixedIn};

use qobee_engine::audio_settings::ResamplerQuality;
use qobee_engine::backend_cpal_shared::sinc_params_for;

/// Number of resampled frames retained for the FFT measurement.
/// 65_536 is a power of two (cheap FFT) and at 48 kHz covers
/// ~1.365 s of audio, comfortably more than the resampler's
/// startup transient (a 256-tap sinc settles in < 6 ms).
const FFT_LEN: usize = 65_536;

/// Skip the first `WARMUP_FRAMES` resampled frames before the FFT
/// window starts. The sinc resampler emits a small ramp while its
/// internal delay-line fills; including those frames would smear
/// the fundamental across neighbouring bins and inflate the noise
/// estimate.
const WARMUP_FRAMES: usize = 4_096;

/// Measure the SNR (in dB) of `rubato::SincFixedIn` resampling a
/// 1 kHz, −6 dBFS sine from `src_sr` to `dst_sr` using the
/// production preset for `quality`. The measurement integrates the
/// noise power in `[20, 20_000]` Hz, excluding a small protective
/// band around the fundamental bin, and returns
/// `10·log10(P_signal / P_noise)`.
///
/// All work is performed on the resampler's first (mono) channel —
/// the SNR figure for a windowed-sinc with cubic interpolation is
/// independent of the channel count.
pub(super) fn measure_resampler_snr_db(
    quality: ResamplerQuality,
    src_sr: u32,
    dst_sr: u32,
    freq_hz: f32,
) -> f32 {
    // -------- 1. Build the resampler with the same params the
    //            engine uses.
    let chunk_size_in: usize = 1024;
    let n_channels = 1;
    let params = sinc_params_for(quality);
    let ratio = dst_sr as f64 / src_sr as f64;
    let mut resampler =
        SincFixedIn::<f32>::new(ratio, 1.1, params, chunk_size_in, n_channels)
            .expect("failed to build SincFixedIn for SNR measurement");

    // -------- 2. Generate enough source frames to produce
    //            (FFT_LEN + WARMUP_FRAMES) output frames after
    //            resampling.
    let target_out = FFT_LEN + WARMUP_FRAMES;
    // Source frames needed ≈ target_out / ratio, rounded up + a
    // generous safety margin so the inner loop never runs short.
    let src_frames_needed =
        ((target_out as f64) / ratio).ceil() as usize + 4 * chunk_size_in;
    let amp = 0.5_f32; // -6 dBFS
    let mut source_signal = Vec::<f32>::with_capacity(src_frames_needed);
    for n in 0..src_frames_needed {
        let t = n as f32 / src_sr as f32;
        source_signal.push(amp * (2.0 * PI * freq_hz * t).sin());
    }

    // -------- 3. Drive the resampler chunk-by-chunk and collect
    //            output frames.
    let mut output_buffer: Vec<Vec<f32>> = resampler.output_buffer_allocate(true);
    let mut collected: Vec<f32> = Vec::with_capacity(target_out + chunk_size_in);
    let mut cursor = 0_usize;
    while collected.len() < target_out {
        if cursor + chunk_size_in > source_signal.len() {
            panic!(
                "ran out of source frames: cursor={cursor}, len={}, collected={}",
                source_signal.len(),
                collected.len()
            );
        }
        let input_slice = &source_signal[cursor..cursor + chunk_size_in];
        let input: [&[f32]; 1] = [input_slice];
        let (_in_frames, out_frames) = resampler
            .process_into_buffer(&input, &mut output_buffer, None)
            .expect("resampler chunk failed");
        collected.extend_from_slice(&output_buffer[0][..out_frames]);
        cursor += chunk_size_in;
    }

    // -------- 4. Drop warmup frames and slice exactly FFT_LEN
    //            samples for the measurement window.
    assert!(
        collected.len() >= WARMUP_FRAMES + FFT_LEN,
        "not enough output frames collected"
    );
    let window: &[f32] = &collected[WARMUP_FRAMES..WARMUP_FRAMES + FFT_LEN];

    // -------- 5. FFT and SNR integration.
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(FFT_LEN);
    let mut input = window.to_vec();
    let mut spectrum = r2c.make_output_vec();
    r2c.process(&mut input, &mut spectrum)
        .expect("FFT failed");

    // Bin width and band indices.
    let bin_hz = (dst_sr as f32) / (FFT_LEN as f32);
    let lo_bin = (20.0_f32 / bin_hz).ceil() as usize;
    let hi_bin = ((20_000.0_f32 / bin_hz).floor() as usize).min(spectrum.len() - 1);
    assert!(lo_bin < hi_bin, "audible band collapsed to empty range");

    // Locate the fundamental bin and protect a small guard around
    // it (±3 bins ≈ ±2.2 Hz at 48 kHz / 65_536) so the residual
    // leakage from a non-perfectly-bin-aligned tone doesn't pollute
    // the noise estimate.
    let fund_bin = (freq_hz / bin_hz).round() as usize;
    let guard = 3_usize;
    let guard_lo = fund_bin.saturating_sub(guard);
    let guard_hi = (fund_bin + guard).min(spectrum.len() - 1);

    // Power per bin (proportional to |X[k]|²). We do not need to
    // normalise: the ratio cancels the scaling factor.
    let mut signal_power = 0.0_f64;
    let mut noise_power = 0.0_f64;
    for k in lo_bin..=hi_bin {
        let c = &spectrum[k];
        let p = (c.re as f64) * (c.re as f64) + (c.im as f64) * (c.im as f64);
        if k >= guard_lo && k <= guard_hi {
            signal_power += p;
        } else {
            noise_power += p;
        }
    }

    if noise_power <= 0.0 {
        // No measurable noise — return a very high SNR sentinel
        // rather than +inf so callers can still apply numeric
        // thresholds.
        return 300.0;
    }
    (10.0_f64 * (signal_power / noise_power).log10()) as f32
}

// -----------------------------------------------------------------------------
// Property 15 — Standard quality SNR ≥ 120 dB
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 15: Standard
// resampler quality SNR ≥ 120 dB on a 1 kHz / -6 dBFS sine
// resampled 44_100 → 48_000 Hz.
//
// **Validates: Requirements R8.3**
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn property_15_standard_resampler_snr_at_least_120_db() {
    let snr =
        measure_resampler_snr_db(ResamplerQuality::Standard, 44_100, 48_000, 1_000.0);
    assert!(
        snr >= 120.0,
        "Standard preset SNR {snr} dB below the 120 dB R8.3 threshold"
    );
}

// -----------------------------------------------------------------------------
// Property 16 — Best quality SNR ≥ 140 dB
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 16: Best resampler
// quality SNR ≥ 140 dB on the same scenario as Property 15.
//
// **Validates: Requirements R8.4**
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn property_16_best_resampler_snr_at_least_140_db() {
    let snr =
        measure_resampler_snr_db(ResamplerQuality::Best, 44_100, 48_000, 1_000.0);
    assert!(
        snr >= 140.0,
        "Best preset SNR {snr} dB below the 140 dB R8.4 threshold"
    );
}
