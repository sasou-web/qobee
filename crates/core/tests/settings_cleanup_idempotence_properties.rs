//! Property test for cleanup idempotence.
//!
//! Feature: audio-engine-simplification, Property 7 — running the
//! cleanup twice produces the same table state as running it once, and
//! a cleanup over a table with none of the obsolete keys completes
//! without error.

use proptest::prelude::*;
use qobee_core::{cleanup_obsolete_audio_settings, OBSOLETE_AUDIO_KEYS};
use qobee_library::Library;

fn fresh_library() -> Library {
    let mut base = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    base.push(format!(
        "qobee-cleanup-idem-{nanos}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&base).expect("temp dir");
    Library::open(&base.join("library.db"), &base.join("covers")).expect("open library")
}

fn snapshot(library: &Library) -> Vec<(String, String)> {
    let mut s = library.list_settings().expect("list");
    s.sort();
    s
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Property 7: cleanup is idempotent (run-twice == run-once).
    #[test]
    fn cleanup_is_idempotent(
        seed_obsolete in proptest::collection::vec(any::<bool>(), OBSOLETE_AUDIO_KEYS.len()),
        keep_non_audio in any::<bool>(),
    ) {
        let library = fresh_library();
        for (i, key) in OBSOLETE_AUDIO_KEYS.iter().enumerate() {
            if seed_obsolete[i] {
                library.set_setting(key, "x").expect("seed");
            }
        }
        if keep_non_audio {
            library.set_setting("ui.theme", "dark").expect("seed");
        }

        cleanup_obsolete_audio_settings(&library).expect("first cleanup");
        let after_once = snapshot(&library);
        cleanup_obsolete_audio_settings(&library).expect("second cleanup");
        let after_twice = snapshot(&library);

        prop_assert_eq!(after_once, after_twice, "cleanup must be idempotent");
    }
}

#[test]
fn cleanup_on_clean_table_does_not_error() {
    let library = fresh_library();
    library.set_setting("ui.theme", "dark").expect("seed");
    // No obsolete keys present at all.
    cleanup_obsolete_audio_settings(&library).expect("cleanup on clean table");
    assert_eq!(
        library.get_setting("ui.theme").expect("read"),
        Some("dark".to_string()),
        "non-audio key preserved"
    );
}
