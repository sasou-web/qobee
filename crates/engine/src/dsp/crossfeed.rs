//! `Crossfeed_Stage` — BS2B-style headphone crossfeed (R6).
//!
//! Inserted in the PCM chain *before* the EQ. Mixes a delayed and
//! low-passed copy of the opposite channel into each output to recreate
//! the cross-talk an ear receives when listening to loudspeakers, so
//! aggressively-panned recordings (early stereo Beatles, Pink Floyd,
//! …) sound natural on headphones.
//!
//! ## Algorithm (BS2B-style)
//!
//! For each stereo frame:
//!
//! ```text
//! l_out = ((1 - mix) * l + mix * lowpass(delay(r))) * gain_compensation
//! r_out = ((1 - mix) * r + mix * lowpass(delay(l))) * gain_compensation
//! ```
//!
//! Three presets are supported:
//!
//! | preset        | mix | delay (µs) | LP cutoff (Hz) |
//! | ------------- | --- | ---------- | -------------- |
//! | `Bauer`       | 0.4 | 300        | 700            |
//! | `BauerStrong` | 0.6 | 400        | 500            |
//! | `Custom`      | 0.5 | settings   | settings       |
//!
//! `gain_compensation` is fixed at -3 dB linear (≈ 0.7079) so the sum
//! `(1 - mix) + mix` cannot exceed unity for in-phase signals.
//!
//! ## Bypass (R6.4 / R6.5 / R6.6)
//!
//! The stage is bypassed exactly when `!enabled || src_channels != 2`.
//! In that state `process_inplace` returns without touching the
//! buffer (sample-for-sample passthrough verifiable by Property 11).
//! Mono and >stereo flows are forcibly bypassed regardless of
//! settings since the BS2B model is defined only for stereo.
//!
//! ## LP cutoff ramp
//!
//! Switching presets at runtime would otherwise click the filter
//! coefficients hard. The stage interpolates `current_lp_freq`
//! linearly toward `target_lp_freq` over a 50 ms window at each
//! preset change, recomputing the biquad coefficients per-frame
//! during the ramp. The cost is paid only inside the ramp window
//! (roughly 2400 frames at 48 kHz) and is dwarfed by the steady
//! state.
//!
//! ## Reset
//!
//! `reset()` clears the delay-line buffers and biquad state so a seek
//! does not bleed previous audio into the new playback position.
//! Re-enabling the stage from a bypassed state also performs a clean
//! restart for the same reason.

use std::f64::consts::PI;

use crate::audio_settings::{AudioSettings, CrossfeedPreset};
use crate::dsp::DspStage;

/// -3 dB linear: `10^(-3/20) ≈ 0.70794578`.
const GAIN_COMPENSATION: f32 = 0.707_945_78;

/// Quality factor for the BS2B-style low-pass biquad. R6.3 specifies
/// a maximally-flat (Butterworth) response; Q = 0.707.
const LP_Q: f64 = 0.707;

/// Ramp length applied to the LP cutoff frequency on a preset change.
const LP_RAMP_MS: f32 = 50.0;

/// The mix amount used for the `Custom` preset. The design pins
/// `delay_us` and `lp_cutoff_hz` for `Custom` but does not pin the
/// mix; we use a neutral midpoint between the two built-in presets.
const CUSTOM_PRESET_MIX: f32 = 0.5;

// -----------------------------------------------------------------------------
// Internal helpers
// -----------------------------------------------------------------------------

/// Single-channel circular delay-line, fixed length.
///
/// `push_pop(x)` writes `x` at the current write position and returns
/// the sample written `len` frames ago — i.e. the oldest sample
/// currently in the buffer.
struct DelayLine {
    buf: Vec<f32>,
    write_idx: usize,
}

impl DelayLine {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            write_idx: 0,
        }
    }

    #[inline]
    fn push_pop(&mut self, x: f32) -> f32 {
        let out = self.buf[self.write_idx];
        self.buf[self.write_idx] = x;
        self.write_idx += 1;
        if self.write_idx >= self.buf.len() {
            self.write_idx = 0;
        }
        out
    }

    fn resize(&mut self, len: usize) {
        let len = len.max(1);
        if self.buf.len() != len {
            self.buf = vec![0.0; len];
        } else {
            for s in self.buf.iter_mut() {
                *s = 0.0;
            }
        }
        self.write_idx = 0;
    }

    fn reset(&mut self) {
        for s in self.buf.iter_mut() {
            *s = 0.0;
        }
        self.write_idx = 0;
    }

    fn len(&self) -> usize {
        self.buf.len()
    }
}

/// RBJ low-pass biquad in Direct Form I, fixed Q = 0.707.
///
/// Same recipe as `crates/engine/src/eq.rs` but specialised for the
/// low-pass shape (the EQ uses peaking biquads). Kept local rather
/// than shared with `eq.rs` because the EQ's `Biquad` exposes
/// peaking-specific construction and the two will diverge as more
/// stages need their own filter shapes.
///
/// Coefficients and the Direct Form I state are held in **f64**. The
/// crossfeed cutoff (500–1500 Hz) is low relative to the sample rate,
/// so the pole sits close to the unit circle and the recursion is
/// sensitive to f32 rounding; the double-precision state keeps the
/// response and noise floor clean. The `process` interface stays f32:
/// input is widened on the way in and narrowed on the way out.
struct BiquadLowpass {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl BiquadLowpass {
    fn new(sample_rate: u32, cutoff_hz: f32) -> Self {
        let mut b = Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        };
        b.set_cutoff(sample_rate, cutoff_hz);
        b
    }

    fn set_cutoff(&mut self, sample_rate: u32, cutoff_hz: f32) {
        let fs = sample_rate.max(1) as f64;
        // Clamp the cutoff well below Nyquist to avoid numerical
        // explosions at the boundary; the design's range
        // [500, 1500] Hz is far from Nyquist at any reasonable rate.
        let f = (cutoff_hz as f64).clamp(20.0, fs * 0.45);
        let w0 = 2.0 * PI * f / fs;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * LP_Q);

        let b0 = (1.0 - cos_w0) * 0.5;
        let b1 = 1.0 - cos_w0;
        let b2 = (1.0 - cos_w0) * 0.5;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;

        let inv = 1.0 / a0;
        self.b0 = b0 * inv;
        self.b1 = b1 * inv;
        self.b2 = b2 * inv;
        self.a1 = a1 * inv;
        self.a2 = a2 * inv;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let x = x as f64;
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y as f32
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Resolve `(mix, delay_us, lp_hz)` for the active preset. `Custom`
/// reads `delay_us` and `lp_hz` from `AudioSettings` directly.
fn preset_params(preset: CrossfeedPreset, settings: &AudioSettings) -> (f32, f32, f32) {
    match preset {
        CrossfeedPreset::Bauer => (0.4, 300.0, 700.0),
        CrossfeedPreset::BauerStrong => (0.6, 400.0, 500.0),
        CrossfeedPreset::Custom => (
            CUSTOM_PRESET_MIX,
            settings.crossfeed_delay_us,
            settings.crossfeed_lp_cutoff_hz,
        ),
    }
}

/// Delay-line length in samples, rounded to the nearest integer
/// frame count, with a floor of 1 to keep the circular buffer valid
/// even at extreme rates.
fn delay_samples(delay_us: f32, sample_rate: u32) -> usize {
    let n = (delay_us * (sample_rate as f32) / 1_000_000.0).round();
    if n.is_finite() && n >= 1.0 {
        n as usize
    } else {
        1
    }
}

/// Ramp length in frames for the 50 ms LP cutoff transition. At least
/// one frame so the ramp counter never gets stuck at zero with a
/// non-zero target delta.
fn ramp_frames_for(sample_rate: u32) -> u32 {
    let frames = ((sample_rate as f32) * (LP_RAMP_MS / 1000.0)).round();
    frames.max(1.0) as u32
}

// -----------------------------------------------------------------------------
// CrossfeedStage
// -----------------------------------------------------------------------------

/// BS2B-style headphone crossfeed (R6).
pub struct CrossfeedStage {
    enabled: bool,
    src_channels: u16,
    sample_rate: u32,
    delay_l_to_r: DelayLine,
    delay_r_to_l: DelayLine,
    lp_l_to_r: BiquadLowpass,
    lp_r_to_l: BiquadLowpass,
    mix_amount: f32,
    gain_compensation: f32,
    target_lp_freq: f32,
    current_lp_freq: f32,
    /// Frames remaining before `current_lp_freq == target_lp_freq`.
    /// Zero ⇒ already at target; the biquad coefficients are not
    /// recomputed each frame outside the ramp.
    lp_ramp_frames_remaining: u32,
    /// Total ramp length in frames (≈ 50 ms × sample_rate).
    ramp_total_frames: u32,
    /// Steady-state bypass: `!enabled || src_channels != 2`.
    bypass: bool,
}

impl CrossfeedStage {
    pub fn new(settings: &AudioSettings, sample_rate: u32, channels: u16) -> Self {
        let (mix, delay_us, lp_hz) = preset_params(settings.crossfeed_preset, settings);
        let n = delay_samples(delay_us, sample_rate);
        Self {
            enabled: settings.crossfeed_enabled,
            src_channels: channels,
            sample_rate,
            delay_l_to_r: DelayLine::new(n),
            delay_r_to_l: DelayLine::new(n),
            lp_l_to_r: BiquadLowpass::new(sample_rate, lp_hz),
            lp_r_to_l: BiquadLowpass::new(sample_rate, lp_hz),
            mix_amount: mix,
            gain_compensation: GAIN_COMPENSATION,
            target_lp_freq: lp_hz,
            current_lp_freq: lp_hz,
            lp_ramp_frames_remaining: 0,
            ramp_total_frames: ramp_frames_for(sample_rate),
            bypass: !settings.crossfeed_enabled || channels != 2,
        }
    }

    /// Recompute internal state from the supplied settings. Caller is
    /// responsible for updating `sample_rate` / `src_channels` first
    /// if those changed (handled by `reconfigure`).
    fn recompute(&mut self, settings: &AudioSettings) {
        let prev_bypass = self.bypass;
        self.enabled = settings.crossfeed_enabled;
        self.bypass = !self.enabled || self.src_channels != 2;

        let (mix, delay_us, lp_hz) = preset_params(settings.crossfeed_preset, settings);
        self.mix_amount = mix;

        // Delay-line length follows `delay_us` × `sample_rate`. A
        // resize zeroes the buffer (the delay is a discrete change
        // and the previous tail is no longer aligned with the new
        // length, so we drop it).
        let n = delay_samples(delay_us, self.sample_rate);
        if n != self.delay_l_to_r.len() {
            self.delay_l_to_r.resize(n);
            self.delay_r_to_l.resize(n);
        }

        // LP target frequency: arm the 50 ms ramp if the target moved
        // by more than 1 Hz. Below that the coefficient swing is
        // inaudible and not worth the per-frame recomputation.
        self.target_lp_freq = lp_hz;
        let needs_ramp = (lp_hz - self.current_lp_freq).abs() > 1.0;
        if needs_ramp {
            self.lp_ramp_frames_remaining = self.ramp_total_frames.max(1);
        }

        // Switching from bypass back to active: clean restart so we
        // do not bleed pre-bypass tail back into the audio. Snap the
        // LP freq too — a fresh enable is a discrete event, no
        // perceptual benefit to ramping it.
        if prev_bypass && !self.bypass {
            self.delay_l_to_r.reset();
            self.delay_r_to_l.reset();
            self.lp_l_to_r.reset();
            self.lp_r_to_l.reset();
            self.current_lp_freq = self.target_lp_freq;
            self.lp_ramp_frames_remaining = 0;
            self.lp_l_to_r
                .set_cutoff(self.sample_rate, self.current_lp_freq);
            self.lp_r_to_l
                .set_cutoff(self.sample_rate, self.current_lp_freq);
        }
    }
}

impl DspStage for CrossfeedStage {
    fn reconfigure(&mut self, settings: &AudioSettings, sample_rate: u32, channels: u16) {
        let sample_rate_changed = sample_rate != self.sample_rate;
        let channels_changed = channels != self.src_channels;
        self.sample_rate = sample_rate;
        self.src_channels = channels;
        if sample_rate_changed {
            self.ramp_total_frames = ramp_frames_for(sample_rate);
        }
        self.recompute(settings);
        if sample_rate_changed || channels_changed {
            // Format change: clear every buffer and snap the LP freq
            // to its new target. The previous filter state is no
            // longer meaningful at the new sample rate.
            self.delay_l_to_r.reset();
            self.delay_r_to_l.reset();
            self.lp_l_to_r.reset();
            self.lp_r_to_l.reset();
            self.current_lp_freq = self.target_lp_freq;
            self.lp_ramp_frames_remaining = 0;
            self.lp_l_to_r
                .set_cutoff(self.sample_rate, self.current_lp_freq);
            self.lp_r_to_l
                .set_cutoff(self.sample_rate, self.current_lp_freq);
        }
    }

    fn is_bypass(&self) -> bool {
        self.bypass
    }

    fn process_inplace(&mut self, samples: &mut [f32]) {
        // R6.4 + R6.5 + R6.6: bypass means no touch.
        if self.bypass || self.src_channels != 2 {
            return;
        }

        let frames = samples.len() / 2;
        for f in 0..frames {
            // LP cutoff ramp: advance current_lp_freq toward
            // target_lp_freq by one frame's worth of step, then
            // recompute the biquad coefficients. The recomputation
            // only runs inside the ramp window — once
            // `lp_ramp_frames_remaining` hits zero, the steady-state
            // filter is left untouched.
            if self.lp_ramp_frames_remaining > 0 {
                let remaining = self.lp_ramp_frames_remaining as f32;
                let step = (self.target_lp_freq - self.current_lp_freq) / remaining;
                self.current_lp_freq += step;
                self.lp_ramp_frames_remaining -= 1;
                if self.lp_ramp_frames_remaining == 0 {
                    // Snap on the last frame so we never accumulate
                    // floating-point drift past the endpoint.
                    self.current_lp_freq = self.target_lp_freq;
                }
                self.lp_l_to_r
                    .set_cutoff(self.sample_rate, self.current_lp_freq);
                self.lp_r_to_l
                    .set_cutoff(self.sample_rate, self.current_lp_freq);
            }

            let l = samples[f * 2];
            let r = samples[f * 2 + 1];

            let l_delayed = self.delay_l_to_r.push_pop(l);
            let l_filtered = self.lp_l_to_r.process(l_delayed);

            let r_delayed = self.delay_r_to_l.push_pop(r);
            let r_filtered = self.lp_r_to_l.process(r_delayed);

            let mix = self.mix_amount;
            let comp = self.gain_compensation;
            let l_out = ((1.0 - mix) * l + mix * r_filtered) * comp;
            let r_out = ((1.0 - mix) * r + mix * l_filtered) * comp;

            samples[f * 2] = l_out;
            samples[f * 2 + 1] = r_out;
        }
    }

    fn reset(&mut self) {
        // Seek/load: every buffer clears, the LP cutoff snaps to its
        // target. Continuing to ramp from a stale `current_lp_freq`
        // would only smear into an audibly clean region.
        self.delay_l_to_r.reset();
        self.delay_r_to_l.reset();
        self.lp_l_to_r.reset();
        self.lp_r_to_l.reset();
        self.current_lp_freq = self.target_lp_freq;
        self.lp_ramp_frames_remaining = 0;
        self.lp_l_to_r
            .set_cutoff(self.sample_rate, self.current_lp_freq);
        self.lp_r_to_l
            .set_cutoff(self.sample_rate, self.current_lp_freq);
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> AudioSettings {
        AudioSettings::default()
    }

    fn enabled_stereo_settings() -> AudioSettings {
        let mut s = settings();
        s.crossfeed_enabled = true;
        s
    }

    fn stereo_buf(frames: usize, l: f32, r: f32) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            v.push(l);
            v.push(r);
        }
        v
    }

    #[test]
    fn default_disabled_is_bypass() {
        let s = CrossfeedStage::new(&settings(), 48_000, 2);
        assert!(
            s.is_bypass(),
            "default settings (crossfeed_enabled = false) must bypass"
        );
    }

    #[test]
    fn enabled_stereo_is_active() {
        let s = CrossfeedStage::new(&enabled_stereo_settings(), 48_000, 2);
        assert!(
            !s.is_bypass(),
            "crossfeed enabled on a stereo flow must NOT be bypassed"
        );
    }

    #[test]
    fn enabled_mono_is_force_bypass() {
        // R6.5 — mono input with crossfeed enabled is force-bypassed.
        let s = CrossfeedStage::new(&enabled_stereo_settings(), 48_000, 1);
        assert!(s.is_bypass(), "mono flow must be force-bypassed");
    }

    #[test]
    fn enabled_surround_is_force_bypass() {
        // R6.6 — flows with more than two channels are force-bypassed.
        for ch in [3u16, 4, 5, 6, 7, 8] {
            let s = CrossfeedStage::new(&enabled_stereo_settings(), 48_000, ch);
            assert!(
                s.is_bypass(),
                "{ch}-channel flow must be force-bypassed (channels != 2)"
            );
        }
    }

    #[test]
    fn bypass_is_sample_passthrough() {
        // Bypass path must not touch the buffer (sample-for-sample
        // equality). We exercise a mix of positives, negatives, zero,
        // near-unity, and small values.
        let mut s = CrossfeedStage::new(&settings(), 48_000, 2);
        let original: Vec<f32> = vec![
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
        ];
        // Truncate to multiple of 2.
        let len = (original.len() / 2) * 2;
        let mut buf: Vec<f32> = original.iter().take(len).copied().collect();
        let expected = buf.clone();
        s.process_inplace(&mut buf);
        assert_eq!(buf, expected, "bypass must be sample-for-sample identity");
    }

    #[test]
    fn enabled_processes_stereo_no_nan_no_inf() {
        // Feed a 1 second window of pseudo-random stereo samples
        // through an enabled crossfeed and verify the output is
        // entirely finite (no NaN, no Inf).
        let mut s = CrossfeedStage::new(&enabled_stereo_settings(), 48_000, 2);
        let frames = 48_000;
        let mut state: u32 = 0xDEAD_BEEF;
        let mut buf = Vec::with_capacity(frames * 2);
        for _ in 0..frames * 2 {
            // xorshift32, kept tiny and deterministic.
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            // Map to [-0.5, 0.5] — well within the linear range of
            // the LP filter, no risk of overflow downstream.
            let v = ((state as f32) / (u32::MAX as f32)) - 0.5;
            buf.push(v);
        }
        s.process_inplace(&mut buf);
        for (i, &x) in buf.iter().enumerate() {
            assert!(x.is_finite(), "sample {i} not finite (got {x})");
        }
    }

    #[test]
    fn reset_clears_delay_lines() {
        // Push some signal, then reset, then push silence: the output
        // must be exactly silent (no leak from previous samples
        // through the delay-line).
        let mut s = CrossfeedStage::new(&enabled_stereo_settings(), 48_000, 2);

        // Drive ~2 ms of full-scale signal so the delay-lines and
        // biquad state are non-trivial.
        let mut warm = stereo_buf(96, 0.9, -0.9);
        s.process_inplace(&mut warm);

        // Reset and feed silence.
        s.reset();
        let mut silence = stereo_buf(2_000, 0.0, 0.0);
        s.process_inplace(&mut silence);
        for (i, &x) in silence.iter().enumerate() {
            assert!(
                x.abs() < 1e-9,
                "post-reset silence leaked at sample {i}: got {x}"
            );
        }
    }

    #[test]
    fn delay_line_push_pop_returns_oldest_sample() {
        // Sanity check on the internal helper: a delay-line of N=4
        // returns 0.0 for the first 4 push_pop calls, then echoes
        // the input from N frames ago.
        let mut d = DelayLine::new(4);
        assert_eq!(d.push_pop(1.0), 0.0);
        assert_eq!(d.push_pop(2.0), 0.0);
        assert_eq!(d.push_pop(3.0), 0.0);
        assert_eq!(d.push_pop(4.0), 0.0);
        assert_eq!(d.push_pop(5.0), 1.0);
        assert_eq!(d.push_pop(6.0), 2.0);
        assert_eq!(d.push_pop(7.0), 3.0);
    }

    #[test]
    fn biquad_lowpass_unit_input_steady_state_dc_gain_is_one() {
        // RBJ low-pass at any cutoff has unity DC gain. Drive a long
        // train of 1.0s through it; once the IIR has settled the
        // output must approach 1.0.
        let mut bq = BiquadLowpass::new(48_000, 700.0);
        let mut last = 0.0;
        for _ in 0..2_000 {
            last = bq.process(1.0);
        }
        assert!(
            (last - 1.0).abs() < 1e-3,
            "DC gain must be ~1.0 (got {last})"
        );
    }

    #[test]
    fn process_then_reconfigure_to_disabled_returns_to_bypass() {
        // Round-trip: enable + process + reconfigure to disabled.
        // After the reconfigure the stage must report bypass and a
        // subsequent process must be sample-for-sample identity.
        let mut audio = enabled_stereo_settings();
        let mut s = CrossfeedStage::new(&audio, 48_000, 2);

        let mut warm = stereo_buf(64, 0.5, -0.5);
        s.process_inplace(&mut warm);

        audio.crossfeed_enabled = false;
        s.reconfigure(&audio, 48_000, 2);
        assert!(s.is_bypass());

        let mut buf = stereo_buf(32, 0.123, -0.456);
        let expected = buf.clone();
        s.process_inplace(&mut buf);
        assert_eq!(buf, expected, "post-disable bypass must be passthrough");
    }
}
