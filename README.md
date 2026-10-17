# Qobee

A simple, fast local audio player for Windows and macOS. Indexes one
or more music folders and plays your library through a small, stable
playback engine built on `rodio` (RustAudio).

The current build delivers a complete listening experience: scanning,
albums / artists / genres / playlists / favorites, search, queue
management, sample-accurate seek, lyrics (synced + plain), a mini
player, themes, accent following the cover, OS media keys, and Discord
Rich Presence. The audio engine is intentionally minimal: it decodes
with Symphonia, outputs through CPAL, and lets you pick the output
device.

## Concept

- Local-first: no streaming, no cloud, no media server.
- Broad local format support (FLAC, MP3, AAC/M4A, ALAC, WAV, OGG,
  Vorbis, MKV).
- No tag editing, no media-server features, no library lock-in.
- Sober, dark UI inspired by Cider (sidebar, album grid, artist list,
  album detail, track list, persistent player bar).
- Simple, maintainable audio engine: rock-solid play / pause / seek /
  volume / queue over fancy DSP.

## Stack

- **Rust** workspace with separate crates for the audio engine,
  library indexing and orchestration:
  - `crates/engine` — a thin `rodio`-backed playback core
    (decoding via Symphonia, output via CPAL, volume, seek, source
    queuing) plus an output-device picker.
  - `crates/library` — `walkdir` + `lofty` for tag extraction,
    `rusqlite` (bundled) for storage, embedded covers cached on
    disk, FTS-style search, settings table.
  - `crates/core` — playback queue (cursor + history),
    repeat / shuffle / endless logic, orchestration between engine
    and library.
- **Tauri 2** desktop shell in `src-tauri/`, including a Discord
  Rich Presence worker and a cover-hosting worker (background
  uploads to a public host so Discord can render local album art).
- **Svelte 5** + **TypeScript strict** + **Vite** frontend in `ui/`.

### Audio engine

The engine is a small wrapper around `rodio`, which itself uses
Symphonia (decoding) and CPAL (output). It owns loading, transport
(play / pause / resume / stop / seek), volume, and an output-device
picker — nothing more. There is no EQ, ReplayGain, crossfeed,
convolver, dither, limiter, DSD pipeline, output-mode switching,
bit-perfect badge, or null-test diagnostic; those were removed in
favour of a simpler, faster, more maintainable core.

Volume maps the slider position linearly to the output gain (0 → mute,
1 → unity). Gapless / crossfade is out of scope: a small inter-track
gap is expected.

Pinned versions:

| Component                    | Version   |
| ---------------------------- | --------- |
| `tauri`                      | 2.11      |
| `tauri-plugin-dialog`        | 2.2       |
| `rodio`                      | 0.21      |
| `symphonia` (via rodio)      | 0.5.5     |
| `cpal` (via rodio)           | 0.16      |
| `lofty`                      | 0.24      |
| `rusqlite` (bundled)         | 0.39      |
| `notify`                     | 8.2       |
| `crossbeam-channel`          | 0.5       |
| `blake3`                     | 1.5       |
| `discord-rich-presence`      | 1.1       |
| `reqwest` (rustls, blocking) | 0.12      |
| `svelte`                     | 5.20      |
| `vite`                       | 6.1       |
| `typescript`                 | 5.7       |
| `@tauri-apps/api`            | 2.11      |

## Architecture notes

- **Cover art never crosses the IPC boundary** for the UI. Embedded
  pictures are written to a sharded on-disk cache
  (`%APPDATA%\Qobee\covers\<aa>\<hash>.<ext>`) and exposed to the
  webview via the custom `qobee-cover://` URI scheme. The frontend
  loads them with plain `<img>` tags.
- **Local covers reach Discord through a separate worker.** Discord
  requires a public HTTPS URL or a registered asset key, neither of
  which fits a local file. A dedicated upload worker pushes each
  cover once to a public host (litterbox.catbox.moe), memoizes the
  URL in the settings table, and patches the active activity in
  place. The Discord IPC worker is never blocked on the network.
- **Drive playback.** Tracks indexed from a Google Drive source use
  a `drv://<source_id>/<file_id>` path; the engine feeds rodio's
  decoder an HTTP byte-range media source for those.
- **Smooth UI under audio cadence.** Position events fire on a short
  poll, not per audio frame, and the Discord and MediaSession
  layers coalesce same-bucket updates so the elapsed bar never
  flickers and Discord's rate limit is never tripped.

## Layout

```
Qobee/
├── Cargo.toml                     # workspace
├── crates/
│   ├── engine/                    # AudioEngine + backends + EQ
│   ├── library/                   # scan + DB + cover cache + settings
│   └── core/                      # queue + player FSM + history
├── src-tauri/                     # Tauri shell
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   ├── icons/
│   └── src/
│       ├── main.rs
│       ├── lib.rs                 # builder + qobee-cover:// protocol
│       ├── commands.rs            # Tauri command surface
│       ├── state.rs               # AppState (library + player + discord)
│       ├── discord.rs             # Discord Rich Presence worker
│       └── cover_host.rs          # async cover uploader for Discord
└── ui/                            # Svelte 5 + TS strict
    ├── package.json
    ├── vite.config.ts
    ├── svelte.config.js
    ├── tsconfig.json
    ├── index.html
    └── src/
        ├── main.ts
        ├── App.svelte
        ├── components/            # Sidebar, AlbumGrid, PlayerBar, …
        ├── lib/
        │   ├── api.ts             # typed wrappers around invoke()
        │   ├── stores.svelte.ts   # reactive app state
        │   ├── settings.svelte.ts # persisted preferences
        │   ├── accent.svelte.ts   # cover-driven accent color
        │   ├── discordPresence.ts # Discord singleton
        │   ├── mediaSession.ts    # OS media keys
        │   └── …                  # context menu, palette, format, etc.
        └── styles/global.css
```

## Tauri commands

All commands live in `src-tauri/src/commands.rs` and are mirrored 1:1
in `ui/src/lib/api.ts` (typed wrappers).

Library / browsing:

| Command                          | Description                                         |
| -------------------------------- | --------------------------------------------------- |
| `scan_library`                   | Index a folder, emit progress events                |
| `scan_all_roots`                 | Rescan every saved library root in sequence         |
| `list_albums` / `list_artists`   | Album grid / artist list data                       |
| `get_album`                      | One album with all its tracks                       |
| `get_track`                      | Resolve a single track by ID                        |
| `get_tracks`                     | Resolve many ids → full Track records               |
| `get_artist_detail`              | Artist page (singles, EPs, albums, kind, length)    |
| `list_genres` / `list_albums_by_genre` | Genre browsing                                |
| `search`                         | Title / artist / album search                       |
| `recently_played_tracks` / `..._albums` / `..._artists` | Home page data           |
| `library_stats`                  | Counts + DB / cover cache size                      |
| `list_library_roots` / `add_library_root` / `remove_library_root` | Multi-root config |
| `track_ids_for_album`            | Track ids in album order                            |
| `track_ids_by_artist`            | Track ids of an artist's full catalog               |
| `album_id_for_track`             | Reverse lookup for the player bar                   |
| `album_size_bytes`               | On-disk size of an album                            |

Playback:

| Command                          | Description                                         |
| -------------------------------- | --------------------------------------------------- |
| `play_track`                     | Load and play a single track                        |
| `play_album_from_track`          | Start an album at a given track                     |
| `play_playlist_from_track`       | Start a playlist at a given track                   |
| `play_tracks`                    | Play an arbitrary id list, optional start index     |
| `play_random_album`              | Pick and play a random album                        |
| `pause` / `resume`               | Engine pause / resume                               |
| `seek`                           | Seek to `position_seconds` (Coarse seek)            |
| `set_volume`                     | Software volume (Shared mode)                       |
| `next` / `prev`                  | Queue navigation                                    |
| `get_player_state`               | Snapshot of `PlayerState`                           |
| `list_output_devices` / `get_selected_output_device` / `set_output_device` | Output device picker |
| `get_repeat_mode` / `set_repeat_mode` | Off / track / queue                            |
| `get_shuffle` / `set_shuffle`    | Shuffle toggle                                      |
| `get_endless` / `set_endless`    | Endless playback                                    |

Queue:

| Command                          | Description                                         |
| -------------------------------- | --------------------------------------------------- |
| `get_queue`                      | Current items + cursor                              |
| `play_next` / `add_to_queue`     | Insert after current / append                       |
| `queue_remove_at` / `queue_move` / `queue_jump_to` | Mutation                          |

Playlists / favorites:

| Command                          | Description                                         |
| -------------------------------- | --------------------------------------------------- |
| `list_playlists` / `get_playlist` | Playlist index / detail                            |
| `create_playlist` / `rename_playlist` / `delete_playlist` | Lifecycle                |
| `add_to_playlist` / `remove_from_playlist` | Track membership                          |
| `add_favorite` / `remove_favorite` / `is_favorite` | Per-track favorite               |
| `list_favorite_track_ids` / `list_favorites` | Favorites screen data                   |

Settings / maintenance / Discord:

| Command                          | Description                                         |
| -------------------------------- | --------------------------------------------------- |
| `get_setting` / `set_setting` / `list_settings` / `clear_settings` | Key/value preferences |
| `clear_history` / `clear_cover_cache` / `reset_library` | Maintenance                  |
| `discord_init`                   | Boot the Rich Presence worker                       |
| `discord_set_client_id`          | Configure the Discord Application ID                |
| `discord_status`                 | Connection state for the UI badge                   |
| `discord_update_track` / `discord_set_paused` / `discord_clear_presence` | Push state |

Events emitted to the UI:

| Topic                    | Payload                                                 |
| ------------------------ | ------------------------------------------------------- |
| `player:state`           | `PlayerEvent::StateChanged { state }`                   |
| `player:position`        | `PlayerEvent::Position { position_seconds }`            |
| `player:end-of-track`    | `PlayerEvent::EndOfTrack`                               |
| `player:error`           | `PlayerEvent::Error { message }`                        |
| `library:scan-progress`  | `ScanProgress { files_visited, files_indexed, current }`|
| `library:scan-finished`  | `ScanResult { files_visited, files_indexed, errors }`   |

## Running in development

Prerequisites:

- Rust stable (≥ 1.85). The exact toolchain is pinned in
  `rust-toolchain.toml` (currently 1.96.0); rustup installs and
  selects it automatically inside the workspace, so your local
  `rustfmt` / `clippy` match CI. CI pins the same version, so the
  "passes locally, fails in CI" drift can't happen.
- Node.js (≥ 20)
- On Windows, the **WebView2 runtime** (preinstalled on Windows 11; on
  Windows 10 install from Microsoft if missing).

One-time setup — enable the git pre-push hook so `cargo fmt --check`
and `cargo clippy -D warnings` run before every push (the two gates
most prone to toolchain drift):

```powershell
./scripts/setup-hooks.ps1   # macOS/Linux: ./scripts/setup-hooks.sh
```

Bypass a single push with `git push --no-verify` if you really need
to.

Steps:

```powershell
# 1. install JS dependencies (only needed once, and after package.json changes)
cd ui
npm install
cd ..

# 2. start the dev app — Tauri spins up vite for the frontend automatically
cargo run -p qobee-app
```

If you prefer the official Tauri CLI workflow, run it from the
workspace root (Tauri 2's CLI looks for `tauri.conf.json` in
subfolders only, so it must see both `ui/` and `src-tauri/` as
siblings beneath its cwd):

```powershell
cd ui
npm install
cd ..
.\ui\node_modules\.bin\tauri.cmd dev
```

(The `tauri.conf.json` points `frontendDist` at `../ui/dist` and `devUrl`
at `http://localhost:1420`, which is what `npm run dev` serves.)

## Building for release

```powershell
cd ui
npm install
npm run build
cd ..
cargo build -p qobee-app --release
```

The bundled installer / `.exe` lands under `target/release/`.

For a fully bundled installer (MSI, NSIS, etc.) run the Tauri CLI
from the **workspace root** (not from `ui/`) so it can locate the
`tauri.conf.json` in `src-tauri/`:

```powershell
cd ui
npm install
cd ..
.\ui\node_modules\.bin\tauri.cmd build
```

The MSI / NSIS installers land under `src-tauri/target/release/bundle/`.

## What works today

- Workspace compiles without warnings; full Vite + Tauri build is clean.
- App launches with sidebar, home, album grid, artist list, artist
  detail, album detail, genres, playlists, favorites, search,
  settings and a persistent player bar.
- **Onboarding** when the library is empty: a welcome screen walks
  the user through picking a music folder, with the same flow
  available later from Settings → Library.
- **Drag-and-drop** a folder anywhere on the window to add it as a
  library root and trigger a scan.
- **Resizable, collapsible sidebar** like Apple Music / Arc: drag
  the right edge to resize, drag below 140 px to collapse to icon-
  only mode. Width and collapsed state are persisted in
  localStorage.
- **System tray icon** (Windows) with right-click menu for show /
  focus, play / pause, prev / next, library, settings, quit. Left
  click brings the window forward.
- **Windows desktop integration** (opt-in per item in Settings →
  Windows Integration): autostart at login, start minimized,
  minimize-to-tray on close, audio file context menu (`Play in
  Qobee`, `Add to Qobee queue`, `Play next`, `Import to library`),
  folder context menu (`Play folder`, `Add folder to queue`, `Scan
  folder`, `Import folder`), `qobee://` protocol handler. Every
  registry write goes to HKCU only; Qobee never claims the system
  default and never modifies machine-wide associations.
- **Single-instance + deep-link forwarding**: launching `qobee.exe
  --play foo.flac` while the app is already running sends the
  command to the existing window instead of spawning a duplicate.
  Same path for `qobee://` deep links.
- Library scan extracts FLAC / MP3 / WAV / M4A / AAC / ALAC / OGG /
  Vorbis / Opus tags and embedded covers via `lofty`.
- Multi-root library: add as many folders as you want, scan each or
  rescan all in one pass.
- SQLite library lives at `%APPDATA%\Qobee\library.sqlite3`.
  Covers are cached at `%APPDATA%\Qobee\covers\` and served via
  `qobee-cover://`. Logs land in `%APPDATA%\Qobee\logs\` with
  daily rotation.
- Decoding via Symphonia, output via CPAL (through rodio), output
  device selectable from Settings with automatic fallback to the
  system default when the chosen device is unavailable.
- Play / pause / resume / next / prev, seek, and software volume
  (linear slider-to-gain mapping).
- Queue: play next, add to queue, reorder, remove, jump to,
  inspect from a popover.
- Playlists (create / rename / delete / add / remove / play from
  any track) and favorites (per-track, global Favorites screen).
- Repeat (off / track / queue), shuffle, and endless playback.
- Search across titles, artists and albums.
- Recently played tracks / albums / artists on the home page.
- Themes (system / dark / light), configurable accent color, and
  an opt-in "follow cover" mode that drives the accent from the
  current album art.
- **Now Playing fullscreen** view: click the cover in the player
  bar (or press `F`) to open an immersive page with large artwork,
  scrubbable progress, transport controls, and a heart toggle.
- **Keyboard shortcuts** when no input is focused:
  `Space` (play/pause), `←/→` (-5s / +5s), `n` / `p` /
  `Alt+←` / `Alt+→` (prev / next track), `F` (toggle Now Playing),
  `Esc` (close Now Playing or clear search), `Ctrl/Cmd+F`
  (focus search).
- **Toasts** for transient confirmations ("Added to favorites",
  "Playing next: 12 tracks") so the UI never falls back to native
  alert dialogs.
- **Window state persistence**: size, position and maximized state
  are restored on next launch via `tauri-plugin-window-state`.
- OS media keys / Bluetooth controls / Windows quick controls via
  the Media Session API (play, pause, next, prev, seekto, stop).
- Discord Rich Presence: title, artist, album, real cover art, and
  an elapsed/remaining timeline pushed in real time. The worker
  reconnects automatically when Discord starts/restarts and falls
  back silently when Discord is closed; pause hides the activity,
  resume restores it. Local cover hosting is **opt-in** for
  privacy: enable it from Settings → Integrations and the matching
  privacy notice explains exactly what gets uploaded and where.

## Migration from the audiophile engine

Earlier builds shipped an audiophile engine (WASAPI Exclusive / ASIO
output modes, a 10-band EQ, ReplayGain, crossfeed, an FFT convolver,
dither, a look-ahead limiter, balance/trim, a DSD pipeline, a
bit-perfect health badge and a null-test diagnostic). That stack was
removed in favour of the simple rodio-backed engine. On first launch
after the upgrade, the obsolete `audio.*` settings rows are cleaned up
automatically; all non-audio preferences (UI, output device,
integrations, session) are preserved.

## Windows integration

Qobee ships a real Windows desktop integration, opt-in per item from
**Settings → Windows Integration** so the user keeps control over
every shell hook. None of the hooks ever require admin privileges:
every registry write goes to `HKCU` only, and Qobee never claims to
be the system-wide default audio player.

### Surface

- **AppUserModelID** — `app.qobee.player`, set on the running
  process via `SetCurrentProcessExplicitAppUserModelID`. Windows
  uses it to group the taskbar icon, notifications, and (static)
  Jump List under a single identity that survives updates.
- **Single instance + argument forwarding** — built on
  `tauri-plugin-single-instance`. A second `qobee.exe` invocation
  forwards its `argv` to the running window and exits; there is
  never a duplicate process or window.
- **Tray icon** — left click brings the main window forward; right
  click opens a menu with `Show / focus`, `Play / Pause`,
  `Previous`, `Next`, `Open library`, `Settings`, `Quit`.
  Toggleable from Settings.
- **Close-to-tray** — when enabled, the window's close button
  hides the window instead of quitting; playback keeps running in
  the background.
- **Autostart** — registers (or removes) a value under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Qobee`,
  optionally with `--minimized`.
- **`qobee://` protocol handler** — registered as a soft handler
  under `HKCU\Software\Classes\qobee`. Examples:
  `qobee://play?path=C%3A%2Fmusic%2Fa.flac`,
  `qobee://enqueue?path=...`, `qobee://play-next?path=...`,
  `qobee://play-folder?path=...`, `qobee://import-folder?path=...`,
  `qobee://library`, `qobee://settings`.
- **Audio file context menu** — installs a soft `OpenWithProgids`
  hint on the 10 supported extensions
  (`.mp3 .flac .wav .ogg .m4a .aac .opus .alac .aiff .wv`) and
  attaches verbs `Open in Qobee`, `Play in Qobee`,
  `Add to Qobee queue`, `Play next in Qobee`,
  `Import to Qobee library`. Never replaces the default audio app.
- **Folder context menu** — adds `Play folder`, `Add folder to
  queue`, `Scan folder`, `Import folder` on
  `Directory\shell`, `Directory\Background\shell` (right-click empty
  space inside an open folder), and `Drive\shell` (right-click a
  drive letter).
- **Command-line arguments** — `--play <paths>`, `--enqueue
  <paths>`, `--play-next <paths>`, `--play-folder <path>`,
  `--enqueue-folder <path>`, `--scan-folder <path>`,
  `--import-folder <path>`, `--open-library`, `--open-settings`,
  `--minimized`. Bare paths are treated as `--play <path>`.

### Registry footprint (HKCU only)

| Key                                                                 | Purpose                                          |
| ------------------------------------------------------------------- | ------------------------------------------------ |
| `Software\Microsoft\Windows\CurrentVersion\Run\Qobee`               | Autostart entry.                                 |
| `Software\Classes\qobee\…`                                          | `qobee://` protocol handler.                     |
| `Software\Classes\Qobee.Music.AudioFile\…`                          | ProgID with the audio file verbs.                |
| `Software\Classes\.<ext>\OpenWithProgids\Qobee.Music.AudioFile`     | Soft "Open with" hint per audio extension.       |
| `Software\Classes\Directory\shell\QobeeFolder.<verb>`               | Folder verbs (right-click a folder).             |
| `Software\Classes\Directory\Background\shell\QobeeFolder.<verb>`    | Folder verbs (empty space inside a folder).      |
| `Software\Classes\Drive\shell\QobeeFolder.<verb>`                   | Folder verbs (right-click a drive).              |
| `Software\Qobee\WindowsIntegration`                                 | Cleanup anchor written by the installer.         |

The NSIS installer (`src-tauri/installer/qobee.nsh`) registers the
protocol + AUMID at install time and reverses every key listed above
on uninstall. Toggling shell hooks at runtime from the Settings
panel performs the equivalent registry writes through the
`set_windows_integration` Tauri command.

## What is intentionally not in yet

- **Crossfade between tracks.** The engine supports gapless
  transitions in place; crossfade would need a second active stream
  in parallel. Wired-in seam exists, no UI yet.
- **Global hotkeys.** Media keys / Bluetooth / lock-screen controls
  work; OS-wide hotkeys (Play/Pause from any focused app) do not.
- **Scrobbling (Last.fm / ListenBrainz).** Recently-played is
  recorded in the local library only.
- **Tag editing.** Qobee is read-only on the audio files.
- **BASS backend.** The `engine-bass` Cargo feature exists as a
  seam; the module is a stub with no external dependency.

## Roadmap

- Crossfade.
- Global hotkeys (system-wide Play/Pause from any focused app).
- Dynamic taskbar Jump List (the static one already follows the
  AppUserModelID).
- Smart playlists (filter / sort presets).
- Scrobbling (Last.fm / ListenBrainz).
- Visualizer.
- BASS backend behind the `engine-bass` Cargo feature.

## License

Dual-licensed under MIT or Apache-2.0. See the workspace `Cargo.toml`.
