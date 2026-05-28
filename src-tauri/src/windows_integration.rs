//! Windows-only desktop integration.
//!
//! Everything in this module is gated behind `cfg(target_os =
//! "windows")` at the call site. The module is compiled on every
//! platform but most functions become no-ops outside Windows so the
//! caller doesn't have to scatter `cfg` blocks.
//!
//! Surface:
//!
//! * [`set_app_user_model_id`] — sets the AUMID on the current
//!   process so Windows groups taskbar / notifications / jump lists
//!   under the same identity (no more split icons for the same app).
//! * [`set_autostart`] — toggles the `Run` key entry that launches
//!   the app at login, optionally with `--minimized`.
//! * [`is_autostart_enabled`] — reads the same key.
//! * [`register_integration`] / [`unregister_integration`] —
//!   user-level (HKCU) registry entries that group the protocol
//!   handler, audio file context menu and folder context menu under
//!   one opt-in surface. We never touch HKLM and never claim to be
//!   the default music app. Each section is opt-in via the function
//!   parameters; the unregister call is idempotent and safe to
//!   invoke even if nothing was registered.
//! * [`AppCommand`] — the typed command surface our deep-link parser
//!   produces. The Tauri side dispatches these onto the existing
//!   command pipeline (player + library), so single-instance
//!   forwarded args trigger the same behavior as a Tauri command.
//!
//! Cleanup: every registry write goes through `winreg` and uses keys
//! under `HKCU\Software\Qobee\WindowsIntegration\*` so an
//! uninstaller can blow the whole subtree away in one call. The
//! `unregister_*` functions also reverse each individual change.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// AppCommand — typed payload extracted from CLI args / deep links.
// ---------------------------------------------------------------------------

/// Action requested by the OS (file double-click, jump list entry,
/// `qobee://` link, secondary launch). Always plumbed through the
/// same command pipeline so we don't have one code path per source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppCommand {
    /// Open files immediately (replaces the queue).
    Play { paths: Vec<PathBuf> },
    /// Append to the end of the current queue.
    Enqueue { paths: Vec<PathBuf> },
    /// Insert right after the current track.
    PlayNext { paths: Vec<PathBuf> },
    /// Crawl a directory and play everything found.
    PlayFolder { path: PathBuf },
    /// Append a directory to the queue.
    EnqueueFolder { path: PathBuf },
    /// Add the directory to the library roots and trigger a scan.
    ImportFolder { path: PathBuf },
    /// Trigger a scan on a directory without making it a root.
    ScanFolder { path: PathBuf },
    /// Bring the app to the foreground; navigate to a known view.
    Navigate { view: NavigateTarget },
    /// Toggle play/pause on the current queue. Used by jumplist /
    /// tray quick-actions (R7.1, R7.2) and the macOS Dock context
    /// menu (R7.6); takes no payload — the player engine resolves
    /// "current" from its own state.
    PlayPause,
    /// Advance to the next track in the current queue.
    Next,
    /// Step back to the previous track in the current queue.
    Previous,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NavigateTarget {
    Home,
    Library,
    Settings,
    Queue,
}

/// Parse a Win32-style argument list (typically `std::env::args()`
/// or the `argv` slice forwarded by `tauri-plugin-single-instance`)
/// into one or more [`AppCommand`]s.
///
/// Recognised forms:
///
/// * `--play <path...>`           — replaces the queue
/// * `--enqueue <path...>`        — appends to the queue
/// * `--play-next <path...>`      — inserts right after current
/// * `--play-folder <path>`       — folder version of `--play`
/// * `--enqueue-folder <path>`    — folder version of `--enqueue`
/// * `--import-folder <path>`     — adds the folder as a library root
/// * `--scan-folder <path>`       — scans a folder without adding it
/// * `--open-library`             — focus + go to library
/// * `--open-settings`            — focus + go to settings
/// * `--minimized`                — start minimized (handled at startup)
/// * `qobee://...`                — see [`parse_deep_link`]
/// * `<bare path>`                — equivalent to `--play <path>`
///
/// Multiple actions can be combined; their order is preserved so a
/// shell can `qobee --enqueue a.flac --play b.flac` and get b.flac
/// playing with a.flac queued.
pub fn parse_args(args: &[String]) -> Vec<AppCommand> {
    let mut out = Vec::new();
    let mut bare_paths: Vec<PathBuf> = Vec::new();
    let mut i = 0;
    // Skip argv[0] which is the executable path on Windows.
    if !args.is_empty() {
        i = 1;
    }
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--play" => {
                let paths = take_path_run(args, &mut i);
                if !paths.is_empty() {
                    out.push(AppCommand::Play { paths });
                }
            }
            "--enqueue" | "--add-to-queue" => {
                let paths = take_path_run(args, &mut i);
                if !paths.is_empty() {
                    out.push(AppCommand::Enqueue { paths });
                }
            }
            "--play-next" => {
                let paths = take_path_run(args, &mut i);
                if !paths.is_empty() {
                    out.push(AppCommand::PlayNext { paths });
                }
            }
            "--play-folder" => {
                if let Some(p) = take_single_path(args, &mut i) {
                    out.push(AppCommand::PlayFolder { path: p });
                }
            }
            "--enqueue-folder" => {
                if let Some(p) = take_single_path(args, &mut i) {
                    out.push(AppCommand::EnqueueFolder { path: p });
                }
            }
            "--import-folder" => {
                if let Some(p) = take_single_path(args, &mut i) {
                    out.push(AppCommand::ImportFolder { path: p });
                }
            }
            "--scan-folder" => {
                if let Some(p) = take_single_path(args, &mut i) {
                    out.push(AppCommand::ScanFolder { path: p });
                }
            }
            "--open-library" => {
                out.push(AppCommand::Navigate {
                    view: NavigateTarget::Library,
                });
                i += 1;
            }
            "--open-settings" => {
                out.push(AppCommand::Navigate {
                    view: NavigateTarget::Settings,
                });
                i += 1;
            }
            "--play-pause" | "--toggle-play-pause" => {
                out.push(AppCommand::PlayPause);
                i += 1;
            }
            "--next" | "--next-track" => {
                out.push(AppCommand::Next);
                i += 1;
            }
            "--previous" | "--prev" | "--previous-track" => {
                out.push(AppCommand::Previous);
                i += 1;
            }
            "--minimized" | "--start-minimized" => {
                // Recognised as an opt-in flag; consumed by the
                // startup code via `wants_start_minimized`.
                i += 1;
            }
            other if other.starts_with("qobee://") => {
                if let Some(cmd) = parse_deep_link(other) {
                    out.push(cmd);
                }
                i += 1;
            }
            // Treat anything else that looks like a path as an
            // implicit `--play`. Windows file associations and
            // Explorer drag-onto-icon both fall in this bucket.
            other if !other.starts_with('-') => {
                bare_paths.push(PathBuf::from(other));
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    if !bare_paths.is_empty() {
        out.push(AppCommand::Play { paths: bare_paths });
    }
    out
}

/// Read a `qobee://` deep link.
///
/// Recognised forms:
///
/// * `qobee://play?path=...`            (path may repeat)
/// * `qobee://enqueue?path=...`
/// * `qobee://play-next?path=...`
/// * `qobee://play-folder?path=...`
/// * `qobee://enqueue-folder?path=...`
/// * `qobee://import-folder?path=...`
/// * `qobee://scan-folder?path=...`
/// * `qobee://library`
/// * `qobee://settings`
/// * `qobee://home`
/// * `qobee://queue`
pub fn parse_deep_link(url: &str) -> Option<AppCommand> {
    let rest = url.strip_prefix("qobee://")?;
    let (host, query) = match rest.find('?') {
        Some(pos) => (&rest[..pos], Some(&rest[pos + 1..])),
        None => (rest, None),
    };
    // Strip any trailing path so `qobee://play/` and `qobee://play`
    // both work.
    let host = host.trim_end_matches('/');
    let paths: Vec<PathBuf> = query
        .map(|q| {
            q.split('&')
                .filter_map(|kv| {
                    let (k, v) = kv.split_once('=')?;

                    if k.eq_ignore_ascii_case("path") {
                        Some(PathBuf::from(percent_decode(v)))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let single = paths.first().cloned();
    match host {
        "play" => {
            if paths.is_empty() {
                None
            } else {
                Some(AppCommand::Play { paths })
            }
        }
        "enqueue" => Some(AppCommand::Enqueue { paths }),
        "play-next" => Some(AppCommand::PlayNext { paths }),
        "play-folder" => single.map(|p| AppCommand::PlayFolder { path: p }),
        "enqueue-folder" => single.map(|p| AppCommand::EnqueueFolder { path: p }),
        "import-folder" => single.map(|p| AppCommand::ImportFolder { path: p }),
        "scan-folder" => single.map(|p| AppCommand::ScanFolder { path: p }),
        "library" => Some(AppCommand::Navigate {
            view: NavigateTarget::Library,
        }),
        "settings" => Some(AppCommand::Navigate {
            view: NavigateTarget::Settings,
        }),
        "queue" => Some(AppCommand::Navigate {
            view: NavigateTarget::Queue,
        }),
        "home" => Some(AppCommand::Navigate {
            view: NavigateTarget::Home,
        }),
        _ => None,
    }
}

/// Returns true if any arg is `--minimized` or `--start-minimized`.
/// Used by the main entry to decide whether to show the window at
/// startup.
pub fn wants_start_minimized(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--minimized" || a == "--start-minimized")
}

// ---------------------------------------------------------------------------
// Argument helpers.
// ---------------------------------------------------------------------------

fn take_path_run(args: &[String], i: &mut usize) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    *i += 1;
    while *i < args.len() {
        let a = &args[*i];
        if a.starts_with("--") || a.starts_with("qobee://") {
            break;
        }
        paths.push(PathBuf::from(a));
        *i += 1;
    }
    paths
}

fn take_single_path(args: &[String], i: &mut usize) -> Option<PathBuf> {
    *i += 1;
    if *i < args.len() {
        let p = PathBuf::from(&args[*i]);
        *i += 1;
        Some(p)
    } else {
        None
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Windows-specific implementations (registry + AUMID).
// ---------------------------------------------------------------------------

/// AppUserModelID used everywhere on the Windows side. Must match the
/// installer-side configuration so the taskbar identifier stays
/// stable across builds.
pub const APP_USER_MODEL_ID: &str = "app.qobee.player";

/// Audio file extensions we register a context menu for. The list is
/// intentionally narrow: only formats the engine actually plays.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "ogg", "m4a", "aac", "opus", "alac", "aiff", "wv",
];

/// Names used as the registry "ProgID" / verb keys. We keep them
/// namespaced to "Qobee" so they never collide with the system file
/// associations.
pub const PROG_ID: &str = "Qobee.Music.AudioFile";
pub const FOLDER_VERB_KEY_PREFIX: &str = "QobeeFolder";

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use winreg::enums::*;
    use winreg::RegKey;

    /// Set the AppUserModelID on the current process. Falls back to
    /// `Ok(())` on non-Windows.
    pub fn set_app_user_model_id(id: &str) -> anyhow::Result<()> {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
        let mut wide: Vec<u16> = std::ffi::OsStr::new(id).encode_wide().collect();
        wide.push(0);
        unsafe {
            SetCurrentProcessExplicitAppUserModelID(PCWSTR(wide.as_ptr()))?;
        }
        Ok(())
    }

    pub fn set_autostart(enabled: bool, exe: &Path, start_minimized: bool) -> anyhow::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run, _) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
        if enabled {
            let mut value = format!("\"{}\"", exe.display());
            if start_minimized {
                value.push_str(" --minimized");
            }
            run.set_value("Qobee", &value)?;
        } else {
            // delete_value returns Err if it doesn't exist; treat
            // that as a non-error (idempotent disable).
            let _ = run.delete_value("Qobee");
        }
        Ok(())
    }

    pub fn is_autostart_enabled() -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        match hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run") {
            Ok(run) => run.get_value::<String, _>("Qobee").is_ok(),
            Err(_) => false,
        }
    }

    /// Register the protocol handler, the audio file context menu
    /// and the folder context menu. Each section is opt-in.
    pub fn register_integration(
        exe: &Path,
        audio_files: bool,
        folders: bool,
        protocol: bool,
    ) -> anyhow::Result<()> {
        if protocol {
            register_protocol_handler(exe)?;
        }
        if audio_files {
            register_file_context_menu(exe)?;
        }
        if folders {
            register_folder_context_menu(exe)?;
        }
        Ok(())
    }

    /// Mirror of [`register_integration`]; safe to call even if
    /// nothing was registered (each removal is idempotent).
    pub fn unregister_integration(
        audio_files: bool,
        folders: bool,
        protocol: bool,
    ) -> anyhow::Result<()> {
        if protocol {
            let _ = unregister_protocol_handler();
        }
        if audio_files {
            let _ = unregister_file_context_menu();
        }
        if folders {
            let _ = unregister_folder_context_menu();
        }
        Ok(())
    }

    /// `qobee://` -> `<exe> "%1"` under HKCU (no admin required).
    fn register_protocol_handler(exe: &Path) -> anyhow::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (root, _) = hkcu.create_subkey(r"Software\Classes\qobee")?;
        root.set_value("", &"URL:Qobee Protocol")?;
        root.set_value("URL Protocol", &"")?;
        let (icon, _) = root.create_subkey("DefaultIcon")?;
        icon.set_value("", &format!("\"{}\",0", exe.display()))?;
        let (cmd, _) = root.create_subkey(r"shell\open\command")?;
        cmd.set_value("", &format!("\"{}\" \"%1\"", exe.display()))?;
        Ok(())
    }

    fn unregister_protocol_handler() -> anyhow::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let _ = hkcu.delete_subkey_all(r"Software\Classes\qobee");
        Ok(())
    }

    /// File context menu — uses a soft (non-default) ProgID under
    /// HKCU so the user keeps their actual default audio app.
    fn register_file_context_menu(exe: &Path) -> anyhow::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        // 1. Create the ProgID with our verbs.
        let (progid, _) = hkcu.create_subkey(format!(r"Software\Classes\{PROG_ID}"))?;
        progid.set_value("", &"Qobee Music")?;

        let (default_icon, _) = progid.create_subkey("DefaultIcon")?;
        default_icon.set_value("", &format!("\"{}\",0", exe.display()))?;

        let (shell, _) = progid.create_subkey("shell")?;

        // Verbs: open / play / enqueue / play-next / import.
        for (verb_key, label, args) in &[
            ("open", "Open in Qobee", "\"%1\""),
            ("qobee.play", "Play in Qobee", "--play \"%1\""),
            ("qobee.enqueue", "Add to Qobee queue", "--enqueue \"%1\""),
            ("qobee.playnext", "Play next in Qobee", "--play-next \"%1\""),
            (
                "qobee.import",
                "Import to Qobee library",
                "--scan-folder \"%1\"",
            ),
        ] {
            let (verb, _) = shell.create_subkey(verb_key)?;
            verb.set_value("", label)?;
            verb.set_value("Icon", &format!("\"{}\",0", exe.display()))?;
            let (cmd, _) = verb.create_subkey("command")?;
            cmd.set_value("", &format!("\"{}\" {}", exe.display(), args))?;
        }

        // 2. Attach the ProgID as a soft handler on each audio
        //    extension via OpenWithProgids (does not change the
        //    default; just adds Qobee to the "Open with" list).
        for ext in AUDIO_EXTENSIONS {
            let key = format!(r"Software\Classes\.{ext}\OpenWithProgids");
            let (k, _) = hkcu.create_subkey(&key)?;
            k.set_value(PROG_ID, &"")?;
        }
        Ok(())
    }

    fn unregister_file_context_menu() -> anyhow::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let _ = hkcu.delete_subkey_all(format!(r"Software\Classes\{PROG_ID}"));
        for ext in AUDIO_EXTENSIONS {
            let key = format!(r"Software\Classes\.{ext}\OpenWithProgids");
            if let Ok(k) = hkcu.open_subkey_with_flags(&key, KEY_ALL_ACCESS) {
                let _ = k.delete_value(PROG_ID);
            }
        }
        Ok(())
    }

    /// Folder context menu — adds Qobee actions when right-clicking
    /// a Folder, Directory, or Directory\Background.
    fn register_folder_context_menu(exe: &Path) -> anyhow::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        let actions = &[
            ("PlayFolder", "Play folder in Qobee", "--play-folder \"%V\""),
            (
                "EnqueueFolder",
                "Add folder to Qobee queue",
                "--enqueue-folder \"%V\"",
            ),
            (
                "ScanFolder",
                "Scan folder with Qobee",
                "--scan-folder \"%V\"",
            ),
            (
                "ImportFolder",
                "Import folder to Qobee library",
                "--import-folder \"%V\"",
            ),
        ];
        // We register against `Directory\shell` (right-click on a
        // folder), `Directory\Background\shell` (right-click empty
        // space inside an open folder), and `Drive\shell` (right-
        // click a drive letter — useful for music drives).
        for parent in &[
            r"Directory\shell",
            r"Directory\Background\shell",
            r"Drive\shell",
        ] {
            for (suffix, label, args) in actions {
                let key = format!(
                    r"Software\Classes\{parent}\{prefix}.{suffix}",
                    prefix = FOLDER_VERB_KEY_PREFIX
                );
                let (verb, _) = hkcu.create_subkey(&key)?;
                verb.set_value("", label)?;
                verb.set_value("Icon", &format!("\"{}\",0", exe.display()))?;
                let (cmd, _) = verb.create_subkey("command")?;
                cmd.set_value("", &format!("\"{}\" {}", exe.display(), args))?;
            }
        }
        Ok(())
    }

    fn unregister_folder_context_menu() -> anyhow::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for parent in &[
            r"Directory\shell",
            r"Directory\Background\shell",
            r"Drive\shell",
        ] {
            for suffix in &["PlayFolder", "EnqueueFolder", "ScanFolder", "ImportFolder"] {
                let key = format!(
                    r"Software\Classes\{parent}\{prefix}.{suffix}",
                    prefix = FOLDER_VERB_KEY_PREFIX
                );
                let _ = hkcu.delete_subkey_all(&key);
            }
        }
        Ok(())
    }

    /// Populate the Windows taskbar / Start jumplist with the
    /// transport quick-actions promised by R7.2: Play/Pause, Next,
    /// Previous. Each item launches `qobee.exe --<verb>` so the
    /// secondary-instance plugin forwards it through
    /// `parse_args` → `commands_integration::dispatch`, hitting the
    /// same `PlayerHandle` methods as the in-app buttons.
    ///
    /// The whole call chain is fail-soft (R7.2 design note): any
    /// `windows_core::Error` from the WinRT API is logged at
    /// `warn` level and the function returns `Ok(())`, so the
    /// caller never has to special-case the "no jumplist" path.
    /// `JumpList::IsSupported` returns `false` on Windows 7 and
    /// inside some sandboxed AppContainer scenarios; we honour it.
    pub fn setup_jumplist() -> anyhow::Result<()> {
        use windows::core::HSTRING;
        use windows::UI::StartScreen::{JumpList, JumpListItem};
        match JumpList::IsSupported() {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!(
                    target: "qobee::win",
                    "JumpList not supported on this host; skipping setup"
                );
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(
                    target: "qobee::win",
                    error = %e,
                    "JumpList::IsSupported failed; skipping setup"
                );
                return Ok(());
            }
        }
        let list = match JumpList::LoadCurrentAsync().and_then(|op| op.get()) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(
                    target: "qobee::win",
                    error = %e,
                    "JumpList::LoadCurrentAsync failed; skipping setup"
                );
                return Ok(());
            }
        };
        let items = match list.Items() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    target: "qobee::win",
                    error = %e,
                    "JumpList::Items failed; skipping setup"
                );
                return Ok(());
            }
        };
        // Idempotent: clear any items we may have populated on a
        // previous launch so the order stays stable across version
        // bumps and label tweaks.
        let _ = items.Clear();
        let group = HSTRING::from("Lecture");
        for (args, label) in &[
            ("--play-pause", "Play / Pause"),
            ("--next", "Lire la suivante"),
            ("--previous", "Lire la précédente"),
        ] {
            let item = match JumpListItem::CreateWithArguments(
                &HSTRING::from(*args),
                &HSTRING::from(*label),
            ) {
                Ok(it) => it,
                Err(e) => {
                    tracing::warn!(
                        target: "qobee::win",
                        args = %args,
                        error = %e,
                        "JumpListItem::CreateWithArguments failed; skipping item"
                    );
                    continue;
                }
            };
            if let Err(e) = item.SetGroupName(&group) {
                tracing::debug!(
                    target: "qobee::win",
                    args = %args,
                    error = %e,
                    "JumpListItem::SetGroupName failed; continuing"
                );
            }
            if let Err(e) = items.Append(&item) {
                tracing::warn!(
                    target: "qobee::win",
                    args = %args,
                    error = %e,
                    "JumpList items.Append failed; skipping item"
                );
            }
        }
        if let Err(e) = list.SaveAsync().and_then(|op| op.get()) {
            tracing::warn!(
                target: "qobee::win",
                error = %e,
                "JumpList::SaveAsync failed; jumplist may be stale"
            );
        }
        Ok(())
    }

    /// Make sure a Start Menu shortcut for the running executable
    /// exists with our `AppUserModelID` set in its property store.
    ///
    /// Why we need it: Windows looks up SMTC ("Now Playing"
    /// flyout), the taskbar tooltip and the right-click jumplist
    /// header by reverse-mapping the AUMID we set on the process to
    /// a registered shortcut. Without a `.lnk` carrying
    /// `System.AppUserModel.ID = app.qobee.player`, the SMTC
    /// flyout shows "Unknown app" (visible in dev builds) and the
    /// taskbar context menu doesn't display the app name.
    ///
    /// The shortcut lives under the user's Programs folder
    /// (`%APPDATA%\Microsoft\Windows\Start Menu\Programs\Qobee.lnk`)
    /// so we never touch HKLM and an uninstaller can wipe it
    /// without admin rights. Idempotent: if the shortcut already
    /// points at the same target with the same AUMID we return
    /// early; otherwise we (re-)write it.
    ///
    /// Fail-soft: any COM/IO error is logged at warn level and the
    /// function returns `Ok(())` — a missing shortcut is annoying
    /// but never blocks playback.
    pub fn ensure_start_menu_shortcut(exe: &Path, aumid: &str) -> anyhow::Result<()> {
        use std::ffi::OsStr;
        use windows::core::{Interface, GUID, PCWSTR};
        use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        use windows::Win32::Foundation::PROPERTYKEY;

        let programs = match dirs::data_dir() {
            Some(d) => d.join(r"Microsoft\Windows\Start Menu\Programs"),
            None => {
                tracing::warn!(
                    target: "qobee::win",
                    "could not resolve %APPDATA%; skipping Start Menu shortcut"
                );
                return Ok(());
            }
        };
        if let Err(e) = std::fs::create_dir_all(&programs) {
            tracing::warn!(
                target: "qobee::win",
                path = %programs.display(),
                error = %e,
                "could not create Programs folder; skipping shortcut"
            );
            return Ok(());
        }
        let lnk = programs.join("Qobee.lnk");

        // System.AppUserModel.ID — fmtid {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, pid 5.
        // Hand-built so we don't have to pull a separate windows-feature for
        // PSGetPropertyKeyFromName.
        const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
            fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3),
            pid: 5,
        };

        // SAFETY: this whole helper is single-threaded; we initialize
        // COM apartment-threaded, drive the shell COM objects, then
        // CoUninitialize on the way out.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let result: anyhow::Result<()> = (|| {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;

                let exe_w = encode_wide(exe.as_os_str());
                link.SetPath(PCWSTR(exe_w.as_ptr()))?;
                if let Some(parent) = exe.parent() {
                    let parent_w = encode_wide(parent.as_os_str());
                    link.SetWorkingDirectory(PCWSTR(parent_w.as_ptr()))?;
                }
                let desc_w = encode_wide(OsStr::new("Qobee Music Player"));
                link.SetDescription(PCWSTR(desc_w.as_ptr()))?;
                // The exe carries the icon resource at index 0.
                link.SetIconLocation(PCWSTR(exe_w.as_ptr()), 0)?;

                // Tag the shortcut with our AUMID so SMTC and
                // friends can map our process back to it.
                let store: IPropertyStore = link.cast()?;
                let pv = PROPVARIANT::from(aumid);
                store.SetValue(&PKEY_APP_USER_MODEL_ID, &pv)?;
                store.Commit()?;

                // Persist to disk.
                let persist: IPersistFile = link.cast()?;
                let lnk_w = encode_wide(lnk.as_os_str());
                persist.Save(PCWSTR(lnk_w.as_ptr()), true)?;
                Ok(())
            })();
            CoUninitialize();
            if let Err(e) = result {
                tracing::warn!(
                    target: "qobee::win",
                    error = %e,
                    "creating Start Menu shortcut failed; SMTC may show 'Unknown app'"
                );
            } else {
                tracing::info!(
                    target: "qobee::win",
                    path = %lnk.display(),
                    "Start Menu shortcut ensured for AUMID lookup"
                );
            }
        }
        Ok(())
    }

    fn encode_wide(s: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(std::iter::once(0)).collect()
    }
}

// ---------------------------------------------------------------------------
// Public façade — same signatures on every platform so the rest of
// the app doesn't need cfgs.
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub use imp::{
    ensure_start_menu_shortcut, is_autostart_enabled, register_integration, set_app_user_model_id,
    set_autostart, setup_jumplist, unregister_integration,
};

#[cfg(not(target_os = "windows"))]
pub fn set_app_user_model_id(_id: &str) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn set_autostart(
    _enabled: bool,
    _exe: &std::path::Path,
    _start_minimized: bool,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn is_autostart_enabled() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn register_integration(
    _exe: &std::path::Path,
    _audio_files: bool,
    _folders: bool,
    _protocol: bool,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn unregister_integration(
    _audio_files: bool,
    _folders: bool,
    _protocol: bool,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn setup_jumplist() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn ensure_start_menu_shortcut(_exe: &std::path::Path, _aumid: &str) -> anyhow::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_play_arg() {
        let args = vec!["qobee.exe".into(), "--play".into(), "C:/a.flac".into()];
        let cmds = parse_args(&args);
        assert_eq!(
            cmds,
            vec![AppCommand::Play {
                paths: vec![PathBuf::from("C:/a.flac")]
            }]
        );
    }

    #[test]
    fn parse_bare_path_treated_as_play() {
        let args = vec!["qobee.exe".into(), "C:/song.mp3".into()];
        let cmds = parse_args(&args);
        assert_eq!(
            cmds,
            vec![AppCommand::Play {
                paths: vec![PathBuf::from("C:/song.mp3")]
            }]
        );
    }

    #[test]
    fn parse_multiple_actions() {
        let args = vec![
            "qobee.exe".into(),
            "--enqueue".into(),
            "a.flac".into(),
            "b.flac".into(),
            "--play".into(),
            "c.flac".into(),
        ];
        let cmds = parse_args(&args);
        assert_eq!(cmds.len(), 2);
        assert!(matches!(&cmds[0], AppCommand::Enqueue { paths } if paths.len() == 2));
        assert!(matches!(&cmds[1], AppCommand::Play { paths } if paths.len() == 1));
    }

    #[test]
    fn parse_navigate_flag() {
        let args = vec!["qobee.exe".into(), "--open-settings".into()];
        let cmds = parse_args(&args);
        assert_eq!(
            cmds,
            vec![AppCommand::Navigate {
                view: NavigateTarget::Settings
            }]
        );
    }

    #[test]
    fn parse_transport_flags() {
        // R7.2 jumplist verbs and macOS Dock items dispatch through
        // the same parser; make sure each one yields a single
        // dedicated `AppCommand`.
        assert_eq!(
            parse_args(&["qobee.exe".into(), "--play-pause".into()]),
            vec![AppCommand::PlayPause]
        );
        assert_eq!(
            parse_args(&["qobee.exe".into(), "--next".into()]),
            vec![AppCommand::Next]
        );
        assert_eq!(
            parse_args(&["qobee.exe".into(), "--previous".into()]),
            vec![AppCommand::Previous]
        );
        // Aliases — `--prev` and `--toggle-play-pause` keep us
        // forward-compatible with anything an external caller might
        // send.
        assert_eq!(
            parse_args(&["qobee.exe".into(), "--prev".into()]),
            vec![AppCommand::Previous]
        );
        assert_eq!(
            parse_args(&["qobee.exe".into(), "--toggle-play-pause".into()]),
            vec![AppCommand::PlayPause]
        );
    }

    #[test]
    fn parse_deep_link_play() {
        let cmd = parse_deep_link("qobee://play?path=C%3A%2Fsong.flac").unwrap();
        assert_eq!(
            cmd,
            AppCommand::Play {
                paths: vec![PathBuf::from("C:/song.flac")]
            }
        );
    }

    #[test]
    fn parse_deep_link_navigate() {
        assert_eq!(
            parse_deep_link("qobee://library").unwrap(),
            AppCommand::Navigate {
                view: NavigateTarget::Library
            }
        );
        assert_eq!(
            parse_deep_link("qobee://settings").unwrap(),
            AppCommand::Navigate {
                view: NavigateTarget::Settings
            }
        );
    }

    #[test]
    fn parse_deep_link_unknown_returns_none() {
        assert!(parse_deep_link("qobee://unknown").is_none());
    }

    #[test]
    fn wants_start_minimized_detects_flag() {
        assert!(wants_start_minimized(&[
            "qobee".into(),
            "--minimized".into()
        ]));
        assert!(!wants_start_minimized(&["qobee".into()]));
    }

    #[test]
    fn percent_decode_handles_common_cases() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("C%3A%2Fmusic"), "C:/music");
    }
}
