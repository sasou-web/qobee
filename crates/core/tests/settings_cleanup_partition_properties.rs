//! Property test for the obsolete-audio-settings cleanup partition.
//!
//! Feature: audio-engine-simplification, Property 6 — the startup
//! cleanup removes every obsolete audio key and leaves every other key
//! (retained audio keys and all non-audio keys) present and unchanged.

use proptest::prelude::*;
use qobee_core::{cleanup_obsolete_audio_settings, OBSOLETE_AUDIO_KEYS};
use qobee_library::Library;

/// Keys that must survive the cleanup: a kept audio device key plus a
/// representative spread of non-audio namespaces.
const RETAINED_KEYS: &[&str] = &[
    "playback.output_device_id",
    "playback.default_volume",
    "ui.theme",
    "ui.accent",
    "integrations.discord_client_id",
    "session.state",
    "windows.close_behavior",
];

fn fresh_library() -> Library {
    let mut base = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    base.push(format!(
        "qobee-cleanup-partition-{nanos}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&base).expect("temp dir");
    Library::open(&base.join("library.db"), &base.join("covers")).expect("open library")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Property 6: cleanup removes exactly the obsolete audio keys and
    /// preserves every retained key with its original value.
    #[test]
    fn cleanup_removes_only_obsolete_keys(
        // Independently choose which obsolete + retained keys to seed.
        seed_obsolete in proptest::collection::vec(any::<bool>(), OBSOLETE_AUDIO_KEYS.len()),
        seed_retained in proptest::collection::vec(any::<bool>(), RETAINED_KEYS.len()),
    ) {
        let library = fresh_library();

        // Seed obsolete keys.
        for (i, key) in OBSOLETE_AUDIO_KEYS.iter().enumerate() {
            if seed_obsolete[i] {
                library.set_setting(key, "seeded-obsolete").expect("seed");
            }
        }
        // Seed retained keys with distinct, recoverable values.
        for (i, key) in RETAINED_KEYS.iter().enumerate() {
            if seed_retained[i] {
                library.set_setting(key, &format!("keep-{i}")).expect("seed");
            }
        }

        cleanup_obsolete_audio_settings(&library).expect("cleanup");

        // Every obsolete key is gone.
        for key in OBSOLETE_AUDIO_KEYS {
            prop_assert_eq!(
                library.get_setting(key).expect("read"),
                None,
                "obsolete key {} should be removed",
                key
            );
        }

        // Every seeded retained key survives with its original value.
        for (i, key) in RETAINED_KEYS.iter().enumerate() {
            if seed_retained[i] {
                prop_assert_eq!(
                    library.get_setting(key).expect("read"),
                    Some(format!("keep-{i}")),
                    "retained key {} must be preserved unchanged",
                    key
                );
            }
        }
    }
}
