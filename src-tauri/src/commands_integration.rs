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
pub fn dispatch(
    app: &AppHandle,
    state: &AppState,
    command: AppCommand,
) -> anyhow::Result<()> {
    use tauri::Emitter;
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
                .scan_folder(
                    &path,
                    qobee_library::ScanOptions::default(),
                    |_| {},
                )
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        AppCommand::ScanFolder { path } => {
            state
                .library()
                .scan_folder(
                    &path,
                    qobee_library::ScanOptions::default(),
                    |_| {},
                )
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
    }
    // Whatever the command was, bring the main window forward.
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
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
                state
                    .library()
                    .find_track_id_by_path(&key)
                    .ok()
                    .flatten()
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
