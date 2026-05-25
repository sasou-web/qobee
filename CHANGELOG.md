# Changelog

All notable changes to Qobee are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows semantic versioning.

## [0.2.0] - 2026-05-25

A fidelity-first overhaul of the audio pipeline plus a handful of
features that were on the roadmap (gapless, sample-accurate seek,
WASAPI Exclusive, lyrics, mini player).

### Audio engine

- **Volume curve**: cubic → quadratic. The slider now lands close to
  what foobar2000 / MusicBee do: ~-12 dB at 50 %, ~-6 dB at 70 %.
- **Resampler**: rubato `FftFixedIn` → `SincFixedIn` with sinc length
  256, oversampling factor 256 and a Blackman-Harris² window. Cleaner
  transients and far less pre-ringing on every track that needs a
  rate conversion.
- **Output format negotiation**: walks `supported_output_configs` and
  prefers, in order, the source SR + matching channel count + F32,
  then I32, then I16. When the driver only advertises surround
  configurations (Sony Inzone, Razer Synapse, etc.) the engine
  forces a stereo stream config so the OS mixer adapts the channel
  count instead of running stereo through the device's surround
  virtualizer.
- **Soft-clip + TPDF dither are now conditional**. With EQ flat,
  ReplayGain at unity and an f32 / i32 device, the chain is a strict
  passthrough — no coloration on a clean setup.
- **Equal-power stereo → mono downmix** instead of dropping the
  right channel, plus per-frame source scratch sized to the actual
  channel count (no more 8-channel cap).
- **Underrun watcher**: the decoder thread observes the audio
  callback's underrun counter and emits a throttled warning, so
  dropouts stop being silent.
- **`bit_depth` propagated** to `PlayerState` so the UI can show
  16/24/32-bit faithfully.

### ReplayGain

- New columns `replaygain_track_db` / `replaygain_album_db` on the
  `tracks` table with idempotent ALTER TABLE migrations so existing
  databases pick them up without a wipe.
- Reads `REPLAYGAIN_TRACK_GAIN` / `REPLAYGAIN_ALBUM_GAIN` tags during
  scan via lofty.
- Off / track / album mode toggle in **Settings → Playback**, with
  the choice persisted in the settings table.

### Sample-accurate seek

- `SymphoniaDecoder::seek` switched from `SeekMode::Coarse` to
  `SeekMode::Accurate`. The demuxer still snaps to a keyframe but
  the decoder discards samples up to the exact requested timestamp.

### Gapless playback

- New `prepare_next` API on the `AudioEngine` trait. The
  orchestrator pre-opens the upcoming track 5 s before EOF and the
  decoder thread swaps it in place when the active stream hits
  end-of-stream — the resampler / EQ / volume ramp keep their state
  and the audio callback sees one continuous output.
- New `EngineEvent::GaplessTransition` lets the orchestrator
  advance the queue cursor and record history without issuing a
  fresh `Load`.
- Format mismatches (different SR or channel count) fall back to
  the standard EOT → Load path.

### WASAPI Exclusive (Windows)

- New `WasapiExclusiveEngine` (single-threaded COM worker) on top of
  wasapi-rs 0.23. Reuses the same Symphonia + sinc + EQ + RG
  pipeline as Shared.
- Format negotiation tries 24-in-32 → S32 → F32 → S16 across the
  source SR, common HD rates (192 / 176.4 / 96 / 88.2 kHz, then
  48 / 44.1) and finally the device mixformat. Reaches a strictly
  bit-perfect chain on devices that natively support the file's SR.
- New `OutputMode::Exclusive` and `EffectiveOutputMode::Exclusive`.
  `is_bit_perfect = true` is reported only when *every* element of
  the chain is at unity (volume, EQ, RG, no resample, no upmix).
- Auto-fallback to Shared on init failure with a soft toast and
  the persisted setting rewritten so the next launch doesn't fail
  the same way.
- Output mode dropdown in **Settings → Playback** (Auto / Shared /
  Exclusive), choice persisted across launches.

### Lyrics

- New Tauri command `get_lyrics(track_id)` reading, in order:
  - a sidecar `.lrc` file next to the audio file (synced),
  - the `Lyrics` tag embedded in the file via lofty (unsynced or LRC).
- The LRC parser handles `[mm:ss]`, `[mm:ss.x]`, `[mm:ss.xx]`,
  `[mm:ss.xxx]`, multi-timestamp lines and meta tags
  (`[ar:]`, `[ti:]`, `[length:]`).
- New `LyricsPanel` component inside Now Playing fullscreen with
  active-line highlight + smooth auto-scroll for synced lyrics, and
  a plain text fallback when only unsynced lyrics are available.

### Mini player

- New compact, always-on-top window (440×170, resizable) created on
  demand via `WebviewWindowBuilder`. Shows cover, title, artist,
  scrubbable progress, prev / play-pause / next, plus a pin and
  a restore button.
- Toggle from the player bar; opening the mini hides the main
  window so only one Qobee surface is visible at a time.

### Smaller polish

- Custom title bar buttons (minimize / maximize / close) wired to
  the right capabilities so they actually fire on Tauri 2.x.
- Fewer dependencies in the audio callback hot path: no allocations
  in the closure body of CPAL's stream callback.
- Build is clean: zero compiler warnings on the engine, core,
  library and Tauri crates.

### Known limits

- WASAPI Exclusive does not yet support gapless transitions
  (each track opens a fresh device session).
- Wireless USB headsets configured as virtual surround (Sony Inzone
  family on the 2.4 GHz dongle, in particular) tend to underrun
  WASAPI Exclusive regardless of the negotiated period. The
  auto-fallback handles this transparently — the user just sees
  a switch back to Shared.

[0.2.0]: https://github.com/qobee/qobee/releases/tag/v0.2.0

## [0.1.0] - 2026-05-25

First public release.

### Added

- Local audio engine: Symphonia decoding, CPAL Shared (WASAPI Shared)
  output, output device picker, sample-rate awareness, truthful
  bit-perfect badge.
- Library: multi-root SQLite catalog, embedded cover extraction,
  recently played, search across titles / artists / albums, genres,
  playlists, favorites.
- Queue: play next, add to queue, reorder, remove, jump to,
  inspectable popover, repeat / shuffle / endless.
- 10-band peaking equalizer at ISO octave centers (Q=1.0, ±12 dB)
  with seven presets and an automatic bypass when every band is at
  0 dB.
- Coarse seek, software volume in Shared mode.
- Themes (system / dark / light), configurable accent color, opt-in
  cover-driven accent.
- Persistent player bar, immersive Now Playing fullscreen view,
  scrubbable progress, transport controls, favorite toggle.
- Welcome / onboarding screen for empty libraries.
- Drag-and-drop a folder onto the window to add it as a library
  root and trigger a scan.
- Keyboard shortcuts: Space (play/pause), arrow keys (±5s),
  n / p / Alt+arrows (prev/next), F (toggle Now Playing),
  Esc (close / clear search), Ctrl/Cmd+F (focus search).
- Toast notifications for transient confirmations.
- Window state persistence across launches via
  `tauri-plugin-window-state`.
- OS Media Session integration: Windows quick controls, Bluetooth
  remotes, AirPods double-tap, lock screen artwork.
- Discord Rich Presence: title, artist, album, real cover art and
  a live elapsed/remaining timeline. Auto-reconnect on Discord
  restart, silent fallback when Discord is closed, pause hides
  the activity. Local cover hosting is opt-in for privacy and
  documented in Settings.
- File-based logging: daily-rotated logs at
  `%APPDATA%\Qobee\logs\qobee.log` plus stdout in dev.

### Bundling

- MSI and NSIS installers built via `tauri build`.
- GitHub Actions workflow `.github/workflows/release.yml` builds
  and drafts a Release on every `v*.*.*` tag.

### Known limits

- WASAPI Exclusive output is intentionally not implemented yet.
  The engine exposes the `Auto` and `Shared` output modes today;
  the integer PCM / Exclusive path is the next milestone.
- Seek is Coarse only (snaps to the nearest demuxer keyframe).
- BASS backend is a stub behind the `engine-bass` Cargo feature.

[0.1.0]: https://github.com/qobee/qobee/releases/tag/v0.1.0
