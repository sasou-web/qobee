//! 10-band peaking-EQ DSP.
//!
//! ## Design
//!
//! Ten cascaded peaking biquad filters at the standard ISO octave
//! center frequencies. Each band has a fixed Q (1.0, smooth and musical
//! rather than surgical) and a runtime-adjustable gain in dB. Channels
//! are processed independently with their own delay lines so stereo
//! imaging is preserved.
//!
//! Coefficients follow Robert Bristow-Johnson's *Audio EQ Cookbook*.
//!
//! ## Real-time safety
//!
//! - Processing is in-place on f32 interleaved samples; no allocations
//!   in the hot path.
//! - When all bands are at 0 dB the bypass flag is honored and
//!   `process_inplace` is a no-op (the cheapest path).
//! - The set of gains is held in a `Mutex<Vec<f32>>` updated only when
//!   the user moves a slider. The audio worker reads them on the
//!   command-handler boundary, so the callback never blocks.
//!
//! ## Numerical precision (f64 internal)
//!
//! Coefficients and the Direct Form I delay state are computed and
//! accumulated in **f64**, even though the interface (input/output
//! buffer, the `set_gains_db` argument) stays f32. The low-frequency
//! bands (31/62 Hz) at high sample rates place the biquad poles very
//! close to the unit circle; in f32 the coefficient quantisation and
//! the recursive state accumulation produce audible response error and
//! a raised noise floor. Doing the recursion in f64 — the standard
//! audiophile practice — removes that ceiling. The per-sample cost is
//! one `f32 → f64` widen on input and one `f64 → f32` narrow on output;
//! everything in between is f64. Bypass (all bands flat) is still an
//! exact f32 passthrough, so the bit-perfect guarantee is untouched.

use std::f64::consts::PI;

/// ISO 1/1-octave centers used by the EQ. Order matters: the UI labels
/// them in this order too.
pub const FREQS_HZ: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

/// Number of bands. Equals `FREQS_HZ.len()`.
pub const NUM_BANDS: usize = 10;

/// Quality factor used for every band. 1.0 gives a smooth, musical
/// shape. Higher values make the bell narrower (more surgical).
const Q: f64 = 1.0;

/// Single biquad section in Direct Form I.
///
/// Coefficients are held in f64. The peaking recipe is evaluated in
/// double precision so the low-frequency bands keep their nominal
/// response instead of drifting on f32 coefficient rounding.
#[derive(Clone, Copy, Default)]
struct Biquad {
    // Coefficients (already normalized: a0 == 1).
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    /// Build a peaking-EQ biquad for `gain_db` at `freq_hz` running at
    /// `sample_rate`. `gain_db = 0.0` produces an identity filter.
    fn peaking(sample_rate: f64, freq_hz: f64, gain_db: f64, q: f64) -> Self {
        // RBJ peaking-EQ recipe.
        let a = 10.0_f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq_hz / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;

        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    fn identity() -> Self {
        Biquad {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
}

/// Per-channel state for one biquad section. Held in f64 so the
/// recursive accumulation does not lose low-order bits between
/// samples.
#[derive(Clone, Copy, Default)]
struct State {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl State {
    #[inline(always)]
    fn process(&mut self, b: &Biquad, x: f64) -> f64 {
        let y = b.b0 * x + b.b1 * self.x1 + b.b2 * self.x2 - b.a1 * self.y1 - b.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// 10-band cascaded peaking EQ.
///
/// One `Equalizer` instance handles `channels` channels in parallel
/// using independent state lines per (channel, band).
pub struct Equalizer {
    sample_rate: f64,
    channels: usize,
    bands: [Biquad; NUM_BANDS],
    /// `state[channel][band]`.
    state: Vec<[State; NUM_BANDS]>,
    /// Last gains (dB) applied via [`Equalizer::set_gains_db`], kept so
    /// the biquads can be rebuilt when the sample rate or channel count
    /// changes (see the [`DspStage`] impl) without the caller having to
    /// re-push the gains. EQ gains are *not* part of `AudioSettings`;
    /// they live on `Shared::eq_gains_db` and are forwarded into the
    /// chain by the decoder thread, so `reconfigure` has to preserve
    /// them locally.
    gains_db: [f32; NUM_BANDS],
    bypass: bool,
}

impl Equalizer {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let n_ch = channels as usize;
        let mut eq = Equalizer {
            sample_rate: sample_rate as f64,
            channels: n_ch,
            bands: [Biquad::identity(); NUM_BANDS],
            state: vec![[State::default(); NUM_BANDS]; n_ch.max(1)],
            gains_db: [0.0; NUM_BANDS],
            bypass: true,
        };
        // Start flat (every band at 0 dB).
        eq.set_gains_db(&[0.0; NUM_BANDS]);
        eq
    }

    /// Update every band gain in one shot. `gains_db` must have length
    /// `NUM_BANDS`. Values are clamped to `[-12, +12]` dB.
    pub fn set_gains_db(&mut self, gains_db: &[f32]) {
        let mut all_zero = true;
        for (i, freq) in FREQS_HZ.iter().enumerate() {
            let g = gains_db.get(i).copied().unwrap_or(0.0).clamp(-12.0, 12.0);
            self.gains_db[i] = g;
            if g.abs() > 0.05 {
                all_zero = false;
            }
            self.bands[i] = if g.abs() < 0.0001 {
                Biquad::identity()
            } else {
                Biquad::peaking(self.sample_rate, *freq as f64, g as f64, Q)
            };
        }
        self.bypass = all_zero;
    }

    /// Currently applied per-band gains in dB (already clamped to
    /// `[-12, +12]`). Length is always [`NUM_BANDS`].
    pub fn gains_db(&self) -> &[f32; NUM_BANDS] {
        &self.gains_db
    }

    pub fn is_bypass(&self) -> bool {
        self.bypass
    }

    /// Process `samples` in place. Layout is interleaved
    /// `[L, R, L, R, ...]` for stereo. The number of channels was set
    /// at construction time and must match.
    #[allow(clippy::needless_range_loop)]
    pub fn process_inplace(&mut self, samples: &mut [f32]) {
        if self.bypass {
            return;
        }
        let n_ch = self.channels.max(1);
        let frames = samples.len() / n_ch;
        for f in 0..frames {
            for c in 0..n_ch {
                let idx = f * n_ch + c;
                // Widen to f64 for the whole biquad cascade, narrow
                // back to f32 only at the very end.
                let mut x = samples[idx] as f64;
                let state = &mut self.state[c];
                for band in 0..NUM_BANDS {
                    x = state[band].process(&self.bands[band], x);
                }
                samples[idx] = x as f32;
            }
        }
    }

    /// Reset internal delay lines (e.g. on seek) so impulse history
    /// from before the seek doesn't smear into the new playback.
    pub fn reset_state(&mut self) {
        for ch in self.state.iter_mut() {
            for s in ch.iter_mut() {
                *s = State::default();
            }
        }
    }

    /// Rebuild the filter bank for a new `(sample_rate, channels)`
    /// pair, preserving the currently configured gains. Used by the
    /// [`DspStage`] impl when the decoder thread observes a device
    /// format change. The biquad coefficients depend on the sample
    /// rate, so they have to be recomputed; the per-channel state
    /// vector is resized (and cleared) to match the new channel count.
    fn reconfigure_format(&mut self, sample_rate: u32, channels: u16) {
        let new_sr = sample_rate as f64;
        let new_ch = (channels as usize).max(1);
        let sr_changed = (new_sr - self.sample_rate).abs() > f64::EPSILON;
        let ch_changed = new_ch != self.channels;
        if !sr_changed && !ch_changed {
            return;
        }
        self.sample_rate = new_sr;
        self.channels = new_ch;
        // A format change invalidates the old delay lines; start the
        // new geometry from a clean state.
        self.state = vec![[State::default(); NUM_BANDS]; new_ch];
        // Recompute the biquads at the new sample rate from the
        // preserved gains.
        let gains = self.gains_db;
        self.set_gains_db(&gains);
    }
}

impl crate::dsp::DspStage for Equalizer {
    /// EQ gains are not carried in `AudioSettings` (they live on
    /// `Shared::eq_gains_db` and are pushed in via
    /// [`Equalizer::set_gains_db`]), so `reconfigure` only reacts to
    /// the `(sample_rate, channels)` format pair. The gains are
    /// preserved across the rebuild.
    fn reconfigure(
        &mut self,
        _settings: &crate::audio_settings::AudioSettings,
        sample_rate: u32,
        channels: u16,
    ) {
        self.reconfigure_format(sample_rate, channels);
    }

    fn is_bypass(&self) -> bool {
        Equalizer::is_bypass(self)
    }

    fn process_inplace(&mut self, samples: &mut [f32]) {
        Equalizer::process_inplace(self, samples);
    }

    fn reset(&mut self) {
        self.reset_state();
    }
}
