//! Null-test diagnostic primitives (Phase F of audio-quality-improvements).
//!
//! Public surface (re-exported via `pub mod diagnostic;` from the
//! crate root):
//!
//!   * [`generate_test_wav`] — deterministic 5 s stereo 44.1 kHz / 16-bit
//!     PCM WAV writer seeded by an `xorshift32` PRNG. Two calls with the
//!     same seed produce byte-for-byte identical files (Property 23).
//!   * [`align::align_by_cross_correlation`] / [`align::compute_diff`] —
//!     FFT cross-correlation aligner + sample-level diff (Properties 24, 25).
//!   * [`loopback::capture_loopback`] — Windows-only WASAPI loopback
//!     capture (R11.3 / R11.8). Stubbed to
//!     [`EngineError::BackendUnavailable`] elsewhere.
//!   * [`pre_render::open_pre_render`] — Exclusive-mode alternative
//!     where the decoder thread writes the post-DSP stream to a WAV
//!     file instead of the device (R11.3, Exclusive branch).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::error::{EngineError, EngineResult};

pub mod align;
pub mod loopback;
pub mod pre_render;

pub use align::{align_by_cross_correlation, compute_diff, NullTestStats};

/// Default seed used by [`generate_test_wav`] when none is supplied
/// from the orchestrator side. Public so tests can reproduce
/// expected file bytes byte-for-byte.
pub const DEFAULT_TEST_WAV_SEED: u32 = 0xDEADBEEF;

/// Spec for the deterministic WAV the diagnostic writes to disk.
/// Mirrors the requirement values in R11.2 verbatim; defaults are
/// what `run_null_test` actually uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestWavSpec {
    pub seed: u32,
    pub duration_secs: u32,
    pub sample_rate: u32,
    pub channels: u16,
}

impl Default for TestWavSpec {
    fn default() -> Self {
        TestWavSpec {
            seed: DEFAULT_TEST_WAV_SEED,
            duration_secs: 5,
            sample_rate: 44_100,
            channels: 2,
        }
    }
}

/// Single xorshift32 step. Inlined so two successive calls with the
/// same seed produce the same byte sequence (Property 23).
#[inline]
fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// Write a 5 s stereo 44.1 kHz / 16-bit PCM WAV at `path` whose
/// payload is a deterministic xorshift32 PRNG seeded on `seed`.
///
/// Two successive calls with the same `seed` produce byte-for-byte
/// identical files — header included. The header is hand-rolled
/// (RIFF + `fmt ` + `data`) to keep the dependency surface small;
/// `hound` is not pulled in for this single writer.
pub fn generate_test_wav(path: &Path, seed: u32) -> EngineResult<()> {
    let spec = TestWavSpec {
        seed,
        ..TestWavSpec::default()
    };
    write_test_wav(path, spec)
}

/// Same as [`generate_test_wav`] but lets the caller pick non-default
/// parameters. Kept private to the module: every external caller
/// uses the 5 s / 44.1 kHz / stereo / 16-bit shape the spec mandates.
fn write_test_wav(path: &Path, spec: TestWavSpec) -> EngineResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let bits_per_sample: u16 = 16;
    let byte_rate: u32 =
        spec.sample_rate * u32::from(spec.channels) * u32::from(bits_per_sample / 8);
    let block_align: u16 = spec.channels * (bits_per_sample / 8);
    let n_samples: u32 = spec.sample_rate * spec.duration_secs;
    let data_bytes: u32 = n_samples * u32::from(spec.channels) * u32::from(bits_per_sample / 8);
    let riff_size: u32 = 36 + data_bytes;

    let f = File::create(path)?;
    let mut w = BufWriter::new(f);

    // RIFF header
    w.write_all(b"RIFF")?;
    w.write_all(&riff_size.to_le_bytes())?;
    w.write_all(b"WAVE")?;

    // fmt sub-chunk
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?; // chunk size
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&spec.channels.to_le_bytes())?;
    w.write_all(&spec.sample_rate.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&bits_per_sample.to_le_bytes())?;

    // data sub-chunk
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())?;

    // Payload: deterministic xorshift32. The seed is sanitised against
    // the PRNG's fixed point at zero so a `seed = 0` does not produce
    // an all-zero stream.
    let mut state = if spec.seed == 0 {
        0xDEADBEEF
    } else {
        spec.seed
    };
    for _ in 0..n_samples {
        for _ in 0..spec.channels {
            let r = xorshift32(&mut state);
            // Map the 32-bit PRNG output into the i16 range. Top 16
            // bits are the most-uniform; reinterpreted as signed.
            let sample = (r >> 16) as i16;
            w.write_all(&sample.to_le_bytes())?;
        }
    }

    w.flush().map_err(EngineError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xorshift_is_deterministic() {
        let mut a = 0xDEADBEEF;
        let mut b = 0xDEADBEEF;
        for _ in 0..1024 {
            assert_eq!(xorshift32(&mut a), xorshift32(&mut b));
        }
    }

    #[test]
    fn test_wav_default_spec_matches_design() {
        let s = TestWavSpec::default();
        assert_eq!(s.duration_secs, 5);
        assert_eq!(s.sample_rate, 44_100);
        assert_eq!(s.channels, 2);
        assert_eq!(s.seed, DEFAULT_TEST_WAV_SEED);
    }

    #[test]
    fn generated_wav_has_correct_size() {
        let dir = std::env::temp_dir().join("qobee-diag-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("size_check.wav");
        generate_test_wav(&path, 0xABCDEF01).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        // 44 byte header + 5 s × 44_100 frames × 2 channels × 2 bytes.
        let expected = 44 + 5 * 44_100 * 2 * 2;
        assert_eq!(meta.len() as u64, expected as u64);
        let _ = std::fs::remove_file(&path);
    }
}
