//! `Convolver_Stage` — uniform-partitioned FFT convolution (R9).
//!
//! Inserted in the PCM chain between the EQ and the `Peak_Limiter`.
//! Routes the left and right channels of a stereo flow through two
//! independent `fft_convolver::FFTConvolver` instances (block size
//! `BLOCK_SIZE = 256`), so per-channel processing stays linear and
//! free of cross-channel coupling (Property 17). Mono and any flow
//! with more than two channels are passthrough.
//!
//! ## Bypass (R9.7)
//!
//! Exact bypass when `!enabled || ir.is_none() || channels != 2`.
//! In that state `process_inplace` returns without touching the
//! buffer (Property 18).
//!
//! ## IR snapshot (R9.2 / R9.4 / R9.5)
//!
//! The IR is held in [`IrData`] outside the hot path. `set_ir` is
//! called from a worker thread (typically `Player::load_convolver_ir`
//! in `crates/core`) once the WAV has been decoded, validated,
//! resampled to the device rate, and gain-compensated. Mono IRs are
//! duplicated to `(left, right)` upstream.
//!
//! ## Reset
//!
//! `reset()` rebuilds the FFT convolver buffers from the active IR
//! so a seek does not bleed previous audio through the partitioned
//! tail. When no IR is loaded `reset()` is a cheap no-op.

use fft_convolver::FFTConvolver;

use crate::audio_settings::AudioSettings;
use crate::dsp::DspStage;

/// Partition size used by the uniform-partitioned FFT convolver.
/// At 48 kHz this gives ≈ 5.3 ms of latency, well under the 20 ms
/// ceiling from R9.6.
const BLOCK_SIZE: usize = 256;

/// Snapshot of the currently-loaded impulse response. Held outside
/// the hot path so the audio callback never sees an allocation.
struct IrData {
    left: Vec<f32>,
    right: Vec<f32>,
    /// Sample rate the IR was resampled to before being handed off.
    /// The convolver itself does not consume this value, but it
    /// stays here so a later mismatch (device rate change) can be
    /// detected by the loader and rebuild the IR.
    #[allow(dead_code)]
    sample_rate: u32,
}

/// `Convolver_Stage` — see module-level documentation.
pub struct ConvolverStage {
    enabled: bool,
    ir: Option<IrData>,
    convolver_l: FFTConvolver<f32>,
    convolver_r: FFTConvolver<f32>,
    /// Linear gain applied to the IR coefficients on `set_ir`. Stored
    /// for diagnostics; the multiplication itself happens before the
    /// `FFTConvolver::set_response` call.
    gain_compensation: f32,
    sample_rate: u32,
    channels: u16,
    bypass: bool,
}

impl ConvolverStage {
    pub fn new(settings: &AudioSettings, sample_rate: u32, channels: u16) -> Self {
        let gain_compensation = 10f32.powf(settings.convolver_gain_db / 20.0);
        Self {
            enabled: settings.convolver_enabled,
            ir: None,
            convolver_l: FFTConvolver::default(),
            convolver_r: FFTConvolver::default(),
            gain_compensation,
            sample_rate,
            channels,
            // No IR loaded at construction: bypass until `set_ir`
            // installs one. R9.7: bypass = !enabled || ir.is_none()
            // || channels != 2.
            bypass: true,
        }
    }

    /// Return `true` when the stage currently holds an IR.
    pub fn has_ir(&self) -> bool {
        self.ir.is_some()
    }

    /// Length of the active IR in taps (mono); `0` when none loaded.
    pub fn ir_len(&self) -> usize {
        self.ir.as_ref().map(|d| d.left.len()).unwrap_or(0)
    }

    /// Replace the active IR with a fresh `(left, right)` pair.
    /// Called off the audio hot path. Rebuilds both `FFTConvolver`
    /// instances at `BLOCK_SIZE` and refreshes the steady-state
    /// bypass flag. Pass two empty vectors to drop the IR (the
    /// stage falls back to bypass on the next chunk).
    ///
    /// Gain compensation (R9.10) is applied by the upstream loader
    /// (`Player::load_convolver_ir`) before calling this method, so
    /// the IR samples passed in are already pre-multiplied by
    /// `10^(convolver_gain_db / 20)`. Keeping the multiplication on
    /// the loader side avoids a double-apply when the engine stage's
    /// `gain_compensation` snapshot is out of sync with the loader's
    /// view of `audio.convolver_gain_db`.
    pub fn set_ir(&mut self, ir_left: Vec<f32>, ir_right: Vec<f32>) {
        if ir_left.is_empty() || ir_right.is_empty() {
            self.ir = None;
            self.convolver_l.reset();
            self.convolver_r.reset();
            self.bypass = !self.enabled || self.channels != 2 || self.ir.is_none();
            return;
        }

        // `init` allocates; that is fine here (worker thread, not
        // the audio callback). Rebuilding from scratch avoids the
        // `FFTConvolver::set_response` "must be ≤ original length"
        // restriction so a longer IR can replace a shorter one.
        let mut conv_l = FFTConvolver::<f32>::default();
        let mut conv_r = FFTConvolver::<f32>::default();
        if conv_l.init(BLOCK_SIZE, &ir_left).is_err()
            || conv_r.init(BLOCK_SIZE, &ir_right).is_err()
        {
            // Build failed: drop the IR and stay in bypass. The
            // loader is expected to validate inputs upstream so this
            // path is purely defensive.
            self.ir = None;
            self.convolver_l = FFTConvolver::<f32>::default();
            self.convolver_r = FFTConvolver::<f32>::default();
            self.bypass = true;
            return;
        }
        self.convolver_l = conv_l;
        self.convolver_r = conv_r;
        self.ir = Some(IrData {
            left: ir_left,
            right: ir_right,
            sample_rate: self.sample_rate,
        });
        self.bypass = !self.enabled || self.channels != 2;
    }
}

impl DspStage for ConvolverStage {
    fn reconfigure(&mut self, settings: &AudioSettings, sample_rate: u32, channels: u16) {
        self.sample_rate = sample_rate;
        self.channels = channels;
        self.gain_compensation = 10f32.powf(settings.convolver_gain_db / 20.0);

        // R9.7 — disabling the convolver drops the IR entirely. The
        // loader is responsible for re-pushing it if the user
        // re-enables the stage later.
        if !settings.convolver_enabled {
            self.enabled = false;
            self.ir = None;
            self.convolver_l.reset();
            self.convolver_r.reset();
            self.bypass = true;
            return;
        }

        self.enabled = settings.convolver_enabled;
        self.bypass = !self.enabled || self.channels != 2 || self.ir.is_none();
    }

    fn is_bypass(&self) -> bool {
        self.bypass
    }

    fn process_inplace(&mut self, samples: &mut [f32]) {
        if self.bypass || self.channels != 2 || self.ir.is_none() {
            return;
        }

        let frames = samples.len() / 2;
        if frames == 0 {
            return;
        }

        // Deinterleave into two scratch buffers, run each through
        // its own convolver, then re-interleave. Allocation is
        // unavoidable here without a pre-sized scratch field, but
        // the chain runs at chunk boundaries (typically 1024 frames
        // post-resample), so the cost is modest. A future
        // optimisation could keep two `Vec<f32>` scratch buffers on
        // the struct.
        let mut in_l: Vec<f32> = Vec::with_capacity(frames);
        let mut in_r: Vec<f32> = Vec::with_capacity(frames);
        for f in 0..frames {
            in_l.push(samples[f * 2]);
            in_r.push(samples[f * 2 + 1]);
        }
        let mut out_l = vec![0.0_f32; frames];
        let mut out_r = vec![0.0_f32; frames];

        if self
            .convolver_l
            .process(&in_l, &mut out_l)
            .is_err()
            || self.convolver_r.process(&in_r, &mut out_r).is_err()
        {
            // Process failed (shouldn't happen with a valid IR);
            // leave the buffer untouched rather than emitting
            // potentially garbage output.
            return;
        }

        for f in 0..frames {
            samples[f * 2] = out_l[f];
            samples[f * 2 + 1] = out_r[f];
        }
    }

    fn reset(&mut self) {
        // Seek/load: rebuild the convolver state from the current IR
        // so the partitioned tail does not bleed across the seek
        // point. With no IR loaded both convolvers are already empty
        // and `reset` is a cheap no-op.
        if let Some(ir) = &self.ir {
            let mut conv_l = FFTConvolver::<f32>::default();
            let mut conv_r = FFTConvolver::<f32>::default();
            if conv_l.init(BLOCK_SIZE, &ir.left).is_ok()
                && conv_r.init(BLOCK_SIZE, &ir.right).is_ok()
            {
                self.convolver_l = conv_l;
                self.convolver_r = conv_r;
            }
        } else {
            self.convolver_l.reset();
            self.convolver_r.reset();
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_settings() -> AudioSettings {
        AudioSettings {
            convolver_enabled: true,
            convolver_gain_db: 0.0, // unity gain so set_ir doesn't scale
            ..AudioSettings::default()
        }
    }

    #[test]
    fn default_disabled_is_bypass() {
        let s = ConvolverStage::new(&AudioSettings::default(), 48_000, 2);
        assert!(s.is_bypass(), "default settings must bypass");
    }

    #[test]
    fn enabled_without_ir_is_bypass() {
        let s = ConvolverStage::new(&enabled_settings(), 48_000, 2);
        // No IR loaded yet → still bypassed.
        assert!(s.is_bypass());
    }

    #[test]
    fn enabled_mono_is_bypass() {
        let s = ConvolverStage::new(&enabled_settings(), 48_000, 1);
        assert!(s.is_bypass(), "mono flow must bypass");
    }

    #[test]
    fn enabled_surround_is_bypass() {
        for ch in [3u16, 4, 5, 6, 7, 8] {
            let s = ConvolverStage::new(&enabled_settings(), 48_000, ch);
            assert!(s.is_bypass(), "{ch}-channel flow must bypass");
        }
    }

    #[test]
    fn delta_ir_is_passthrough_after_warmup() {
        // With ir = [1.0] each channel becomes y[n] = x[n].
        let mut s = ConvolverStage::new(&enabled_settings(), 48_000, 2);
        s.set_ir(vec![1.0], vec![1.0]);
        assert!(!s.is_bypass());

        // Drive enough samples for the partitioned FFT to fill its
        // first block. At BLOCK_SIZE = 256 the latency is one block.
        let frames = BLOCK_SIZE * 2;
        let mut buf = Vec::with_capacity(frames * 2);
        for f in 0..frames {
            let v = ((f as f32) * 0.001).sin();
            buf.push(v);
            buf.push(-v);
        }
        let original = buf.clone();
        s.process_inplace(&mut buf);

        // After the first block, output equals input.
        for f in BLOCK_SIZE..frames {
            assert!(
                (buf[f * 2] - original[f * 2]).abs() < 1e-3,
                "L channel: frame {f} got {} expected {}",
                buf[f * 2],
                original[f * 2]
            );
            assert!(
                (buf[f * 2 + 1] - original[f * 2 + 1]).abs() < 1e-3,
                "R channel: frame {f} got {} expected {}",
                buf[f * 2 + 1],
                original[f * 2 + 1]
            );
        }
    }

    #[test]
    fn zero_ir_produces_silence() {
        let mut s = ConvolverStage::new(&enabled_settings(), 48_000, 2);
        s.set_ir(vec![0.0; 64], vec![0.0; 64]);
        let mut buf = vec![0.5_f32; BLOCK_SIZE * 4];
        s.process_inplace(&mut buf);
        for (i, &y) in buf.iter().enumerate() {
            assert!(y.abs() < 1e-6, "non-silent sample at {i}: {y}");
        }
    }

    #[test]
    fn reconfigure_disabled_drops_ir() {
        let mut s = ConvolverStage::new(&enabled_settings(), 48_000, 2);
        s.set_ir(vec![1.0], vec![1.0]);
        assert!(!s.is_bypass());

        // Disable: stage drops the IR and goes back to bypass.
        let disabled = AudioSettings::default();
        s.reconfigure(&disabled, 48_000, 2);
        assert!(s.is_bypass());
        assert!(!s.has_ir());
    }

    #[test]
    fn empty_ir_drops_active_ir() {
        let mut s = ConvolverStage::new(&enabled_settings(), 48_000, 2);
        s.set_ir(vec![1.0, 0.5], vec![0.5, 1.0]);
        assert!(s.has_ir());
        s.set_ir(vec![], vec![]);
        assert!(!s.has_ir());
        assert!(s.is_bypass());
    }
}
