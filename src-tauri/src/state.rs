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
use std::sync::Arc;

use parking_lot::Mutex;

use qobee_core::{AudioSettingsApply, AudioSettingsStore, Player, PlayerHandle};
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
    /// Persistent store for the `audio.*` settings. Loaded from the
    /// SQLite `settings` table at boot, mirrored to a lock-free
    /// snapshot read by the engine on each chunk boundary.
    audio_settings: AudioSettingsStore,
    /// Kept alive for the lifetime of the app so the engine pump thread
    /// does not exit prematurely. Wrapped in a `Mutex<Option<_>>` to
    /// allow tests / shutdown logic to drop it explicitly.
    _player_owner: Mutex<Option<Player>>,
    cover_cache_dir: PathBuf,
    discord: DiscordPresence,
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

        // Restore persisted audio settings.
        if let Ok(Some(rg)) = library.get_setting("audio.replaygain_mode") {
            handle.set_replaygain_mode(qobee_core::ReplayGainMode::from_setting(&rg));
        }
        if let Ok(Some(mode_str)) = library.get_setting("audio.output_mode") {
            let mode = match mode_str.as_str() {
                "exclusive" => qobee_engine::OutputMode::Exclusive,
                "shared" => qobee_engine::OutputMode::Shared,
                _ => qobee_engine::OutputMode::Auto,
            };
            // Errors here are non-fatal: if Exclusive can't init
            // (no device, etc.) we fall back to Shared and log the
            // failure so the user can see it in Settings.
            if let Err(e) = handle.set_output_mode(mode) {
                tracing::warn!(
                    target: "qobee::app",
                    error = %e,
                    "could not restore persisted output mode; falling back to Shared"
                );
            }
        }

        let discord = DiscordPresence::new();
        // Hook the local cover cache up to Discord: covers will be
        // uploaded to a public host on demand and patched into the
        // active activity once a URL is available.
        discord.attach_cover_host(library.clone());

        // Build the audio-settings store and load (or initialise)
        // the persisted `audio.*` keys. `load_or_init` writes any
        // missing/invalid rows, clamps stale values, and pushes the
        // resulting snapshot into the engine via the `apply` impl on
        // `PlayerHandle`.
        let engine_apply: Arc<dyn AudioSettingsApply> = Arc::new(handle.clone());
        let audio_settings = AudioSettingsStore::new(library.clone(), engine_apply);
        audio_settings.load_or_init();

        Ok(AppState {
            inner: Arc::new(Inner {
                library,
                player: handle,
                audio_settings,
                _player_owner: Mutex::new(Some(player)),
                cover_cache_dir,
                discord,
            }),
        })
    }

    pub fn library(&self) -> &Library {
        &self.inner.library
    }

    pub fn player(&self) -> &PlayerHandle {
        &self.inner.player
    }

    pub fn audio_settings(&self) -> &AudioSettingsStore {
        &self.inner.audio_settings
    }

    pub fn cover_cache_dir(&self) -> &Path {
        &self.inner.cover_cache_dir
    }

    pub fn discord(&self) -> &DiscordPresence {
        &self.inner.discord
    }
}
