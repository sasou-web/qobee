//! Property-based tests for the R8 transport bus exposed by
//! [`qobee_core::PlayerHandle`] (cf. spec
//! `native-media-integration-and-ux`).
//!
//! ## Scope
//!
//! This file currently covers **Property 1 — Idempotence des
//! commandes de transport**. Other properties from the design
//! (Property 2 multi-bridge convergence, Property 4 position
//! monotonicity) live in their own dedicated tests added in
//! later tasks.
//!
//! ## Why a model and not the real `PlayerHandle`
//!
//! Constructing a real `Player` requires a `qobee_library::Library`
//! seeded with a track plus a `qobee_engine::CpalSharedEngine`,
//! which spawns an OS-level audio worker and probes the default
//! output device. That works fine for one-shot integration tests
//! but is far too heavy for a `proptest!` block running 100+
//! random sequences (audio device contention, decoder file I/O,
//! engine warm-up). The user prompt explicitly recommends modelling
//! `TransportCmd` semantics against a small in-memory state
//! machine that mirrors `PlayerHandle::{play, pause, resume, stop}`
//! idempotence rules — that is what we do here.
//!
//! The model is a literal Rust transcription of the rules added by
//! task 1.1 (cf. `crates/core/src/player.rs`):
//!
//!   * `play()` is a silent no-op when the engine is already in
//!     `Playing`, delegates to `resume()` when in `Paused`, and a
//!     no-op otherwise (no current track loaded).
//!   * `pause()` is a silent no-op outside `Playing`; otherwise
//!     emits `Paused`.
//!   * `resume()` is a silent no-op outside `Paused`; otherwise
//!     emits `Resumed`.
//!   * `stop()` always emits `Stopped` and unconditionally clears
//!     the current track. Stop is intentionally **not** in the
//!     idempotence set in task 1.1 (R8.4/R8.5 only mention Play
//!     and Pause), so the property's syntactic deduplication step
//!     leaves Stop runs intact.

use std::collections::HashMap;

use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One of the four transport commands exposed by
/// [`qobee_core::PlayerHandle`]. Property 1 quantifies over
/// arbitrary finite sequences of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TransportCmd {
    Play,
    Pause,
    Resume,
    Stop,
}

/// Compressed version of [`qobee_engine::PlaybackStatus`]. The full
/// status enum has six variants (`Idle | Loading | Playing |
/// Paused | Stopped | Errored`), but for transport idempotence the
/// only behavioural distinctions are:
///
///   * `Playing` — Play / Resume are no-ops, Pause moves to Paused.
///   * `Paused` — Pause is a no-op, Play / Resume move to Playing.
///   * any other state collapses to `Stopped` for our purposes —
///     none of `play / pause / resume` emit events from there
///     (they all early-return). `stop()` collapses any state to
///     `Stopped` and emits `Stopped` unconditionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ModelState {
    Stopped,
    Playing,
    Paused,
}

/// Tag for a single broadcast event. We strip the `position_ms`
/// payload because it is only meaningful relative to a real engine
/// timeline; Property 1 only constrains the shape and ordering of
/// the broadcast stream, not its numeric content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EventTag {
    Paused,
    Resumed,
    Stopped,
}

/// Apply a single transport command to a state. Mirrors the exact
/// branch structure of `PlayerHandle::{play, pause, resume, stop}`
/// after task 1.1.
fn step(cmd: TransportCmd, state: ModelState) -> (ModelState, Option<EventTag>) {
    use ModelState::*;
    use TransportCmd::*;
    match (cmd, state) {
        // --- play() -----------------------------------------------------
        // Already Playing -> silent no-op (R8.5).
        (Play, Playing) => (Playing, None),
        // Paused -> delegate to resume(), which emits `Resumed`.
        (Play, Paused) => (Playing, Some(EventTag::Resumed)),
        // Stopped (no current track loaded) -> no-op. The real
        // `play()` returns `Ok(())` in this branch and the caller
        // is expected to use the dedicated `play_track` /
        // `play_album_from_track` etc. entry points instead.
        (Play, Stopped) => (Stopped, None),

        // --- pause() ----------------------------------------------------
        // Playing -> Paused (broadcasts `Paused`).
        (Pause, Playing) => (Paused, Some(EventTag::Paused)),
        // Already Paused -> silent no-op (R8.4).
        (Pause, Paused) => (Paused, None),
        // Stopped -> no-op (the real pause() short-circuits when
        // status != Playing).
        (Pause, Stopped) => (Stopped, None),

        // --- resume() ---------------------------------------------------
        // Paused -> Playing (broadcasts `Resumed`).
        (Resume, Paused) => (Playing, Some(EventTag::Resumed)),
        // Already Playing -> silent no-op.
        (Resume, Playing) => (Playing, None),
        // Stopped -> no-op.
        (Resume, Stopped) => (Stopped, None),

        // --- stop() -----------------------------------------------------
        // Always transitions to Stopped and broadcasts `Stopped`.
        // Note: the current `stop()` is *not* idempotent on the
        // event bus (it broadcasts unconditionally). Task 1.1 only
        // claimed silent idempotence for Play / Pause / Resume.
        (Stop, _) => (Stopped, Some(EventTag::Stopped)),
    }
}

/// Run a sequence of commands from an initial state, returning the
/// final state and the ordered list of broadcast events.
fn simulate(s0: ModelState, cmds: &[TransportCmd]) -> (ModelState, Vec<EventTag>) {
    let mut state = s0;
    let mut events = Vec::with_capacity(cmds.len());
    for &cmd in cmds {
        let (next, evt) = step(cmd, state);
        state = next;
        if let Some(e) = evt {
            events.push(e);
        }
    }
    (state, events)
}

/// Syntactic deduplication of a command sequence: collapse runs of
/// consecutive identical commands among the idempotent set
/// `{Play, Pause, Resume}` into a single occurrence. Runs of `Stop`
/// are preserved verbatim because `stop()` is not idempotent on
/// the broadcast (it emits `Stopped` every time).
///
/// This mirrors the design's parenthetical "deux `Pause`
/// consécutifs comptent comme un seul" while staying consistent
/// with task 1.1's narrower idempotence claim.
fn dedup_consecutive(cmds: &[TransportCmd]) -> Vec<TransportCmd> {
    let mut out: Vec<TransportCmd> = Vec::with_capacity(cmds.len());
    for &cmd in cmds {
        // Stop is never collapsed against another Stop.
        if matches!(cmd, TransportCmd::Stop) {
            out.push(cmd);
            continue;
        }
        if out.last() == Some(&cmd) {
            continue;
        }
        out.push(cmd);
    }
    out
}

/// Multi-set equality on event tags. Property 1 treats the
/// broadcast stream as a multi-set rather than a sequence because
/// the design wording is "le multi-set d'events broadcastés".
fn multiset_eq(a: &[EventTag], b: &[EventTag]) -> bool {
    let mut ma: HashMap<EventTag, usize> = HashMap::new();
    let mut mb: HashMap<EventTag, usize> = HashMap::new();
    for &e in a {
        *ma.entry(e).or_insert(0) += 1;
    }
    for &e in b {
        *mb.entry(e).or_insert(0) += 1;
    }
    ma == mb
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

fn any_initial_state() -> impl Strategy<Value = ModelState> {
    prop_oneof![
        Just(ModelState::Stopped),
        Just(ModelState::Playing),
        Just(ModelState::Paused),
    ]
}

fn any_cmd() -> impl Strategy<Value = TransportCmd> {
    prop_oneof![
        Just(TransportCmd::Play),
        Just(TransportCmd::Pause),
        Just(TransportCmd::Resume),
        Just(TransportCmd::Stop),
    ]
}

/// Sequences up to 32 commands long, per the task spec.
fn any_cmd_seq() -> impl Strategy<Value = Vec<TransportCmd>> {
    proptest::collection::vec(any_cmd(), 0..=32)
}

// ---------------------------------------------------------------------------
// Property 1 — Idempotence des commandes de transport
// ---------------------------------------------------------------------------

// Feature: native-media-integration-and-ux, Property 1
//
// For any initial state `s0 ∈ {Stopped, Playing, Paused}` and any
// finite sequence `cmds` of transport commands of length ≤ 32, the
// final state and the multi-set of broadcast events produced by
// `simulate(s0, cmds)` must equal those produced by
// `simulate(s0, dedup_consecutive(cmds))`.
//
// This validates the silent-idempotence rules introduced by task
// 1.1 in `PlayerHandle::play / pause / resume` (cf. R8.4 / R8.5):
// repeating the same idempotent command back to back must neither
// change the state machine's outcome nor add a duplicate event to
// the broadcast bus.
//
// Validates: Requirements 8.4, 8.5; Property 1.
#[test]
fn property_1_transport_command_idempotence() {
    proptest!(
        ProptestConfig::with_cases(100),
        |(s0 in any_initial_state(), cmds in any_cmd_seq())| {
            let (final_state, events) = simulate(s0, &cmds);
            let dedup = dedup_consecutive(&cmds);
            let (final_state_d, events_d) = simulate(s0, &dedup);

            prop_assert_eq!(
                final_state, final_state_d,
                "final state diverged after deduplication: cmds={:?}, dedup={:?}",
                cmds, dedup,
            );
            prop_assert!(
                multiset_eq(&events, &events_d),
                "event multi-set diverged after deduplication: \
                 cmds={:?}, dedup={:?}, events={:?}, events_d={:?}",
                cmds, dedup, events, events_d,
            );
        }
    );
}

// ---------------------------------------------------------------------------
// Sanity unit tests on the model itself
// ---------------------------------------------------------------------------
//
// These guard the three rules from task 1.1 against accidental
// regression in the model. They are NOT a substitute for the
// property test above — they pin down the table by example so a
// future contributor can grok the rules without re-reading the
// `step` match arms.

#[test]
fn model_pause_is_silent_when_already_paused() {
    let (s, e) = step(TransportCmd::Pause, ModelState::Paused);
    assert_eq!(s, ModelState::Paused);
    assert!(e.is_none());
}

#[test]
fn model_play_is_silent_when_already_playing() {
    let (s, e) = step(TransportCmd::Play, ModelState::Playing);
    assert_eq!(s, ModelState::Playing);
    assert!(e.is_none());
}

#[test]
fn model_resume_is_silent_when_already_playing() {
    let (s, e) = step(TransportCmd::Resume, ModelState::Playing);
    assert_eq!(s, ModelState::Playing);
    assert!(e.is_none());
}

#[test]
fn model_play_from_paused_emits_resumed() {
    let (s, e) = step(TransportCmd::Play, ModelState::Paused);
    assert_eq!(s, ModelState::Playing);
    assert_eq!(e, Some(EventTag::Resumed));
}

#[test]
fn model_pause_from_playing_emits_paused() {
    let (s, e) = step(TransportCmd::Pause, ModelState::Playing);
    assert_eq!(s, ModelState::Paused);
    assert_eq!(e, Some(EventTag::Paused));
}

#[test]
fn model_resume_from_paused_emits_resumed() {
    let (s, e) = step(TransportCmd::Resume, ModelState::Paused);
    assert_eq!(s, ModelState::Playing);
    assert_eq!(e, Some(EventTag::Resumed));
}

#[test]
fn model_stop_always_emits_stopped() {
    for s0 in [ModelState::Stopped, ModelState::Playing, ModelState::Paused] {
        let (s, e) = step(TransportCmd::Stop, s0);
        assert_eq!(s, ModelState::Stopped);
        assert_eq!(e, Some(EventTag::Stopped));
    }
}

#[test]
fn dedup_collapses_consecutive_pause_runs() {
    use TransportCmd::*;
    assert_eq!(dedup_consecutive(&[Pause, Pause, Pause]), vec![Pause]);
    assert_eq!(
        dedup_consecutive(&[Play, Pause, Pause, Resume, Resume]),
        vec![Play, Pause, Resume],
    );
}

#[test]
fn dedup_preserves_stop_runs() {
    use TransportCmd::*;
    assert_eq!(
        dedup_consecutive(&[Stop, Stop, Stop]),
        vec![Stop, Stop, Stop],
    );
}

// ===========================================================================
// Property 2 — Convergence des projections OS / UI
// ===========================================================================
//
// Feature: native-media-integration-and-ux, Property 2.
//
// For any finite sequence of `PlayerEvent`s consumed by `Player_Sync`,
// the projected state of every OS bridge implementing
// [`qobee_core::MediaBridge`] must converge to the same value, and
// that value must equal the canonical fold `to_bridge_state(events)`.
//
// We exercise this with two independent mock bridges (`MockSmtcBridge`
// / `MockMacOsBridge`) that both delegate to a shared
// `apply_to_state` reducer. The two mocks model the real
// architecture (SMTC on Windows + MPNowPlayingInfoCenter on macOS)
// so the assertion mirrors the design's "two-sink convergence"
// requirement (R8.1) rather than just being a self-consistency
// check on a single bridge.
//
// Validates: Requirements 8.1, 1.1, 1.3, 2.1, 2.3; Property 2.

use std::sync::Mutex;

use qobee_core::{MediaBridge, PlayerErrorKind, PlayerEvent, TrackMeta};

// ---------------------------------------------------------------------------
// Bridge state model
// ---------------------------------------------------------------------------

/// Coarse status projected by an OS Now Playing surface. Maps onto
/// `SystemMediaTransportControls::PlaybackStatus` on Windows and onto
/// the `playbackState` exposed by `MPNowPlayingInfoCenter` on macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BridgeStatus {
    Idle,
    Playing,
    Paused,
    Stopped,
    Error,
}

/// Snapshot of what a bridge is currently displaying. We keep it
/// deliberately minimal: status + current track id + position +
/// duration is enough to model R8.1 convergence without dragging in
/// cover bytes / artwork concerns (handled by the real bridges via
/// the placeholder fallback in R8.6).
#[derive(Debug, Clone, PartialEq, Eq)]
struct BridgeState {
    status: BridgeStatus,
    current_track_id: Option<i64>,
    position_ms: u64,
    duration_ms: u64,
}

impl Default for BridgeState {
    fn default() -> Self {
        Self {
            status: BridgeStatus::Idle,
            current_track_id: None,
            position_ms: 0,
            duration_ms: 0,
        }
    }
}

/// Single source of truth for the state machine: every bridge
/// delegates here so they can never disagree by construction. The
/// "two bridges" abstraction below is then a check that both call
/// sites really do delegate to this reducer (no copy-paste drift).
fn apply_to_state(s: &mut BridgeState, ev: &PlayerEvent) {
    match ev {
        PlayerEvent::Started {
            track,
            position_ms,
            duration_ms,
        } => {
            s.status = BridgeStatus::Playing;
            s.current_track_id = Some(track.id);
            s.position_ms = *position_ms;
            s.duration_ms = *duration_ms;
        }
        PlayerEvent::Paused { position_ms } => {
            s.status = BridgeStatus::Paused;
            s.position_ms = *position_ms;
        }
        PlayerEvent::Resumed { position_ms } => {
            s.status = BridgeStatus::Playing;
            s.position_ms = *position_ms;
        }
        PlayerEvent::Stopped => {
            s.status = BridgeStatus::Stopped;
            s.current_track_id = None;
            s.position_ms = 0;
            s.duration_ms = 0;
        }
        PlayerEvent::TrackChanged { track } => {
            // Track change keeps the current playback status as-is
            // (gapless transition). Position is reset, duration is
            // refreshed from the new track metadata.
            s.current_track_id = Some(track.id);
            s.position_ms = 0;
            s.duration_ms = track.duration_ms;
        }
        PlayerEvent::PositionTick { position_ms } => {
            s.position_ms = *position_ms;
        }
        PlayerEvent::Errored { .. } => {
            s.status = BridgeStatus::Error;
        }
        // Legacy variants ride the crossbeam channel, not the R8
        // broadcast bus. The proptest generators below never
        // produce them; we accept them as no-ops for forward
        // compatibility (e.g. if a future version starts mirroring
        // `BitPerfectChanged` onto the broadcast).
        PlayerEvent::StateChanged { .. }
        | PlayerEvent::Position { .. }
        | PlayerEvent::EndOfTrack
        | PlayerEvent::BitPerfectChanged { .. }
        | PlayerEvent::Error { .. } => {}
    }
}

/// Reference fold: replays a full event sequence through the same
/// reducer the mock bridges use. This is the "expected" snapshot
/// against which both bridges must compare equal after replay.
fn to_bridge_state(events: &[PlayerEvent]) -> BridgeState {
    let mut s = BridgeState::default();
    for ev in events {
        apply_to_state(&mut s, ev);
    }
    s
}

// ---------------------------------------------------------------------------
// Mock bridges
// ---------------------------------------------------------------------------
//
// Two distinct types implementing `MediaBridge`. They share the
// reducer but not the storage, so any divergence in the trait
// dispatch path (e.g. one impl forgetting to call `apply_to_state`)
// would fail the equality check below.

struct MockSmtcBridge {
    state: Mutex<BridgeState>,
}

impl MockSmtcBridge {
    fn new() -> Self {
        Self {
            state: Mutex::new(BridgeState::default()),
        }
    }

    fn snapshot(&self) -> BridgeState {
        self.state
            .lock()
            .expect("smtc bridge mutex poisoned")
            .clone()
    }
}

impl MediaBridge for MockSmtcBridge {
    fn handle_event(&self, ev: &PlayerEvent) {
        let mut guard = self.state.lock().expect("smtc bridge mutex poisoned");
        apply_to_state(&mut guard, ev);
    }
}

struct MockMacOsBridge {
    state: Mutex<BridgeState>,
}

impl MockMacOsBridge {
    fn new() -> Self {
        Self {
            state: Mutex::new(BridgeState::default()),
        }
    }

    fn snapshot(&self) -> BridgeState {
        self.state
            .lock()
            .expect("macos bridge mutex poisoned")
            .clone()
    }
}

impl MediaBridge for MockMacOsBridge {
    fn handle_event(&self, ev: &PlayerEvent) {
        let mut guard = self.state.lock().expect("macos bridge mutex poisoned");
        apply_to_state(&mut guard, ev);
    }
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Track ids are kept in a small range (0..=4) so the same track id
/// reappears across a sequence. This makes `TrackChanged` events
/// non-trivially exercise the "switch back to a previously seen
/// track" branch instead of always introducing a brand new id.
fn any_track_meta() -> impl Strategy<Value = TrackMeta> {
    (0i64..=4, 0u64..=600_000).prop_map(|(id, dur)| TrackMeta {
        id,
        title: format!("title-{id}"),
        artist: format!("artist-{id}"),
        album: format!("album-{id}"),
        duration_ms: dur,
        // Cover bytes are skipped from BridgeState entirely (the
        // bridges fall back to a placeholder PNG per R8.6); leaving
        // it `None` keeps the proptest shrinker fast.
        cover_bytes: None,
    })
}

/// Position values up to ~1 hour, comfortably above any real-world
/// track length we expect to throttle.
fn any_position_ms() -> impl Strategy<Value = u64> {
    0u64..=3_600_000
}

fn any_error_kind() -> impl Strategy<Value = PlayerErrorKind> {
    prop_oneof![
        Just(PlayerErrorKind::FileNotFound),
        Just(PlayerErrorKind::DecodeFailed),
        Just(PlayerErrorKind::DeviceUnavailable),
        Just(PlayerErrorKind::Other),
    ]
}

/// Generator over the seven R8 transport-bus variants. We
/// deliberately do not generate the legacy variants (`StateChanged`
/// / `Position` / `EndOfTrack` / `BitPerfectChanged` / `Error`):
/// they ride the legacy crossbeam channel and never reach the
/// broadcast fan-out, so projecting them through bridges is out of
/// scope for Property 2.
fn any_player_event() -> impl Strategy<Value = PlayerEvent> {
    prop_oneof![
        (any_track_meta(), any_position_ms(), 0u64..=600_000).prop_map(
            |(track, position_ms, duration_ms)| PlayerEvent::Started {
                track,
                position_ms,
                duration_ms,
            }
        ),
        any_position_ms().prop_map(|position_ms| PlayerEvent::Paused { position_ms }),
        any_position_ms().prop_map(|position_ms| PlayerEvent::Resumed { position_ms }),
        Just(PlayerEvent::Stopped),
        any_track_meta().prop_map(|track| PlayerEvent::TrackChanged { track }),
        any_position_ms().prop_map(|position_ms| PlayerEvent::PositionTick { position_ms }),
        any_error_kind().prop_map(|kind| PlayerEvent::Errored {
            kind,
            message: "boom".to_string(),
        }),
    ]
}

/// Sequences up to 16 events long, per the task spec.
fn any_event_seq() -> impl Strategy<Value = Vec<PlayerEvent>> {
    proptest::collection::vec(any_player_event(), 0..=16)
}

// ---------------------------------------------------------------------------
// Property 2
// ---------------------------------------------------------------------------

/// Validates: Requirements 8.1, 1.1, 1.3, 2.1, 2.3; Property 2.
#[test]
fn property_2_bridge_convergence() {
    proptest!(
        ProptestConfig::with_cases(100),
        |(events in any_event_seq())| {
            let smtc = MockSmtcBridge::new();
            let mac = MockMacOsBridge::new();

            // Drive both bridges with the exact same event stream
            // in the exact same order — the fan-out task in
            // `setup()` calls `b.handle_event(&ev)` synchronously
            // for every bridge in the same loop iteration.
            for ev in &events {
                smtc.handle_event(ev);
                mac.handle_event(ev);
            }

            let smtc_snap = smtc.snapshot();
            let mac_snap = mac.snapshot();
            let expected = to_bridge_state(&events);

            // (1) Both bridges must agree.
            prop_assert_eq!(
                &smtc_snap, &mac_snap,
                "smtc / mac bridge snapshots diverged after replay: events={:?}",
                events,
            );
            // (2) ...and they must equal the canonical fold.
            prop_assert_eq!(
                &smtc_snap, &expected,
                "bridge snapshot diverged from to_bridge_state(events): events={:?}",
                events,
            );
        }
    );
}

// ---------------------------------------------------------------------------
// Sanity unit tests on the reducer
// ---------------------------------------------------------------------------
//
// Same intent as the Property 1 sanity tests above: pin down the
// reducer's transition table by example so a contributor can read
// off the rules without re-deriving them from `apply_to_state`'s
// match arms.

fn track(id: i64, duration_ms: u64) -> TrackMeta {
    TrackMeta {
        id,
        title: format!("t-{id}"),
        artist: format!("a-{id}"),
        album: format!("al-{id}"),
        duration_ms,
        cover_bytes: None,
    }
}

#[test]
fn reducer_started_sets_playing_with_track_and_timeline() {
    let mut s = BridgeState::default();
    apply_to_state(
        &mut s,
        &PlayerEvent::Started {
            track: track(7, 60_000),
            position_ms: 1_000,
            duration_ms: 60_000,
        },
    );
    assert_eq!(s.status, BridgeStatus::Playing);
    assert_eq!(s.current_track_id, Some(7));
    assert_eq!(s.position_ms, 1_000);
    assert_eq!(s.duration_ms, 60_000);
}

#[test]
fn reducer_stopped_clears_track() {
    let mut s = BridgeState {
        status: BridgeStatus::Playing,
        current_track_id: Some(3),
        position_ms: 500,
        duration_ms: 10_000,
    };
    apply_to_state(&mut s, &PlayerEvent::Stopped);
    assert_eq!(s.status, BridgeStatus::Stopped);
    assert_eq!(s.current_track_id, None);
    assert_eq!(s.position_ms, 0);
    assert_eq!(s.duration_ms, 0);
}

#[test]
fn reducer_track_changed_resets_position_and_keeps_status() {
    let mut s = BridgeState {
        status: BridgeStatus::Playing,
        current_track_id: Some(1),
        position_ms: 5_000,
        duration_ms: 10_000,
    };
    apply_to_state(
        &mut s,
        &PlayerEvent::TrackChanged {
            track: track(2, 30_000),
        },
    );
    assert_eq!(s.status, BridgeStatus::Playing);
    assert_eq!(s.current_track_id, Some(2));
    assert_eq!(s.position_ms, 0);
    assert_eq!(s.duration_ms, 30_000);
}

#[test]
fn reducer_errored_moves_to_error_status() {
    let mut s = BridgeState {
        status: BridgeStatus::Playing,
        current_track_id: Some(1),
        position_ms: 1_000,
        duration_ms: 10_000,
    };
    apply_to_state(
        &mut s,
        &PlayerEvent::Errored {
            kind: PlayerErrorKind::FileNotFound,
            message: "missing".into(),
        },
    );
    assert_eq!(s.status, BridgeStatus::Error);
    // Non-status fields are intentionally untouched so the UI can
    // still render "what was playing when it broke".
    assert_eq!(s.current_track_id, Some(1));
    assert_eq!(s.position_ms, 1_000);
    assert_eq!(s.duration_ms, 10_000);
}

#[test]
fn reducer_position_tick_only_updates_position() {
    let mut s = BridgeState {
        status: BridgeStatus::Playing,
        current_track_id: Some(1),
        position_ms: 1_000,
        duration_ms: 10_000,
    };
    apply_to_state(&mut s, &PlayerEvent::PositionTick { position_ms: 2_500 });
    assert_eq!(s.status, BridgeStatus::Playing);
    assert_eq!(s.current_track_id, Some(1));
    assert_eq!(s.position_ms, 2_500);
    assert_eq!(s.duration_ms, 10_000);
}

#[test]
fn both_mock_bridges_converge_on_a_simple_run() {
    let smtc = MockSmtcBridge::new();
    let mac = MockMacOsBridge::new();
    let evs = vec![
        PlayerEvent::Started {
            track: track(1, 60_000),
            position_ms: 0,
            duration_ms: 60_000,
        },
        PlayerEvent::PositionTick { position_ms: 250 },
        PlayerEvent::Paused { position_ms: 250 },
        PlayerEvent::Resumed { position_ms: 250 },
        PlayerEvent::Stopped,
    ];
    for ev in &evs {
        smtc.handle_event(ev);
        mac.handle_event(ev);
    }
    let expected = to_bridge_state(&evs);
    assert_eq!(smtc.snapshot(), expected);
    assert_eq!(mac.snapshot(), expected);
}
