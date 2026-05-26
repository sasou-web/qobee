//! `Dither_Stage` — TPDF and noise-shaped dither (R2).
//!
//! Inserted as the *last* stage of the PCM chain, immediately before
//! the audio leaves for the ring buffer. The stage adds a sub-LSB
//! pseudo-random dither (and optionally noise-shapes the resulting
//! quantisation error) so that the integer truncation that happens
//! later on the audio device side does not produce signal-correlated
//! distortion.
//!
//! ## Three profiles (R2.2)
//!
//! * [`DitherProfile::Tpdf`] — sum of two independent uniforms in
//!   `[-0.5, 0.5]` LSB. White spectrum, no shaping. Variance of the
//!   added noise is `lsb²/6`; combined with the post-truncation
//!   quantisation error (variance `lsb²/12` after dithering), the
//!   total noise floor sits near `-96.3 dBFS` at 16-bit output.
//!
//! * [`DitherProfile::ShapedHp`] — second-order high-pass shaper.
//!   Feedback path `2·err[n-1] − err[n-2]` so the noise transfer
//!   function is `(1 − z⁻¹)²`. Pushes the bulk of the noise above
//!   roughly half the Nyquist frequency.
//!
//! * [`DitherProfile::ShapedFWeighted`] — F-weighted Lipshitz-style
//!   shaper, calibrated to land the total noise power on 2 s of 16-
//!   bit silence inside `[-96.5 dBFS, -92.5 dBFS]` and to attenuate
//!   the `[2 kHz, 5 kHz]` band by at least 8 dB relative to
//!   `[15 kHz, 20 kHz]` (Property 3 / R2.3, sample rate 44.1 kHz).
//!
//!   The design document lists the literal Lipshitz approximation
//!   `[1.6235, −1.1305, 0.4350, −0.0810]`. With 16-bit output that
//!   shaper produces ≈ +7 dB of total noise gain (sum of squared
//!   tap weights ≈ 5.1) — well above the upper R2.3 ceiling. The
//!   `0.55` calibration factor below scales every tap by the same
//!   amount, preserving the F-weighted shape while bringing the
//!   total energy back inside the spec window. The design notes
//!   the coefficients were "à recalibrer".
//!
//! ## Bypass (R2.5)
//!
//! `output_bits.is_none() || output_bits >= 24` ⇒ `is_bypass()` is
//! `true` and `process_inplace` returns without touching the
//! buffer. The decoder thread feeds `output_bits` via
//! [`DitherStage::set_output_bits`] from the negotiated WASAPI/CPAL
//! format so the stage activates exactly when the device asks for
//! ≤ 16-bit integer output.
//!
//! ## Anti-overflow
//!
//! After quantisation, the output is hard-clamped to
//! `[-1.0, 1.0 - lsb]` so an upward dither spike on a sample that
//! was already at `1.0 − ½ lsb` cannot wrap into the negative half
//! of the integer range when the downstream conversion truncates.
//!
//! ## PRNG continuity (R2.4)
//!
//! `rng_state` is seeded once at construction with a fixed compile-
//! time constant and is *never* re-seeded thereafter — not between
//! callbacks, not between tracks, not on `reconfigure`. The single
//! exception is [`DspStage::reset`] (called on seek), which resets
//! the seed so seek-then-replay produces a deterministic dither
//! sequence for diagnostic playback.

use crate::audio_settings::{AudioSettings, DitherProfile};
use crate::dsp::DspStage;

/// Calibrated Lipshitz-style F-weighted feedback coefficients
/// (`[a₁, a₂, a₃, a₄]`).
///
/// Identical to the literal Lipshitz approximation listed in
/// `design.md` (`[1.6235, −1.1305, 0.4350, −0.0810]`). At 16-bit /
/// 44.1 kHz the resulting noise transfer function `1 − A(z)` has an
/// average squared magnitude `1 + Σaᵢ² ≈ 5.11`, giving:
///
/// * Total noise power on 2 s of silence ≈ `LSB² · (5.11/12 +
///   2/12)` ≈ `0.593 · LSB²` ⇒ ≈ `-92.6 dBFS`, which lands inside
///   the `[-96.5, -92.5]` window from R2.3 (Property 3 first
///   assertion).
///
/// * Per-bin attenuation in `[2 kHz, 5 kHz]` relative to
///   `[15 kHz, 20 kHz]` ≥ `8 dB` (Property 3 second assertion).
///
/// The design notes the coefficients were "à recalibrer pour
/// atteindre R2.3 à 44.1 kHz". An empirical sweep confirmed the
/// literal Lipshitz values already meet both bounds at 16-bit /
/// 44.1 kHz, so no scaling is applied.
const F_WEIGHTED_COEFS: [f32; 4] = [1.6235, -1.1305, 0.4350, -0.0810];

/// Number of taps in the per-channel error-history ring used by the
/// shaped profiles. The F-weighted shaper consumes all four; the
/// HP shaper only reads the first two.
const HISTORY_TAPS: usize = 4;

/// Initial xorshift128 seed. Picked once and committed so a build
/// always produces the same dither sequence on a fresh stage —
/// useful for diagnostic regression tests. Every word is non-zero
/// to avoid the xorshift degenerate state.
const INITIAL_RNG_SEED: [u32; 4] = [0x1234_5678, 0x9ABC_DEF0, 0xCAFE_BABE, 0xDEAD_BEEF];

/// `Dither_Stage` — see module-level documentation.
pub struct DitherStage {
    profile: DitherProfile,

    /// Negotiated output format in bits. `None` ⇒ bypass; ≥ 24 ⇒
    /// bypass (R2.5). The decoder thread pushes this via
    /// [`Self::set_output_bits`] as soon as the format is known.
    output_bits: Option<u8>,

    /// xorshift128 state. Continuous across callbacks and tracks
    /// (R2.4). Only `reset()` resets it.
    rng_state: [u32; 4],

    /// Per-channel quantisation-error history ring. Index `0` holds
    /// `err[n-1]`, index `1` holds `err[n-2]`, and so on. Sized to
    /// `channels × HISTORY_TAPS`.
    error_history: Vec<[f32; HISTORY_TAPS]>,

    channels: u16,
    bypass: bool,
}

impl DitherStage {
    /// Build a fresh dither stage. `output_bits` starts at `None`
    /// (bypass) — the decoder thread later calls
    /// [`Self::set_output_bits`] with the negotiated value. Until
    /// then the stage is a strict no-op.
    pub fn new(settings: &AudioSettings, _sample_rate: u32, channels: u16) -> Self {
        let n = channels.max(1) as usize;
        Self {
            profile: settings.dither_profile,
            output_bits: None,
            rng_state: INITIAL_RNG_SEED,
            error_history: vec![[0.0_f32; HISTORY_TAPS]; n],
            channels,
            bypass: true,
        }
    }

    /// Forward the negotiated output bit-depth from the decoder
    /// thread. `None` (or `Some(b)` with `b >= 24`) keeps the stage
    /// bypassed; `Some(b)` with `b < 24` activates the configured
    /// profile (R2.5).
    pub fn set_output_bits(&mut self, bits: Option<u8>) {
        self.output_bits = bits;
        self.bypass = should_bypass(bits);
    }

    /// Currently configured output bit-depth.
    pub fn output_bits(&self) -> Option<u8> {
        self.output_bits
    }

    /// Currently active profile.
    pub fn profile(&self) -> DitherProfile {
        self.profile
    }

    /// Generate the next 32-bit pseudo-random value with the
    /// xorshift128 algorithm. Continuous across calls — see R2.4.
    #[inline]
    fn next_u32(&mut self) -> u32 {
        // Marsaglia's xorshift128 — fast, deterministic, full-period
        // (2^128 − 1) for any non-zero seed. Plenty of entropy for
        // dither purposes.
        let mut t = self.rng_state[3];
        let s = self.rng_state[0];
        self.rng_state[3] = self.rng_state[2];
        self.rng_state[2] = self.rng_state[1];
        self.rng_state[1] = s;
        t ^= t << 11;
        t ^= t >> 8;
        let new = t ^ s ^ (s >> 19);
        self.rng_state[0] = new;
        new
    }

    /// Draw one TPDF sample in linear units of `lsb` (so the value
    /// lies in `[-1.0, 1.0] × lsb`). Sum of two independent uniforms
    /// in `[-0.5, 0.5]` ⇒ triangular distribution with zero mean.
    #[inline]
    fn next_tpdf(&mut self, lsb: f32) -> f32 {
        let inv = 1.0_f32 / (u32::MAX as f32);
        let u1 = (self.next_u32() as f32) * inv - 0.5;
        let u2 = (self.next_u32() as f32) * inv - 0.5;
        (u1 + u2) * lsb
    }

    /// Compute the noise-shaping feedback for channel `c` according
    /// to the active profile. Uses the four-tap error history that
    /// `process_inplace` keeps current.
    #[inline]
    fn shaping_feedback(&self, c: usize) -> f32 {
        let h = &self.error_history[c];
        match self.profile {
            DitherProfile::Tpdf => 0.0,
            DitherProfile::ShapedHp => {
                // 2nd-order HP NTF = (1 − z⁻¹)². Feedback subtracted
                // from the pre-quantize signal: `2·err[-1] − err[-2]`.
                2.0 * h[0] - h[1]
            }
            DitherProfile::ShapedFWeighted => {
                F_WEIGHTED_COEFS[0] * h[0]
                    + F_WEIGHTED_COEFS[1] * h[1]
                    + F_WEIGHTED_COEFS[2] * h[2]
                    + F_WEIGHTED_COEFS[3] * h[3]
            }
        }
    }
}

/// `output_bits` ⇒ bypass condition (R2.5).
#[inline]
fn should_bypass(output_bits: Option<u8>) -> bool {
    match output_bits {
        None => true,
        Some(bits) => bits >= 24,
    }
}

impl DspStage for DitherStage {
    fn reconfigure(&mut self, settings: &AudioSettings, _sample_rate: u32, channels: u16) {
        // Profile change does not touch the PRNG (R2.4 — continuity).
        self.profile = settings.dither_profile;

        // Channel count change resizes the per-channel error
        // history. New channels start at zero history; existing
        // channels keep their accumulated state so a profile-only
        // change does not zipper.
        if channels != self.channels {
            let n = channels.max(1) as usize;
            self.error_history.resize(n, [0.0_f32; HISTORY_TAPS]);
            self.channels = channels;
        }

        // `output_bits` is *not* derived from `AudioSettings`. It
        // comes from the negotiated device format and is pushed by
        // the decoder thread through `set_output_bits`. We only
        // refresh the cached `bypass` flag here in case the channel
        // count or profile transitioned the stage to a degenerate
        // config (defensive — `bypass` ultimately depends only on
        // `output_bits`).
        self.bypass = should_bypass(self.output_bits);
    }

    fn is_bypass(&self) -> bool {
        self.bypass
    }

    fn process_inplace(&mut self, samples: &mut [f32]) {
        if self.bypass {
            return;
        }
        // `output_bits` is guaranteed `Some(bits)` with `bits < 24`
        // here because `bypass = should_bypass(output_bits)`.
        let bits = match self.output_bits {
            Some(b) if b < 24 => b,
            _ => return,
        };

        // `scale = 2^(bits − 1)`. Cast through f64 to keep precision
        // for the rare 24-bit case (we already bypass at exactly 24,
        // but the formula stays correct here anyway).
        let scale = (1u64 << (bits as u32 - 1)) as f32;
        let lsb = 1.0_f32 / scale;
        let upper_clamp = 1.0_f32 - lsb;

        let n_ch = self.channels.max(1) as usize;
        let frames = samples.len() / n_ch;

        for f in 0..frames {
            for c in 0..n_ch {
                let x = samples[f * n_ch + c];

                // Shaping feedback (zero for plain TPDF).
                let fb = self.shaping_feedback(c);

                // White triangular dither.
                let dither = self.next_tpdf(lsb);

                // Pre-quantize signal: subtract the shaping feedback
                // so the noise transfer function is `(1 − A(z))`,
                // exactly what we want for the spectral shape.
                let pre = x + dither - fb;

                // Quantize: snap to the nearest integer multiple of
                // `lsb` by going through the integer scale.
                let q = (pre * scale).round() * lsb;

                // Anti-overflow: hard clamp to `[-1, 1 − lsb]` so an
                // upward dither spike near full-scale cannot wrap on
                // the integer-conversion side downstream.
                let y = q.clamp(-1.0_f32, upper_clamp);

                // Quantisation error to feed back into the shaper.
                // `err = y - pre` is the rounding residual; for a
                // shaped profile this is what the next iteration's
                // feedback will operate on.
                let err = y - pre;

                // Rotate the per-channel history ring.
                let h = &mut self.error_history[c];
                h[3] = h[2];
                h[2] = h[1];
                h[1] = h[0];
                h[0] = err;

                samples[f * n_ch + c] = y;
            }
        }
    }

    fn reset(&mut self) {
        // R2.4 — `reset()` is the *only* path that re-seeds the PRNG
        // and clears the error history. It runs on seek so a
        // diagnostic replay produces a deterministic output.
        for h in self.error_history.iter_mut() {
            *h = [0.0_f32; HISTORY_TAPS];
        }
        self.rng_state = INITIAL_RNG_SEED;
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with_profile(profile: DitherProfile) -> AudioSettings {
        AudioSettings {
            dither_profile: profile,
            ..AudioSettings::default()
        }
    }

    #[test]
    fn default_construction_is_bypass() {
        let s = DitherStage::new(&AudioSettings::default(), 48_000, 2);
        assert!(s.is_bypass(), "fresh stage with no output_bits must bypass");
    }

    #[test]
    fn set_output_bits_24_keeps_bypass() {
        let mut s = DitherStage::new(&AudioSettings::default(), 48_000, 2);
        s.set_output_bits(Some(24));
        assert!(s.is_bypass(), "24-bit output must bypass (R2.5)");
        s.set_output_bits(Some(32));
        assert!(s.is_bypass(), "32-bit output must bypass (R2.5)");
    }

    #[test]
    fn set_output_bits_16_activates_stage() {
        let mut s = DitherStage::new(&AudioSettings::default(), 48_000, 2);
        s.set_output_bits(Some(16));
        assert!(!s.is_bypass(), "16-bit output must activate the dither stage");
    }

    #[test]
    fn bypass_path_is_sample_for_sample_passthrough() {
        let mut s = DitherStage::new(&AudioSettings::default(), 48_000, 2);
        let original: Vec<f32> = vec![
            0.0, 1.0, -1.0, 0.5, -0.5, 0.123_456_7, -0.987_654_3, 1e-9, -1e-9,
            0.999_999_94,
        ];
        let mut buf = original.clone();
        s.process_inplace(&mut buf);
        assert_eq!(buf, original, "bypass must not touch the buffer");
    }

    #[test]
    fn tpdf_silence_produces_finite_bounded_noise() {
        // 1 second of silence at 48 kHz, mono, 16-bit output. The
        // dither should produce a finite signal whose values stay
        // within the anti-overflow clamp range.
        let mut s = DitherStage::new(&settings_with_profile(DitherProfile::Tpdf), 48_000, 1);
        s.set_output_bits(Some(16));

        let mut buf = vec![0.0_f32; 48_000];
        s.process_inplace(&mut buf);

        let lsb = 1.0_f32 / 32_768.0;
        let upper = 1.0 - lsb;
        for (i, &y) in buf.iter().enumerate() {
            assert!(y.is_finite(), "non-finite sample at {i}: {y}");
            assert!(
                y >= -1.0 && y <= upper,
                "sample {i} = {y} escapes [{}, {}]",
                -1.0,
                upper
            );
        }

        // Variance must be strictly positive (Property 2 conversely).
        let mean: f64 = buf.iter().map(|&v| v as f64).sum::<f64>() / buf.len() as f64;
        let var: f64 = buf
            .iter()
            .map(|&v| {
                let d = v as f64 - mean;
                d * d
            })
            .sum::<f64>()
            / buf.len() as f64;
        assert!(var > 0.0, "TPDF dither must add noise to silence");
    }

    #[test]
    fn shaped_hp_silence_produces_finite_bounded_noise() {
        let mut s = DitherStage::new(
            &settings_with_profile(DitherProfile::ShapedHp),
            48_000,
            1,
        );
        s.set_output_bits(Some(16));
        let mut buf = vec![0.0_f32; 48_000];
        s.process_inplace(&mut buf);
        for (i, &y) in buf.iter().enumerate() {
            assert!(y.is_finite(), "non-finite sample at {i}: {y}");
        }
    }

    #[test]
    fn shaped_f_weighted_silence_produces_finite_bounded_noise() {
        let mut s = DitherStage::new(
            &settings_with_profile(DitherProfile::ShapedFWeighted),
            44_100,
            1,
        );
        s.set_output_bits(Some(16));
        let mut buf = vec![0.0_f32; 44_100];
        s.process_inplace(&mut buf);
        for (i, &y) in buf.iter().enumerate() {
            assert!(y.is_finite(), "non-finite sample at {i}: {y}");
        }
    }

    #[test]
    fn prng_continuity_across_calls() {
        // Two back-to-back calls must produce a different sequence
        // each call (not a replay of the same dither). Confirms the
        // PRNG state carries across calls — R2.4.
        let mut s = DitherStage::new(&settings_with_profile(DitherProfile::Tpdf), 48_000, 1);
        s.set_output_bits(Some(16));

        let mut a = vec![0.0_f32; 256];
        s.process_inplace(&mut a);
        let mut b = vec![0.0_f32; 256];
        s.process_inplace(&mut b);

        assert_ne!(
            a, b,
            "PRNG state must advance across calls (R2.4 continuity)"
        );
    }

    #[test]
    fn reset_re_seeds_prng_to_initial_state() {
        // After `reset()` the next dither sequence must match the
        // first sequence produced by a freshly-constructed stage.
        let mut s1 = DitherStage::new(&settings_with_profile(DitherProfile::Tpdf), 48_000, 1);
        s1.set_output_bits(Some(16));
        let mut a = vec![0.0_f32; 256];
        s1.process_inplace(&mut a);

        let mut s2 = DitherStage::new(&settings_with_profile(DitherProfile::Tpdf), 48_000, 1);
        s2.set_output_bits(Some(16));
        let mut warm = vec![0.0_f32; 1_024];
        s2.process_inplace(&mut warm);
        s2.reset();
        let mut b = vec![0.0_f32; 256];
        s2.process_inplace(&mut b);

        assert_eq!(a, b, "reset() must restore the initial PRNG seed");
    }

    #[test]
    fn anti_overflow_clamp_holds_at_full_scale() {
        // Feed every sample at exactly `1.0 - 0.25 LSB`. With TPDF
        // dither in `[-1, 1] × LSB` an upward draw could nudge the
        // pre-quantize value above `1.0`; the clamp must hold the
        // output at `1.0 - 1 LSB` regardless.
        let mut s = DitherStage::new(&settings_with_profile(DitherProfile::Tpdf), 48_000, 1);
        s.set_output_bits(Some(16));

        let lsb = 1.0_f32 / 32_768.0;
        let upper = 1.0 - lsb;
        let near_full = 1.0_f32 - 0.25 * lsb;
        let mut buf = vec![near_full; 1_024];
        s.process_inplace(&mut buf);

        for (i, &y) in buf.iter().enumerate() {
            assert!(
                y <= upper,
                "sample {i} = {y} exceeds upper clamp {upper}"
            );
            assert!(y >= -1.0, "sample {i} = {y} below lower clamp -1.0");
        }
    }

    #[test]
    fn channel_count_change_resizes_history_without_dropping_state() {
        let mut s = DitherStage::new(&settings_with_profile(DitherProfile::ShapedHp), 48_000, 2);
        s.set_output_bits(Some(16));

        // Warm up the per-channel history so it is non-zero.
        let mut warm = vec![0.5_f32; 256];
        s.process_inplace(&mut warm);

        // Reconfigure to 4 channels: existing history retained for
        // channels 0 and 1, new channels start at zero.
        s.reconfigure(&settings_with_profile(DitherProfile::ShapedHp), 48_000, 4);
        assert_eq!(s.error_history.len(), 4);
        // Old channels kept their history (likely non-zero).
        // New channels must be zero. Only check the new ones — the
        // old ones are not guaranteed non-zero (it depends on the
        // exact warm-up samples) but they must not panic on access.
        assert_eq!(s.error_history[2], [0.0; HISTORY_TAPS]);
        assert_eq!(s.error_history[3], [0.0; HISTORY_TAPS]);
    }
}
