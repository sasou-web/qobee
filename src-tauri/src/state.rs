//! Tauri-managed application state.
//!
//! A single [`AppState`] is built at startup, registered with
//! `Builder::manage`, and accessed from every command via
//! `tauri::State<AppState>`.
//!
//! It owns the [`Library`] (SQLite + cover cache) and the [`Player`]
//! (engine + queue). It also keeps the cover cache directory around so
//! the custom URI protocol can serve files from it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use qobee_core::{cleanup_obsolete_audio_settings, Player, PlayerHandle};
use qobee_library::Library;

use crate::discord::DiscordPresence;

/// Application-wide handle. Cheap to clone (`Arc` internally).
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    library: Library,
    player: PlayerHandle,
    /// Kept alive for the lifetime of the app so the engine pump thread
    /// does not exit prematurely. Wrapped in a `Mutex<Option<_>>` to
    /// allow tests / shutdown logic to drop it explicitly.
    _player_owner: Mutex<Option<Player>>,
    cover_cache_dir: PathBuf,
    discord: DiscordPresence,
    /// Latched once an explicit "Quit" intent is observed (menu /
    /// tray / `Cmd+Q` / `Ctrl+Q` / `RunEvent::ExitRequested`). When
    /// `true`, the `WindowEvent::CloseRequested` hook stops calling
    /// `api.prevent_close()` and lets the platform tear the app
    /// down, regardless of the `window.close_behavior` preference
    /// (R7.5).
    quit_requested: AtomicBool,
}

impl AppState {
    /// Initialize the app state. Creates the data directories if they
    /// don't exist, opens the SQLite database, and spawns the engine
    /// worker.
    pub fn initialize() -> anyhow::Result<Self> {
        let library = Library::open_default().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let cover_cache_dir = library.cover_cache_dir().to_path_buf();

        let player = Player::new(library.clone()).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let handle = player.handle();

        // Remove obsolete `audio.*` settings rows left over from the
        // previous audiophile engine (EQ, ReplayGain, output modes,
        // crossfeed, convolver, dither, limiter, balance/trim, volume
        // curve, resampler quality). Idempotent and tolerant of absent
        // keys; non-audio settings are preserved.
        if let Err(e) = cleanup_obsolete_audio_settings(&library) {
            tracing::warn!(
                target: "qobee::app",
                error = %e,
                "obsolete audio-settings cleanup failed"
            );
        }

        let discord = DiscordPresence::new();
        // Hook the local cover cache up to Discord: covers will be
        // uploaded to a public host on demand and patched into the
        // active activity once a URL is available.
        discord.attach_cover_host(library.clone());

        Ok(AppState {
            inner: Arc::new(Inner {
                library,
                player: handle,
                _player_owner: Mutex::new(Some(player)),
                cover_cache_dir,
                discord,
                quit_requested: AtomicBool::new(false),
            }),
        })
    }

    pub fn library(&self) -> &Library {
        &self.inner.library
    }

    pub fn player(&self) -> &PlayerHandle {
        &self.inner.player
    }

    pub fn cover_cache_dir(&self) -> &Path {
        &self.inner.cover_cache_dir
    }

    pub fn discord(&self) -> &DiscordPresence {
        &self.inner.discord
    }

    /// Mark the app as on the way out. Called by every "Quit"
    /// affordance (menu, tray, `Cmd+Q` / `Ctrl+Q`, NSIS uninstall,
    /// the `RunEvent::ExitRequested` arm in `lib::run`). Once set,
    /// the close-behavior hook stops intercepting
    /// `WindowEvent::CloseRequested` so the OS can finish tearing
    /// the process down (R7.5).
    pub fn request_quit(&self) {
        self.inner.quit_requested.store(true, Ordering::SeqCst);
    }

    /// Read the latched quit flag. Cheap (`Ordering::SeqCst` on a
    /// single `AtomicBool`); called from the close-event hook on
    /// every window close.
    pub fn quit_requested(&self) -> bool {
        self.inner.quit_requested.load(Ordering::SeqCst)
    }
}
