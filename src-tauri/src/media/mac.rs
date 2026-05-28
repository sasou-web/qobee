//! macOS Now Playing bridge — projects the R8 [`PlayerEvent`] bus
//! onto `MPNowPlayingInfoCenter` and wires `MPRemoteCommandCenter`
//! back to [`qobee_core::PlayerHandle`].
//!
//! Validates: Requirements R2.1, R2.2, R2.3, R2.4, R2.5
//!
//! ## Threading model
//!
//! The MediaPlayer / AppKit objects are `!Send + !Sync` (singletons
//! tied to NSApp / the main run-loop). [`qobee_core::MediaBridge`]
//! requires `Send + Sync`, so this bridge stores nothing OS-specific
//! itself: it only keeps a [`tauri::AppHandle`] (`Send + Sync` clone)
//! and shovels every operation onto the main thread via
//! [`tauri::AppHandle::run_on_main_thread`]. The MP* singletons are
//! re-resolved on each call (cheap — `defaultCenter` returns the
//! process-wide instance).
//!
//! `init` registers the `MPRemoteCommand` handlers and the NSApp
//! menu bar exactly once. It MUST be called from the main thread,
//! which is the case in Tauri's `setup` callback on macOS.
//!
//! ## Binding strategy
//!
//! The spec asks for the four `objc2*` crates pinned at `0.6.x`. In
//! practice only the runtime crate (`objc2`) ships at 0.6; the
//! framework binding crates live on the matching `0.3` release
//! track (see the Cargo.toml comment in
//! `[target.'cfg(target_os = "macos")'.dependencies]`).
//!
//! To keep the bridge resilient against further binding-version
//! drift, every OS-side mutation goes through [`objc2::msg_send`]
//! against classes resolved by name. This style is the explicit
//! fallback recommended by the spec ("manual binding via
//! `objc2::msg_send`") and makes it trivial to swap in a different
//! binding version, including dropping the framework crates
//! entirely if Apple changes the underlying ABI.

use std::ptr::NonNull;
use std::sync::Arc;

use anyhow::Result;
use block2::RcBlock;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::MainThreadMarker;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Runtime};

use qobee_core::{MediaBridge, PlayerEvent, PlayerHandle, TrackMeta};

use crate::state::AppState;

// ---------------------------------------------------------------------
// Menu item ids — exposed publicly so a future Dock-menu extension
// (task 10.2) can reuse the same dispatcher.
// ---------------------------------------------------------------------

pub const MENU_ID_PREFERENCES: &str = "mac-menu-preferences";
pub const MENU_ID_PLAY_PAUSE: &str = "mac-menu-play-pause";
pub const MENU_ID_NEXT: &str = "mac-menu-next";
pub const MENU_ID_PREV: &str = "mac-menu-prev";

// ---------------------------------------------------------------------
// `MPNowPlayingPlaybackState` constants — encoded directly to avoid
// depending on whether the binding crate exposes them as a Rust
// `enum`, an opaque struct, or `pub const`s. Apple's headers define
// the values as `NS_ENUM(NSUInteger, MPNowPlayingPlaybackState)`:
//   Unknown=0, Playing=1, Paused=2, Stopped=3, Interrupted=4.
// `usize` is a safe stand-in for `NSUInteger` on every Apple
// platform supported by Tauri.
// ---------------------------------------------------------------------

const MP_PB_STATE_PLAYING: usize = 1;
const MP_PB_STATE_PAUSED: usize = 2;
const MP_PB_STATE_STOPPED: usize = 3;

/// `MPRemoteCommandHandlerStatus::Success` is `0` per Apple's
/// header. `NSInteger` is `i64` on every 64-bit Apple platform we
/// support.
const MP_REMOTE_HANDLER_SUCCESS: isize = 0;

// ---------------------------------------------------------------------
// MPNowPlayingInfo / MPMediaItem property keys.
//
// Apple defines these as `extern NSString * const`. Their string
// values are stable public API: see e.g.
// https://developer.apple.com/documentation/mediaplayer/mpmediaitempropertytitle
// We hard-code the strings instead of importing extern statics so
// the bridge keeps compiling regardless of binding-version churn.
// ---------------------------------------------------------------------

const KEY_TITLE: &str = "title";
const KEY_ARTIST: &str = "artist";
const KEY_ALBUM_TITLE: &str = "albumTitle";
const KEY_PLAYBACK_DURATION: &str = "playbackDuration";
const KEY_ELAPSED_PLAYBACK_TIME: &str = "MPNowPlayingInfoPropertyElapsedPlaybackTime";
const KEY_PLAYBACK_RATE: &str = "MPNowPlayingInfoPropertyPlaybackRate";

// ---------------------------------------------------------------------
// Bridge handle
// ---------------------------------------------------------------------

/// macOS Now Playing bridge.
///
/// `Send + Sync`: only carries an [`AppHandle`]. All MP* / AppKit
/// work happens inside [`AppHandle::run_on_main_thread`] callbacks.
pub struct MacOsMediaBridge {
    app: AppHandle,
}

impl MacOsMediaBridge {
    /// Initialize the bridge:
    ///   - register `MPRemoteCommandCenter` handlers,
    ///   - install the macOS menu bar (`Qobee` / `Playback` /
    ///     `Window` submenus).
    ///
    /// Must be invoked on the main thread (Tauri's `setup` callback
    /// satisfies this on macOS). The [`MainThreadMarker`] is taken
    /// as a witness to make the precondition explicit at the type
    /// level.
    ///
    /// Returns `Arc<Self>` so the same handle can be installed both
    /// in Tauri-managed state (for the fan-out task) and kept around
    /// by anyone else that wants to observe the bridge directly.
    pub fn init(app: AppHandle, _mtm: MainThreadMarker) -> Result<Arc<Self>> {
        let bridge = Arc::new(MacOsMediaBridge { app: app.clone() });

        // 1. Menu bar (R2.4) — install before we return so it's
        //    available the moment NSApp finishes launching, which
        //    is *before* the main `WebviewWindow` becomes visible
        //    to the user.
        if let Err(e) = install_menu_bar(&app) {
            tracing::warn!(
                target: "qobee::media::mac",
                error = %e,
                "failed to install macOS menu bar; continuing without it"
            );
        }

        // 2. Remote command handlers (R2.2 / R2.5). Each handler
        //    captures a clone of `AppHandle` and routes the command
        //    through the same `PlayerHandle` the UI uses.
        if let Err(e) = register_remote_commands(&app) {
            tracing::warn!(
                target: "qobee::media::mac",
                error = %e,
                "failed to register MPRemoteCommandCenter handlers; \
                 OS media keys will not be wired"
            );
        }

        Ok(bridge)
    }
}

impl MediaBridge for MacOsMediaBridge {
    fn handle_event(&self, ev: &PlayerEvent) {
        // Cloning the event is cheap relative to the `cover_bytes`
        // payload it may carry; we need to ship it to the main
        // thread without holding a borrow.
        let ev = ev.clone();
        if let Err(e) = self.app.run_on_main_thread(move || {
            // SAFETY: `run_on_main_thread` guarantees we execute on
            // the main thread, which is the only context where the
            // MP* singletons are documented to be safe to mutate.
            unsafe { apply_event_on_main(&ev) };
        }) {
            tracing::warn!(
                target: "qobee::media::mac",
                error = %e,
                "could not dispatch PlayerEvent to the main thread"
            );
        }
    }
}

// ---------------------------------------------------------------------
// Now-playing dictionary projection
// ---------------------------------------------------------------------

/// Apply a single `PlayerEvent` to `MPNowPlayingInfoCenter`.
///
/// # Safety
///
/// Must run on the main thread. The objc2 `msg_send!` invocations
/// target the public Apple API surface for the MediaPlayer
/// framework.
unsafe fn apply_event_on_main(ev: &PlayerEvent) {
    let Some(center) = now_playing_center() else {
        // The MediaPlayer framework is not loaded — most likely
        // running under a stripped test harness. Nothing to do.
        return;
    };
    let center: &AnyObject = &*center;

    match ev {
        PlayerEvent::Started {
            track,
            position_ms,
            duration_ms,
        } => {
            let info = build_now_playing_info(track, *position_ms, *duration_ms, 1.0);
            set_now_playing_info(center, info.as_deref());
            set_playback_state(center, MP_PB_STATE_PLAYING);
        }
        PlayerEvent::TrackChanged { track } => {
            // Reset elapsed time at every track boundary; the next
            // `PositionTick` will refine it within 250 ms.
            let info = build_now_playing_info(track, 0, track.duration_ms, 1.0);
            set_now_playing_info(center, info.as_deref());
            set_playback_state(center, MP_PB_STATE_PLAYING);
        }
        PlayerEvent::Paused { position_ms } => {
            update_progress_in_place(center, *position_ms, 0.0);
            set_playback_state(center, MP_PB_STATE_PAUSED);
        }
        PlayerEvent::Resumed { position_ms } => {
            update_progress_in_place(center, *position_ms, 1.0);
            set_playback_state(center, MP_PB_STATE_PLAYING);
        }
        PlayerEvent::Stopped => {
            set_now_playing_info(center, None);
            set_playback_state(center, MP_PB_STATE_STOPPED);
        }
        PlayerEvent::PositionTick { position_ms } => {
            // Keep the playback rate consistent with whatever the
            // OS currently believes the state to be.
            let rate = if get_playback_state(center) == MP_PB_STATE_PLAYING {
                1.0
            } else {
                0.0
            };
            update_progress_in_place(center, *position_ms, rate);
        }
        PlayerEvent::Errored { .. } => {
            set_now_playing_info(center, None);
            set_playback_state(center, MP_PB_STATE_STOPPED);
        }
        // Legacy variants travel on the crossbeam channel only;
        // the bridge is bound to the R8 transport bus exclusively.
        PlayerEvent::StateChanged { .. }
        | PlayerEvent::Position { .. }
        | PlayerEvent::EndOfTrack
        | PlayerEvent::BitPerfectChanged { .. }
        | PlayerEvent::Error { .. } => {}
    }
}

/// Resolve `MPNowPlayingInfoCenter.defaultCenter`, returning a
/// retained handle valid for the duration of the call. Returns
/// `None` if the MediaPlayer framework is not linked at runtime.
unsafe fn now_playing_center() -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"MPNowPlayingInfoCenter")?;
    let raw: *mut AnyObject = msg_send![cls, defaultCenter];
    if raw.is_null() {
        None
    } else {
        // `defaultCenter` returns a +0 (autoreleased) reference; we
        // retain it for the duration of this call.
        let _: () = msg_send![raw, retain];
        Retained::from_raw(raw)
    }
}

unsafe fn set_now_playing_info(center: &AnyObject, info: Option<&AnyObject>) {
    let dict_ptr: *const AnyObject = match info {
        Some(d) => d,
        None => std::ptr::null(),
    };
    let _: () = msg_send![center, setNowPlayingInfo: dict_ptr];
}

unsafe fn set_playback_state(center: &AnyObject, state: usize) {
    let _: () = msg_send![center, setPlaybackState: state];
}

unsafe fn get_playback_state(center: &AnyObject) -> usize {
    msg_send![center, playbackState]
}

/// Build a fresh now-playing dictionary as an `NSMutableDictionary`.
/// Always sets Title / Artist / AlbumTitle / PlaybackDuration /
/// ElapsedPlaybackTime / PlaybackRate.
///
/// Artwork is intentionally skipped for the v1 of this bridge:
/// building an `MPMediaItemArtwork` requires a request-handler
/// block whose lifetime / threading guarantees are awkward to
/// validate without a macOS dev box. The OS gracefully falls back
/// to the app icon when the key is absent, which already satisfies
/// R8.6 (placeholder neutre) on first delivery.
unsafe fn build_now_playing_info(
    track: &TrackMeta,
    position_ms: u64,
    duration_ms: u64,
    rate: f64,
) -> Option<Retained<AnyObject>> {
    let dict = mutable_dictionary()?;
    let dict_ref: &AnyObject = &*dict;

    set_string(dict_ref, KEY_TITLE, &track.title);
    set_string(dict_ref, KEY_ARTIST, &track.artist);
    set_string(dict_ref, KEY_ALBUM_TITLE, &track.album);

    let duration_seconds = (duration_ms as f64) / 1000.0;
    let elapsed_seconds = (position_ms as f64) / 1000.0;
    set_number_f64(dict_ref, KEY_PLAYBACK_DURATION, duration_seconds);
    set_number_f64(dict_ref, KEY_ELAPSED_PLAYBACK_TIME, elapsed_seconds);
    set_number_f64(dict_ref, KEY_PLAYBACK_RATE, rate);

    Some(dict)
}

/// Mutate the existing now-playing dictionary in place, updating
/// only `ElapsedPlaybackTime` and `PlaybackRate`. Falls back to a
/// minimal dictionary if no info is currently set.
unsafe fn update_progress_in_place(center: &AnyObject, position_ms: u64, rate: f64) {
    let elapsed_seconds = (position_ms as f64) / 1000.0;

    let existing: *mut AnyObject = msg_send![center, nowPlayingInfo];
    let dict: Retained<AnyObject> = if existing.is_null() {
        match mutable_dictionary() {
            Some(d) => d,
            None => return,
        }
    } else {
        // `mutableCopy` returns a +1 reference. The runtime class is
        // an `NSMutableDictionary`; we keep it as `AnyObject` for
        // the rest of this function since we only need the
        // `setObject:forKey:` selector.
        let copied: *mut AnyObject = msg_send![existing, mutableCopy];
        match Retained::from_raw(copied) {
            Some(r) => r,
            None => match mutable_dictionary() {
                Some(d) => d,
                None => return,
            },
        }
    };

    set_number_f64(&*dict, KEY_ELAPSED_PLAYBACK_TIME, elapsed_seconds);
    set_number_f64(&*dict, KEY_PLAYBACK_RATE, rate);

    set_now_playing_info(center, Some(&*dict));
}

/// Build a fresh empty `NSMutableDictionary`. Returns `None` if the
/// Foundation framework is not loaded.
unsafe fn mutable_dictionary() -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"NSMutableDictionary")?;
    let raw: *mut AnyObject = msg_send![cls, dictionary];
    if raw.is_null() {
        return None;
    }
    let _: () = msg_send![raw, retain];
    Retained::from_raw(raw)
}

/// Build a fresh `NSString` from a Rust `&str`.
///
/// Uses `[NSString stringWithUTF8String:]` after a small allocation
/// to NUL-terminate (Apple's UTF-8 init does not take a length). For
/// the typical track-title / artist / album payload the alloc cost
/// is negligible compared to the message-send overhead.
unsafe fn ns_string(s: &str) -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"NSString")?;
    let mut buf: Vec<u8> = Vec::with_capacity(s.len() + 1);
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
    let raw: *mut AnyObject = msg_send![
        cls,
        stringWithUTF8String: buf.as_ptr() as *const std::ffi::c_char
    ];
    if raw.is_null() {
        return None;
    }
    let _: () = msg_send![raw, retain];
    Retained::from_raw(raw)
}

/// Build an `NSNumber` from a `f64`.
unsafe fn ns_number_double(value: f64) -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"NSNumber")?;
    let raw: *mut AnyObject = msg_send![cls, numberWithDouble: value];
    if raw.is_null() {
        return None;
    }
    let _: () = msg_send![raw, retain];
    Retained::from_raw(raw)
}

/// Helper: insert a `&str` value into the dict under a Rust string
/// key (auto-converted to `NSString`).
unsafe fn set_string(dict: &AnyObject, key: &str, value: &str) {
    let Some(k) = ns_string(key) else { return };
    let Some(v) = ns_string(value) else { return };
    let _: () = msg_send![dict, setObject: &*v, forKey: &*k];
}

/// Helper: insert a `f64` value boxed as `NSNumber`.
unsafe fn set_number_f64(dict: &AnyObject, key: &str, value: f64) {
    let Some(k) = ns_string(key) else { return };
    let Some(n) = ns_number_double(value) else { return };
    let _: () = msg_send![dict, setObject: &*n, forKey: &*k];
}

// ---------------------------------------------------------------------
// MPRemoteCommandCenter wiring
// ---------------------------------------------------------------------

/// Register handlers on the shared `MPRemoteCommandCenter`.
///
/// Each handler captures a clone of `AppHandle` and dispatches the
/// command through the same `PlayerHandle` the UI uses, so transport
/// bus idempotence (R8.4 / R8.5) and ≤200 ms convergence (R8.1)
/// hold for OS-driven commands automatically.
fn register_remote_commands(app: &AppHandle) -> Result<()> {
    // SAFETY: `MPRemoteCommandCenter.shared` is a documented public
    // singleton accessor; `addTargetWithHandler:` accepts a copy of
    // the block and retains it internally, so dropping our local
    // `RcBlock` after registration is safe.
    unsafe {
        let Some(cls) = AnyClass::get(c"MPRemoteCommandCenter") else {
            return Err(anyhow::anyhow!("MPRemoteCommandCenter class not loaded"));
        };
        let center_raw: *mut AnyObject = msg_send![cls, sharedCommandCenter];
        if center_raw.is_null() {
            return Err(anyhow::anyhow!("MPRemoteCommandCenter shared returned nil"));
        }
        let center: &AnyObject = &*center_raw;

        wire_command(center, RemoteCmd::Play, app.clone());
        wire_command(center, RemoteCmd::Pause, app.clone());
        wire_command(center, RemoteCmd::TogglePlayPause, app.clone());
        wire_command(center, RemoteCmd::Next, app.clone());
        wire_command(center, RemoteCmd::Previous, app.clone());

        // ---- changePlaybackPositionCommand ----
        // The seek target lives on the event itself
        // (`positionTime`, in seconds, `NSTimeInterval` = `f64`).
        let seek_cmd: *mut AnyObject = msg_send![center, changePlaybackPositionCommand];
        if !seek_cmd.is_null() {
            let _: () = msg_send![seek_cmd, setEnabled: true];
            let app_for_seek = app.clone();
            let seek_block = RcBlock::new(
                move |event: NonNull<AnyObject>| -> isize {
                    let position_seconds: f64 = msg_send![event.as_ptr(), positionTime];
                    if let Some(player) = with_player(&app_for_seek) {
                        let _ = player.seek(position_seconds);
                    }
                    MP_REMOTE_HANDLER_SUCCESS
                },
            );
            let _: *mut AnyObject = msg_send![seek_cmd, addTargetWithHandler: &*seek_block];
            // `addTargetWithHandler:` copies the block into its
            // internal storage; releasing our local copy here is
            // safe.
            drop(seek_block);
        }

        Ok(())
    }
}

/// Identifies one of the simple `MPRemoteCommandCenter` commands we
/// route to a single `PlayerHandle` method without parameters.
#[derive(Clone, Copy)]
enum RemoteCmd {
    Play,
    Pause,
    TogglePlayPause,
    Next,
    Previous,
}

impl RemoteCmd {
    fn dispatch(self, player: &PlayerHandle) {
        match self {
            RemoteCmd::Play => {
                let _ = player.resume();
            }
            RemoteCmd::Pause => {
                let _ = player.pause();
            }
            RemoteCmd::TogglePlayPause => {
                if matches!(
                    player.state().status,
                    qobee_engine::PlaybackStatus::Playing
                ) {
                    let _ = player.pause();
                } else {
                    let _ = player.resume();
                }
            }
            RemoteCmd::Next => {
                let _ = player.next();
            }
            RemoteCmd::Previous => {
                let _ = player.previous();
            }
        }
    }
}

/// Resolve a remote command by selector, enable it, and attach a
/// handler that routes the command to its `PlayerHandle` method.
///
/// # Safety
///
/// Caller must run on the main thread.
unsafe fn wire_command(center: &AnyObject, kind: RemoteCmd, app: AppHandle) {
    let cmd: *mut AnyObject = match kind {
        RemoteCmd::Play => msg_send![center, playCommand],
        RemoteCmd::Pause => msg_send![center, pauseCommand],
        RemoteCmd::TogglePlayPause => msg_send![center, togglePlayPauseCommand],
        RemoteCmd::Next => msg_send![center, nextTrackCommand],
        RemoteCmd::Previous => msg_send![center, previousTrackCommand],
    };
    if cmd.is_null() {
        return;
    }
    let _: () = msg_send![cmd, setEnabled: true];

    let block = RcBlock::new(move |_event: NonNull<AnyObject>| -> isize {
        if let Some(player) = with_player(&app) {
            kind.dispatch(&player);
        }
        MP_REMOTE_HANDLER_SUCCESS
    });
    let _: *mut AnyObject = msg_send![cmd, addTargetWithHandler: &*block];
    drop(block);
}

/// Look up the player handle from the Tauri-managed [`AppState`].
fn with_player(app: &AppHandle) -> Option<PlayerHandle> {
    app.try_state::<AppState>().map(|st| st.player().clone())
}

// ---------------------------------------------------------------------
// macOS menu bar (R2.4)
// ---------------------------------------------------------------------

/// Build and install the macOS menu bar.
///
/// Three submenus :
/// - **Qobee** : About (predefined), Préférences…, Quit (predefined).
/// - **Playback** : Lecture / Pause, Suivant, Précédent.
/// - **Window** : Minimize (predefined). The "Zoom" predefined item
///   is not exposed by Tauri 2's `PredefinedMenuItem` API yet, so we
///   omit it; the OS still renders the green stoplight which covers
///   the same affordance.
fn install_menu_bar<R: Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<()> {
    // ----- Qobee submenu -----
    let about = PredefinedMenuItem::about(app, Some("À propos de Qobee"), None)?;
    let prefs = MenuItem::with_id(
        app,
        MENU_ID_PREFERENCES,
        "Préférences…",
        true,
        Some("Cmd+,"),
    )?;
    let quit = PredefinedMenuItem::quit(app, None)?;
    let qobee_submenu = Submenu::with_items(
        app,
        "Qobee",
        true,
        &[
            &about,
            &PredefinedMenuItem::separator(app)?,
            &prefs,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    // ----- Playback submenu -----
    let play_pause = MenuItem::with_id(
        app,
        MENU_ID_PLAY_PAUSE,
        "Lecture / Pause",
        true,
        Some("Space"),
    )?;
    let next = MenuItem::with_id(app, MENU_ID_NEXT, "Suivant", true, Some("Cmd+Right"))?;
    let prev = MenuItem::with_id(app, MENU_ID_PREV, "Précédent", true, Some("Cmd+Left"))?;
    let playback_submenu =
        Submenu::with_items(app, "Playback", true, &[&play_pause, &next, &prev])?;

    // ----- Window submenu -----
    let minimize = PredefinedMenuItem::minimize(app, None)?;
    let window_submenu = Submenu::with_items(app, "Window", true, &[&minimize])?;

    let menu = Menu::with_items(app, &[&qobee_submenu, &playback_submenu, &window_submenu])?;

    // Hook menu events on the menu itself so any item routes
    // through the same dispatch closure.
    let app_for_events = app.clone();
    menu.on_menu_event(move |_app, event| {
        dispatch_menu_event(&app_for_events, event.id().as_ref());
    });

    app.set_menu(menu)?;
    Ok(())
}

/// Route a menu-bar item id to its action.
fn dispatch_menu_event<R: Runtime>(app: &tauri::AppHandle<R>, id: &str) {
    match id {
        MENU_ID_PREFERENCES => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
            use tauri::Emitter;
            let _ = app.emit_to("main", "deep-link:navigate", "settings");
        }
        MENU_ID_PLAY_PAUSE => {
            if let Some(state) = app.try_state::<AppState>() {
                let player = state.player();
                if matches!(
                    player.state().status,
                    qobee_engine::PlaybackStatus::Playing
                ) {
                    let _ = player.pause();
                } else {
                    let _ = player.resume();
                }
            }
        }
        MENU_ID_NEXT => {
            if let Some(state) = app.try_state::<AppState>() {
                let _ = state.player().next();
            }
        }
        MENU_ID_PREV => {
            if let Some(state) = app.try_state::<AppState>() {
                let _ = state.player().previous();
            }
        }
        _ => {}
    }
}
