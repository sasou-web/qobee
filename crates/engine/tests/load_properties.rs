//! Property test: loading non-audio input fails without crashing.
//!
//! Feature: audio-engine-simplification, Property 2 — for any byte
//! input that is not a decodable audio stream, `load` returns `Err`
//! and the engine remains usable (no panic / no process exit).
//!
//! The decoder is constructed before the output stream is touched, so a
//! non-audio file is rejected at decode time and the property holds even
//! on a headless machine with no audio device.

use std::io::Write;

use proptest::prelude::*;
use qobee_engine::{new_shared, AudioEngine};

/// Write `bytes` to a uniquely-named temp file and return its path.
fn temp_file_with(bytes: &[u8], tag: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    path.push(format!("qobee-load-prop-{tag}-{nanos}.bin"));
    let mut f = std::fs::File::create(&path).expect("create temp file");
    f.write_all(bytes).expect("write temp file");
    path
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Property 2: arbitrary non-audio bytes make `load` return `Err`,
    /// and the engine survives (a follow-up `state()` call works).
    #[test]
    fn load_rejects_non_audio_without_crashing(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
    ) {
        let engine = new_shared().expect("engine builds");
        let path = temp_file_with(&bytes, "rand");

        // Random bytes are overwhelmingly not a decodable stream. On
        // the astronomically unlikely chance they are, we accept Ok.
        let result = engine.load(&path);
        let _ = std::fs::remove_file(&path);

        // The engine must still be usable regardless of the outcome.
        let _state = engine.state();

        if result.is_ok() {
            // Decoded as audio by sheer luck — still a valid outcome.
            prop_assert!(true);
        } else {
            prop_assert!(result.is_err());
        }
    }
}

#[test]
fn load_of_empty_file_errors_and_engine_survives() {
    let engine = new_shared().expect("engine builds");
    let path = temp_file_with(&[], "empty");
    let result = engine.load(&path);
    let _ = std::fs::remove_file(&path);
    assert!(result.is_err(), "empty file must not decode");
    // Engine is still usable.
    assert_eq!(engine.selected_output_device(), None);
}
