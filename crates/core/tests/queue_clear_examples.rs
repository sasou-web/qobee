//! Targeted example tests for the queue-clear path backing the
//! `clear_queue` command (cf. spec `qobee-beta-feedback-improvements`,
//! task 7.3, Requirement 3.7).
//!
//! ## Scope and placement
//!
//! `PlayerHandle::clear_queue` delegates verbatim to
//! [`qobee_core::Queue::clear`] (`self.inner.queue.clear(); Ok(())`),
//! so the behaviour worth pinning by example lives on the `Queue`.
//! Constructing a real `Player` to drive `clear_queue` is impractical
//! in a unit/integration test: `Player::new` builds a
//! `qobee_engine::CpalSharedEngine`, which spawns an OS audio worker
//! and probes the default output device (the same reason
//! `player_sync_properties.rs` models the transport instead of
//! building a real player). These examples therefore exercise the
//! genuine `Queue` directly.
//!
//! ## Relationship to `queue_properties.rs` (task 7.2)
//!
//! The property test and its sanity units already cover the two
//! literal cases named in task 7.3 — "file non vide → vide" and "file
//! déjà vide → no-op" — asserting `items`/`cursor`/`len`/`is_empty`.
//! To avoid duplicating those identical assertions, the examples here
//! target the *distinct* facets of `clear` that those tests do not:
//!
//!   * the design's decoupling note — `clear` purges the "up next"
//!     list and the cursor, so the read-side accessors `current()` and
//!     `peek_next()` both collapse to `None` (R3.7);
//!   * `clear` also resets the hidden `original`-order snapshot, so a
//!     later shuffle toggle cannot resurrect cleared tracks;
//!   * `clear` is idempotent once the queue is empty, even after the
//!     queue had previously held tracks.

use qobee_core::queue::{Queue, RepeatMode};

/// Non-empty → empty: clearing a populated queue with an active cursor
/// empties the "up next" list *and* decouples the read side. Beyond the
/// `items`/`cursor` assertions already in `queue_properties.rs`, this
/// pins the `current()` / `peek_next()` accessors to `None` (R3.7: the
/// engine's playing track is not the queue's concern — clear only
/// purges the pending list and the cursor).
#[test]
fn clear_on_populated_queue_empties_and_decouples_read_side() {
    let q = Queue::new();
    q.replace(vec![10, 20, 30, 40], Some(1)); // cursor on id 20
    assert_eq!(q.current(), Some(20));
    assert_eq!(q.peek_next(), Some(30));

    q.clear();

    assert!(q.is_empty());
    assert_eq!(q.len(), 0);
    assert_eq!(q.snapshot().cursor, None);
    // Read-side accessors collapse: nothing current, nothing up next.
    assert_eq!(q.current(), None, "clear must leave no current track");
    assert_eq!(q.peek_next(), None, "clear must leave no next track");
}

/// `clear` resets the hidden `original` snapshot, not just the live
/// `items`. We prove it by repopulating after a clear and toggling
/// shuffle off: the restored order must be the *new* tracks only — if
/// `clear` had left stale `original` data behind, the old ids would
/// reappear when shuffle is switched off.
#[test]
fn clear_purges_original_order_snapshot() {
    let q = Queue::new();
    q.replace(vec![1, 2, 3, 4, 5], Some(0));
    q.clear();

    // Fresh contents after the clear.
    q.append(vec![7, 8, 9]);
    assert_eq!(q.snapshot().items, vec![7, 8, 9]);

    // A shuffle round-trip restores `original`. It must restore the
    // post-clear contents, never the pre-clear ones.
    q.set_shuffle(true);
    q.set_shuffle(false);
    let restored = q.snapshot().items;
    let mut sorted = restored.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        vec![7, 8, 9],
        "shuffle off must restore only post-clear tracks (original snapshot was reset): {restored:?}"
    );
}

/// Already empty → no-op, and idempotent: clearing a queue that was
/// populated then cleared once leaves it empty on the second clear too.
/// Distinct from `queue_properties.rs::clear_on_empty_queue_is_noop`,
/// which clears a never-populated queue exactly once.
#[test]
fn clear_is_idempotent_once_empty() {
    let q = Queue::new();
    q.replace(vec![1, 2, 3], Some(2));
    q.set_repeat(RepeatMode::Queue);

    q.clear();
    assert!(q.is_empty());
    assert_eq!(q.snapshot().cursor, None);

    // Second clear on the now-empty queue is a no-op.
    q.clear();
    let snap = q.snapshot();
    assert!(
        snap.items.is_empty(),
        "second clear must keep the queue empty"
    );
    assert_eq!(snap.cursor, None, "second clear must keep the cursor None");
    // Unrelated state (repeat mode) is untouched by clear.
    assert_eq!(q.repeat(), RepeatMode::Queue);
}
