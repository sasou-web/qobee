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
pub mod commands_integration;
pub mod cover_host;
pub mod discord;
pub mod logging;
pub mod lyrics;
pub mod state;
pub mod tray;
pub mod windows_integration;

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use tauri::http::{Request, Response};
use tauri::{Emitter, Manager, UriSchemeContext, WindowEvent};

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
    if let Err(e) =
        windows_integration::set_app_user_model_id(windows_integration::APP_USER_MODEL_ID)
    {
        tracing::warn!(target: "qobee::win", error = %e, "could not set AUMID");
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
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(app_state.clone())
        .manage(tray_state.clone())
        .register_uri_scheme_protocol("qobee-cover", move |ctx, request| {
            cover_protocol(ctx, request)
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_library,
            commands::list_albums,
            commands::list_artists,
            commands::get_album,
            commands::get_track,
            commands::play_track,
            commands::play_album_from_track,
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
            commands::load_convolver_ir,
            commands::unload_convolver_ir,
            commands::get_convolver_status,
            commands::run_null_test,
            commands::cancel_null_test,
            commands::list_library_roots,
            commands::add_library_root,
            commands::remove_library_root,
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
        ])
        .setup(move |app| {
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
                        };
                        if let Err(e) = app_handle.emit(topic, &event) {
                            tracing::warn!(error = %e, "failed to emit player event");
                        }
                    }
                })
                .expect("failed to spawn event pump thread");

            // Tray: created if the user enabled it. Default true so
            // the affordance shows up at first run.
            let show_tray = app
                .state::<AppState>()
                .library()
                .get_setting("windows.show_tray_icon")
                .ok()
                .flatten()
                .map(|v| v != "false")
                .unwrap_or(true);
            if let Err(e) = tray::ensure_tray(app.handle(), tray_state_for_setup.clone(), show_tray)
            {
                tracing::warn!(target: "qobee::tray", error = %e, "could not build tray");
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

            // Hook the main window close to "minimize to tray"
            // when that setting is on.
            if let Some(window) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        let min_to_tray = app_handle
                            .state::<AppState>()
                            .library()
                            .get_setting("windows.minimize_to_tray_on_close")
                            .ok()
                            .flatten()
                            .map(|v| v == "true")
                            .unwrap_or(false);
                        if min_to_tray {
                            if let Some(w) = app_handle.get_webview_window("main") {
                                let _ = w.hide();
                            }
                            api.prevent_close();
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
        .run(|_app, event| {
            // Default exit behaviour is fine; we don't need to
            // intercept anything here. The placeholder closure
            // future-proofs us against extra hooks (e.g. tray
            // tooltip refresh on track changes).
            let _ = event;
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
