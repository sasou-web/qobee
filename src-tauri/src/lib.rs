//! Tauri shell for Qobee.
//!
//! Wires together:
//! - [`AppState`] (library + player) registered via `Builder::manage`;
//! - the `qobee-cover://` custom URI protocol that streams cover-art
//!   files from disk to the webview (no base64 over IPC, ever);
//! - the command surface in [`crate::commands`];
//! - a background pump that re-publishes [`qobee_core::PlayerEvent`] as
//!   Tauri events the frontend can listen to;
//! - on Windows, the desktop integration: AUMID, tray, single-
//!   instance, deep-link / `qobee://` protocol, file/folder context
//!   menu registration, autostart entry. See
//!   [`crate::windows_integration`] and [`crate::tray`].

pub mod commands;
pub mod commands_drive;
pub mod commands_integration;
pub mod cover_host;
pub mod discord;
pub mod logging;
pub mod lyrics;
pub mod media;
pub mod state;
pub mod tray;
pub mod windows_integration;

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::http::{Request, Response};
use tauri::{Emitter, Manager, UriSchemeContext, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;
use crate::tray::TrayState;
use crate::windows_integration::{parse_args, AppCommand};

/// Run the Tauri application. Called from `main.rs`.
pub fn run() {
    // The guard owns the background writer for the rolling log
    // file; dropping it flushes pending lines and closes the file.
    // Keeping it on the stack here ties its lifetime to the Tauri
    // event loop.
    let _log_guard = logging::init();

    // Set the Windows AppUserModelID before any windows or
    // notifications are created so the taskbar groups everything
    // under a single icon.
    //
    // This MUST run BEFORE `tauri::Builder::default()` below: once
    // the Tauri builder starts spinning up windows / tray icons /
    // notifications, Windows snapshots whatever AUMID was active
    // at creation time and any later call has no effect on those
    // surfaces. See requirements R1.5 and R6.3 in
    // `.kiro/specs/native-media-integration-and-ux/requirements.md`.
    if let Err(e) =
        windows_integration::set_app_user_model_id(windows_integration::APP_USER_MODEL_ID)
    {
        tracing::warn!(target: "qobee::win", error = %e, "could not set AUMID");
    }

    // Ensure a Start Menu `.lnk` exists carrying the same AUMID so
    // Windows can map the SMTC "Now Playing" tile and the taskbar
    // jumplist header to a real app name + icon. Without this, dev
    // builds (and unsigned distributions) show "Unknown app" in the
    // Now Playing flyout. Fail-soft: a missing shortcut is logged
    // and the app keeps booting.
    #[cfg(target_os = "windows")]
    {
        if let Ok(exe) = std::env::current_exe() {
            let _ = windows_integration::ensure_start_menu_shortcut(
                &exe,
                windows_integration::APP_USER_MODEL_ID,
            );
        }
    }

    let app_state = match AppState::initialize() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "failed to initialize app state");
            std::process::exit(1);
        }
    };

    let initial_args: Vec<String> = std::env::args().collect();
    let start_minimized = windows_integration::wants_start_minimized(&initial_args);
    let initial_commands = parse_args(&initial_args);
    let tray_state: Arc<TrayState<tauri::Wry>> = Arc::new(TrayState::default());
    let tray_state_for_setup = tray_state.clone();

    let mut builder = tauri::Builder::default();

    // Single-instance: when a second `qobee.exe …` invocation
    // happens (Explorer file double-click, jump list, deep link,
    // etc.) the args are forwarded here and the existing window is
    // brought forward.
    builder = builder.plugin(tauri_plugin_single_instance::init(
        |app, argv: Vec<String>, _cwd| {
            tracing::info!(target: "qobee::win", argv = ?argv, "secondary instance forwarded");
            forward_args_to_main(app, argv);
        },
    ));

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(app_state.clone())
        .manage(tray_state.clone())
        .manage(commands_drive::OAuthSessions::default())
        .register_uri_scheme_protocol("qobee-cover", move |ctx, request| {
            cover_protocol(ctx, request)
        })
        .invoke_handler(tauri::generate_handler![
            commands::show_main_window,
            commands::scan_library,
            commands::list_albums,
            commands::list_artists,
            commands::get_album,
            commands::get_track,
            commands::play_track,
            commands::play_album_from_track,
            commands::play_album,
            commands::pause,
            commands::resume,
            commands::seek,
            commands::set_volume,
            commands::next,
            commands::prev,
            commands::get_player_state,
            commands::get_output_mode,
            commands::set_output_mode,
            commands::get_user_output_mode,
            commands::list_output_devices,
            commands::get_selected_output_device,
            commands::set_output_device,
            commands::search,
            commands::list_genres,
            commands::list_albums_by_genre,
            commands::recently_played_tracks,
            commands::recently_played_albums,
            commands::recently_played_artists,
            commands::list_playlists,
            commands::get_playlist,
            commands::create_playlist,
            commands::delete_playlist,
            commands::rename_playlist,
            commands::add_to_playlist,
            commands::remove_from_playlist,
            commands::play_playlist_from_track,
            commands::play_playlist,
            commands::play_tracks,
            commands::play_next,
            commands::add_to_queue,
            commands::track_ids_for_album,
            commands::album_id_for_track,
            commands::track_ids_by_artist,
            commands::add_favorite,
            commands::remove_favorite,
            commands::is_favorite,
            commands::list_favorite_track_ids,
            commands::list_favorites,
            commands::set_rating,
            commands::get_rating,
            commands::get_play_count,
            commands::set_sleep_timer,
            commands::set_sleep_after_track,
            commands::get_sleep_timer,
            commands::export_playlist_m3u,
            commands::import_playlist_m3u,
            commands::save_session,
            commands::restore_session,
            commands::set_crossfade_ms,
            commands::get_crossfade_ms,
            commands::get_queue,
            commands::queue_remove_at,
            commands::queue_move,
            commands::queue_jump_to,
            commands::get_tracks,
            commands::get_setting,
            commands::set_setting,
            commands::list_settings,
            commands::clear_settings,
            commands::get_audio_setting,
            commands::set_audio_setting,
            commands::list_audio_settings,
            commands::reset_audio_settings,
            commands::get_bit_perfect_health,
            commands::get_device_mix_format,
            commands::open_windows_sound_settings,
            commands::shell_open,
            commands::load_convolver_ir,
            commands::unload_convolver_ir,
            commands::get_convolver_status,
            commands::run_null_test,
            commands::cancel_null_test,
            commands::list_library_roots,
            commands::add_library_root,
            commands::remove_library_root,
            commands::list_library_sources,
            commands::add_library_source,
            commands::remove_library_source,
            commands::set_library_source_enabled,
            commands_drive::drive_oauth_start,
            commands_drive::drive_oauth_wait,
            commands_drive::drive_oauth_cancel,
            commands_drive::drive_status,
            commands_drive::drive_disconnect,
            commands_drive::drive_list_folder,
            commands_drive::drive_set_folder,
            commands_drive::drive_index,
            commands_drive::drive_sync,
            commands_drive::drive_sync_push,
            commands_drive::drive_sync_pull,
            commands::scan_all_roots,
            commands::library_stats,
            commands::clear_history,
            commands::clear_cover_cache,
            commands::reset_library,
            commands::get_eq_gains,
            commands::set_eq_gains,
            commands::get_lyrics,
            commands::toggle_mini_player,
            commands::get_replaygain_mode,
            commands::set_replaygain_mode,
            commands::get_repeat_mode,
            commands::set_repeat_mode,
            commands::get_shuffle,
            commands::set_shuffle,
            commands::play_random_album,
            commands::get_endless,
            commands::set_endless,
            commands::get_artist_detail,
            commands::album_size_bytes,
            commands::discord_init,
            commands::discord_set_client_id,
            commands::discord_status,
            commands::discord_set_cover_upload_enabled,
            commands::discord_update_track,
            commands::discord_set_paused,
            commands::discord_clear_presence,
            commands_integration::get_windows_integration_status,
            commands_integration::set_windows_integration,
            commands_integration::dispatch_app_command,
            commands_integration::get_window_settings,
            commands_integration::set_close_behavior,
            commands_integration::set_tray_enabled,
            commands_integration::set_notify_on_track_change,
        ])
        .setup(move |app| {
            // R2.4 — install the macOS Now Playing bridge BEFORE
            // any window visibility transition. The MediaPlayer
            // singletons (`MPNowPlayingInfoCenter`,
            // `MPRemoteCommandCenter`) and the NSApp menu bar must
            // be live the moment the user sees Qobee in the Dock,
            // including when the Tauri runtime decides to hide the
            // main window for `--minimized`.
            //
            // Tauri's `setup` callback runs on the main thread on
            // macOS, so the `MainThreadMarker` is sound. We store
            // the bridge as `Arc<MacOsMediaBridge>` in the
            // Tauri-managed state so the R8 fan-out task below can
            // pick it up via `app_handle.try_state()`.
            #[cfg(target_os = "macos")]
            {
                // SAFETY: the Tauri `setup` callback is invoked
                // on the main thread on macOS.
                let mtm = unsafe { objc2::MainThreadMarker::new_unchecked() };
                match crate::media::mac::MacOsMediaBridge::init(app.handle().clone(), mtm) {
                    Ok(bridge) => {
                        app.manage(bridge);
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "qobee::media::mac",
                            error = %e,
                            "MacOsMediaBridge::init failed; \
                             continuing without macOS Now Playing"
                        );
                    }
                }
            }

            // R1 — install the Windows SMTC bridge. Mirrors the
            // macOS block above: best-effort, log + continue on
            // failure, store as `Arc<SmtcBridge>` so the R8 fan-out
            // task can fetch it via `app_handle.try_state()`.
            //
            // SMTC needs a top-level HWND, so we grab the main
            // window here. If it does not exist yet (extremely
            // unusual — Tauri creates it before `setup` runs), we
            // simply skip; the rest of the app keeps working.
            #[cfg(target_os = "windows")]
            {
                match app.get_webview_window("main") {
                    Some(window) => {
                        match crate::media::smtc::SmtcBridge::init(
                            &window,
                            app.handle().clone(),
                        ) {
                            Ok(bridge) => {
                                app.manage(Arc::new(bridge));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    target: "qobee::media::smtc",
                                    error = %e,
                                    "SmtcBridge::init failed; \
                                     continuing without Windows SMTC"
                                );
                            }
                        }
                    }
                    None => {
                        tracing::warn!(
                            target: "qobee::media::smtc",
                            "main window unavailable in setup(); skipping SMTC bridge"
                        );
                    }
                }
            }

            // Spawn a thread that pumps player events into Tauri events.
            let app_handle = app.handle().clone();
            let state: tauri::State<AppState> = app_handle.state();
            let rx = state.player().subscribe();
            thread::Builder::new()
                .name("qobee-event-pump".into())
                .spawn(move || {
                    while let Ok(event) = rx.recv() {
                        let topic = match &event {
                            qobee_core::PlayerEvent::StateChanged { .. } => "player:state",
                            qobee_core::PlayerEvent::Position { .. } => "player:position",
                            qobee_core::PlayerEvent::EndOfTrack => "player:end-of-track",
                            qobee_core::PlayerEvent::BitPerfectChanged { .. } => {
                                "player:bit-perfect"
                            }
                            qobee_core::PlayerEvent::Error { .. } => "player:error",
                            // The R8 transport-bus variants (`Started`,
                            // `Paused`, `Resumed`, `Stopped`,
                            // `TrackChanged`, `PositionTick`,
                            // `Errored`) are not emitted on this
                            // legacy crossbeam channel — they travel
                            // on `PlayerHandle::subscribe_broadcast`
                            // and are republished by the dedicated
                            // fan-out task added in task 2.1. Map any
                            // accidental crossover to `player:event`
                            // so we never panic on an unhandled
                            // variant.
                            _ => "player:event",
                        };
                        if let Err(e) = app_handle.emit(topic, &event) {
                            tracing::warn!(error = %e, "failed to emit player event");
                        }
                    }
                })
                .expect("failed to spawn event pump thread");

            // ---------------------------------------------------------
            // R8 transport bus fan-out (task 2.1)
            // ---------------------------------------------------------
            //
            // The legacy crossbeam pump above keeps republishing the
            // historical topics (`player:state`, `player:position`,
            // `player:end-of-track`, `player:bit-perfect`,
            // `player:error`) for back-compat. The new fan-out task
            // sits next to it and consumes the dedicated
            // `tokio::sync::broadcast` channel exposed by
            // `PlayerHandle::subscribe_broadcast`.
            //
            // It dispatches each event to:
            // 1. the OS bridges (`cfg`-gated; SMTC on Windows, macOS
            //    `MPNowPlayingInfoCenter` — wired up in tasks 3.1 / 4.1),
            // 2. the Tauri webview as `player:event` (consumed by
            //    `ui/src/lib/playerStore.svelte.ts` in task 5.1),
            // 3. an opt-in OS native notification on `TrackChanged`
            //    (R7.7), throttled to one per 5 seconds.
            //
            // On `RecvError::Lagged`, we resnapshot via
            // `PlayerHandle::state()` and re-publish a synthetic
            // `Started`/`Paused` so all sinks resync without dropped
            // transitions silently leaving them stale.
            let app_handle_fanout = app.handle().clone();
            let mut broadcast_rx = app_handle_fanout
                .state::<AppState>()
                .player()
                .subscribe_broadcast();
            // Throttle state for R7.7 native notifications. The
            // `Mutex` is cheap (single thread accesses it) but we
            // keep it explicit to make the invariant visible.
            let last_notify_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
            tauri::async_runtime::spawn(async move {
                loop {
                    match broadcast_rx.recv().await {
                        Ok(ev) => {
                            dispatch_player_event(&app_handle_fanout, &ev, &last_notify_at);
                        }
                        Err(RecvError::Lagged(n)) => {
                            // The bridges and the UI store may have
                            // missed up to `n` transitions; resnapshot
                            // from the source of truth.
                            tracing::warn!(
                                target: "qobee::sync",
                                lagged = n,
                                "broadcast receiver lagged; resyncing from PlayerHandle::state"
                            );
                            let state_handle =
                                app_handle_fanout.state::<AppState>().player().clone();
                            let snap = state_handle.state();
                            let track = state_handle.current_track_meta();
                            let resync = match (snap.status, track) {
                                (qobee_engine::PlaybackStatus::Playing, Some(meta)) => {
                                    let position_ms =
                                        (snap.position_seconds * 1000.0).max(0.0) as u64;
                                    let duration_ms = meta.duration_ms;
                                    Some(qobee_core::PlayerEvent::Started {
                                        track: meta,
                                        position_ms,
                                        duration_ms,
                                    })
                                }
                                (qobee_engine::PlaybackStatus::Paused, _) => {
                                    let position_ms =
                                        (snap.position_seconds * 1000.0).max(0.0) as u64;
                                    Some(qobee_core::PlayerEvent::Paused { position_ms })
                                }
                                _ => Some(qobee_core::PlayerEvent::Stopped),
                            };
                            if let Some(ev) = resync {
                                dispatch_player_event(&app_handle_fanout, &ev, &last_notify_at);
                            }
                        }
                        Err(RecvError::Closed) => {
                            tracing::info!(
                                target: "qobee::sync",
                                "player broadcast channel closed; stopping fan-out task"
                            );
                            break;
                        }
                    }
                }
            });

            // Tray: created if the user enabled it. Default true so
            // the affordance shows up at first run.
            //
            // Two settings keys are honored for back-compat with
            // task 10.1 / pre-10.1 builds:
            //   - `windows.tray_enabled` (R7 design §Window_Manager,
            //     default `false`)
            //   - `windows.show_tray_icon` (legacy "Windows
            //     Integration" panel, default `true`)
            // Whichever is `true` wins so a user who enabled the
            // tray on a pre-10.1 build keeps it visible after the
            // upgrade, and a user who toggles the new "Tray icon"
            // switch in Settings → Fenêtre also lights it up.
            let lib_for_tray = app.state::<AppState>();
            let lib_for_tray = lib_for_tray.library();
            let tray_enabled_new = lib_for_tray
                .get_setting("windows.tray_enabled")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(false);
            let show_tray_legacy = lib_for_tray
                .get_setting("windows.show_tray_icon")
                .ok()
                .flatten()
                .map(|v| v != "false")
                .unwrap_or(true);
            let show_tray = tray_enabled_new || show_tray_legacy;
            if let Err(e) = tray::ensure_tray(app.handle(), tray_state_for_setup.clone(), show_tray)
            {
                tracing::warn!(target: "qobee::tray", error = %e, "could not build tray");
            }

            // R7.2 — populate the Windows taskbar / Start jumplist
            // with the Play/Pause, Next, Previous quick actions.
            // Fail-soft: any WinRT error inside is logged at warn
            // level and swallowed by `setup_jumplist`.
            #[cfg(target_os = "windows")]
            {
                if let Err(e) = windows_integration::setup_jumplist() {
                    tracing::warn!(
                        target: "qobee::win",
                        error = %e,
                        "jumplist setup failed; continuing without quick actions"
                    );
                }
            }

            // Listen for `qobee://` deep links (registered protocol
            // forwarded by the OS when the app is already running).
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let app_handle = app.handle().clone();
                let _ = app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        if let Some(cmd) = windows_integration::parse_deep_link(url.as_str()) {
                            run_app_command(&app_handle, cmd);
                        }
                    }
                });
            }

            // Apply the start-minimized flag — hide the main window
            // on first show if `--minimized` was on the CLI or the
            // user has the setting enabled.
            let want_min = start_minimized
                || app
                    .state::<AppState>()
                    .library()
                    .get_setting("windows.start_minimized")
                    .ok()
                    .flatten()
                    .map(|v| v == "true")
                    .unwrap_or(false);
            if want_min {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }

            // Hook the main window close. We honour three knobs in
            // priority order:
            //
            // 1. `AppState::quit_requested` — set to `true` by every
            //    explicit "Quit" affordance (tray menu, NSApp Quit
            //    item, `RunEvent::ExitRequested`). When this latch
            //    is on we never call `api.prevent_close()`, so the
            //    window closes naturally and the OS finishes
            //    tearing the process down (R7.5).
            // 2. `windows.close_behavior` (task 10.1, default
            //    `quit`): one of `quit`, `minimize_to_tray`,
            //    `keep_running_in_background`.
            // 3. Legacy `windows.minimize_to_tray_on_close` boolean:
            //    consulted only when `close_behavior` is missing
            //    or unparseable, so users who customised the old
            //    "Windows Integration" panel keep their behaviour
            //    after the upgrade.
            if let Some(window) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                let tray_state_for_close = tray_state.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        let state = app_handle.state::<AppState>();
                        if state.quit_requested() {
                            // Explicit Quit intent: do not
                            // intercept, let the window close.
                            return;
                        }
                        let behavior = read_close_behavior(&app_handle);
                        match behavior {
                            CloseBehavior::Quit => {
                                // Default: latch the quit flag so
                                // any subsequent
                                // `RunEvent::ExitRequested` /
                                // window-event volley does not try
                                // to intercept anything either.
                                state.request_quit();
                            }
                            CloseBehavior::MinimizeToTray => {
                                api.prevent_close();
                                if let Some(w) = app_handle.get_webview_window("main") {
                                    let _ = w.hide();
                                }
                                if let Err(e) = tray::ensure_tray(
                                    &app_handle,
                                    tray_state_for_close.clone(),
                                    true,
                                ) {
                                    tracing::warn!(
                                        target: "qobee::tray",
                                        error = %e,
                                        "could not ensure tray on close"
                                    );
                                }
                            }
                            CloseBehavior::KeepRunningInBackground => {
                                api.prevent_close();
                                if let Some(w) = app_handle.get_webview_window("main") {
                                    let _ = w.hide();
                                }
                                // Tray is intentionally NOT shown
                                // here — the user opted into the
                                // "no UI surface at all" mode.
                            }
                        }
                    }
                });
            }

            // Run any commands the user passed on the original
            // command line (file double-click, jump list, etc.).
            for cmd in initial_commands.clone() {
                run_app_command(app.handle(), cmd);
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Qobee")
        .run(|app_handle, event| {
            // R7.5 — every path that arrives at the OS-level "exit"
            // intent (Cmd+Q on macOS, Alt+F4 → quit decision, NSIS
            // uninstall, `app.exit(0)`) routes through
            // `RunEvent::ExitRequested`. We latch the quit flag
            // here so the `WindowEvent::CloseRequested` hook
            // installed above stops calling
            // `api.prevent_close()` while the runtime is tearing
            // the windows down.
            //
            // R7.6 — on macOS, clicking the Dock icon while the
            // window is hidden fires `RunEvent::Reopen`. Bring
            // the main window back instead of leaving the user
            // staring at a bouncing icon.
            match event {
                tauri::RunEvent::ExitRequested { .. } => {
                    if let Some(state) = app_handle.try_state::<AppState>() {
                        state.request_quit();
                    }
                }
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }
                _ => {}
            }
        });
}

/// Forward a secondary-instance argv to the main app: parse, focus
/// the window, and run each [`AppCommand`].
fn forward_args_to_main(app: &tauri::AppHandle, argv: Vec<String>) {
    let cmds = parse_args(&argv);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    for cmd in cmds {
        run_app_command(app, cmd);
    }
}

/// Dispatch an [`AppCommand`] using the integration command pipeline.
fn run_app_command(app: &tauri::AppHandle, cmd: AppCommand) {
    let state = app.state::<AppState>();
    if let Err(e) = commands_integration::dispatch(app, &state, cmd) {
        tracing::warn!(target: "qobee::deeplink", error = %e, "AppCommand dispatch failed");
    }
}

/// Throttle window for the R7.7 native track-change notifications.
/// One notification per `NOTIFY_THROTTLE` keeps a quick-skip burst
/// from flooding the system tray (R8 design §Player_Sync).
const NOTIFY_THROTTLE: Duration = Duration::from_secs(5);

/// Dispatch a single `PlayerEvent` from the R8 fan-out task to:
/// 1. the OS bridges (`cfg`-gated; wired up in tasks 3.1 / 4.1),
/// 2. the webview through the `player:event` Tauri topic (consumed
///    by `ui/src/lib/playerStore.svelte.ts` in task 5.1),
/// 3. an opt-in native notification on `TrackChanged` only, with a
///    5-second throttle and gated on the
///    `windows.notify_on_track_change` setting (default `true` per
///    task 10.1; the migration / UI for the toggle land there).
fn dispatch_player_event(
    app_handle: &tauri::AppHandle,
    ev: &qobee_core::PlayerEvent,
    last_notify_at: &Arc<Mutex<Option<Instant>>>,
) {
    // 1. OS bridges. The bridges themselves (SMTC on Windows,
    //    `MPNowPlayingInfoCenter` on macOS) project the R8
    //    transport bus onto their respective Now Playing
    //    surfaces. Both are stored as managed state during
    //    `setup()`; we look them up here without holding any
    //    lock so the fan-out loop stays non-blocking.
    #[cfg(target_os = "windows")]
    {
        if let Some(bridge) = app_handle.try_state::<Arc<crate::media::smtc::SmtcBridge>>() {
            use qobee_core::MediaBridge;
            bridge.handle_event(ev);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // The bridge is registered in Tauri-managed state during
        // `setup()`. If init failed (logged at warn level there) we
        // skip it silently — the UI / notification path still
        // works.
        if let Some(bridge) =
            app_handle.try_state::<std::sync::Arc<crate::media::mac::MacOsMediaBridge>>()
        {
            use qobee_core::MediaBridge;
            bridge.handle_event(ev);
        }
    }

    // 2. Frontend. The Svelte player store listens on
    //    `player:event` and dispatches each variant by its
    //    `serde(tag = "type")` discriminator.
    if let Err(e) = app_handle.emit("player:event", ev) {
        tracing::warn!(
            target: "qobee::sync",
            error = %e,
            "failed to emit player:event to webview"
        );
    }

    // 3. R7.7 — native notification on track change only.
    if let qobee_core::PlayerEvent::TrackChanged { track } = ev {
        if !notify_on_track_change_enabled(app_handle) {
            return;
        }
        let now = Instant::now();
        let mut slot = last_notify_at.lock();
        let allow = match *slot {
            None => true,
            Some(prev) => now.duration_since(prev) >= NOTIFY_THROTTLE,
        };
        if !allow {
            return;
        }
        *slot = Some(now);
        drop(slot);

        let title = track.title.clone();
        let body = if track.artist.is_empty() {
            track.album.clone()
        } else if track.album.is_empty() {
            track.artist.clone()
        } else {
            format!("{} — {}", track.artist, track.album)
        };
        if let Err(e) = app_handle
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
        {
            tracing::warn!(
                target: "qobee::sync",
                error = %e,
                "failed to show track-change notification"
            );
        }
    }
}

/// Read `windows.notify_on_track_change` from the persisted
/// settings table. Defaults to `true` per task 10.1's planned
/// migration; until that migration lands, missing rows mean the
/// setting was never explicitly disabled and notifications stay
/// on.
fn notify_on_track_change_enabled(app_handle: &tauri::AppHandle) -> bool {
    let state = app_handle.state::<AppState>();
    match state.library().get_setting("windows.notify_on_track_change") {
        Ok(Some(v)) => v != "false",
        Ok(None) => true,
        Err(e) => {
            tracing::debug!(
                target: "qobee::sync",
                error = %e,
                "could not read notify_on_track_change setting; defaulting to true"
            );
            true
        }
    }
}

/// What the `WindowEvent::CloseRequested` hook should do when the
/// user closes the main window (R7.3). Mirrored 1:1 by the
/// `windows.close_behavior` enum persisted in the settings table by
/// task 10.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseBehavior {
    /// Default. Let the window close and the process exit.
    Quit,
    /// Hide the window, ensure the tray icon is visible.
    MinimizeToTray,
    /// Hide the window, do NOT touch the tray. The app is
    /// invisible; only the audio output keeps it alive.
    KeepRunningInBackground,
}

/// Resolve the active close-behavior preference. Reads
/// `windows.close_behavior` (the new key from task 10.1) first; if
/// the row is missing OR carries an unknown value, falls back to the
/// legacy `windows.minimize_to_tray_on_close` boolean so a user who
/// customised the old "Windows Integration" panel keeps their
/// behaviour after the upgrade. When neither is set the default is
/// `Quit` (R7.3 default per requirements §Settings).
fn read_close_behavior(app_handle: &tauri::AppHandle) -> CloseBehavior {
    let lib = app_handle.state::<AppState>();
    let lib = lib.library();
    if let Ok(Some(v)) = lib.get_setting("windows.close_behavior") {
        match v.as_str() {
            "quit" => return CloseBehavior::Quit,
            "minimize_to_tray" => return CloseBehavior::MinimizeToTray,
            "keep_running_in_background" => return CloseBehavior::KeepRunningInBackground,
            other => {
                tracing::warn!(
                    target: "qobee::win",
                    value = %other,
                    "ignoring unknown windows.close_behavior; falling back to legacy flag"
                );
            }
        }
    }
    let legacy_min_to_tray = lib
        .get_setting("windows.minimize_to_tray_on_close")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);
    if legacy_min_to_tray {
        CloseBehavior::MinimizeToTray
    } else {
        CloseBehavior::Quit
    }
}

/// `qobee-cover://<cache_key>` -> file from the cover cache directory.
///
/// `cache_key` is `aa/<hash>.<ext>` (sharded). We resolve it against
/// [`AppState::cover_cache_dir`] and refuse anything that would escape
/// that directory.
fn cover_protocol<R: tauri::Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let app = ctx.app_handle();
    let state: tauri::State<AppState> = app.state();
    let cache_dir = state.cover_cache_dir().to_path_buf();

    let uri = request.uri();
    let raw = uri.to_string();

    // Tauri 2 normalizes custom URIs differently per platform:
    //   - Windows: `http://qobee-cover.localhost/<key>`
    //   - macOS:   `qobee-cover://localhost/<key>`
    //   - Linux:   `qobee-cover://localhost/<key>` (WebKitGTK)
    // Plus the original `qobee-cover://<key>` form. Accept all of them
    // and strip down to the cache key.
    let key = if let Some(rest) = raw.strip_prefix("qobee-cover://localhost/") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("qobee-cover://") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("https://qobee-cover.localhost/") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("http://qobee-cover.localhost/") {
        rest.to_string()
    } else {
        tracing::debug!(target: "qobee::cover", uri = %raw, "unrecognized cover URI");
        return not_found();
    };

    let key = match urlencoding_decode(&key) {
        Some(k) => k,
        None => return not_found(),
    };
    // Normalize separators: cache keys store `/`, but Path joins with
    // the platform separator, and canonicalize is picky.
    let key = key.replace('/', std::path::MAIN_SEPARATOR_STR);

    let candidate: PathBuf = cache_dir.join(&key);
    let canon_root = match cache_dir.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(target: "qobee::cover", error = %e, "cover cache dir not canonicalize-able");
            return not_found();
        }
    };
    let canon_target = match candidate.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            tracing::debug!(
                target: "qobee::cover",
                candidate = %candidate.display(),
                "cover not on disk"
            );
            return not_found();
        }
    };
    if !canon_target.starts_with(&canon_root) {
        return not_found();
    }

    let bytes = match std::fs::read(&canon_target) {
        Ok(b) => b,
        Err(_) => return not_found(),
    };

    let mime = match canon_target
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "application/octet-stream",
    };

    Response::builder()
        .status(200)
        .header("Content-Type", mime)
        .header("Cache-Control", "public, max-age=86400")
        // CORS headers so the webview can read the bytes back from a
        // canvas (palette extraction in the Properties dialog calls
        // `getImageData`, which would otherwise throw a SecurityError
        // because Tauri exposes this scheme as a cross-origin one on
        // Windows / macOS).
        .header("Access-Control-Allow-Origin", "*")
        .header("Cross-Origin-Resource-Policy", "cross-origin")
        .body(bytes)
        .unwrap_or_else(|_| not_found())
}

fn not_found() -> Response<Vec<u8>> {
    Response::builder()
        .status(404)
        .header("Content-Type", "text/plain")
        .body(b"not found".to_vec())
        .unwrap()
}

/// Minimal percent-decoder: the only characters we care about in a
/// cover key are the slash `/`, dot `.`, and hex digits, so a tiny
/// hand-rolled decoder keeps us from pulling in another crate.
fn urlencoding_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                let v = u8::from_str_radix(hex, 16).ok()?;
                out.push(v);
                i += 3;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}
