//! Tauri commands that wrap the Windows integration surface and the
//! deep-link / single-instance dispatcher.
//!
//! Frontend usage:
//!
//! ```ts
//! await invoke("get_windows_integration_status");
//! await invoke("set_windows_integration", {
//!   audioFiles: true, folders: true, protocol: true,
//! });
//! await invoke("dispatch_app_command", {
//!   command: { kind: "play", paths: ["C:/song.flac"] },
//! });
//! ```
//!
//! Each command is a thin wrapper. The heavy lifting lives in
//! [`crate::windows_integration`] and in the existing audio
//! commands.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::state::AppState;
use crate::windows_integration::{
    self as wi, is_autostart_enabled, register_integration, set_autostart, unregister_integration,
    AppCommand, NavigateTarget,
};

// ---------------------------------------------------------------------------
// Window_Manager preferences (R7 — task 10.1)
// ---------------------------------------------------------------------------
//
// Three new rows in the `settings` SQLite table back the preferences
// surfaced in Settings → Fenêtre:
//
// | key                              | values                                                | default |
// | -------------------------------- | ----------------------------------------------------- | ------- |
// | `windows.close_behavior`         | `quit`, `minimize_to_tray`, `keep_running_in_background` | `quit`  |
// | `windows.tray_enabled`           | `true`, `false`                                       | `false` |
// | `windows.notify_on_track_change` | `true`, `false`                                       | `true`  |
//
// Keys reuse the existing `windows.*` convention used by the
// `Windows Integration` panel (`windows.show_tray_icon`,
// `windows.minimize_to_tray_on_close`, `windows.start_minimized`).
// `windows.notify_on_track_change` is the same key already read by
// `dispatch_player_event` in `lib.rs` (R7.7), so the existing
// fan-out throttle keeps working without changes.
//
// Defaults are *read-side*: missing rows materialize as the defaults
// shown above. No DB migration is needed — the rows are written on
// first explicit write through `set_close_behavior`,
// `set_tray_enabled`, or `set_notify_on_track_change`.
//
// Task 10.2 will reconcile `windows.close_behavior` with the older
// `windows.minimize_to_tray_on_close` / `windows.show_tray_icon`
// flags exposed by `Windows Integration`.

const KEY_WIN_CLOSE_BEHAVIOR: &str = "windows.close_behavior";
const KEY_WIN_TRAY_ENABLED: &str = "windows.tray_enabled";
const KEY_WIN_NOTIFY_ON_TRACK_CHANGE: &str = "windows.notify_on_track_change";

const CLOSE_BEHAVIOR_QUIT: &str = "quit";
const CLOSE_BEHAVIOR_MIN_TO_TRAY: &str = "minimize_to_tray";
const CLOSE_BEHAVIOR_KEEP_BACKGROUND: &str = "keep_running_in_background";

const DEFAULT_CLOSE_BEHAVIOR: &str = CLOSE_BEHAVIOR_QUIT;
const DEFAULT_TRAY_ENABLED: bool = false;
const DEFAULT_NOTIFY_ON_TRACK_CHANGE: bool = true;

/// Snapshot returned by [`get_window_settings`] / each `set_*`
/// command. `close_behavior` is one of `quit`,
/// `minimize_to_tray`, `keep_running_in_background`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSettings {
    pub close_behavior: String,
    pub tray_enabled: bool,
    pub notify_on_track_change: bool,
}

fn read_window_settings(state: &AppState) -> WindowSettings {
    let lib = state.library();

    // close_behavior: only accept the three known variants. Anything
    // else (legacy / corrupt row) collapses to the default so the UI
    // is never stuck on an unknown value.
    let close_behavior = match lib.get_setting(KEY_WIN_CLOSE_BEHAVIOR) {
        Ok(Some(v))
            if matches!(
                v.as_str(),
                CLOSE_BEHAVIOR_QUIT | CLOSE_BEHAVIOR_MIN_TO_TRAY | CLOSE_BEHAVIOR_KEEP_BACKGROUND
            ) =>
        {
            v
        }
        _ => DEFAULT_CLOSE_BEHAVIOR.to_string(),
    };

    let read_bool = |key: &str, default: bool| -> bool {
        match lib.get_setting(key) {
            Ok(Some(v)) => v == "true",
            _ => default,
        }
    };

    WindowSettings {
        close_behavior,
        tray_enabled: read_bool(KEY_WIN_TRAY_ENABLED, DEFAULT_TRAY_ENABLED),
        notify_on_track_change: read_bool(
            KEY_WIN_NOTIFY_ON_TRACK_CHANGE,
            DEFAULT_NOTIFY_ON_TRACK_CHANGE,
        ),
    }
}

/// Read the persisted Window_Manager preferences (R7.3, R7.7, R7.8).
/// Missing rows materialize as the spec'd defaults so the panel can
/// be opened on a brand-new install with no DB migration.
#[tauri::command]
pub fn get_window_settings(state: State<'_, AppState>) -> Result<WindowSettings, String> {
    Ok(read_window_settings(&state))
}

/// Persist `windows.close_behavior`. Validates the enum strictly —
/// any value outside `quit`, `minimize_to_tray`,
/// `keep_running_in_background` is rejected before touching the
/// database. Returns the resulting full snapshot for the UI.
#[tauri::command]
pub fn set_close_behavior(
    value: String,
    state: State<'_, AppState>,
) -> Result<WindowSettings, String> {
    match value.as_str() {
        CLOSE_BEHAVIOR_QUIT | CLOSE_BEHAVIOR_MIN_TO_TRAY | CLOSE_BEHAVIOR_KEEP_BACKGROUND => {}
        other => {
            return Err(format!(
                "invalid close_behavior: {other:?} (expected one of {CLOSE_BEHAVIOR_QUIT:?}, \
                 {CLOSE_BEHAVIOR_MIN_TO_TRAY:?}, {CLOSE_BEHAVIOR_KEEP_BACKGROUND:?})"
            ));
        }
    }
    state
        .library()
        .set_setting(KEY_WIN_CLOSE_BEHAVIOR, &value)
        .map_err(|e| e.to_string())?;
    Ok(read_window_settings(&state))
}

/// Persist `windows.tray_enabled` and hot-toggle the tray icon
/// without restarting the app (R7.1, R7.3 design §Window_Manager).
/// Creates the tray when `value == true`; drops it when `false`.
/// Tray icon errors are surfaced to the caller so the UI toggle
/// can roll back the visual state on failure.
#[tauri::command]
pub fn set_tray_enabled(
    value: bool,
    state: State<'_, AppState>,
    app: AppHandle,
    tray_state: State<'_, std::sync::Arc<crate::tray::TrayState<tauri::Wry>>>,
) -> Result<WindowSettings, String> {
    state
        .library()
        .set_setting(KEY_WIN_TRAY_ENABLED, &value.to_string())
        .map_err(|e| e.to_string())?;
    // Mirror the legacy `windows.show_tray_icon` flag used by the
    // `setup` hook in `lib::run` so the next launch picks up the
    // new value too. The two flags are kept in sync until we
    // collapse them into one in a follow-up cleanup task.
    let _ = state
        .library()
        .set_setting("windows.show_tray_icon", &value.to_string());
    crate::tray::set_tray_enabled(&app, (*tray_state).clone(), value)
        .map_err(|e| format!("tray toggle failed: {e}"))?;
    Ok(read_window_settings(&state))
}

/// Persist `windows.notify_on_track_change`. The fan-out task in
/// `lib.rs::dispatch_player_event` reads this key on every track
/// change to gate the OS notification (R7.7).
#[tauri::command]
pub fn set_notify_on_track_change(
    value: bool,
    state: State<'_, AppState>,
) -> Result<WindowSettings, String> {
    state
        .library()
        .set_setting(KEY_WIN_NOTIFY_ON_TRACK_CHANGE, &value.to_string())
        .map_err(|e| e.to_string())?;
    Ok(read_window_settings(&state))
}

/// Snapshot returned by [`get_windows_integration_status`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationStatus {
    pub platform_supported: bool,
    pub autostart: bool,
    pub start_minimized: bool,
    pub minimize_to_tray_on_close: bool,
    pub show_tray_icon: bool,
    pub audio_file_context_menu: bool,
    pub folder_context_menu: bool,
    pub protocol_handler: bool,
    pub jump_list: bool,
    pub app_user_model_id: String,
}

const KEY_AUTOSTART: &str = "windows.autostart";
const KEY_START_MINIMIZED: &str = "windows.start_minimized";
const KEY_MIN_TO_TRAY: &str = "windows.minimize_to_tray_on_close";
const KEY_SHOW_TRAY: &str = "windows.show_tray_icon";
const KEY_FILE_MENU: &str = "windows.audio_file_context_menu";
const KEY_FOLDER_MENU: &str = "windows.folder_context_menu";
const KEY_PROTOCOL: &str = "windows.protocol_handler";
const KEY_JUMP_LIST: &str = "windows.jump_list";

#[tauri::command]
pub fn get_windows_integration_status(
    state: State<'_, AppState>,
) -> Result<IntegrationStatus, String> {
    let lib = state.library();
    let read_bool = |key: &str, default: bool| -> bool {
        match lib.get_setting(key) {
            Ok(Some(v)) => v == "true",
            _ => default,
        }
    };
    Ok(IntegrationStatus {
        platform_supported: cfg!(target_os = "windows"),
        autostart: is_autostart_enabled(),
        start_minimized: read_bool(KEY_START_MINIMIZED, false),
        minimize_to_tray_on_close: read_bool(KEY_MIN_TO_TRAY, false),
        show_tray_icon: read_bool(KEY_SHOW_TRAY, true),
        audio_file_context_menu: read_bool(KEY_FILE_MENU, false),
        folder_context_menu: read_bool(KEY_FOLDER_MENU, false),
        protocol_handler: read_bool(KEY_PROTOCOL, false),
        jump_list: read_bool(KEY_JUMP_LIST, false),
        app_user_model_id: wi::APP_USER_MODEL_ID.to_string(),
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct IntegrationToggle {
    /// When set, the corresponding integration is registered;
    /// when cleared, it's unregistered. `None` means "leave as-is".
    pub autostart: Option<bool>,
    pub start_minimized: Option<bool>,
    pub minimize_to_tray_on_close: Option<bool>,
    pub show_tray_icon: Option<bool>,
    pub audio_file_context_menu: Option<bool>,
    pub folder_context_menu: Option<bool>,
    pub protocol_handler: Option<bool>,
    pub jump_list: Option<bool>,
}

#[tauri::command]
pub fn set_windows_integration(
    update: IntegrationToggle,
    state: State<'_, AppState>,
) -> Result<IntegrationStatus, String> {
    let lib = state.library();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    // Helper to persist a bool setting + return the new value.
    let put = |key: &str, value: bool| -> Result<(), String> {
        lib.set_setting(key, &value.to_string())
            .map_err(|e| e.to_string())
    };

    if let Some(b) = update.autostart {
        let start_min = update.start_minimized.unwrap_or_else(|| {
            lib.get_setting(KEY_START_MINIMIZED)
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(false)
        });
        set_autostart(b, &exe, start_min).map_err(|e| e.to_string())?;
        put(KEY_AUTOSTART, b)?;
    }
    if let Some(b) = update.start_minimized {
        put(KEY_START_MINIMIZED, b)?;
        // Keep the autostart entry in sync if it's already on.
        if is_autostart_enabled() {
            set_autostart(true, &exe, b).map_err(|e| e.to_string())?;
        }
    }
    if let Some(b) = update.minimize_to_tray_on_close {
        put(KEY_MIN_TO_TRAY, b)?;
    }
    if let Some(b) = update.show_tray_icon {
        put(KEY_SHOW_TRAY, b)?;
    }
    if let Some(b) = update.audio_file_context_menu {
        put(KEY_FILE_MENU, b)?;
        if b {
            register_integration(&exe, true, false, false).map_err(|e| e.to_string())?;
        } else {
            unregister_integration(true, false, false).map_err(|e| e.to_string())?;
        }
    }
    if let Some(b) = update.folder_context_menu {
        put(KEY_FOLDER_MENU, b)?;
        if b {
            register_integration(&exe, false, true, false).map_err(|e| e.to_string())?;
        } else {
            unregister_integration(false, true, false).map_err(|e| e.to_string())?;
        }
    }
    if let Some(b) = update.protocol_handler {
        put(KEY_PROTOCOL, b)?;
        if b {
            register_integration(&exe, false, false, true).map_err(|e| e.to_string())?;
        } else {
            unregister_integration(false, false, true).map_err(|e| e.to_string())?;
        }
    }
    if let Some(b) = update.jump_list {
        put(KEY_JUMP_LIST, b)?;
        // Jump lists are populated lazily at startup; nothing to do
        // beyond persisting the flag.
    }
    get_windows_integration_status(state)
}

/// Manually invoke an [`AppCommand`] from the UI (used by the
/// "Test integration" buttons in Settings).
#[tauri::command]
pub fn dispatch_app_command(
    command: AppCommand,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    dispatch(&app, &state, command).map_err(|e| e.to_string())
}

/// Internal entry point used by `lib.rs` when single-instance
/// forwards args from a secondary launch.
pub fn dispatch(app: &AppHandle, state: &AppState, command: AppCommand) -> anyhow::Result<()> {
    use tauri::Emitter;
    // Transport-only commands (R7.2 jumplist quick actions, the
    // macOS Dock context menu) MUST NOT yank focus back to Qobee:
    // the user is using another window and just wants the engine
    // to flip state. Library / play / navigate commands DO bring
    // the window forward because they imply the user wants to
    // interact with the app.
    let surface_main_window = !matches!(
        &command,
        AppCommand::PlayPause | AppCommand::Next | AppCommand::Previous
    );
    match command {
        AppCommand::Play { paths } => play_paths(state, &paths, PlayMode::Replace)?,
        AppCommand::Enqueue { paths } => play_paths(state, &paths, PlayMode::Enqueue)?,
        AppCommand::PlayNext { paths } => play_paths(state, &paths, PlayMode::PlayNext)?,
        AppCommand::PlayFolder { path } => play_folder(state, &path, PlayMode::Replace)?,
        AppCommand::EnqueueFolder { path } => play_folder(state, &path, PlayMode::Enqueue)?,
        AppCommand::ImportFolder { path } => {
            state
                .library()
                .add_library_root(&path.to_string_lossy())
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            state
                .library()
                .scan_folder(&path, qobee_library::ScanOptions::default(), |_| {})
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        AppCommand::ScanFolder { path } => {
            state
                .library()
                .scan_folder(&path, qobee_library::ScanOptions::default(), |_| {})
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        AppCommand::Navigate { view } => {
            let target = match view {
                NavigateTarget::Home => "home",
                NavigateTarget::Library => "library",
                NavigateTarget::Settings => "settings",
                NavigateTarget::Queue => "queue",
            };
            let _ = app.emit("deep-link:navigate", target);
        }
        AppCommand::PlayPause => {
            // Mirrors the tray "Play / Pause" item: read the
            // current engine status and flip it. No-op when there
            // is nothing in the queue (the engine returns its own
            // `PlayerError::NoTrack` which we surface upstream).
            let player = state.player();
            let snap = player.state();
            if matches!(snap.status, qobee_engine::PlaybackStatus::Playing) {
                player.pause().map_err(|e| anyhow::anyhow!(e.to_string()))?;
            } else {
                player
                    .resume()
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            }
        }
        AppCommand::Next => {
            state
                .player()
                .next()
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        AppCommand::Previous => {
            state
                .player()
                .previous()
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
    }
    if surface_main_window {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum PlayMode {
    Replace,
    Enqueue,
    PlayNext,
}

/// Resolve a list of paths to track ids in the library and apply the
/// requested play mode. Paths not yet in the library are imported on
/// the fly through a lightweight scan of the parent directory.
fn play_paths(state: &AppState, paths: &[PathBuf], mode: PlayMode) -> anyhow::Result<()> {
    let mut ids: Vec<i64> = Vec::new();
    for path in paths {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        let key = canonical.to_string_lossy().to_string();
        let id = match state.library().find_track_id_by_path(&key) {
            Ok(Some(id)) => Some(id),
            _ => {
                // Not in the library yet: scan its parent so a
                // single-shot play imports it without making the
                // parent a permanent root.
                if let Some(parent) = canonical.parent() {
                    let _ = state.library().scan_folder(
                        parent,
                        qobee_library::ScanOptions::default(),
                        |_| {},
                    );
                }
                state.library().find_track_id_by_path(&key).ok().flatten()
            }
        };
        if let Some(id) = id {
            ids.push(id);
        } else {
            tracing::warn!(target: "qobee::deeplink", path = %path.display(),
                "could not resolve track for play command");
        }
    }
    if ids.is_empty() {
        return Ok(());
    }
    apply_play_mode(state, ids, mode)
}

fn play_folder(state: &AppState, path: &Path, mode: PlayMode) -> anyhow::Result<()> {
    state
        .library()
        .scan_folder(path, qobee_library::ScanOptions::default(), |_| {})
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let key = path.to_string_lossy().to_string();
    let tracks = state
        .library()
        .tracks_in_folder(&key)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    apply_play_mode(state, ids, mode)
}

/// Plumb a resolved id list through the player using the right
/// PlayerHandle method for the requested mode. Centralised so both
/// `play_paths` and `play_folder` share the same logic.
fn apply_play_mode(state: &AppState, ids: Vec<i64>, mode: PlayMode) -> anyhow::Result<()> {
    match mode {
        PlayMode::Replace => state
            .player()
            .play_tracks(ids, Some(0))
            .map_err(|e| anyhow::anyhow!(e.to_string()))?,
        PlayMode::Enqueue => state
            .player()
            .add_to_queue(ids)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?,
        PlayMode::PlayNext => state
            .player()
            .play_next(ids)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?,
    }
    Ok(())
}
