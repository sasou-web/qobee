//! Property-based tests for playlist persistence (Phase A, task 1.3).
//!
//! These tests exercise the *real* `Database::add_to_playlist`
//! persistence path that backs the "Add to playlist" feature (the
//! root cause of beta-test constat T17). They open a fresh on-disk
//! SQLite database per case, append tracks through the public API,
//! and verify the resulting `playlist_tracks` rows by reading them
//! back with an independent connection — no mocking of the storage
//! layer.
//!
//! `playlist_tracks` carries a `FOREIGN KEY(track_id) REFERENCES
//! tracks(id)` and is keyed by `(playlist_id, position)`, so the
//! track ids appended must reference real `tracks` rows while still
//! allowing repeats (the position, not the track id, is the primary
//! key). Each case therefore materialises one real track row per
//! distinct *selector* and maps the generated sequences onto those
//! real ids — preserving the property's intent (arbitrary ordered
//! sequences of valid track ids, with repeats) without weakening it.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use proptest::collection::vec;
use proptest::prelude::*;
use qobee_library::{Database, Track};
use rusqlite::{params, Connection};

/// Fixed logical timestamp; the value is irrelevant to the ordering
/// invariants under test.
const NOW: i64 = 1_000_000;

/// Allocate a unique scratch database path under the system temp dir.
/// A per-case unique path guarantees each proptest iteration starts
/// from an empty database with no state leaking across cases.
fn fresh_db_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join("qobee-playlist-pbt");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join(format!("playlist-{nanos}-{n}.sqlite3"))
}

/// Best-effort removal of the database file and its WAL/SHM sidecars.
fn cleanup(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
}

/// Synthetic track row with a unique path. `upsert_track` calls
/// `fs::metadata` for an mtime; that lookup fails silently for a
/// non-existent path and yields `mtime = 0`, which is fine here.
fn make_track(idx: usize) -> Track {
    Track {
        id: 0,
        track_uid: String::new(),
        path: format!("/synthetic/track-{idx}.flac"),
        title: format!("Title {idx}"),
        artist: "Artist".into(),
        album: "Album".into(),
        album_artist: None,
        track_number: None,
        disc_number: None,
        year: None,
        genre: None,
        duration_seconds: 0.0,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        replaygain_track_db: None,
        replaygain_album_db: None,
        replaygain_track_peak: None,
        replaygain_album_peak: None,
        cover_key: None,
    }
}

/// Read the `(track_id, position)` rows of a playlist, ordered by
/// position, using a separate read connection to confirm the data was
/// actually committed to disk.
fn read_playlist_rows(path: &PathBuf, playlist_id: i64) -> Vec<(i64, i64)> {
    let conn = Connection::open(path).expect("open read connection");
    let mut stmt = conn
        .prepare("SELECT track_id, position FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position")
        .expect("prepare select");
    let rows = stmt
        .query_map(params![playlist_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .expect("query_map")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect rows");
    rows
}

// Feature: qobee-beta-feedback-improvements, Property 1: Persistance d'ajout à une playlist
//
// Validates: Requirements 3.2
//
// For any two batches of track ids appended to a playlist:
//   * the final content ordered by `position` equals `initial ++ added`;
//   * the count delta from the second append equals `added.len()`;
//   * positions stay unique and contiguous starting at 0;
//   * the value returned by `add_to_playlist` equals `added.len()`.
//
// The generated `initial_sel` / `added_sel` are *selectors* (small
// indices, repeats allowed) mapped onto freshly inserted real track
// ids, so the FK on `playlist_tracks.track_id` holds for every case.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn add_to_playlist_persists_in_order(
        initial_sel in vec(0usize..8, 0..=20usize),
        added_sel in vec(0usize..8, 0..=20usize),
    ) {
        let path = fresh_db_path();
        let mut db = Database::open(&path).expect("open db");

        // Materialise one real track per distinct selector so every
        // appended track id references an existing `tracks` row.
        let distinct: BTreeSet<usize> =
            initial_sel.iter().chain(added_sel.iter()).copied().collect();
        let mut id_for: HashMap<usize, i64> = HashMap::new();
        for &sel in &distinct {
            let track_id = db
                .upsert_track(&make_track(sel), None)
                .expect("insert track");
            id_for.insert(sel, track_id);
        }
        let initial: Vec<i64> = initial_sel.iter().map(|s| id_for[s]).collect();
        let added: Vec<i64> = added_sel.iter().map(|s| id_for[s]).collect();

        let playlist = db.create_playlist("P", NOW).expect("create playlist");

        // Seed the playlist with the initial batch.
        let initial_inserted = db
            .add_to_playlist(playlist.id, &initial, NOW)
            .expect("add initial batch");
        prop_assert_eq!(initial_inserted, initial.len());

        // Count after the initial batch, before the measured append.
        let count_before = read_playlist_rows(&path, playlist.id).len();
        prop_assert_eq!(count_before, initial.len());

        // The measured append: its return value must equal added.len().
        let added_inserted = db
            .add_to_playlist(playlist.id, &added, NOW)
            .expect("add second batch");
        prop_assert_eq!(added_inserted, added.len());

        let rows = read_playlist_rows(&path, playlist.id);

        // Count delta equals added.len().
        let count_after = rows.len();
        prop_assert_eq!(count_after - count_before, added.len());

        // Content ordered by position equals initial ++ added.
        let expected_content: Vec<i64> =
            initial.iter().chain(added.iter()).copied().collect();
        let actual_content: Vec<i64> = rows.iter().map(|(tid, _)| *tid).collect();
        prop_assert_eq!(&actual_content, &expected_content);

        // Positions are unique and contiguous starting at 0.
        let positions: Vec<i64> = rows.iter().map(|(_, pos)| *pos).collect();
        let expected_positions: Vec<i64> = (0..rows.len() as i64).collect();
        prop_assert_eq!(&positions, &expected_positions);

        drop(db);
        cleanup(&path);
    }
}
