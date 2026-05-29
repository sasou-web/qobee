//! System tray integration.
//!
//! The tray is built once at startup and updated whenever the
//! player state changes so the menu reflects the current track and
//! play/pause label. Hidden behind a setting (`Show tray icon`).
//!
//! **Windows only.** On macOS a `TrayIconBuilder` creates an
//! `NSStatusItem` — the icon that shows up in the top menu bar —
//! which Qobee deliberately avoids: the app already projects onto
//! the native Now Playing surface (`MPNowPlayingInfoCenter`) and the
//! macOS menu bar / Dock, so a redundant status-bar logo only adds
//! clutter. The menu wording here is Windows-flavoured anyway. The
//! public functions stay callable on every platform (so shared call
//! sites in `lib.rs` need no `cfg` gates) but their bodies are
//! no-ops outside Windows.

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::tray::TrayIcon;
use tauri::{AppHandle, Runtime};

// Windows-only imports — the tray menu construction and click
// dispatch only compile into the Windows build. Keeping them gated
// avoids `unused_import` warnings on macOS / Linux under the
// `-D warnings` CI gate.
#[cfg(target_os = "windows")]
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
#[cfg(target_os = "windows")]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
#[cfg(target_os = "windows")]
use tauri::{Emitter as _, Manager};

#[cfg(target_os = "windows")]
use crate::state::AppState;

/// Menu item ids the click handler dispatches on.
#[cfg(target_os = "windows")]
const ID_SHOW: &str = "tray-show";
#[cfg(target_os = "windows")]
const ID_PLAY_PAUSE: &str = "tray-play-pause";
#[cfg(target_os = "windows")]
const ID_NEXT: &str = "tray-next";
#[cfg(target_os = "windows")]
const ID_PREV: &str = "tray-prev";
#[cfg(target_os = "windows")]
const ID_LIBRARY: &str = "tray-library";
#[cfg(target_os = "windows")]
const ID_SETTINGS: &str = "tray-settings";
#[cfg(target_os = "windows")]
const ID_QUIT: &str = "tray-quit";

/// State the tray needs to keep mutable across rebuilds.
///
/// On non-Windows the slot simply stays `None` for the whole
/// process lifetime — [`ensure_tray`] never populates it.
pub struct TrayState<R: Runtime> {
    #[allow(dead_code)] // read only on Windows; field kept cross-platform
    icon: Mutex<Option<TrayIcon<R>>>,
}

impl<R: Runtime> Default for TrayState<R> {
    fn default() -> Self {
        Self {
            icon: Mutex::new(None),
        }
    }
}

/// Build (or rebuild) the tray icon. Called once at startup; can
/// also be called when settings change to toggle the icon on/off.
///
/// macOS / Linux: this is a no-op (see the module doc for why).
pub fn ensure_tray<R: Runtime>(
    app: &AppHandle<R>,
    state: Arc<TrayState<R>>,
    enabled: bool,
) -> tauri::Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, state, enabled);
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        ensure_tray_windows(app, state, enabled)
    }
}

/// Windows-only tray implementation.
#[cfg(target_os = "windows")]
fn ensure_tray_windows<R: Runtime>(
    app: &AppHandle<R>,
    state: Arc<TrayState<R>>,
    enabled: bool,
) -> tauri::Result<()> {
    let mut slot = state.icon.lock();
    if !enabled {
        if let Some(t) = slot.take() {
            // Dropping the TrayIcon removes the shell-notification
            // icon. tauri exposes no explicit "destroy" call.
            drop(t);
        }
        return Ok(());
    }
    if slot.is_some() {
        // Already built. Refresh the menu so the now-playing line
        // is up-to-date.
        if let Some(t) = slot.as_ref() {
            let menu = build_menu(app, /* now_playing_label = */ "Qobee")?;
            t.set_menu(Some(menu))?;
        }
        return Ok(());
    }

    let menu = build_menu(app, "Qobee")?;
    let tray = TrayIconBuilder::with_id("qobee-main")
        .icon(
            app.default_window_icon()
                .cloned()
                .ok_or_else(|| tauri::Error::Anyhow(anyhow::anyhow!("no default window icon")))?,
        )
        .tooltip("Qobee")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| dispatch_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            // Left click = show/focus main window. Right click is
            // handled by the OS (it pops the menu we attached).
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    *slot = Some(tray);
    Ok(())
}

/// Update the now-playing label in the tray menu. Called from the
/// player event pump whenever the current track changes. No-op on
/// non-Windows.
pub fn set_now_playing<R: Runtime>(
    app: &AppHandle<R>,
    state: Arc<TrayState<R>>,
    label: &str,
) -> tauri::Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, state, label);
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let slot = state.icon.lock();
        let Some(tray) = slot.as_ref() else {
            return Ok(());
        };
        let menu = build_menu(app, label)?;
        tray.set_menu(Some(menu))?;
        tray.set_tooltip(Some(format!("Qobee — {label}")))?;
        Ok(())
    }
}

/// Hot-toggle the tray icon at runtime: create it if `enabled` and
/// it doesn't exist yet, drop it if `!enabled` and it does.
///
/// Wired to the `set_tray_enabled` Tauri command added in task 10.1
/// so the user can flip the toggle in Settings → Window without
/// restarting the app (R7.1, R7.3 design §Window_Manager). No-op on
/// non-Windows.
pub fn set_tray_enabled<R: Runtime>(
    app: &AppHandle<R>,
    state: Arc<TrayState<R>>,
    enabled: bool,
) -> tauri::Result<()> {
    ensure_tray(app, state, enabled)
}

#[cfg(target_os = "windows")]
fn build_menu<R: Runtime>(app: &AppHandle<R>, now_playing_label: &str) -> tauri::Result<Menu<R>> {
    let np_text = if now_playing_label.is_empty() {
        "Nothing playing".to_string()
    } else {
        now_playing_label.to_string()
    };
    // The now-playing line is informational only — disabled so a
    // misclick never does anything weird.
    let np_item = MenuItem::with_id(app, "tray-now-playing", np_text, false, None::<&str>)?;

    let show = MenuItem::with_id(app, ID_SHOW, "Show / focus", true, None::<&str>)?;
    let play_pause = MenuItem::with_id(app, ID_PLAY_PAUSE, "Play / Pause", true, Some("Space"))?;
    let next = MenuItem::with_id(app, ID_NEXT, "Next track", true, None::<&str>)?;
    let prev = MenuItem::with_id(app, ID_PREV, "Previous track", true, None::<&str>)?;
    let library = MenuItem::with_id(app, ID_LIBRARY, "Open library", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, ID_SETTINGS, "Settings…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit Qobee", true, None::<&str>)?;

    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;

    Menu::with_items(
        app,
        &[
            &np_item,
            &sep1,
            &show,
            &sep2,
            &play_pause,
            &prev,
            &next,
            &sep3,
            &library,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )
}

#[cfg(target_os = "windows")]
fn dispatch_menu<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        ID_SHOW => focus_main(app),
        ID_PLAY_PAUSE => {
            if let Some(state) = app.try_state::<AppState>() {
                let player = state.player();
                let snap = player.state();
                if matches!(snap.status, qobee_engine::PlaybackStatus::Playing) {
                    let _ = player.pause();
                } else {
                    let _ = player.resume();
                }
            }
        }
        ID_NEXT => {
            if let Some(state) = app.try_state::<AppState>() {
                let _ = state.player().next();
            }
        }
        ID_PREV => {
            if let Some(state) = app.try_state::<AppState>() {
                let _ = state.player().previous();
            }
        }
        ID_LIBRARY => {
            focus_main(app);
            let _ = app.emit_to("main", "deep-link:navigate", "library");
        }
        ID_SETTINGS => {
            focus_main(app);
            let _ = app.emit_to("main", "deep-link:navigate", "settings");
        }
        ID_QUIT => {
            // R7.5 — record the explicit Quit intent before
            // calling `app.exit(0)` so the
            // `WindowEvent::CloseRequested` hook does not try to
            // hide the window or keep the process alive while the
            // event loop is shutting down.
            if let Some(state) = app.try_state::<AppState>() {
                state.request_quit();
            }
            app.exit(0);
        }
        _ => {}
    }
}

#[cfg(target_os = "windows")]
fn focus_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
