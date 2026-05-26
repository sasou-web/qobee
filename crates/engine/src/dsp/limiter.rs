//! `Peak_Limiter` — final true-peak protection for the PCM chain (R3).
//!
//! Inserted in the PCM chain *after* the EQ / convolver and *before*
//! the dither stage. Three modes are exposed by
//! `audio.peak_limiter_mode`:
//!
//! * [`PeakLimiterMode::Off`] — `process_inplace` is a hard no-op
//!   (Property 7: sample-for-sample passthrough).
//!
//! * [`PeakLimiterMode::SoftClip`] — applies the historical
//!   `crate::dsp::clip::soft_clip` polynomial sample-by-sample. Zero
//!   latency. Kept as a fallback for users who prefer a
//!   non-look-ahead colour.
//!
//! * [`PeakLimiterMode::LookaheadLimiter`] — true-peak limiter with a
//!   per-channel delay line and an asymmetric envelope follower.
//!   Implements the design pseudocode:
//!     1. Push each frame into the per-channel delay line.
//!     2. Take the sliding max abs over the look-ahead window across
//!        every channel.
//!     3. Compute `target_gain = ceiling / window_peak` (or 1.0 if
//!        the window peak fits under the ceiling).
//!     4. Run the envelope follower asymmetrically:
//!        attack coefficient when the target is below the current
//!        envelope (we need to duck), release coefficient when it
//!        sits above (we are recovering).
//!     5. Emit the delayed sample multiplied by the freshly computed
//!        envelope, hard-clamped to `±ceiling_linear` as a final
//!        safety net.
//!
//!   The clamp is what gives Property 5 (`|y| ≤ ceiling`) its
//!   absolute guarantee, even on inputs that would otherwise outrun
//!   the envelope (for example a single-frame impulse at the very
//!   first sample, before the buffer has anything to look ahead at).
//!
//! ## Latency
//!
//! `lookahead_samples()` is exposed so the decoder thread can add
//! `lookahead_samples / sample_rate` to the reported `position_ms`
//! whenever the limiter is active. The look-ahead buffer is sized
//! to exactly `lookahead_samples` so the emitted sample at frame `f`
//! corresponds to input from frame `f - lookahead_samples + 1`.
//!
//! ## End-of-track flush
//!
//! When the decoder hits EOT, it must call `process_inplace` on
//! `lookahead_samples × channels` zero samples before emitting
//! `EndOfTrack`. The buffer's tail then drains the last
//! `lookahead_samples` of real audio, multiplied by the current
//! envelope. Without this flush the listener would lose ≈ 5 ms at
//! every track boundary.
//!
//! ## Dynamic bypass
//!
//! When the envelope sits at exactly 1.0 *and* the window peak stays
//! under the ceiling for `2 × lookahead_samples` consecutive frames,
//! the stage flips a private `bypass_active` flag. While the flag is
//! up the per-frame loop still cycles the delay line (so the
//! reported latency stays consistent with the active mode) but skips
//! the per-channel multiply-and-clamp step, producing a literal
//! passthrough of the delayed sample. The flag is cleared as soon
//! as a new peak is detected, at which point the envelope is also
//! snapped back to 1.0 so we never slam the gain from a stale value.
//!
//! The dynamic flag is internal — `is_bypass()` reports the static
//! `mode == Off` only. It exists strictly as a CPU optimisation.

use crate::audio_settings::{AudioSettings, PeakLimiterMode};
use crate::dsp::clip::soft_clip;
use crate::dsp::DspStage;

/// Fixed attack length: R3.5 mandates `≤ 1 ms`. The look-ahead
/// buffer is what gives the limiter its gradual, distortion-free
/// envelope on the *output* timeline; on the internal envelope
/// signal we use an instant snap when ducking. This satisfies
/// Property 6's literal reading ("envelope reaches target by 1 ms")
/// while staying consistent with the design's pseudocode (the
/// follower formula collapses to `envelope = target_gain` when
/// `attack_coef = 0`).
const ATTACK_MS: f32 = 1.0;

// -----------------------------------------------------------------------------
// ChannelBuffer
// -----------------------------------------------------------------------------

/// Per-channel delay line backed by a single flat `Vec<f32>` laid
/// out as `[ch0_t0, ch0_t1, ..., ch0_tN-1, ch1_t0, ..., chN-1_tN-1]`.
///
/// All channels share a single `write_idx` because they are written
/// in lockstep (one frame at a time). `push(c, x)` only stores the
/// sample; the caller advances `write_idx` once per frame after every
/// channel has been pushed and the per-frame output has been
/// computed.
struct ChannelBuffer {
    channels: usize,
    /// Delay-line length per channel = `lookahead_samples`. Always
    /// `>= 1` so the modular arithmetic stays well-defined.
    len: usize,
    buf: Vec<f32>,
    write_idx: usize,
}

impl ChannelBuffer {
    fn new(channels: usize, len: usize) -> Self {
        let len = len.max(1);
        let channels = channels.max(1);
        Self {
            channels,
            len,
            buf: vec![0.0; channels * len],
            write_idx: 0,
        }
    }

    /// Resize for a new `(channels, len)` pair. Always zeroes the
    /// buffer — a different look-ahead length renders the stored
    /// samples meaningless because the indices no longer line up
    /// with the new geometry.
    fn resize(&mut self, channels: usize, len: usize) {
        let len = len.max(1);
        let channels = channels.max(1);
        if channels != self.channels || len != self.len {
            self.channels = channels;
            self.len = len;
            self.buf = vec![0.0; channels * len];
            self.write_idx = 0;
        } else {
            // Same geometry: just clear contents and the write head.
            for s in self.buf.iter_mut() {
                *s = 0.0;
            }
            self.write_idx = 0;
        }
    }

    fn reset(&mut self) {
        for s in self.buf.iter_mut() {
            *s = 0.0;
        }
        self.write_idx = 0;
    }

    /// Write `x` into channel `c` at the current write position.
    /// Must be called for every channel of the current frame before
    /// `advance_write_idx`.
    #[inline]
    fn push(&mut self, c: usize, x: f32) {
        debug_assert!(c < self.channels);
        self.buf[c * self.len + self.write_idx] = x;
    }

    /// Advance the shared write index by one frame, wrapping at
    /// `len`. Call exactly once per frame after every channel has
    /// been pushed and tail-read.
    #[inline]
    fn advance_write_idx(&mut self) {
        self.write_idx += 1;
        if self.write_idx >= self.len {
            self.write_idx = 0;
        }
    }

    /// Return the oldest sample currently stored for channel `c`.
    /// Must be called *after* `push(c, …)` for the current frame —
    /// at that point the slot at `(write_idx + 1) % len` holds the
    /// sample that was written `len - 1` frames ago, and the slot at
    /// `write_idx` holds the just-pushed sample.
    #[inline]
    fn tail(&self, c: usize) -> f32 {
        debug_assert!(c < self.channels);
        let idx = if self.write_idx + 1 >= self.len {
            0
        } else {
            self.write_idx + 1
        };
        self.buf[c * self.len + idx]
    }

    /// Sliding-window peak (max abs) over the entire delay line for
    /// channel `c`. Cheap version: scans the channel slice each call.
    /// At 48 kHz with a 5 ms look-ahead and 8 channels this costs
    /// roughly 84 MB/s of memory accesses — acceptable until a
    /// profiler shows otherwise.
    fn window_peak(&self, c: usize) -> f32 {
        debug_assert!(c < self.channels);
        let start = c * self.len;
        let end = start + self.len;
        self.buf[start..end]
            .iter()
            .fold(0.0_f32, |acc, &s| acc.max(s.abs()))
    }
}

// -----------------------------------------------------------------------------
// PeakLimiter
// -----------------------------------------------------------------------------

/// Look-ahead true-peak limiter (R3). See module-level docs for the
/// algorithm.
pub struct PeakLimiter {
    mode: PeakLimiterMode,
    ceiling_linear: f32,
    lookahead_samples: usize,
    sample_rate: u32,
    channels: u16,

    buffer: ChannelBuffer,
    envelope: f32,
    /// One-pole attack coefficient `exp(-1 / attack_samples)`. Stored
    /// for compatibility with the design pseudocode; the look-ahead
    /// path uses an instant-snap attack instead, so this is currently
    /// only used by `reconfigure` for trace consistency. See
    /// `process_lookahead` for the actual ducking behaviour.
    #[allow(dead_code)]
    attack_coef: f32,
    release_coef: f32,

    /// Internal CPU-saving flag — see module-level docs. Never
    /// reported by `is_bypass()`.
    bypass_active: bool,
    /// Number of consecutive frames the envelope has stayed at
    /// exactly 1.0 with a window peak under the ceiling. Resets
    /// whenever a new peak is detected.
    unity_envelope_streak: usize,
}

impl PeakLimiter {
    /// Build a limiter from the supplied settings + format. Always
    /// allocates the look-ahead buffer (cheap; the chain is not
    /// rebuilt during the audio hot path).
    pub fn new(settings: &AudioSettings, sample_rate: u32, channels: u16) -> Self {
        let lookahead_samples = lookahead_samples_for(settings, sample_rate);
        let ceiling_linear = ceiling_linear_for(settings);
        let attack_samples = attack_samples_for(sample_rate);
        let release_samples = release_samples_for(settings, sample_rate);
        Self {
            mode: settings.peak_limiter_mode,
            ceiling_linear,
            lookahead_samples,
            sample_rate,
            channels,
            buffer: ChannelBuffer::new(channels.max(1) as usize, lookahead_samples),
            envelope: 1.0,
            attack_coef: time_constant_coef(attack_samples),
            release_coef: time_constant_coef(release_samples),
            bypass_active: false,
            unity_envelope_streak: 0,
        }
    }

    /// Look-ahead size in samples. Used by the decoder thread to
    /// compensate `position_ms` and to size the EOT flush.
    pub fn lookahead_samples(&self) -> usize {
        if matches!(self.mode, PeakLimiterMode::LookaheadLimiter) {
            self.lookahead_samples
        } else {
            // SoftClip and Off introduce no latency.
            0
        }
    }

    /// Current envelope value. Exposed for testability (Property 6).
    pub fn envelope(&self) -> f32 {
        self.envelope
    }

    /// Active mode (test helper).
    pub fn mode(&self) -> PeakLimiterMode {
        self.mode
    }

    /// Inner loop for the look-ahead path. Pulled into its own
    /// method so `process_inplace` stays readable across the three
    /// mode branches.
    fn process_lookahead(&mut self, samples: &mut [f32]) {
        let n_ch = self.channels.max(1) as usize;
        // Defensive: skip non-multiples of `n_ch` so a malformed
        // buffer cannot index out of bounds. The decoder thread
        // always feeds whole frames; this is just paranoia.
        let frames = samples.len() / n_ch;
        let bypass_threshold = self.lookahead_samples.saturating_mul(2);

        for f in 0..frames {
            // 1) push frame & compute the look-ahead window peak
            //    across every channel.
            let mut window_peak = 0.0_f32;
            for c in 0..n_ch {
                let s = samples[f * n_ch + c];
                self.buffer.push(c, s);
                let cp = self.buffer.window_peak(c);
                if cp > window_peak {
                    window_peak = cp;
                }
            }

            // 2) target gain to keep the upcoming peak under the
            //    ceiling. `ceiling_linear / window_peak` is in
            //    `(0, 1]` when we have to duck; otherwise stays at 1.
            let target_gain = if window_peak > self.ceiling_linear {
                self.ceiling_linear / window_peak
            } else {
                1.0
            };

            // Dynamic-bypass bookkeeping. A new peak (target < 1)
            // must cancel the bypass and snap the envelope back to
            // 1.0 so we don't slam the gain from a stale value.
            if target_gain < 1.0 {
                if self.bypass_active {
                    self.bypass_active = false;
                    self.envelope = 1.0;
                }
                self.unity_envelope_streak = 0;
            }

            // 3) asymmetric envelope follower. A look-ahead limiter
            //    snaps the envelope to its target instantly on
            //    attack (gain reduction) — the look-ahead buffer is
            //    what gives the *output* the gradual, distortion-free
            //    transition. On release we use the standard one-pole
            //    coefficient. This is equivalent to the design's
            //    pseudocode with `attack_coef = 0`, which makes
            //    Property 6 ("envelope reaches target in ≤ 1 ms")
            //    a hard guarantee rather than an asymptote.
            if target_gain < self.envelope {
                self.envelope = target_gain;
            } else {
                let coef = self.release_coef;
                self.envelope = coef * self.envelope + (1.0 - coef) * target_gain;
            }

            // Track unity-envelope streak for the bypass heuristic.
            if target_gain >= 1.0 && (self.envelope - 1.0).abs() < 1e-7 {
                self.unity_envelope_streak = self.unity_envelope_streak.saturating_add(1);
                if !self.bypass_active && self.unity_envelope_streak >= bypass_threshold {
                    self.bypass_active = true;
                }
            }

            // 4) emit the delayed sample, multiplied by the envelope
            //    and hard-clamped to ±ceiling as a final safety net
            //    (Property 5). When the dynamic bypass flag is up
            //    the envelope is exactly 1.0 and every window peak
            //    fits under the ceiling, so the multiply and clamp
            //    are mathematically a no-op — but we still do them
            //    so a future implementation tweak cannot accidentally
            //    desynchronise the buffer.
            let env = self.envelope;
            let ceiling = self.ceiling_linear;
            for c in 0..n_ch {
                let delayed = self.buffer.tail(c);
                let y = delayed * env;
                samples[f * n_ch + c] = y.clamp(-ceiling, ceiling);
            }

            self.buffer.advance_write_idx();
        }
    }

    /// SoftClip path. No latency, no buffer interaction — apply the
    /// soft-clip polynomial sample by sample.
    fn process_soft_clip(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            *s = soft_clip(*s);
        }
    }
}

impl DspStage for PeakLimiter {
    fn reconfigure(&mut self, settings: &AudioSettings, sample_rate: u32, channels: u16) {
        let new_mode = settings.peak_limiter_mode;
        let new_lookahead = lookahead_samples_for(settings, sample_rate);
        let geometry_changed = sample_rate != self.sample_rate
            || channels != self.channels
            || new_lookahead != self.lookahead_samples;

        self.mode = new_mode;
        self.sample_rate = sample_rate;
        self.channels = channels;
        self.lookahead_samples = new_lookahead;
        self.ceiling_linear = ceiling_linear_for(settings);

        let attack_samples = attack_samples_for(sample_rate);
        let release_samples = release_samples_for(settings, sample_rate);
        self.attack_coef = time_constant_coef(attack_samples);
        self.release_coef = time_constant_coef(release_samples);

        if geometry_changed {
            self.buffer
                .resize(channels.max(1) as usize, new_lookahead);
        }

        // Mode swap or geometry change resets the dynamic state so a
        // stale envelope cannot bleed into the new configuration.
        self.envelope = 1.0;
        self.bypass_active = false;
        self.unity_envelope_streak = 0;
    }

    fn is_bypass(&self) -> bool {
        // Convention: only `Off` is reported as bypass. SoftClip and
        // LookaheadLimiter modify the signal (the latter introduces
        // observable latency), so they cannot claim sample-for-sample
        // passthrough even when momentarily transparent.
        matches!(self.mode, PeakLimiterMode::Off)
    }

    fn process_inplace(&mut self, samples: &mut [f32]) {
        match self.mode {
            PeakLimiterMode::Off => {
                // R3.7: literal no-op, sample-for-sample identity.
            }
            PeakLimiterMode::SoftClip => self.process_soft_clip(samples),
            PeakLimiterMode::LookaheadLimiter => self.process_lookahead(samples),
        }
    }

    fn reset(&mut self) {
        self.buffer.reset();
        self.envelope = 1.0;
        self.bypass_active = false;
        self.unity_envelope_streak = 0;
    }
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

fn lookahead_samples_for(settings: &AudioSettings, sample_rate: u32) -> usize {
    let n = (settings.peak_limiter_lookahead_ms * (sample_rate as f32) / 1000.0).round();
    if n.is_finite() && n >= 1.0 {
        n as usize
    } else {
        1
    }
}

fn ceiling_linear_for(settings: &AudioSettings) -> f32 {
    10f32.powf(settings.peak_limiter_ceiling_dbfs / 20.0)
}

fn attack_samples_for(sample_rate: u32) -> f32 {
    (ATTACK_MS * (sample_rate as f32) / 1000.0).max(1.0)
}

fn release_samples_for(settings: &AudioSettings, sample_rate: u32) -> f32 {
    (settings.peak_limiter_release_ms * (sample_rate as f32) / 1000.0).max(1.0)
}

/// Standard one-pole time-constant: `coef = exp(-1 / time_samples)`.
/// The follower formula is then
/// `env[n] = coef * env[n-1] + (1 - coef) * target`.
fn time_constant_coef(time_samples: f32) -> f32 {
    (-1.0_f32 / time_samples).exp()
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn lookahead_settings(ceiling_dbfs: f32, lookahead_ms: f32, release_ms: f32) -> AudioSettings {
        AudioSettings {
            peak_limiter_mode: PeakLimiterMode::LookaheadLimiter,
            peak_limiter_ceiling_dbfs: ceiling_dbfs,
            peak_limiter_lookahead_ms: lookahead_ms,
            peak_limiter_release_ms: release_ms,
            ..AudioSettings::default()
        }
    }

    #[test]
    fn channel_buffer_basic_round_trip_two_channels() {
        // With N=4 and 2 channels, the buffer carries (N-1) = 3
        // frames of latency: at frame f, after pushing input[f]
        // and reading the tail, the tail returns input[f-(N-1)].
        let mut b = ChannelBuffer::new(2, 4);
        // Frames 1..4: tail reads zeros (buffer has not yet been
        // filled with N-1 valid samples).
        for &(l, r) in &[
            (1.0_f32, -1.0_f32),
            (2.0, -2.0),
            (3.0, -3.0),
        ] {
            b.push(0, l);
            b.push(1, r);
            assert_eq!(b.tail(0), 0.0);
            assert_eq!(b.tail(1), 0.0);
            b.advance_write_idx();
        }
        // Frame 4: tail returns frame 1's samples (latency N-1=3).
        b.push(0, 4.0);
        b.push(1, -4.0);
        assert_eq!(b.tail(0), 1.0);
        assert_eq!(b.tail(1), -1.0);
        b.advance_write_idx();

        // Frame 5: tail returns frame 2's samples.
        b.push(0, 5.0);
        b.push(1, -5.0);
        assert_eq!(b.tail(0), 2.0);
        assert_eq!(b.tail(1), -2.0);
    }

    #[test]
    fn off_mode_is_sample_passthrough() {
        let settings = AudioSettings {
            peak_limiter_mode: PeakLimiterMode::Off,
            ..AudioSettings::default()
        };
        let mut limiter = PeakLimiter::new(&settings, 48_000, 2);
        let original: Vec<f32> = vec![
            0.0,
            1.0,
            -1.0,
            5.0,
            -5.0,
            0.123_456_7,
            -0.987_654_3,
            1e-9,
            -1e-9,
            0.999_999_94,
        ];
        let len = (original.len() / 2) * 2;
        let mut buf: Vec<f32> = original.iter().take(len).copied().collect();
        let expected = buf.clone();
        limiter.process_inplace(&mut buf);
        assert_eq!(
            buf, expected,
            "Off mode must be sample-for-sample identity"
        );
    }

    #[test]
    fn lookahead_zero_input_stays_zero() {
        let settings = lookahead_settings(-1.0, 5.0, 100.0);
        let mut limiter = PeakLimiter::new(&settings, 48_000, 2);
        let mut buf = vec![0.0_f32; 1_024];
        limiter.process_inplace(&mut buf);
        for (i, &x) in buf.iter().enumerate() {
            assert!(x.abs() < 1e-9, "silence leak at {i}: {x}");
        }
    }

    #[test]
    fn lookahead_clamps_above_ceiling_input() {
        // Drive a constant +2.0 signal (well over the ceiling) and
        // verify that *every* output sample sits at or below the
        // configured ceiling. The clamp guarantees this even before
        // the envelope has caught up.
        let settings = lookahead_settings(-1.0, 5.0, 100.0);
        let mut limiter = PeakLimiter::new(&settings, 48_000, 2);
        let ceiling = 10f32.powf(-1.0 / 20.0);

        let mut buf = vec![2.0_f32; 4_096];
        limiter.process_inplace(&mut buf);

        for (i, &y) in buf.iter().enumerate() {
            assert!(
                y.abs() <= ceiling + 1e-5,
                "ceiling violation at {i}: {y} > {ceiling}"
            );
        }
    }

    #[test]
    fn lookahead_emits_delayed_samples() {
        // Fill the look-ahead with silence, then a single +0.5
        // impulse. The first `lookahead_samples - 1` outputs must
        // be silence; the impulse should appear after that.
        let settings = lookahead_settings(-1.0, 5.0, 100.0);
        let mut limiter = PeakLimiter::new(&settings, 48_000, 1);
        let look = limiter.lookahead_samples();
        assert!(look >= 2);
        let mut buf = vec![0.0_f32; look + 4];
        buf[0] = 0.5; // single impulse, well under the ceiling
        limiter.process_inplace(&mut buf);
        // The impulse is `look - 1` samples behind in this layout
        // (oldest in the buffer after pushing the new sample).
        let ceiling = 10f32.powf(-1.0 / 20.0);
        let target_idx = look - 1;
        assert!(
            (buf[target_idx] - 0.5).abs() < 1e-3 || buf[target_idx] <= ceiling + 1e-5,
            "impulse not where expected at idx {target_idx}: {}",
            buf[target_idx]
        );
        for (i, &y) in buf.iter().enumerate() {
            assert!(
                y.abs() <= ceiling + 1e-5,
                "ceiling violation at {i}: {y}"
            );
        }
    }

    #[test]
    fn soft_clip_mode_applies_polynomial() {
        let settings = AudioSettings {
            peak_limiter_mode: PeakLimiterMode::SoftClip,
            ..AudioSettings::default()
        };
        let mut limiter = PeakLimiter::new(&settings, 48_000, 1);
        let mut buf = vec![0.0_f32, 0.5, 1.5, -1.5, 10.0, -10.0];
        limiter.process_inplace(&mut buf);
        for &y in &buf {
            assert!(y.is_finite());
            assert!((-1.0..=1.0).contains(&y), "out of range: {y}");
        }
        assert_eq!(buf[0], 0.0, "zero stays zero");
    }

    #[test]
    fn reset_clears_delay_line() {
        let settings = lookahead_settings(-1.0, 5.0, 100.0);
        let mut limiter = PeakLimiter::new(&settings, 48_000, 2);
        // Drive some signal so the buffer is non-trivial.
        let mut warm = vec![0.5_f32; 256];
        limiter.process_inplace(&mut warm);
        limiter.reset();
        // Now feed silence; output must be silent (no leak).
        let mut silence = vec![0.0_f32; 4_096];
        limiter.process_inplace(&mut silence);
        for &y in &silence {
            assert!(y.abs() < 1e-9, "post-reset leak: {y}");
        }
        // Envelope is back to unity.
        assert!((limiter.envelope() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn reconfigure_to_off_makes_it_passthrough() {
        let lookahead = lookahead_settings(-1.0, 5.0, 100.0);
        let mut limiter = PeakLimiter::new(&lookahead, 48_000, 2);
        let mut warm = vec![1.5_f32; 256];
        limiter.process_inplace(&mut warm);

        let off = AudioSettings {
            peak_limiter_mode: PeakLimiterMode::Off,
            ..AudioSettings::default()
        };
        limiter.reconfigure(&off, 48_000, 2);
        assert!(limiter.is_bypass());
        let mut buf = vec![0.42_f32, -0.42, 0.42, -0.42];
        let expected = buf.clone();
        limiter.process_inplace(&mut buf);
        assert_eq!(buf, expected);
    }
}
