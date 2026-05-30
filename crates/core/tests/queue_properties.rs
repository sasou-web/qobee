//! Property-based tests for the playback queue exposed by
//! [`qobee_core::Queue`] (cf. spec
//! `qobee-beta-feedback-improvements`).
//!
//! ## Scope
//!
//! This file covers **Property 2 — Invariants des opérations de file
//! d'attente**, exercising the three mutating operations a user can
//! drive from the Queue_Panel (R3.4 "Add to queue", R3.7 "Vider la
//! file" / réorganisation) directly against the real
//! [`qobee_core::Queue`]:
//!
//!   * `append(tracks)` produces `items_initiaux ++ tracks` — order
//!     preserved, length grows by exactly `tracks.len()`.
//!   * `move_item(from, to)` for any valid `(from, to)` preserves the
//!     multiset of track ids and its cardinality, lands the moved item
//!     at the target index, and keeps the cursor pointing at the same
//!     track id.
//!   * `clear()` produces an empty queue with cursor `None`.
//!
//! ## Why the real `Queue` and not a model
//!
//! Unlike `PlayerHandle` (which needs a seeded `Library` plus a live
//! `CpalSharedEngine` audio worker — see `player_sync_properties.rs`
//! for the rationale behind modelling that one), `Queue` is a pure
//! in-memory structure with no I/O, no device probing and no async
//! runtime. It is cheap to construct hundreds of times inside a
//! `proptest!` block, so we test the genuine implementation rather
//! than a transcription of it.

use proptest::prelude::*;

use qobee_core::queue::{Queue, TrackId};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Multiset equality on track ids: two slices contain the same ids
/// with the same multiplicities, regardless of order. Used to assert
/// that `move_item` is a pure permutation of the queue contents.
fn multiset_eq(a: &[TrackId], b: &[TrackId]) -> bool {
    let mut sa = a.to_vec();
    let mut sb = b.to_vec();
    sa.sort_unstable();
    sb.sort_unstable();
    sa == sb
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Track ids are drawn from a deliberately small range so the same id
/// reappears within a sequence. This makes the multiset / cursor
/// invariants for `move_item` meaningfully exercise the
/// duplicate-track case (the user can legitimately add the same track
/// to the queue twice).
fn any_track_id() -> impl Strategy<Value = TrackId> {
    0i64..=8
}

/// A possibly-empty queue payload, up to 16 tracks.
fn any_track_vec() -> impl Strategy<Value = Vec<TrackId>> {
    proptest::collection::vec(any_track_id(), 0..=16)
}

/// Initial queue contents plus a batch to append. The initial vec may
/// be empty (fresh queue) and the appended batch may be empty (append
/// is documented as a no-op for an empty input).
fn append_scenario() -> impl Strategy<Value = (Vec<TrackId>, Vec<TrackId>)> {
    (any_track_vec(), any_track_vec())
}

/// A non-empty queue together with a cursor position and a valid
/// `(from, to)` move, all constrained to in-range indices `0..len`.
/// `from == to` is allowed: `move_item` treats it as a no-op and the
/// invariants must still hold trivially.
fn move_scenario() -> impl Strategy<Value = (Vec<TrackId>, usize, usize, usize)> {
    proptest::collection::vec(any_track_id(), 1..=16).prop_flat_map(|items| {
        let n = items.len();
        (Just(items), 0..n, 0..n, 0..n)
    })
}

/// A queue payload plus an optional starting cursor for the `clear`
/// scenario. `None` models a stopped/empty queue; `Some(i)` an
/// actively-positioned one.
fn clear_scenario() -> impl Strategy<Value = (Vec<TrackId>, Option<usize>)> {
    any_track_vec().prop_flat_map(|items| {
        let start = if items.is_empty() {
            Just(None).boxed()
        } else {
            prop_oneof![Just(None), (0..items.len()).prop_map(Some)].boxed()
        };
        (Just(items), start)
    })
}

// ---------------------------------------------------------------------------
// Property 2 — Invariants des opérations de file d'attente
// ---------------------------------------------------------------------------

// Feature: qobee-beta-feedback-improvements, Property 2: Invariants des opérations de file d'attente
//
// For the three mutating queue operations, on the real
// `qobee_core::Queue`:
//
//   * append: `snapshot().items` after == items before `++ tracks`,
//     and length grows by exactly `tracks.len()`.
//   * move_item(from, to) (valid indices): the multiset of ids is
//     preserved, the cardinality is preserved, the moved item lands at
//     `to` (`after.items[to] == before.items[from]`), and the cursor
//     still points at the same track id it pointed at before the move.
//     The boolean result is `true` iff `from != to`.
//   * clear: `snapshot().items` is empty and `snapshot().cursor` is
//     `None` (and the convenience accessors agree: `len() == 0`,
//     `is_empty()`).
//
// Validates: Requirements 3.4, 3.7; Property 2.
#[test]
fn property_2_queue_operation_invariants() {
    proptest!(
        ProptestConfig::with_cases(256),
        |(
            (initial, appended) in append_scenario(),
            (move_items, cursor_idx, from, to) in move_scenario(),
            (clear_items, clear_start) in clear_scenario(),
        )| {
            // --- append ---------------------------------------------------
            // Seed via `replace` (shuffle is off by default, so items
            // are kept verbatim) then append and compare snapshots.
            let q = Queue::new();
            q.replace(initial.clone(), None);
            let before = q.snapshot();
            q.append(appended.clone());
            let after = q.snapshot();

            let mut expected = before.items.clone();
            expected.extend(appended.iter().copied());
            prop_assert_eq!(
                &after.items, &expected,
                "append must yield items_initiaux ++ tracks: initial={:?}, appended={:?}",
                initial, appended,
            );
            prop_assert_eq!(
                after.items.len(),
                before.items.len() + appended.len(),
                "append must grow length by exactly tracks.len()",
            );

            // --- move_item ------------------------------------------------
            let q = Queue::new();
            q.replace(move_items.clone(), Some(cursor_idx));
            let before = q.snapshot();
            // The cursor is guaranteed `Some` because the queue is
            // non-empty and we seeded an in-range start index.
            let cur_idx = before.cursor.expect("seeded cursor must be Some on a non-empty queue");
            let cur_id = before.items[cur_idx];

            let moved = q.move_item(from, to);
            let after = q.snapshot();

            // Boolean contract: a real move happens iff the indices differ.
            prop_assert_eq!(
                moved,
                from != to,
                "move_item return must be (from != to): from={}, to={}",
                from, to,
            );
            // Pure permutation: same ids, same multiplicities, same count.
            prop_assert!(
                multiset_eq(&before.items, &after.items),
                "move_item must preserve the multiset: before={:?}, after={:?}, from={}, to={}",
                before.items, after.items, from, to,
            );
            prop_assert_eq!(
                after.items.len(),
                before.items.len(),
                "move_item must preserve cardinality",
            );
            // The moved item lands at the target index (positional,
            // robust to duplicate ids).
            prop_assert_eq!(
                after.items[to],
                before.items[from],
                "moved item must land at target index: before={:?}, after={:?}, from={}, to={}",
                before.items, after.items, from, to,
            );
            // The cursor keeps pointing at the same track id. With
            // duplicate ids the numeric index may shift, so we assert
            // on the id under the cursor rather than the index itself.
            let new_cur = after.cursor.expect("cursor must remain Some after a move on a non-empty queue");
            prop_assert_eq!(
                after.items[new_cur],
                cur_id,
                "cursor must still point at the same track id: \
                 before={:?}, after={:?}, cur_id={}, from={}, to={}",
                before.items, after.items, cur_id, from, to,
            );

            // --- clear ----------------------------------------------------
            let q = Queue::new();
            q.replace(clear_items.clone(), clear_start);
            q.clear();
            let after = q.snapshot();
            prop_assert!(
                after.items.is_empty(),
                "clear must empty the queue: items={:?}",
                clear_items,
            );
            prop_assert_eq!(
                after.cursor,
                None,
                "clear must reset the cursor to None",
            );
            prop_assert_eq!(q.len(), 0, "clear must leave len() == 0");
            prop_assert!(q.is_empty(), "clear must leave is_empty() == true");
        }
    );
}

// ---------------------------------------------------------------------------
// Sanity unit tests
// ---------------------------------------------------------------------------
//
// These pin the three invariant families down by example so a
// contributor can grok them without re-deriving from the proptest
// block. They are not a substitute for the property above.

#[test]
fn append_concatenates_preserving_order() {
    let q = Queue::new();
    q.replace(vec![1, 2, 3], Some(1));
    q.append(vec![4, 5]);
    let snap = q.snapshot();
    assert_eq!(snap.items, vec![1, 2, 3, 4, 5]);
    // Cursor is untouched by append.
    assert_eq!(snap.cursor, Some(1));
}

#[test]
fn append_empty_is_a_noop() {
    let q = Queue::new();
    q.replace(vec![1, 2], None);
    let before = q.snapshot();
    q.append(vec![]);
    let after = q.snapshot();
    assert_eq!(before.items, after.items);
}

#[test]
fn move_item_lands_at_target_and_tracks_cursor() {
    let q = Queue::new();
    q.replace(vec![10, 20, 30, 40], Some(2)); // cursor on id 30
    assert!(q.move_item(0, 3)); // move 10 to the end
    let snap = q.snapshot();
    assert_eq!(snap.items, vec![20, 30, 40, 10]);
    // Cursor followed id 30 from index 2 to index 1.
    assert_eq!(snap.cursor.map(|c| snap.items[c]), Some(30));
}

#[test]
fn move_item_same_index_is_noop() {
    let q = Queue::new();
    q.replace(vec![1, 2, 3], Some(0));
    assert!(!q.move_item(1, 1));
    assert_eq!(q.snapshot().items, vec![1, 2, 3]);
}

#[test]
fn clear_empties_queue_and_resets_cursor() {
    let q = Queue::new();
    q.replace(vec![1, 2, 3], Some(2));
    q.clear();
    let snap = q.snapshot();
    assert!(snap.items.is_empty());
    assert_eq!(snap.cursor, None);
    assert_eq!(q.len(), 0);
    assert!(q.is_empty());
}

#[test]
fn clear_on_empty_queue_is_noop() {
    let q = Queue::new();
    q.clear();
    let snap = q.snapshot();
    assert!(snap.items.is_empty());
    assert_eq!(snap.cursor, None);
}
