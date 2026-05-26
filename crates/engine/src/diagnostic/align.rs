//! FFT cross-correlation aligner + sample-level diff for the
//! null-test diagnostic (R11.4 / R11.5 / R11.6).
//!
//! The aligner runs a real FFT cross-correlation over the entire
//! supplied buffers and searches for the peak within a fixed
//! `[-MAX_LAG_SAMPLES, MAX_LAG_SAMPLES]` window. The diff routine
//! aligns the two buffers using the recovered lag and reports the
//! peak / RMS / sample-mismatch counts the UI surfaces in
//! `NullTestReport`.

use realfft::RealFftPlanner;
use serde::{Deserialize, Serialize};

/// Half-window of the lag search. Mirrors the design's R11.4
/// requirement: search range `[-5_000, 5_000]` samples.
pub const MAX_LAG_SAMPLES: i64 = 5_000;

/// Per-sample tolerance when counting "differing" samples in
/// [`compute_diff`]. Captures the f32 round-trip noise that any
/// loopback / pre-render path introduces; anything tighter than
/// this is treated as identical for the purposes of R11.7.
const SAMPLE_TOLERANCE: f32 = 1e-6;

/// Aggregated null-test statistics returned by [`compute_diff`].
/// Mirrors the `NullTestStats` shape called for in task 49.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NullTestStats {
    pub peak_diff_dbfs: f32,
    pub rms_diff_dbfs: f32,
    pub samples_diff_count: u64,
    pub aligned_frames: u64,
}

impl Default for NullTestStats {
    fn default() -> Self {
        NullTestStats {
            peak_diff_dbfs: f32::NEG_INFINITY,
            rms_diff_dbfs: f32::NEG_INFINITY,
            samples_diff_count: 0,
            aligned_frames: 0,
        }
    }
}

/// Estimate the lag that maximises the cross-correlation between
/// `source` and `captured`. Positive lag means the captured signal
/// is delayed relative to the source (the typical case for a
/// loopback round-trip).
///
/// Uses an FFT-based cross-correlation so the cost stays
/// `O(n log n)` even for the 5 s test signal (~221 k samples). Falls
/// back to a single zero-lag answer when either input is empty.
pub fn align_by_cross_correlation(source: &[f32], captured: &[f32]) -> i64 {
    if source.is_empty() || captured.is_empty() {
        return 0;
    }

    // Cross-correlation via FFT: R[τ] = Σ source[n] * captured[n + τ]
    // = ifft(conj(FFT(s)) * FFT(c)). Pad both inputs to a length
    // that fits the linear correlation (`source.len() +
    // captured.len()`) so the cyclic FFT result equals the linear
    // cross-correlation on the indices we search.
    let n_lin = source.len() + captured.len();
    let n_fft = n_lin.next_power_of_two().max(2);

    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(n_fft);
    let c2r = planner.plan_fft_inverse(n_fft);

    let mut s_buf = vec![0f32; n_fft];
    let mut c_buf = vec![0f32; n_fft];
    s_buf[..source.len()].copy_from_slice(source);
    c_buf[..captured.len()].copy_from_slice(captured);

    let mut s_freq = r2c.make_output_vec();
    let mut c_freq = r2c.make_output_vec();
    let mut prod = r2c.make_output_vec();
    let mut out_time = c2r.make_output_vec();
    if r2c.process(&mut s_buf, &mut s_freq).is_err() {
        return 0;
    }
    if r2c.process(&mut c_buf, &mut c_freq).is_err() {
        return 0;
    }
    // conj(S) * C in the frequency domain; the inverse FFT of this
    // product is R[τ] = Σ source[n] * captured[n + τ]. With
    // captured[n] = source[n - L] (delay L), the peak lands at
    // τ = L: positive τ → out_time[τ], negative τ → out_time[n_fft + τ].
    for i in 0..prod.len() {
        prod[i] = s_freq[i].conj() * c_freq[i];
    }
    if c2r.process(&mut prod, &mut out_time).is_err() {
        return 0;
    }

    // Restrict the search to the design's `[-MAX_LAG_SAMPLES,
    // +MAX_LAG_SAMPLES]` window so a noisy capture cannot pick up a
    // spurious far-out peak.
    let max_lag = MAX_LAG_SAMPLES;
    let mut best_lag: i64 = 0;
    let mut best_val = f32::NEG_INFINITY;
    for tau in -max_lag..=max_lag {
        let idx = if tau >= 0 {
            tau as usize
        } else {
            ((n_fft as i64) + tau) as usize
        };
        if idx >= n_fft {
            continue;
        }
        let v = out_time[idx];
        if v > best_val {
            best_val = v;
            best_lag = tau;
        }
    }

    // Sign convention (R11.4): positive return means the captured
    // signal is delayed by N samples relative to the source (the
    // typical loopback round-trip).
    best_lag
}

/// Apply the supplied lag to `captured` and compare it with `source`
/// sample-by-sample, returning the [`NullTestStats`] the diagnostic
/// surfaces in `NullTestReport`.
///
/// `lag > 0`  → captured was delayed relative to source; we drop
///              the first `lag` samples of `captured`.
/// `lag < 0`  → captured was advanced relative to source; we drop
///              the first `|lag|` samples of `source`.
pub fn compute_diff(source: &[f32], captured: &[f32], lag: i64) -> NullTestStats {
    let (s_aligned, c_aligned): (&[f32], &[f32]) = if lag >= 0 {
        let drop = lag as usize;
        if drop >= captured.len() {
            return NullTestStats::default();
        }
        let c = &captured[drop..];
        let n = source.len().min(c.len());
        (&source[..n], &c[..n])
    } else {
        let drop = (-lag) as usize;
        if drop >= source.len() {
            return NullTestStats::default();
        }
        let s = &source[drop..];
        let n = s.len().min(captured.len());
        (&s[..n], &captured[..n])
    };

    let mut peak_abs: f32 = 0.0;
    let mut sum_sq: f64 = 0.0;
    let mut diff_count: u64 = 0;
    for (a, b) in s_aligned.iter().zip(c_aligned.iter()) {
        let d = *a - *b;
        let abs = d.abs();
        if abs > peak_abs {
            peak_abs = abs;
        }
        sum_sq += (d as f64) * (d as f64);
        if abs > SAMPLE_TOLERANCE {
            diff_count += 1;
        }
    }
    let n = s_aligned.len() as u64;
    let rms = if n == 0 {
        0.0
    } else {
        (sum_sq / n as f64).sqrt() as f32
    };

    let to_dbfs = |x: f32| -> f32 {
        if x.abs() < f32::EPSILON {
            f32::NEG_INFINITY
        } else {
            20.0 * x.abs().log10()
        }
    };

    NullTestStats {
        peak_diff_dbfs: to_dbfs(peak_abs),
        rms_diff_dbfs: to_dbfs(rms),
        samples_diff_count: diff_count,
        aligned_frames: n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_buffers_yield_zero_diff_count() {
        let s: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.001).sin()).collect();
        let stats = compute_diff(&s, &s, 0);
        assert_eq!(stats.samples_diff_count, 0);
        assert_eq!(stats.aligned_frames, 2048);
    }

    #[test]
    fn one_lsb_difference_is_counted() {
        let mut s: Vec<f32> = vec![0.0; 4];
        let mut c = s.clone();
        s[0] = 0.0;
        c[0] = 1.0 / 32_768.0; // 1 LSB at 16-bit
        let stats = compute_diff(&s, &c, 0);
        assert!(stats.samples_diff_count >= 1);
    }

    #[test]
    fn aligner_recovers_small_positive_lag_on_random_signal() {
        // Build a 1024-sample PRNG signal and shift it by 17.
        let mut state: u32 = 0xDEADBEEF;
        let mut s = Vec::with_capacity(2048);
        for _ in 0..2048 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            s.push(((state >> 16) as i16) as f32 / 32_768.0);
        }
        let lag = 17usize;
        let mut c = vec![0f32; lag];
        c.extend_from_slice(&s);
        let recovered = align_by_cross_correlation(&s, &c);
        assert!(
            (recovered - lag as i64).abs() <= 1,
            "expected lag near {lag}, got {recovered}"
        );
    }
}
