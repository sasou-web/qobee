//! Simple in-memory playback queue.
//!
//! No shuffle, no repeat in the MVP — just a linear list with a cursor.
//! The shape is intentionally minimal so the orchestration layer can be
//! tested without dragging in the audio engine; richer features
//! (shuffle, repeat, history) live in a follow-up.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Numeric track identifier as stored in the SQLite library
/// (`tracks.id`).
pub type TrackId = i64;

/// Read-only view of the queue's state. The cursor index points into
/// `items` and may be `None` when the queue is empty or stopped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueSnapshot {
    pub items: Vec<TrackId>,
    pub cursor: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    /// No repeat. End of queue stops playback (or triggers
    /// auto-shuffle, depending on Player config).
    Off,
    /// Loop on the current track forever.
    Track,
    /// Loop the entire queue.
    Queue,
}

impl Default for RepeatMode {
    fn default() -> Self {
        RepeatMode::Off
    }
}

#[derive(Default)]
pub struct Queue {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    items: Vec<TrackId>,
    cursor: Option<usize>,
    /// Snapshot of the original order. When shuffle is on we shuffle
    /// `items` and keep the original here so toggling shuffle off
    /// restores it.
    original: Vec<TrackId>,
    shuffle: bool,
    repeat: RepeatMode,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the queue with a single track and position the cursor on
    /// it.
    pub fn replace_with_track(&self, track_id: TrackId) {
        let mut g = self.inner.lock();
        g.items = vec![track_id];
        g.original = vec![track_id];
        g.cursor = Some(0);
    }

    /// Replace the queue with `tracks`, optionally starting at the index
    /// of `start`. Honors the current shuffle mode.
    pub fn replace(&self, tracks: Vec<TrackId>, start: Option<usize>) {
        let mut g = self.inner.lock();
        let cursor = if tracks.is_empty() {
            None
        } else {
            Some(start.unwrap_or(0).min(tracks.len() - 1))
        };
        g.original = tracks.clone();
        if g.shuffle && tracks.len() > 1 {
            // Shuffle but keep the explicitly-started track at index 0
            // so the listener actually starts on what they clicked.
            let head = cursor
                .and_then(|i| tracks.get(i).copied())
                .unwrap_or_else(|| tracks[0]);
            let mut rest: Vec<TrackId> =
                tracks.iter().copied().filter(|id| *id != head).collect();
            shuffle_in_place(&mut rest);
            let mut shuffled = Vec::with_capacity(tracks.len());
            shuffled.push(head);
            shuffled.extend(rest);
            g.items = shuffled;
            g.cursor = Some(0);
        } else {
            g.items = tracks;
            g.cursor = cursor;
        }
    }

    /// Insert `tracks` immediately after the current cursor. They will
    /// play next, before the rest of the queue. If the queue is empty
    /// the inserted tracks become the queue and the cursor sits before
    /// the first one (caller is responsible for advancing if they want
    /// playback to start immediately).
    pub fn play_next(&self, tracks: Vec<TrackId>) {
        if tracks.is_empty() {
            return;
        }
        let mut g = self.inner.lock();
        let insert_at = match g.cursor {
            Some(i) => i + 1,
            None => g.items.len(),
        };
        // Splice into the live queue and the snapshot so toggling
        // shuffle later still keeps the inserted tracks.
        for (offset, id) in tracks.iter().copied().enumerate() {
            g.items.insert(insert_at + offset, id);
        }
        // Mirror the insert into `original` so it survives shuffle.
        let original_anchor = g
            .cursor
            .and_then(|i| g.items.get(i).copied())
            .and_then(|id| g.original.iter().position(|x| *x == id));
        let orig_at = match original_anchor {
            Some(i) => i + 1,
            None => g.original.len(),
        };
        for (offset, id) in tracks.iter().copied().enumerate() {
            g.original.insert(orig_at + offset, id);
        }
    }

    /// Append `tracks` at the end of the queue. Used by "Add to queue".
    pub fn append(&self, tracks: Vec<TrackId>) {
        if tracks.is_empty() {
            return;
        }
        let mut g = self.inner.lock();
        g.items.extend(tracks.iter().copied());
        g.original.extend(tracks);
    }

    pub fn current(&self) -> Option<TrackId> {
        let g = self.inner.lock();
        g.cursor.and_then(|i| g.items.get(i).copied())
    }

    /// Snapshot of the queue's current state (live order, cursor
    /// position). Callers get a clone so they can render without
    /// holding the lock; mutations from another thread are not
    /// reflected until the next call.
    pub fn snapshot(&self) -> QueueSnapshot {
        let g = self.inner.lock();
        QueueSnapshot {
            items: g.items.clone(),
            cursor: g.cursor,
        }
    }

    /// Remove the track at index `idx`. Returns `true` if a track was
    /// removed. The cursor is shifted left when the removed item sat
    /// before it; if the cursor itself was removed, it stays on the
    /// same index (which is now the *next* track, naturally).
    pub fn remove_at(&self, idx: usize) -> bool {
        let mut g = self.inner.lock();
        if idx >= g.items.len() {
            return false;
        }
        let removed_id = g.items.remove(idx);
        // Mirror in `original` so toggling shuffle doesn't bring it
        // back. We remove the *first* occurrence — duplicates can
        // exist if the user added the same track twice.
        if let Some(pos) = g.original.iter().position(|x| *x == removed_id) {
            g.original.remove(pos);
        }
        match g.cursor {
            Some(c) if c > idx => g.cursor = Some(c - 1),
            Some(c) if c == idx => {
                if g.items.is_empty() {
                    g.cursor = None;
                } else if c >= g.items.len() {
                    g.cursor = Some(g.items.len() - 1);
                }
            }
            _ => {}
        }
        true
    }

    /// Move the track at `from` to position `to`. No-op when either
    /// index is out of range or both are equal.
    pub fn move_item(&self, from: usize, to: usize) -> bool {
        let mut g = self.inner.lock();
        if from == to || from >= g.items.len() || to >= g.items.len() {
            return false;
        }
        let v = g.items.remove(from);
        g.items.insert(to, v);

        // Update cursor: easiest to recompute by tracking which id
        // the cursor points at and finding it again.
        let cur_id = g.cursor.and_then(|c| {
            if c == from {
                Some(v)
            } else {
                g.items.get(c).copied()
            }
        });
        if let Some(id) = cur_id {
            g.cursor = g.items.iter().position(|x| *x == id);
        }
        true
    }

    /// Look up the *next* track id without moving the cursor. Honors
    /// the current repeat mode.
    pub fn peek_next(&self) -> Option<TrackId> {
        let g = self.inner.lock();
        match (g.repeat, g.cursor) {
            (RepeatMode::Track, Some(i)) => g.items.get(i).copied(),
            (_, Some(i)) if i + 1 < g.items.len() => g.items.get(i + 1).copied(),
            (RepeatMode::Queue, Some(_)) if !g.items.is_empty() => g.items.first().copied(),
            _ => None,
        }
    }

    /// Advance the cursor by one. Returns the new current track. With
    /// `RepeatMode::Track` the cursor stays put. With
    /// `RepeatMode::Queue` it wraps to the start. With `Off` it returns
    /// `None` when we walk past the end.
    pub fn advance(&self) -> Option<TrackId> {
        let mut g = self.inner.lock();
        match (g.repeat, g.cursor) {
            (RepeatMode::Track, Some(i)) => g.items.get(i).copied(),
            (_mode, Some(i)) if i + 1 < g.items.len() => {
                g.cursor = Some(i + 1);
                Some(g.items[i + 1])
            }
            (RepeatMode::Queue, Some(_)) if !g.items.is_empty() => {
                g.cursor = Some(0);
                Some(g.items[0])
            }
            _ => {
                g.cursor = None;
                None
            }
        }
    }

    /// Step the cursor back by one. Returns the new current track, or
    /// `None` if already at (or before) the start.
    pub fn previous(&self) -> Option<TrackId> {
        let mut g = self.inner.lock();
        match g.cursor {
            Some(i) if i > 0 => {
                g.cursor = Some(i - 1);
                Some(g.items[i - 1])
            }
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().items.is_empty()
    }

    pub fn repeat(&self) -> RepeatMode {
        self.inner.lock().repeat
    }

    pub fn set_repeat(&self, mode: RepeatMode) {
        self.inner.lock().repeat = mode;
    }

    pub fn shuffle(&self) -> bool {
        self.inner.lock().shuffle
    }

    /// Toggle shuffle on or off. Switching on permutes the queue (the
    /// current track stays current). Switching off restores the
    /// original order around the current track.
    pub fn set_shuffle(&self, on: bool) {
        let mut g = self.inner.lock();
        if g.shuffle == on || g.items.is_empty() {
            g.shuffle = on;
            return;
        }
        let cur_id = g.cursor.and_then(|i| g.items.get(i).copied());
        if on {
            // Save original (already saved when replace() was called)
            // and reshuffle the tail around the current track.
            let head = cur_id.unwrap_or_else(|| g.items[0]);
            let mut rest: Vec<TrackId> =
                g.items.iter().copied().filter(|id| *id != head).collect();
            shuffle_in_place(&mut rest);
            let mut shuffled = Vec::with_capacity(g.items.len());
            shuffled.push(head);
            shuffled.extend(rest);
            g.items = shuffled;
            g.cursor = Some(0);
        } else {
            g.items = g.original.clone();
            g.cursor = cur_id.and_then(|id| g.items.iter().position(|x| *x == id));
        }
        g.shuffle = on;
    }
}

/// Tiny in-process shuffle. Avoids pulling the `rand` crate just for
/// this; we use the OS clock as a seed for a Lehmer LCG. Quality is
/// fine for "rearrange a track list".
fn shuffle_in_place<T>(v: &mut [T]) {
    use std::time::SystemTime;
    let mut state: u64 = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| (d.as_nanos() as u64) | 1)
        .unwrap_or(0x9E3779B97F4A7C15);
    let n = v.len();
    if n < 2 {
        return;
    }
    for i in (1..n).rev() {
        // xorshift*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let r = state.wrapping_mul(0x2545F4914F6CDD1D);
        let j = (r % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}
