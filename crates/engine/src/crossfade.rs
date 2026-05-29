//! Crossfade support for the CPAL Shared decoder thread.
//!
//! The decoder thread is **not** realtime-constrained (only the CPAL
//! audio callback is), so it can decode + resample two tracks at once
//! and mix them. This module encapsulates a single track's
//! decode→resample pipeline as a [`DecodeStream`] with a `pull`
//! method that yields an exact number of interleaved frames
//! (silence-padded past end-of-stream), plus the equal-power fade
//! curve used to blend the outgoing and incoming streams.
//!
//! The steady-state (non-crossfade) decode path in
//! `backend_cpal_shared.rs` is left untouched: crossfade is a purely
//! additive branch entered only when `Shared::crossfade_ms() > 0` and
//! a format-compatible next track is prepared. When the fade
//! completes the incoming stream's `decoder` / `resampler` /
//! `pending_planar` / `output_buffer` are handed back to the main
//! loop via [`DecodeStream::into_parts`] so normal playback resumes
//! with zero changes to the hot path.

use rubato::{Resampler, SincFixedIn};

use crate::backend_symphonia::SymphoniaDecoder;
use crate::types::PcmBuffer;

/// The pipeline parts handed back to the main decode loop when a
/// crossfade completes: `(decoder, resampler, output_buffer,
/// pending_planar, leftover_ready)`.
type StreamParts = (
    SymphoniaDecoder,
    Option<SincFixedIn<f32>>,
    Vec<Vec<f32>>,
    Vec<Vec<f32>>,
    Vec<f32>,
);

/// Equal-power (constant-energy) crossfade gains for fade position
/// `t ∈ [0, 1]`. Returns `(out_gain, in_gain)`:
///
///   * `t = 0` → `(1.0, 0.0)` (only the outgoing track),
///   * `t = 1` → `(0.0, 1.0)` (only the incoming track),
///   * `out_gain² + in_gain² == 1` for every `t` (constant power, so
///     the perceived loudness stays flat across the blend instead of
///     dipping in the middle the way a linear fade does).
pub(crate) fn equal_power(t: f32) -> (f32, f32) {
    let t = t.clamp(0.0, 1.0);
    let angle = t * std::f32::consts::FRAC_PI_2;
    (angle.cos(), angle.sin())
}

/// One track's decode + (optional) resample pipeline, with a small
/// FIFO of ready interleaved frames so callers can pull exact frame
/// counts regardless of packet / resampler chunk boundaries.
pub(crate) struct DecodeStream {
    decoder: SymphoniaDecoder,
    resampler: Option<SincFixedIn<f32>>,
    output_buffer: Vec<Vec<f32>>,
    pending_planar: Vec<Vec<f32>>,
    chunk_size_in: usize,
    n_ch: usize,
    /// Ready interleaved frames, consumed from `ready_head` forward.
    ready: Vec<f32>,
    ready_head: usize,
    /// Set once the decoder has returned end-of-stream and the
    /// resampler tail has been flushed.
    eos: bool,
}

impl DecodeStream {
    /// Build a stream from already-constructed parts (used to wrap the
    /// main loop's current track when a crossfade starts).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        decoder: SymphoniaDecoder,
        resampler: Option<SincFixedIn<f32>>,
        output_buffer: Vec<Vec<f32>>,
        pending_planar: Vec<Vec<f32>>,
        chunk_size_in: usize,
        n_ch: usize,
    ) -> Self {
        Self {
            decoder,
            resampler,
            output_buffer,
            pending_planar,
            chunk_size_in,
            n_ch,
            ready: Vec::with_capacity(chunk_size_in * n_ch * 4),
            ready_head: 0,
            eos: false,
        }
    }

    /// Whether the underlying source is exhausted *and* the FIFO is
    /// drained — i.e. every subsequent `pull` would be pure silence.
    #[allow(dead_code)]
    pub(crate) fn is_drained(&self) -> bool {
        self.eos && self.available_samples() == 0
    }

    fn available_samples(&self) -> usize {
        self.ready.len() - self.ready_head
    }

    /// Pull exactly `frames` interleaved frames, appending them to
    /// `out`. Real audio is produced until the source ends; any
    /// shortfall is padded with silence so the caller always gets a
    /// full block. Returns the number of *real* (non-silence) frames
    /// produced this call.
    pub(crate) fn pull(&mut self, frames: usize, out: &mut Vec<f32>) -> usize {
        let need = frames * self.n_ch;
        while self.available_samples() < need && !self.eos {
            self.produce_more();
        }
        let avail = self.available_samples();
        let take = need.min(avail);
        out.extend_from_slice(&self.ready[self.ready_head..self.ready_head + take]);
        self.ready_head += take;
        // Silence-pad the remainder when the source ran short.
        if take < need {
            out.resize(out.len() + (need - take), 0.0);
        }
        self.compact();
        take / self.n_ch
    }

    /// Drop the consumed prefix of the FIFO once it grows large, so
    /// `ready` doesn't accumulate unboundedly during a long fade.
    fn compact(&mut self) {
        if self.ready_head >= 8192 {
            self.ready.drain(..self.ready_head);
            self.ready_head = 0;
        }
    }

    /// Decode one packet (or flush the resampler tail at EOS) and
    /// append the resulting interleaved frames to `ready`.
    fn produce_more(&mut self) {
        match self.decoder.next_packet() {
            Ok(Some(packet)) => {
                let PcmBuffer::F32Interleaved(samples) = packet.buffer else {
                    // Non-f32 buffers never occur on this path; treat
                    // as EOS defensively.
                    self.eos = true;
                    return;
                };
                if self.resampler.is_some() {
                    self.feed_resampler(&samples);
                } else {
                    self.ready.extend_from_slice(&samples);
                }
            }
            Ok(None) => {
                // End of stream: flush any remaining resampler tail
                // so the last partial chunk still reaches the mix.
                self.flush_tail();
                self.eos = true;
            }
            Err(_) => {
                self.eos = true;
            }
        }
    }

    fn feed_resampler(&mut self, samples: &[f32]) {
        let n_ch = self.n_ch;
        deinterleave_append(samples, n_ch, &mut self.pending_planar);
        let Some(r) = self.resampler.as_mut() else {
            return;
        };
        while self.pending_planar[0].len() >= self.chunk_size_in {
            let input_slices: Vec<&[f32]> = self
                .pending_planar
                .iter()
                .map(|ch| &ch[..self.chunk_size_in])
                .collect();
            if let Ok((_in_f, out_f)) =
                r.process_into_buffer(&input_slices, &mut self.output_buffer, None)
            {
                interleave_append(&self.output_buffer, out_f, n_ch, &mut self.ready);
            }
            for ch in self.pending_planar.iter_mut() {
                ch.drain(..self.chunk_size_in);
            }
        }
    }

    fn flush_tail(&mut self) {
        let n_ch = self.n_ch;
        let chunk = self.chunk_size_in;
        let Some(r) = self.resampler.as_mut() else {
            return;
        };
        if !self.pending_planar[0].is_empty() {
            for ch in self.pending_planar.iter_mut() {
                ch.resize(chunk, 0.0);
            }
            let input_slices: Vec<&[f32]> =
                self.pending_planar.iter().map(|ch| &ch[..chunk]).collect();
            if let Ok((_in_f, out_f)) =
                r.process_into_buffer(&input_slices, &mut self.output_buffer, None)
            {
                interleave_append(&self.output_buffer, out_f, n_ch, &mut self.ready);
            }
            for ch in self.pending_planar.iter_mut() {
                ch.clear();
            }
        }
        if let Ok(out) = r.process_partial::<&[f32]>(None, None) {
            let frames = out.first().map(|c| c.len()).unwrap_or(0);
            for f in 0..frames {
                for c in 0..n_ch {
                    self.ready
                        .push(out.get(c).and_then(|v| v.get(f).copied()).unwrap_or(0.0));
                }
            }
        }
    }

    /// Hand the incoming stream's pipeline back to the main decode
    /// loop after the fade completes. Returns
    /// `(decoder, resampler, output_buffer, pending_planar, leftover_ready)`
    /// where `leftover_ready` is the interleaved frames already
    /// decoded past the fade window; the caller must push them
    /// (through the DSP chain) before reading further from the
    /// decoder so no audio is dropped.
    pub(crate) fn into_parts(mut self) -> StreamParts {
        let leftover = self.ready.split_off(self.ready_head);
        (
            self.decoder,
            self.resampler,
            self.output_buffer,
            self.pending_planar,
            leftover,
        )
    }
}

/// Deinterleave `samples` (interleaved `n_ch` channels) into the
/// per-channel planar vectors, appending to whatever is there.
#[allow(clippy::needless_range_loop)]
fn deinterleave_append(samples: &[f32], n_ch: usize, planar: &mut [Vec<f32>]) {
    let frames = samples.len() / n_ch;
    for f in 0..frames {
        for c in 0..n_ch {
            planar[c].push(samples[f * n_ch + c]);
        }
    }
}

/// Interleave the first `frames` of each channel in `planar` and
/// append to `out`.
#[allow(clippy::needless_range_loop)]
fn interleave_append(planar: &[Vec<f32>], frames: usize, n_ch: usize, out: &mut Vec<f32>) {
    out.reserve(frames * n_ch);
    for f in 0..frames {
        for c in 0..n_ch {
            out.push(planar[c][f]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_power_endpoints_and_energy() {
        let (o0, i0) = equal_power(0.0);
        assert!((o0 - 1.0).abs() < 1e-6);
        assert!(i0.abs() < 1e-6);

        let (o1, i1) = equal_power(1.0);
        assert!(o1.abs() < 1e-6);
        assert!((i1 - 1.0).abs() < 1e-6);

        // Constant power across the whole sweep.
        for k in 0..=100 {
            let t = k as f32 / 100.0;
            let (o, i) = equal_power(t);
            assert!((o * o + i * i - 1.0).abs() < 1e-5, "energy drift at t={t}");
        }
    }

    #[test]
    fn equal_power_clamps_out_of_range() {
        assert_eq!(equal_power(-1.0), equal_power(0.0));
        assert_eq!(equal_power(2.0), equal_power(1.0));
    }
}
