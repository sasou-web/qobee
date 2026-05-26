//! Property tests for the null-test diagnostic primitives
//! (Phase F of audio-quality-improvements).
//!
//! Tests:
//!   * Property 23 (R11.2): `generate_test_wav` is byte-for-byte
//!     deterministic.
//!   * Property 24 (R11.4): `align_by_cross_correlation` recovers
//!     a known lag in `[-5_000, 5_000]` samples.
//!   * Property 25 (R11.7): `compute_diff` reports
//!     `samples_diff_count == 0` on identical buffers and `>= 1` when
//!     a single sample differs by 1 LSB.

use proptest::prelude::*;
use qobee_engine::diagnostic::{
    align_by_cross_correlation, compute_diff, generate_test_wav,
};

/// Resolve a unique scratch path under `%TEMP%/qobee-null-test/`.
fn temp_path(stem: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("qobee-null-test-pbt");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{stem}.wav"))
}

// Feature: audio-quality-improvements, Property 23
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        // Each case writes a 5 s WAV to disk; cap the case count
        // so the property remains cheap to run.
        ..ProptestConfig::default()
    })]

    /// Two successive calls to `generate_test_wav` with the same seed
    /// produce byte-for-byte identical files (Property 23).
    #[test]
    fn property_23_generator_is_byte_for_byte_deterministic(seed in any::<u32>()) {
        let a = temp_path(&format!("p23_a_{seed:08x}"));
        let b = temp_path(&format!("p23_b_{seed:08x}"));
        generate_test_wav(&a, seed).unwrap();
        generate_test_wav(&b, seed).unwrap();
        let bytes_a = std::fs::read(&a).unwrap();
        let bytes_b = std::fs::read(&b).unwrap();
        prop_assert_eq!(bytes_a, bytes_b);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }
}

/// Build a deterministic random source with `len` samples seeded on
/// `seed`. Uses the same xorshift32 step as the WAV generator so the
/// signal looks like the one the diagnostic actually plays.
fn random_source(len: usize, seed: u32) -> Vec<f32> {
    let mut state = if seed == 0 { 0xDEADBEEF } else { seed };
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        out.push(((state >> 16) as i16) as f32 / 32_768.0);
    }
    out
}

// Feature: audio-quality-improvements, Property 24
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    /// The aligner recovers a known lag within the design's
    /// `[-5_000, 5_000]` window with accuracy ≤ 1 sample
    /// (Property 24).
    #[test]
    fn property_24_aligner_round_trip(
        seed in 1u32..=1_000_000u32,
        lag in -5_000i64..=5_000i64,
    ) {
        let source_len: usize = 16_384;
        let source = random_source(source_len, seed);

        // Apply the known lag and pad with silence so the captured
        // buffer is at least as long as the source.
        let mut captured = Vec::with_capacity(source_len + 8_192);
        if lag >= 0 {
            captured.extend(std::iter::repeat(0.0f32).take(lag as usize));
            captured.extend_from_slice(&source);
            captured.extend(std::iter::repeat(0.0f32).take(8_192));
        } else {
            // Negative lag: captured is advanced. Drop the first
            // `|lag|` samples of source.
            let drop = (-lag) as usize;
            if drop < source.len() {
                captured.extend_from_slice(&source[drop..]);
                captured.extend(std::iter::repeat(0.0f32).take(8_192));
            } else {
                // Degenerate case: skip.
                return Ok(());
            }
        }

        let recovered = align_by_cross_correlation(&source, &captured);
        prop_assert!(
            (recovered - lag).abs() <= 1,
            "expected lag ≈ {lag}, got {recovered}"
        );
    }
}

// Feature: audio-quality-improvements, Property 25
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        ..ProptestConfig::default()
    })]

    /// `compute_diff` reports `samples_diff_count == 0` when the
    /// buffers are identical, and `>= 1` when one sample differs by
    /// at least 1 LSB at 16-bit (Property 25).
    #[test]
    fn property_25_conclusion_matches_diff(
        seed in 1u32..=1_000_000u32,
        flip_idx in 0usize..1024usize,
    ) {
        let n = 1024;
        let source = random_source(n, seed);
        let mut captured = source.clone();

        let stats_eq = compute_diff(&source, &captured, 0);
        prop_assert_eq!(stats_eq.samples_diff_count, 0);
        prop_assert_eq!(stats_eq.aligned_frames as usize, n);

        // Perturb one sample by exactly 1 LSB at 16-bit.
        let lsb = 1.0 / 32_768.0;
        let idx = flip_idx % n;
        captured[idx] += lsb;

        let stats_perturbed = compute_diff(&source, &captured, 0);
        prop_assert!(
            stats_perturbed.samples_diff_count >= 1,
            "expected ≥ 1 diff sample after a 1 LSB perturbation, got {}",
            stats_perturbed.samples_diff_count
        );
    }
}
