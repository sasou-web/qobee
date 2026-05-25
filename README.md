# Qobee

A local audio player for Windows, focused on fidelity. Indexes one or
more music folders, plays FLAC and other lossless formats through the
OS mixer, and is built so that a future bit-perfect output path can
drop in without rewriting the rest of the app.

The current build delivers a complete listening experience: scanning,
albums / artists / genres / playlists / favorites, search, queue
management, 10-band equalizer, ReplayGain, gapless playback,
sample-accurate seek, lyrics (synced + plain), a mini player,
themes, accent following the cover, OS media keys, and Discord
Rich Presence. The audio engine ships with two output backends —
**Shared** (default, OS mixer) and **WASAPI Exclusive** (opt-in,
strictly bit-perfect when the device cooperates). See
"Bit-perfect honesty" below for the truth about each chain.

## Concept

- Local-first: no streaming, no cloud, no media server.
- FLAC priority, other lossless formats next.
- No tag editing, no media-server features, no library lock-in.
- Sober, dark UI inspired by Cider (sidebar, album grid, artist list,
  album detail, track list, persistent player bar).
- Audio engine designed up front to deliver clean Shared output
  with a real bit-perfect path available via WASAPI Exclusive on
  Windows.

## Bit-perfect honesty

Qobee will tell you the truth about its current playback chain rather
than ship a misleading badge.

- The default backend, **Shared (CpalSharedEngine)**, talks to WASAPI
  in shared mode on Windows. The OS mixer can resample, dither,
  apply effects, and mix Qobee's stream with other apps. **It is not
  bit-perfect.** The player bar reports `Bit-perfect: false` on
  this backend even when the in-app DSP is at unity.
- The opt-in backend, **WASAPI Exclusive (WasapiExclusiveEngine)**,
  bypasses the OS mixer and reaches a strictly bit-perfect chain
  *only* when the device natively accepts the source's sample rate
  and channel count, the EQ is flat, ReplayGain is at unity and the
  volume is at 100 %. Any departure from that — resampling, upmix to
  a surround layout because the OS is configured that way, an EQ
  band that isn't 0 dB, a volume slider that isn't at the top —
  flips `is_bit_perfect` back to `false`. While Exclusive is active
  no other application can play to the same device.
- Software volume, the EQ and ReplayGain are honored on both paths
  because users still want them; the badge tracks them honestly.

## Stack

- **Rust** workspace with separate crates for the audio engine,
  library indexing and orchestration:
  - `crates/engine` — Symphonia (decoding), CPAL (Shared output),
    biquad EQ, future WASAPI Exclusive seam.
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

Pinned versions:

| Component                    | Version   |
| ---------------------------- | --------- |
| `tauri`                      | 2.11      |
| `tauri-plugin-dialog`        | 2.2       |
| `symphonia`                  | 0.5.5     |
| `cpal`                       | 0.17      |
| `wasapi` (Windows-only)      | 0.23      |
| `windows` (Windows-only)     | 0.62      |
| `lofty`                      | 0.24      |
| `rusqlite` (bundled)         | 0.39      |
| `notify`                     | 8.2       |
| `crossbeam-channel`          | 0.5       |
| `rtrb`                       | 0.3       |
| `rubato`                     | 0.16      |
| `blake3`                     | 1.5       |
| `discord-rich-presence`      | 1.1       |
| `reqwest` (rustls, blocking) | 0.12      |
| `svelte`                     | 5.20      |
| `vite`                       | 6.1       |
| `typescript`                 | 5.7       |
| `@tauri-apps/api`            | 2.4       |

## Architecture notes

- **PCM buffer is an enum, not just `Vec<f32>`.** The engine exposes
  `F32Interleaved`, `I16Interleaved`, `I24In32Interleaved` and
  `I32Interleaved`. Only `F32Interleaved` is produced today, but the
  integer variants are part of the public API so a future
  bit-perfect path can plug in without breaking call sites.
- **SPSC ring** between decoder and audio callback uses `rtrb`. The
  current ring carries `f32`; an integer ring will be added
  alongside the Exclusive backend.
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
- **No silent resampling.** When the device's sample rate differs
  from the source, the engine logs an explicit warning and the
  player state continues to report `is_bit_perfect: false`.
- **Smooth UI under audio cadence.** Position events fire at decoder
  cadence, not per audio frame, and the Discord and MediaSession
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
| `get_output_mode` / `set_output_mode` | Auto / Shared (Exclusive reserved)             |
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

Equalizer / settings / maintenance / Discord:

| Command                          | Description                                         |
| -------------------------------- | --------------------------------------------------- |
| `get_eq_gains` / `set_eq_gains`  | 10-band peaking EQ (±12 dB)                         |
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

- Rust stable (≥ 1.85)
- Node.js (≥ 20)
- On Windows, the **WebView2 runtime** (preinstalled on Windows 11; on
  Windows 10 install from Microsoft if missing).

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
- Library scan extracts FLAC / MP3 / WAV / M4A / AAC / ALAC / OGG /
  Vorbis / Opus tags and embedded covers via `lofty`.
- Multi-root library: add as many folders as you want, scan each or
  rescan all in one pass.
- SQLite library lives at `%APPDATA%\Qobee\library.sqlite3`.
  Covers are cached at `%APPDATA%\Qobee\covers\` and served via
  `qobee-cover://`. Logs land in `%APPDATA%\Qobee\logs\` with
  daily rotation.
- Decoding via Symphonia, output via CPAL Shared, output device
  selectable from Settings.
- Play / pause / resume / next / prev, seek (Coarse), software
  volume in Shared mode.
- Queue: play next, add to queue, reorder, remove, jump to,
  inspect from a popover.
- Playlists (create / rename / delete / add / remove / play from
  any track) and favorites (per-track, global Favorites screen).
- Repeat (off / track / queue), shuffle, and endless playback.
- 10-band peaking EQ at ISO octave centers (Q=1.0, ±12 dB), with
  presets and an automatic bypass when every band is at 0 dB.
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
- The player bar always shows the effective output mode and a
  truthful bit-perfect badge.

## What is intentionally not in yet

- **Crossfade between tracks.** The engine supports gapless
  transitions in place; crossfade would need a second active stream
  in parallel. Wired-in seam exists, no UI yet.
- **Tray icon.** Closing the window today exits Qobee. A tray icon
  would let the player keep running in the background.
- **Global hotkeys.** Media keys / Bluetooth / lock-screen controls
  work; OS-wide hotkeys (Play/Pause from any focused app) do not.
- **Scrobbling (Last.fm / ListenBrainz).** Recently-played is
  recorded in the local library only.
- **Tag editing.** Qobee is read-only on the audio files.
- **BASS backend.** The `engine-bass` Cargo feature exists as a
  seam; the module is a stub with no external dependency.

## Roadmap

- Crossfade.
- Tray icon and global hotkeys.
- Smart playlists (filter / sort presets).
- Scrobbling (Last.fm / ListenBrainz).
- Visualizer.
- BASS backend behind the `engine-bass` Cargo feature.

## License

Dual-licensed under MIT or Apache-2.0. See the workspace `Cargo.toml`.
