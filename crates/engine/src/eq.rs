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

use std::f32::consts::PI;

/// ISO 1/1-octave centers used by the EQ. Order matters: the UI labels
/// them in this order too.
pub const FREQS_HZ: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

/// Number of bands. Equals `FREQS_HZ.len()`.
pub const NUM_BANDS: usize = 10;

/// Quality factor used for every band. 1.0 gives a smooth, musical
/// shape. Higher values make the bell narrower (more surgical).
const Q: f32 = 1.0;

/// Single biquad section in Direct Form I.
#[derive(Clone, Copy, Default)]
struct Biquad {
    // Coefficients (already normalized: a0 == 1).
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Biquad {
    /// Build a peaking-EQ biquad for `gain_db` at `freq_hz` running at
    /// `sample_rate`. `gain_db = 0.0` produces an identity filter.
    fn peaking(sample_rate: f32, freq_hz: f32, gain_db: f32, q: f32) -> Self {
        // RBJ peaking-EQ recipe.
        let a = 10.0_f32.powf(gain_db / 40.0);
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

/// Per-channel state for one biquad section.
#[derive(Clone, Copy, Default)]
struct State {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl State {
    #[inline(always)]
    fn process(&mut self, b: &Biquad, x: f32) -> f32 {
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
    sample_rate: f32,
    channels: usize,
    bands: [Biquad; NUM_BANDS],
    /// `state[channel][band]`.
    state: Vec<[State; NUM_BANDS]>,
    bypass: bool,
}

impl Equalizer {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let n_ch = channels as usize;
        let mut eq = Equalizer {
            sample_rate: sample_rate as f32,
            channels: n_ch,
            bands: [Biquad::identity(); NUM_BANDS],
            state: vec![[State::default(); NUM_BANDS]; n_ch.max(1)],
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
            if g.abs() > 0.05 {
                all_zero = false;
            }
            self.bands[i] = if g.abs() < 0.0001 {
                Biquad::identity()
            } else {
                Biquad::peaking(self.sample_rate, *freq, g, Q)
            };
        }
        self.bypass = all_zero;
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
                let mut x = samples[idx];
                let state = &mut self.state[c];
                for band in 0..NUM_BANDS {
                    x = state[band].process(&self.bands[band], x);
                }
                samples[idx] = x;
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
}
