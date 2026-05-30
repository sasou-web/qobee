//! Discord Rich Presence integration.
//!
//! Architecture:
//!
//! - One **singleton** [`DiscordPresence`] is constructed at app
//!   startup and lives in [`AppState`](crate::state::AppState).
//! - The Discord IPC client is **never** touched on the Tauri / UI
//!   thread. All work runs on a dedicated worker thread that owns the
//!   socket and consumes commands via a `crossbeam-channel`.
//! - The worker handles **automatic reconnection** with exponential
//!   backoff (up to 30s) so that closing / restarting Discord at any
//!   time recovers silently.
//! - Updates are **coalesced**: if multiple `update_track` /
//!   `set_paused` calls land before the previous IPC write finishes,
//!   only the latest desired state is pushed. Discord's activity
//!   endpoint is rate-limited (≈5 updates / 15s); a small min-interval
//!   smoothing makes sure rapid track changes don't trip it.
//! - Failures fall back **silently**. If Discord is closed, the
//!   crate is unavailable, or the IPC pipe drops, callers get no
//!   error: they just keep posting state, the worker keeps trying to
//!   reconnect.
//!
//! Public API mirrors what the frontend exposes:
//!
//! ```ignore
//! presence.init();                   // start the worker
//! presence.update_track(...);        // metadata changed
//! presence.set_paused(true);         // playing/paused toggled
//! presence.clear();                  // hide rich presence
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{bounded, select, tick, Receiver, Sender};
use discord_rich_presence::{
    activity::{Activity, ActivityType, Assets, Timestamps},
    DiscordIpc, DiscordIpcClient,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Default Discord application client id used by the public Qobee
/// presence. Users can still override it via:
/// - the `QOBEE_DISCORD_CLIENT_ID` environment variable, or
/// - the `integrations.discord_client_id` setting (Settings UI),
///
/// which take precedence over this default.
const DEFAULT_CLIENT_ID: &str = "1507871800871354478";

/// Smallest interval between two outbound IPC writes. Discord
/// rate-limits the activity endpoint at ~5/15s; staying well under
/// that prevents bursts when several events fire in the same tick.
const MIN_PUBLISH_INTERVAL: Duration = Duration::from_millis(1_500);

/// Backoff bounds for the reconnect loop.
const RECONNECT_MIN: Duration = Duration::from_millis(500);
const RECONNECT_MAX: Duration = Duration::from_secs(30);

/// Description of the currently playing track, sent to the worker.
///
/// Mirrors what the frontend posts via `update_track`: only the
/// fields actually shown by Discord are kept.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackPresence {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Optional remote URL (https) for the cover art. When `None`
    /// **and** a `cover_key` is provided, the worker will ask the
    /// cover host to upload the local cover and re-publish once a
    /// public URL is available.
    pub cover_url: Option<String>,
    /// Local cache key for the album cover (e.g. `aa/<hash>.jpg`).
    /// Resolved against the library's cover cache directory by the
    /// cover host worker.
    #[serde(default)]
    pub cover_key: Option<String>,
    /// Total track length, in seconds. Used to compute the `end`
    /// timestamp so Discord renders the elapsed/remaining bar.
    pub duration_seconds: f64,
    /// Position when the update was generated, in seconds. Combined
    /// with `instant_at_unix_ms` to keep the timeline aligned even
    /// when the message is processed slightly later.
    pub position_seconds: f64,
}

/// Commands consumed by the worker thread.
#[derive(Debug)]
enum Cmd {
    /// Start (or restart) presence: connect and publish the current
    /// state right away. Idempotent.
    Init,
    /// Replace the active Discord application id. Triggers a clean
    /// reconnect so the new id is used immediately.
    SetClientId(String),
    /// Replace the whole presence (track + paused state) atomically.
    Update {
        track: Option<TrackPresence>,
        paused: bool,
    },
    /// Flip the play/pause flag. The worker keeps the last known
    /// track and republishes with the new state.
    SetPaused(bool),
    /// Clear the activity. The worker stays connected.
    Clear,
    /// A cover upload finished. If the resolved key matches the
    /// active track, patch the cover URL into the desired state and
    /// republish on the next cycle.
    CoverResolved { cover_key: String, url: String },
    /// Drop the connection and exit the worker. Used on shutdown.
    Shutdown,
}

/// Connection state surfaced to the UI for diagnostics. Mirrors what
/// the user actually sees on their Discord profile.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscordStatus {
    /// `init()` hasn't been called yet, or the user disabled the
    /// integration.
    Disabled,
    /// No client id is configured — nothing can be sent to Discord.
    NoClientId,
    /// Trying to connect (Discord may be closed).
    Connecting,
    /// Connected and publishing.
    Connected,
}

/// Cheap-to-clone handle to the Discord presence worker.
///
/// Internally a single sender — every call is a non-blocking send on
/// a small bounded channel. If the channel is full (worker stuck) the
/// caller drops the message rather than waiting, so the UI is never
/// frozen by Discord.
#[derive(Clone)]
pub struct DiscordPresence {
    inner: Arc<Inner>,
    /// Worker-written, UI-read connection state. Stored here (not in
    /// `Inner`) so cloning the handle remains cheap.
    shared_status: Arc<Mutex<DiscordStatus>>,
    /// Cover hosting helper. Filled in via [`Self::attach_cover_host`]
    /// after construction so we can pass the discord sender into the
    /// host callback (chicken-and-egg with the Library).
    cover_host: Arc<Mutex<Option<crate::cover_host::CoverHost>>>,
}

struct Inner {
    tx: Sender<Cmd>,
    /// `true` when [`DiscordPresence::init`] has already been called.
    /// Prevents the worker from being kicked twice.
    started: Mutex<bool>,
}

impl DiscordPresence {
    /// Spawn the worker thread (lazy: it sits idle until [`Self::init`]).
    pub fn new() -> Self {
        let (tx, rx) = bounded::<Cmd>(64);
        let status = Arc::new(Mutex::new(DiscordStatus::Disabled));
        let status_for_worker = status.clone();

        std::thread::Builder::new()
            .name("qobee-discord-rpc".into())
            .spawn(move || worker_main(rx, status_for_worker))
            .expect("failed to spawn Discord presence worker");

        Self {
            inner: Arc::new(Inner {
                tx,
                started: Mutex::new(false),
            }),
            shared_status: status,
            cover_host: Arc::new(Mutex::new(None)),
        }
    }

    /// Attach the cover host. Called once at startup with a fresh
    /// [`qobee_library::Library`] handle. The host uploads local covers
    /// and pushes `Cmd::CoverResolved` back into the worker so Discord
    /// ends up displaying the actual album art.
    pub fn attach_cover_host(&self, library: qobee_library::Library) {
        let tx = self.inner.tx.clone();
        let host = crate::cover_host::CoverHost::new(library, move |uploaded| {
            // Best-effort: dropping the message is fine, the next
            // track change re-asks for the URL anyway.
            let _ = tx.try_send(Cmd::CoverResolved {
                cover_key: uploaded.cover_key,
                url: uploaded.url,
            });
        });
        *self.cover_host.lock() = Some(host);
    }

    /// Look up a cached hosted URL for `cover_key`, if we have one,
    /// and request an upload otherwise. Returns the URL when it is
    /// already known so the caller can publish immediately.
    fn ensure_cover(&self, cover_key: &str) -> Option<String> {
        let host = self.cover_host.lock();
        let host = host.as_ref()?;
        if let Some(url) = host.get_cached_via_settings(cover_key) {
            return Some(url);
        }
        host.request_upload(cover_key);
        None
    }

    /// Wake the worker so it tries to connect to Discord and publish
    /// the next state it receives. Safe to call multiple times.
    pub fn init(&self) {
        let mut started = self.inner.started.lock();
        if *started {
            return;
        }
        *started = true;
        self.send(Cmd::Init);
    }

    /// Replace the Discord application id used for the handshake.
    /// Called when the user pastes a new value in Settings.
    pub fn set_client_id(&self, id: String) {
        self.send(Cmd::SetClientId(id));
    }

    /// Snapshot the worker's current connection state. Returned to
    /// the UI so it can show "Connected" / "Connecting…" / "No
    /// client ID set" without round-tripping Discord itself.
    pub fn status(&self) -> DiscordStatus {
        *self.shared_status.lock()
    }

    /// Toggle whether the cover host is allowed to upload local
    /// album art to a public service. Off by default; the user must
    /// opt in via Settings after acknowledging the privacy notice.
    pub fn set_cover_upload_enabled(&self, on: bool) {
        if let Some(host) = self.cover_host.lock().as_ref() {
            host.set_enabled(on);
        }
    }

    /// Drop the cover host's in-memory URL cache so a purged link is
    /// not re-served for the rest of the session (R8.4). No-op when
    /// the host hasn't been attached yet. The persisted rows are
    /// deleted separately by the command via the Library.
    pub fn clear_uploaded_cover_links(&self) {
        if let Some(host) = self.cover_host.lock().as_ref() {
            host.clear_link_cache();
        }
    }

    /// Replace the whole rich-presence atomically. `track` of `None`
    /// hides the activity but keeps the connection.
    pub fn update_track(&self, track: Option<TrackPresence>, paused: bool) {
        // Resolve the cover synchronously if we already have a
        // hosted URL for this `cover_key`. Otherwise the cover host
        // is asked to upload, and the resolved URL flows back via
        // `Cmd::CoverResolved` for an in-place patch.
        let track = track.map(|mut t| {
            if t.cover_url.is_none() {
                if let Some(key) = t.cover_key.clone() {
                    if let Some(url) = self.ensure_cover(&key) {
                        t.cover_url = Some(url);
                    }
                }
            }
            t
        });
        self.send(Cmd::Update { track, paused });
    }

    /// Toggle play/paused without changing the track metadata.
    pub fn set_paused(&self, paused: bool) {
        self.send(Cmd::SetPaused(paused));
    }

    /// Hide the rich presence. The worker stays alive and keeps the
    /// connection so a later `update_track` shows up immediately.
    pub fn clear(&self) {
        self.send(Cmd::Clear);
    }

    /// Stop the worker and drop the connection. Called on app exit.
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        let _ = self.inner.tx.send(Cmd::Shutdown);
    }

    /// Non-blocking send. If the queue is full (Discord IPC stalled),
    /// the message is dropped silently — the next state update will
    /// supersede it anyway, so we'd rather drop than freeze the UI.
    fn send(&self, cmd: Cmd) {
        if self.inner.tx.try_send(cmd).is_err() {
            tracing::trace!(target: "qobee::discord", "presence queue saturated, dropping command");
        }
    }
}

impl Default for DiscordPresence {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

/// Latest desired state. The worker collapses bursts of incoming
/// messages into this value, then publishes once per cycle.
#[derive(Debug, Clone, Default)]
struct Desired {
    track: Option<TrackPresence>,
    paused: bool,
    /// `true` when the user wants the activity hidden (last cmd was
    /// `Clear` or there is no track yet). When `true`, we send
    /// `clear_activity`; otherwise we serialize the activity from
    /// `track` + `paused`.
    hidden: bool,
}

fn worker_main(rx: Receiver<Cmd>, status: Arc<Mutex<DiscordStatus>>) {
    let mut client_id = std::env::var("QOBEE_DISCORD_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string());

    // The crate doesn't validate ids, so we keep the client lazily:
    // an empty / invalid id should leave us in `NoClientId` instead
    // of silently looping on a doomed connect.
    let mut client: Option<DiscordIpcClient> = None;

    let mut desired = Desired {
        hidden: true,
        ..Default::default()
    };
    let mut connected = false;
    let mut next_reconnect_at: Option<Instant> = None;
    let mut backoff = RECONNECT_MIN;
    let mut last_publish: Option<Instant> = None;
    let mut active = false; // becomes true after the first Init

    // Last state that was actually published. Used to skip redundant
    // IPC writes (e.g. position-only updates on a track Discord
    // already shows the right way).
    let mut published: Option<PublishedKey> = None;

    let set_status = |s: DiscordStatus| {
        *status.lock() = s;
    };

    // Periodic tick: wakes the worker even with no incoming messages,
    // so reconnect attempts can fire on schedule.
    let timer = tick(Duration::from_millis(500));

    loop {
        select! {
            recv(rx) -> msg => {
                let Ok(msg) = msg else { break };
                match msg {
                    Cmd::Init => {
                        active = true;
                        next_reconnect_at = Some(Instant::now());
                    }
                    Cmd::SetClientId(new_id) => {
                        // Empty string from the UI = "use the
                        // default baked in", so the user clearing
                        // the field reverts to the public Qobee app
                        // rather than disabling the integration.
                        let new_id = new_id.trim().to_string();
                        let resolved = if new_id.is_empty() {
                            std::env::var("QOBEE_DISCORD_CLIENT_ID")
                                .ok()
                                .filter(|s| !s.trim().is_empty())
                                .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string())
                        } else {
                            new_id
                        };
                        if resolved == client_id {
                            continue;
                        }
                        client_id = resolved;
                        // Force a clean reconnect with the new id.
                        if let Some(mut c) = client.take() {
                            let _ = c.close();
                        }
                        connected = false;
                        published = None;
                        backoff = RECONNECT_MIN;
                        if active {
                            next_reconnect_at = Some(Instant::now());
                        }
                    }
                    Cmd::Update { track, paused } => {
                        let hidden = track.is_none() || paused;
                        desired = Desired { track, paused, hidden };
                    }
                    Cmd::SetPaused(p) => {
                        desired.paused = p;
                        // Pause = hide entirely. Resume = show the
                        // last-known track again (still in `desired.track`).
                        desired.hidden = p || desired.track.is_none();
                    }
                    Cmd::Clear => {
                        desired.hidden = true;
                    }
                    Cmd::CoverResolved { cover_key, url } => {
                        if let Some(t) = desired.track.as_mut() {
                            // Only patch when this resolution
                            // matches the active track. A late
                            // reply for an old cover is dropped.
                            if t.cover_key.as_deref() == Some(cover_key.as_str())
                                && t.cover_url.is_none()
                            {
                                t.cover_url = Some(url);
                                // Force the next pass to publish
                                // even if smoothing/dedup would
                                // otherwise skip it.
                                published = None;
                                last_publish = None;
                            }
                        }
                    }
                    Cmd::Shutdown => {
                        if let Some(mut c) = client.take() {
                            let _ = c.close();
                        }
                        set_status(DiscordStatus::Disabled);
                        break;
                    }
                }
            }
            recv(timer) -> _ => {}
        }

        if !active {
            set_status(DiscordStatus::Disabled);
            continue;
        }

        if client_id.is_empty() {
            set_status(DiscordStatus::NoClientId);
            continue;
        }

        // (Re)connect if needed.
        if !connected {
            set_status(DiscordStatus::Connecting);
            if let Some(at) = next_reconnect_at {
                if Instant::now() >= at {
                    let mut c = client
                        .take()
                        .unwrap_or_else(|| DiscordIpcClient::new(&client_id));
                    match c.connect() {
                        Ok(()) => {
                            tracing::info!(
                                target: "qobee::discord",
                                client_id = %client_id,
                                "connected to Discord IPC"
                            );
                            connected = true;
                            backoff = RECONNECT_MIN;
                            next_reconnect_at = None;
                            published = None;
                            client = Some(c);
                            set_status(DiscordStatus::Connected);
                        }
                        Err(e) => {
                            tracing::debug!(
                                target: "qobee::discord",
                                error = %e,
                                "Discord IPC connect failed (Discord likely closed); will retry"
                            );
                            // Drop the half-connected client so the
                            // next attempt opens a fresh socket.
                            client = None;
                            backoff = (backoff * 2).min(RECONNECT_MAX);
                            next_reconnect_at = Some(Instant::now() + backoff);
                        }
                    }
                }
            } else {
                next_reconnect_at = Some(Instant::now() + backoff);
            }
            continue;
        }

        // Skip if we just published recently (smoothing).
        if let Some(t) = last_publish {
            if t.elapsed() < MIN_PUBLISH_INTERVAL {
                continue;
            }
        }

        // Skip when nothing to do.
        let key = PublishedKey::from(&desired);
        if published.as_ref() == Some(&key) {
            continue;
        }

        let Some(c) = client.as_mut() else {
            connected = false;
            continue;
        };

        let publish_result = if desired.hidden {
            c.clear_activity()
        } else if let Some(track) = &desired.track {
            c.set_activity(build_activity(track))
        } else {
            c.clear_activity()
        };

        match publish_result {
            Ok(()) => {
                published = Some(key);
                last_publish = Some(Instant::now());
            }
            Err(e) => {
                tracing::debug!(
                    target: "qobee::discord",
                    error = %e,
                    "Discord IPC write failed, dropping connection and reconnecting"
                );
                if let Some(mut c) = client.take() {
                    let _ = c.close();
                }
                connected = false;
                published = None;
                backoff = RECONNECT_MIN;
                next_reconnect_at = Some(Instant::now() + backoff);
                set_status(DiscordStatus::Connecting);
            }
        }
    }
}

/// Compact representation of a published state so the worker can
/// detect "nothing changed" and skip a Discord write.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishedKey {
    hidden: bool,
    paused: bool,
    title: String,
    artist: String,
    album: String,
    cover_url: Option<String>,
    duration_ms: i64,
    /// Quantized "epoch the track started at" so position drift
    /// inside the same playback session doesn't republish needlessly,
    /// but a real seek does.
    start_bucket: i64,
}

impl From<&Desired> for PublishedKey {
    fn from(d: &Desired) -> Self {
        let track = d.track.as_ref();
        let now_ms = unix_now_ms();
        let start_ms = track
            .map(|t| now_ms - (t.position_seconds * 1000.0) as i64)
            .unwrap_or(0);
        // 5-second buckets: small seek/drift below the bucket size
        // doesn't trigger a republish.
        let start_bucket = start_ms / 5_000;
        Self {
            hidden: d.hidden,
            paused: d.paused,
            title: track.map(|t| t.title.clone()).unwrap_or_default(),
            artist: track.map(|t| t.artist.clone()).unwrap_or_default(),
            album: track.map(|t| t.album.clone()).unwrap_or_default(),
            cover_url: track.and_then(|t| t.cover_url.clone()),
            duration_ms: track
                .map(|t| (t.duration_seconds * 1000.0) as i64)
                .unwrap_or(0),
            start_bucket,
        }
    }
}

/// Build the `Activity` payload Discord renders. Constructed fresh on
/// every publish to avoid borrow-spaghetti with `Cow<'a, str>`.
///
/// Note: this is only ever called when the activity is **visible**
/// (i.e. playing). Pause hides the rich presence entirely, so we
/// don't need a "Paused" badge here.
fn build_activity<'a>(track: &'a TrackPresence) -> Activity<'a> {
    let mut activity = Activity::new()
        .activity_type(ActivityType::Listening)
        .details(track.title.as_str())
        .state(format!("by {}", track.artist));

    if track.duration_seconds > 0.0 {
        let now_ms = unix_now_ms();
        let pos_ms = (track.position_seconds * 1000.0) as i64;
        let dur_ms = (track.duration_seconds * 1000.0) as i64;
        let start_ms = now_ms - pos_ms;
        let end_ms = start_ms + dur_ms;
        activity = activity.timestamps(
            Timestamps::new()
                // Discord expects milliseconds since the Unix
                // epoch; the activity API uses i64 internally.
                .start(start_ms)
                .end(end_ms),
        );
    }

    let mut assets = Assets::new();
    if let Some(url) = track.cover_url.as_deref() {
        assets = assets.large_image(url);
    } else {
        // Fall back to the application's default art configured in
        // the Discord developer portal under the "qobee" key.
        assets = assets.large_image("qobee");
    }
    if !track.album.is_empty() {
        assets = assets.large_text(track.album.as_str());
    }
    activity.assets(assets)
}

fn unix_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
