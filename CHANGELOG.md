# Changelog

All notable changes to Qobee are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows semantic versioning.

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
