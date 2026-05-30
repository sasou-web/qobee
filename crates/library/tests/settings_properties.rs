//! Property-based tests for prefix-targeted settings purge (Phase A,
//! task 2.2).
//!
//! These tests exercise the *real* `Database::clear_settings_by_prefix`
//! path that backs the "Clear uploaded cover links" feature (R8.4).
//! Discord uploaded-cover URLs are persisted under keys shaped like
//! `discord.cover_url::<k>`; the purge must delete *exactly* those
//! keys and leave every other setting untouched.
//!
//! Each case opens a fresh on-disk SQLite database, writes a mix of
//! prefixed and non-prefixed settings through the public `set_setting`
//! API, runs the purge, then reads everything back through the public
//! `get_setting` API — no mocking of the storage layer.
//!
//! Subtlety covered by the generators: SQLite's `LIKE` is
//! case-insensitive for ASCII by default, and the production prefix
//! `discord.cover_url::` contains an underscore (a `LIKE` wildcard)
//! that the implementation escapes. The non-prefixed keys are
//! therefore filtered case-insensitively so the test's notion of
//! "non-prefixed" matches what the `LIKE` purge would actually spare.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use proptest::prelude::*;
use qobee_library::Database;

/// Production prefix under which Discord uploaded-cover links live.
const PREFIX: &str = "discord.cover_url::";

/// Allocate a unique scratch database path under the system temp dir,
/// so each proptest iteration starts from an empty database with no
/// state leaking across cases.
fn fresh_db_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join("qobee-settings-pbt");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join(format!("settings-{nanos}-{n}.sqlite3"))
}

/// Best-effort removal of the database file and its WAL/SHM sidecars.
fn cleanup(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
}

/// Suffixes appended to `PREFIX` to form prefixed keys. Restricted to
/// printable, NUL-free characters; empty suffix is allowed (the bare
/// prefix is itself a valid prefixed key and must be purged).
fn suffix_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9._:/+=-]{0,16}"
}

/// Arbitrary non-prefixed setting keys. Drawn from a character set
/// rich enough to *almost* form the prefix (so the filter actually
/// does work), but always at least one character long.
fn other_key_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9._:/+=-]{1,24}"
}

/// Arbitrary setting values (TEXT NOT NULL). Any NUL-free string.
fn value_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9._:/+= -]{0,24}"
}

// Feature: qobee-beta-feedback-improvements, Property 4: Purge ciblée des liens de covers
//
// Validates: Requirements 8.4
//
// For any HashMap mixing keys prefixed with `discord.cover_url::` and
// arbitrary non-prefixed keys:
//   * `clear_settings_by_prefix("discord.cover_url::")` returns a count
//     exactly equal to the number of prefixed keys present;
//   * every prefixed key is gone afterwards;
//   * every non-prefixed key still holds its original value.
//
// `cover_suffixes` are mapped onto full prefixed keys; `other` keys
// are filtered (case-insensitively, since SQLite `LIKE` ignores ASCII
// case) to guarantee they neither start with the prefix nor collide
// with a prefixed key.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn clear_settings_by_prefix_purges_only_prefixed_keys(
        cover_suffixes in prop::collection::hash_map(suffix_strategy(), value_strategy(), 0..=16),
        other in prop::collection::hash_map(other_key_strategy(), value_strategy(), 0..=16),
    ) {
        let path = fresh_db_path();
        let mut db = Database::open(&path).expect("open db");

        // Build the full prefixed keys. Distinct suffixes -> distinct
        // full keys, all of which start with the exact lowercase prefix.
        let cover_keys: HashMap<String, String> = cover_suffixes
            .iter()
            .map(|(suffix, value)| (format!("{PREFIX}{suffix}"), value.clone()))
            .collect();

        // Keep only genuinely non-prefixed keys. SQLite's LIKE ignores
        // ASCII case, so a key like `DISCORD.COVER_URL::x` *would* be
        // matched by the purge; filtering case-insensitively keeps the
        // test's "non-prefixed" set aligned with what LIKE spares.
        let other_keys: HashMap<String, String> = other
            .iter()
            .filter(|(key, _)| !key.to_ascii_lowercase().starts_with(PREFIX))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        // Write every setting through the public API.
        for (key, value) in cover_keys.iter().chain(other_keys.iter()) {
            db.set_setting(key, value).expect("set_setting");
        }

        // Purge the prefixed keys.
        let removed = db
            .clear_settings_by_prefix(PREFIX)
            .expect("clear_settings_by_prefix");

        // The returned count equals the number of prefixed keys present.
        prop_assert_eq!(removed, cover_keys.len());

        // Every prefixed key is gone.
        for key in cover_keys.keys() {
            let got = db.get_setting(key).expect("get_setting");
            prop_assert_eq!(got, None, "prefixed key {:?} should have been purged", key);
        }

        // Every non-prefixed key still holds its original value.
        for (key, value) in &other_keys {
            let got = db.get_setting(key).expect("get_setting");
            prop_assert_eq!(
                got.as_deref(),
                Some(value.as_str()),
                "non-prefixed key {:?} should be intact",
                key
            );
        }

        drop(db);
        cleanup(&path);
    }
}

// Targeted example test pinning the prefix boundary: the bare prefix
// and a deeper prefixed key are purged, while a key that is a strict
// (shorter) prefix of `PREFIX` is spared. This guards the off-by-one
// boundary that a naive substring match could get wrong.
#[test]
fn clear_settings_prefix_boundary_is_exact() {
    let path = fresh_db_path();
    let mut db = Database::open(&path).expect("open db");

    db.set_setting("discord.cover_url::abc", "1").unwrap();
    db.set_setting(PREFIX, "2").unwrap(); // bare prefix -> purged
    db.set_setting("discord.cover_url", "3").unwrap(); // shorter -> spared
    db.set_setting("discord.cover_url:", "4").unwrap(); // shorter -> spared
    db.set_setting("other.setting", "5").unwrap();

    let removed = db.clear_settings_by_prefix(PREFIX).unwrap();
    assert_eq!(removed, 2, "only the two prefixed keys should be removed");

    assert_eq!(db.get_setting("discord.cover_url::abc").unwrap(), None);
    assert_eq!(db.get_setting(PREFIX).unwrap(), None);
    assert_eq!(
        db.get_setting("discord.cover_url").unwrap().as_deref(),
        Some("3")
    );
    assert_eq!(
        db.get_setting("discord.cover_url:").unwrap().as_deref(),
        Some("4")
    );
    assert_eq!(
        db.get_setting("other.setting").unwrap().as_deref(),
        Some("5")
    );

    drop(db);
    cleanup(&path);
}
