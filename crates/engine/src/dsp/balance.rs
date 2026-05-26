//! `Channel_Balance_Stage` — balance + per-channel trim (R10).
//!
//! This stage applies a scalar gain per channel built from two
//! independent contributions:
//!
//!   * **Balance law** (R10.2). Linear (not equal-power), defined for
//!     stereo: for `balance ∈ [-1, 0]` the left channel stays at unity
//!     and the right channel scales as `1 + balance`; for
//!     `balance ∈ [0, 1]` the right channel stays at unity and the left
//!     channel scales as `1 - balance`. Channels beyond stereo see no
//!     balance contribution (multiplier = 1.0).
//!
//!   * **Per-channel trim** (R10.3). `trim_db_per_channel[c]` in
//!     `[-12.0, 0.0]` dB, converted to linear as `10^(db/20)`. Missing
//!     entries fall back to `0.0 dB` (R10 — graceful degradation when
//!     a track changes channel count between two pieces).
//!
//! The effective per-channel gain is `balance_gain[c] × trim_linear[c]`.
//!
//! ## Bypass (R10.4)
//!
//! Exact bypass when `|balance| < 1e-9` AND every `|trim_db[c]| < 1e-6`.
//! In that state `process_inplace` returns without touching the buffer.
//!
//! ## Mono (R10 — graceful degradation)
//!
//! For `channels == 1` the balance law has no semantic meaning, so the
//! stage is forcibly bypassed regardless of settings. A single trim
//! entry could in principle still apply, but the design specifies a
//! forced bypass on mono flows to keep the chain's invariant simple.
//!
//! ## Ramp (R10 — 50 ms, no zipper noise)
//!
//! Settings changes are not applied instantaneously; the stage
//! interpolates linearly from `current_gain[c]` toward
//! `target_gain[c]` over 50 ms × sample_rate frames. While the ramp
//! is active `is_bypass()` returns `false` — even if both endpoints
//! happen to be unity — because the audio is being multiplied by a
//! non-constant gain. After the ramp completes, `is_bypass()` reflects
//! the steady-state condition again.

use crate::audio_settings::AudioSettings;
use crate::dsp::DspStage;

pub struct ChannelBalanceStage {
    channels: u16,
    /// Currently-applied gain per channel (smoothed). Sized to
    /// `channels`.
    current_gain: Vec<f32>,
    /// Target gain per channel (after a settings change). Sized to
    /// `channels`.
    target_gain: Vec<f32>,
    /// Number of frames remaining to ramp `current_gain` toward
    /// `target_gain`. Zero ⇒ already at the target.
    ramp_frames_remaining: u32,
    /// Total ramp length, in frames (≈ 50 ms × sample_rate).
    ramp_total_frames: u32,
    /// Steady-state bypass: `balance == 0` and every trim is `0 dB`,
    /// or the flow is mono. Read by `is_bypass()` together with the
    /// ramp counter.
    bypass: bool,
}

impl ChannelBalanceStage {
    pub fn new(settings: &AudioSettings, sample_rate: u32, channels: u16) -> Self {
        let n = channels as usize;
        let mut s = Self {
            channels,
            current_gain: vec![1.0; n],
            target_gain: vec![1.0; n],
            ramp_frames_remaining: 0,
            ramp_total_frames: ramp_frames_for(sample_rate),
            bypass: true,
        };
        s.recompute_targets(settings);
        // First load: snap `current` onto `target`. No audio is
        // playing yet so there is no click to smooth out, and we
        // don't want to spend the first 50 ms of a track ramping in
        // a balance/trim that the user explicitly chose.
        s.current_gain.clone_from(&s.target_gain);
        s.ramp_frames_remaining = 0;
        s
    }

    /// Compute the new per-channel target gain from the supplied
    /// settings. Resizes the gain vectors to match `self.channels`,
    /// updates `self.bypass`, and arms the ramp if the targets
    /// differ from the current gains.
    fn recompute_targets(&mut self, settings: &AudioSettings) {
        let n = self.channels as usize;

        // Force bypass on degenerate or mono flows. Either way the
        // gain vectors are kept in sync with `n` so subsequent
        // reconfigures stay coherent.
        if n <= 1 {
            self.target_gain.resize(n, 1.0);
            for g in self.target_gain.iter_mut() {
                *g = 1.0;
            }
            if self.current_gain.len() != n {
                self.current_gain.resize(n, 1.0);
            }
            self.bypass = true;
            self.ramp_frames_remaining = 0;
            return;
        }

        // Resize target_gain to channel count, pad with unity. The
        // current_gain vector also follows the channel count: when a
        // new track raises the channel count, the new channels start
        // at unity (we have no smoothed history for them).
        if self.target_gain.len() != n {
            self.target_gain.resize(n, 1.0);
        }
        if self.current_gain.len() != n {
            self.current_gain.resize(n, 1.0);
        }

        // Balance law (linear, stereo-only).
        let bal = settings.balance.clamp(-1.0, 1.0);
        let (gain_l, gain_r) = if bal <= 0.0 {
            (1.0, 1.0 + bal)
        } else {
            (1.0 - bal, 1.0)
        };

        // Track whether every per-channel gain ends up at unity.
        // Used to decide on exact bypass (R10.4).
        let mut all_unity = bal.abs() < 1e-9;

        let trim = &settings.trim_db_per_channel;
        for c in 0..n {
            // Balance contribution: stereo only. Channels beyond
            // L/R are not touched by the balance law.
            let base = match c {
                0 => gain_l,
                1 => gain_r,
                _ => 1.0,
            };
            // Trim contribution: pad with 0 dB if the user's array
            // is shorter than the actual channel count (graceful
            // degradation when the new track has more channels).
            let trim_db = trim.get(c).copied().unwrap_or(0.0);
            if trim_db.abs() >= 1e-6 {
                all_unity = false;
            }
            let trim_linear = if trim_db.abs() < 1e-6 {
                1.0
            } else {
                10f32.powf(trim_db / 20.0)
            };
            self.target_gain[c] = base * trim_linear;
        }

        self.bypass = all_unity;

        // Arm the ramp if the new targets differ from the current
        // smoothed gains. When `current == target` (e.g. just after
        // construction or `reset`), the ramp is a no-op.
        let needs_ramp = self
            .current_gain
            .iter()
            .zip(self.target_gain.iter())
            .any(|(c, t)| (c - t).abs() > 1e-9);
        if needs_ramp {
            self.ramp_frames_remaining = self.ramp_total_frames.max(1);
        }
    }
}

/// Ramp length in frames. 50 ms × sample_rate, rounded to the
/// nearest integer, with a minimum of 1 to avoid a zero-length
/// ramp on absurdly low rates.
fn ramp_frames_for(sample_rate: u32) -> u32 {
    let frames = ((sample_rate as f32) * 0.050).round() as i64;
    frames.max(1) as u32
}

impl DspStage for ChannelBalanceStage {
    fn reconfigure(&mut self, settings: &AudioSettings, sample_rate: u32, channels: u16) {
        self.channels = channels;
        self.ramp_total_frames = ramp_frames_for(sample_rate);
        self.recompute_targets(settings);
    }

    fn is_bypass(&self) -> bool {
        // Bypass is exact only when the steady-state is unity AND no
        // ramp is in flight. While ramping, we are actively
        // multiplying samples by a non-constant gain, so the stage
        // is not a passthrough.
        self.bypass && self.ramp_frames_remaining == 0
    }

    fn process_inplace(&mut self, samples: &mut [f32]) {
        if self.is_bypass() {
            return;
        }

        let n = self.channels as usize;
        if n == 0 || samples.is_empty() {
            return;
        }

        let frames = samples.len() / n;

        for f in 0..frames {
            // Advance current_gain toward target_gain by one frame's
            // worth of step. Step size = (target − current) /
            // remaining, which guarantees we hit the target on the
            // last frame regardless of accumulated rounding.
            if self.ramp_frames_remaining > 0 {
                let remaining = self.ramp_frames_remaining as f32;
                for c in 0..n {
                    let cur = self.current_gain[c];
                    let tgt = self.target_gain[c];
                    let step = (tgt - cur) / remaining;
                    self.current_gain[c] = cur + step;
                }
                self.ramp_frames_remaining -= 1;
                // Snap to target on the last frame so we never
                // accumulate floating-point drift past the endpoint.
                if self.ramp_frames_remaining == 0 {
                    self.current_gain.clone_from(&self.target_gain);
                }
            }

            for c in 0..n {
                samples[f * n + c] *= self.current_gain[c];
            }
        }
    }

    fn reset(&mut self) {
        // On seek/load: snap to target. A discontinuity is expected
        // (the user just jumped to a new position) so there is no
        // need to ramp; preserving the smoothed state would actually
        // produce a small artefact at the seek point.
        self.current_gain.clone_from(&self.target_gain);
        self.ramp_frames_remaining = 0;
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

    fn stereo_buf(frames: usize, l: f32, r: f32) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            v.push(l);
            v.push(r);
        }
        v
    }

    #[test]
    fn default_is_bypass() {
        let s = ChannelBalanceStage::new(&settings(), 48_000, 2);
        assert!(
            s.is_bypass(),
            "default settings (balance = 0, all trims = 0) must bypass"
        );
    }

    #[test]
    fn mono_is_force_bypassed_even_with_nondefault_settings() {
        // Build settings that would otherwise activate the stage.
        let mut audio = settings();
        audio.balance = -1.0;
        audio.trim_db_per_channel = vec![-6.0];

        let s = ChannelBalanceStage::new(&audio, 48_000, 1);
        assert!(
            s.is_bypass(),
            "mono flow must be force-bypassed regardless of balance/trim"
        );
    }

    #[test]
    fn process_inplace_passthrough_when_bypass() {
        let mut s = ChannelBalanceStage::new(&settings(), 48_000, 2);
        let original = stereo_buf(64, 0.5, -0.25);
        let mut buf = original.clone();
        s.process_inplace(&mut buf);
        assert_eq!(
            buf, original,
            "bypass path must not touch the buffer (sample-for-sample passthrough)"
        );
    }

    #[test]
    fn balance_negative_one_silences_right() {
        // balance = -1 ⇒ steady-state right gain = 0.
        // Reset() snaps to target, so the first frame already shows
        // the silenced right channel without going through the ramp.
        let mut audio = settings();
        audio.balance = -1.0;

        let mut s = ChannelBalanceStage::new(&audio, 48_000, 2);
        s.reset(); // snap to target

        let mut buf = stereo_buf(8, 1.0, 1.0);
        s.process_inplace(&mut buf);

        for f in 0..8 {
            assert!((buf[f * 2] - 1.0).abs() < 1e-6, "left must stay at 1.0");
            assert!(
                buf[f * 2 + 1].abs() < 1e-6,
                "right must be silenced at frame {f}, got {}",
                buf[f * 2 + 1]
            );
        }
    }

    #[test]
    fn balance_positive_one_silences_left() {
        let mut audio = settings();
        audio.balance = 1.0;

        let mut s = ChannelBalanceStage::new(&audio, 48_000, 2);
        s.reset();

        let mut buf = stereo_buf(8, 1.0, 1.0);
        s.process_inplace(&mut buf);

        for f in 0..8 {
            assert!(
                buf[f * 2].abs() < 1e-6,
                "left must be silenced at frame {f}, got {}",
                buf[f * 2]
            );
            assert!(
                (buf[f * 2 + 1] - 1.0).abs() < 1e-6,
                "right must stay at 1.0"
            );
        }
    }

    #[test]
    fn trim_minus_six_db_attenuates_left_channel() {
        // -6 dB ≈ 0.5012 linear.
        let mut audio = settings();
        audio.trim_db_per_channel = vec![-6.0, 0.0];

        let mut s = ChannelBalanceStage::new(&audio, 48_000, 2);
        s.reset();

        let mut buf = stereo_buf(4, 1.0, 1.0);
        s.process_inplace(&mut buf);

        let expected_l = 10f32.powf(-6.0 / 20.0);
        for f in 0..4 {
            assert!(
                (buf[f * 2] - expected_l).abs() < 1e-5,
                "left frame {f}: got {}, want {expected_l}",
                buf[f * 2]
            );
            assert!(
                (buf[f * 2 + 1] - 1.0).abs() < 1e-6,
                "right must stay unchanged"
            );
        }
    }

    #[test]
    fn channel_count_change_pads_extra_channels_with_zero_db() {
        // Build a stereo stage with -6 dB on the left.
        let mut audio = settings();
        audio.trim_db_per_channel = vec![-6.0, 0.0];
        let mut s = ChannelBalanceStage::new(&audio, 48_000, 2);

        // Now reconfigure for a 6-channel track (e.g. 5.1). The
        // user's trim array only has two entries; channels 2..6 must
        // fall back to 0 dB. The first two channels keep their
        // originally configured trim.
        s.reconfigure(&audio, 48_000, 6);
        s.reset();

        // Build a 6-channel input frame of unity samples.
        let n = 6;
        let mut buf: Vec<f32> = (0..n).map(|_| 1.0).collect();
        s.process_inplace(&mut buf);

        let expected_l = 10f32.powf(-6.0 / 20.0);
        assert!((buf[0] - expected_l).abs() < 1e-5, "ch0 = trim -6 dB");
        assert!((buf[1] - 1.0).abs() < 1e-6, "ch1 = trim 0 dB");
        for (c, &v) in buf.iter().enumerate().skip(2) {
            assert!(
                (v - 1.0).abs() < 1e-6,
                "ch{c} must stay at unity (no trim specified)"
            );
        }
    }

    #[test]
    fn ramp_smooths_step_change() {
        // 1 kHz fictional sample rate ⇒ 50 ms = 50 frames of ramp.
        // Build a stage with balance = 0 (bypass), then reconfigure
        // to balance = -1 (right channel target = 0). Process 50
        // frames of stereo unity samples and verify the right-channel
        // gain ramps from ~1.0 to ~0.0 monotonically.
        let mut audio = settings();
        let mut s = ChannelBalanceStage::new(&audio, 1_000, 2);
        // Stage is at unity. Now change balance.
        audio.balance = -1.0;
        s.reconfigure(&audio, 1_000, 2);

        let frames = 50;
        let mut buf = stereo_buf(frames, 1.0, 1.0);
        s.process_inplace(&mut buf);

        // First frame: very close to 1.0 on the right (ramp just
        // started — one step's worth applied).
        assert!(
            buf[1] > 0.95,
            "right channel at frame 0 should still be close to 1.0, got {}",
            buf[1]
        );
        // Last frame: at the target (0.0).
        let last = buf[(frames - 1) * 2 + 1];
        assert!(
            last.abs() < 1e-6,
            "right channel at last frame should be 0.0, got {last}"
        );
        // Monotone non-increasing on the right channel.
        for f in 1..frames {
            let prev = buf[(f - 1) * 2 + 1];
            let cur = buf[f * 2 + 1];
            assert!(
                cur <= prev + 1e-6,
                "right channel must decrease monotonically; frame {f}: {cur} > prev {prev}"
            );
        }
        // After the ramp, is_bypass remains false because balance
        // = -1 ⇒ steady-state right gain = 0 (not unity).
        assert!(!s.is_bypass());
    }

    #[test]
    fn bypass_is_false_during_active_ramp() {
        // Even when both endpoints (current and target) end up unity,
        // an in-flight ramp must not advertise bypass — the stage is
        // actively multiplying samples by a non-constant gain.
        //
        // Reproduce by going from balance = -1 (steady right = 0) to
        // balance = 0 (steady right = 1.0) and check is_bypass()
        // during the ramp.
        let mut audio = settings();
        audio.balance = -1.0;
        let mut s = ChannelBalanceStage::new(&audio, 1_000, 2);
        // Construction snaps current = target; no ramp.
        // Now reconfigure back to neutral: triggers a ramp toward
        // unity. During that ramp, is_bypass() must be false.
        let mut audio2 = settings();
        audio2.balance = 0.0;
        s.reconfigure(&audio2, 1_000, 2);

        // Steady-state target is unity, so `bypass` flag is true,
        // but a ramp is armed. is_bypass() must combine both.
        assert!(
            !s.is_bypass(),
            "active ramp must report not-bypass even with unity targets"
        );

        // Drain the ramp by processing a chunk of frames.
        let mut buf = stereo_buf(50, 0.5, 0.5);
        s.process_inplace(&mut buf);

        // Once the ramp has completed and the targets are unity, the
        // stage is back to a true passthrough.
        assert!(
            s.is_bypass(),
            "after ramp finishes, is_bypass() must be true again"
        );
    }
}
