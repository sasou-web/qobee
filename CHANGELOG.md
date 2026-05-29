# Changelog

All notable changes to Qobee are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows semantic versioning.

## [Unreleased]

## [0.5.1] - 2026-05-29

A feature release focused on playback quality-of-life and
distribution: automatic updates from GitHub, smoother track
transitions (crossfade + anti-click fades), a fullscreen karaoke
lyrics mode, a sleep timer, star ratings, M3U import/export and
session resume. The audio engine also recovers gracefully when the
output device changes mid-playback, and macOS no longer plants an
icon in the menu bar.

### Added

- **Anti-click micro-fade on play/pause**. Pausing now ramps the
  output down to silence over ~12 ms *before* the device stream is
  stopped, and resuming (and every fresh track start) ramps it back
  up, so the DAC never sees an abrupt amplitude step — the source of
  the click/pop some setups produce on pause. Implemented as a second
  smoothed gain in the CPAL Shared audio callback driven by a
  `fade_target` atomic; the WASAPI Exclusive bit-perfect path is left
  untouched. Gapless and crossfade transitions reuse the same stream,
  so they fade-through cleanly without re-triggering the ramp.
- **Crossfade between tracks**. A new Settings → Playback slider
  (0–12 s, off by default) fades the outgoing track into the next with
  an equal-power (constant-energy) curve so perceived loudness stays
  flat across the blend. The mix runs on the decoder thread (not the
  realtime audio callback) by driving two decode+resample pipelines
  (`DecodeStream`) and blending their output before the shared DSP
  chain. When off, the existing gapless path is used unchanged.
  Crossfade requires the two tracks to share sample rate + channel
  count (same constraint as gapless), else it falls back to the
  standard transition; the orchestrator widens its next-track
  pre-fetch lead to `crossfade + 2 s`. Persisted under
  `playback.crossfade_ms`.
- **Synced lyrics — fullscreen karaoke mode**. The Now Playing view's
  lyrics button now cycles cover → side panel → fullscreen karaoke. In
  karaoke mode the synced lines fill the stage with a large centred
  active line, dimmed neighbours and a soft top/bottom fade, the sung
  line auto-scrolling to centre.
- **Sleep timer**. A moon button in the player bar arms a 15/30/45/60/
  90-minute timer or "stop at end of current track". A wall-clock timer
  pauses playback when it elapses; the button shows the remaining
  minutes and can be cancelled. Backed by `set_sleep_timer` /
  `set_sleep_after_track` / `get_sleep_timer`.
- **Star ratings + play counts**. Tracks can be rated 1–5 stars from
  the Properties dialog (click the current rating again to clear it),
  which also shows how many times the track has been played. Ratings
  live in a dedicated `track_ratings` table so they survive a rescan;
  play counts are derived from the existing play history.
- **Playlist import / export (M3U / M3U8)**. Export any playlist to an
  extended-M3U file from its header, and import an `.m3u` / `.m3u8`
  file into a new playlist from the Playlists screen (entries are
  resolved against the library by path; unknown entries are skipped).
- **Resume on launch**. The queue, cursor and playback position are
  persisted every 10 seconds and on exit, then restored **paused** on
  the next launch so you pick up exactly where you left off without
  audio starting unprompted.
- **Automatic updates from GitHub Releases**. The Tauri updater checks
  `releases/latest` on launch, verifies a minisign signature against
  the public key pinned in `tauri.conf.json`, installs in place and
  relaunches — no manual re-download. A single toast announces the new
  version; the check is silent when already current or offline. CI
  signs the Windows + macOS artifacts (`TAURI_SIGNING_PRIVATE_KEY`
  secret) and publishes `latest.json` via `tauri-apps/tauri-action`.

### Fixed

- **Lyrics no longer overflow the screen**. The lyrics panel used a
  viewport-relative padding and a fixed max-width that fought its
  container in the fullscreen view, spilling lines past the edges.
  Lines now wrap, the list scrolls within its own box, and the panel
  sizes to its parent in both the compact and karaoke layouts.
- **macOS: no more menu-bar (status-bar) icon**. The system tray was
  created on every platform, which on macOS surfaces an `NSStatusItem`
  in the top menu bar. The tray is now gated to Windows; macOS already
  integrates through the native Now Playing surface, menu bar and Dock.
- **Audio survives an output-device change** (e.g. taking a call). When
  a voice/video call app switches the OS default output or a Bluetooth
  headset drops, the engine now catches the `DeviceNotAvailable` error,
  rebuilds the stream on the current default device, seeks back to the
  exact position and restores the play/pause state — instead of dying
  with a hard error.

[0.5.1]: https://github.com/qobee/qobee/releases/tag/v0.5.1

## [0.5.0] - 2026-05-28

A major release that lands the **Native Media Integration & UX**
spec end to end, reworks the immersive Now Playing screen so it
stays legible on any artwork, and finishes a broad UI/UX polish
pass across the player bar, Settings page, Home, and the icon set.

### Highlights

- **Online lyrics fetching with on-disk cache**. `lyrics::read_for_track`
  now walks five providers in order: sidecar `.lrc` → embedded
  `Lyrics` tag → persistent JSON cache → LRCLib (synced + plain,
  no key required) → Genius (best-effort page scrape, plain
  only). Successful fetches are written to
  `<APPDATA>\Qobee\lyrics\<sha1>.json` so the second open of any
  track is instant and offline-friendly. The Tauri command
  `get_lyrics` was updated to plumb the full `TrackLookup`
  (title / artist / album / duration) and a cache directory
  through, with no front-end changes required.
- **Unified `PlayerEvent` bus**. `qobee-core` now exposes a
  single broadcast channel (capacity 256) carrying `Started /
  Paused / Resumed / Stopped / TrackChanged / PositionTick /
  Errored` plus the legacy diagnostic variants. Transport
  commands are silently idempotent (no event for a no-op),
  position ticks are throttled to 250 ms, and a property test
  pins the idempotence law on random command sequences.
- **OS now-playing surfaces**. A new `MediaBridge` trait + a
  single tokio fan-out task in `src-tauri` projects every event
  onto Windows SMTC (`SystemMediaTransportControls`, hardware
  keys + lock screen tile) and macOS
  `MPNowPlayingInfoCenter` + `MPRemoteCommandCenter` (Touch
  Bar, Control Center, AirPods double-tap), with placeholder
  artwork bundled at compile time so the OS thumbnail always
  has a frame to draw. A second property test asserts both
  bridges and the UI converge to the same snapshot for any
  event sequence.
- **Predictable `PlayButton`**. A new `<PlayButton>` component
  drives every play action in the app (track row, album hero,
  mini player, player bar). Click handling is fully scoped:
  `e.stopPropagation()` + `e.preventDefault()` so a play click
  never bubbles into row navigation, the error state is
  triggered only by an `Errored` event matching the button's
  own `target.id`, and the aria-label flips between "Play" and
  "Pause" off the derived state. The dated "loading ring" mid-
  click visual is gone — clicks feel instant and the engine's
  `Started` event drives the visual flip.
- **Album / artist cards: separated click targets**. On Home,
  Albums grid and Artist detail, the cover hover overlay is now
  a real `<PlayButton>`. Clicking the cover navigates to the
  album, clicking the purple disc starts playback — no more
  navigate-instead-of-play accidents.
- **EQ formatters and reset**. `formatGainDb` /
  `formatFrequency` render the gain readout (`+3 dB`, `0 dB`,
  `-2 dB`) and the band labels (Hz under 1 kHz, kHz above) in
  Settings → Audio. A "Réinitialiser" button zeroes all ten
  bands. A fast-check property test pins the round-trip on
  every gain in `[-12, 12]`.
- **Google Drive: graceful auth failures**. `qobee-drive`
  distinguishes `AccessDenied` (`error=access_denied |
  admin_policy_enforced | unauthorized_client`, HTTP 403) from
  every other failure mode and ships a dedicated
  `DriveErrorScreen` in French explaining the cause, with a
  one-click link to `docs/google-cloud-setup.md` (full GCP
  walkthrough) and a Retry button. `NeedsReauth` triggers a
  silent re-flow so the session never crashes.
- **Window manager**. Settings → Fenêtre exposes the close
  behaviour (`quit / minimize_to_tray / keep_running_in_background`),
  a tray toggle and a "notify on track change" toggle. The
  tray is created or destroyed at runtime without a restart,
  the `WindowEvent::CloseRequested` hook routes to the right
  branch (and a `quit_requested` flag short-circuits it on
  Cmd+Q / Ctrl+Q / tray Quit), the Windows jumplist exposes
  Play-Pause / Next / Previous via `--play-pause` / `--next` /
  `--previous` args, and the macOS Dock reopens / tile menu
  call into the same handlers.
- **OS identity for packaging**. `productName: Qobee`,
  `identifier: app.qobee.player`, `bundle.macOS.category:
  public.app-category.music`, `minimumSystemVersion: 11.0`,
  `set_app_user_model_id` is wired up before the Tauri builder
  runs, and a new `ensure_start_menu_shortcut` writes a
  user-level `.lnk` (`%APPDATA%\…\Programs\Qobee.lnk`) carrying
  `System.AppUserModel.ID`. With it, the SMTC "Now Playing"
  flyout no longer reads "Unknown app" on dev builds and the
  Windows 11 taskbar right-click jumplist actually shows our
  Play / Pause / Next / Previous entries.

### UI / UX polish

- **Settings redesign**. The 9-section accordion is replaced by
  a Cider / Spotify-style two-column layout: a 220 px sidebar
  with a search field plus icon-labelled categories on the
  left, and the active category's content on the right. Long
  prose explanations under every control are removed; toggles
  become iOS-style switches (rounded pill, accent-coloured
  when on); rows align on a single 160 px label column for a
  calmer rhythm. The danger button (Wipe library) is an
  outlined red chip that fills on hover.
- **Home: two full rows per section, no holes**. Recent
  albums, Genres and Recent artists each render exactly two
  full rows. The column count adapts to the actual container
  width (sidebar collapsed / expanded both supported via a
  `ResizeObserver`), and the slice is recomputed so the second
  row is never half-empty: `cols = min(natural, ceil(N/2))`.
- **Player bar**. Top divider gone (footer fades into the
  content via the shared `--bg-shell`); the now-playing card
  has visible-but-discreet borders at rest that brighten
  slightly on hover, with a soft drop-shadow to lift it from
  the bar; the heart hugs the title rather than floating off
  to the right; the quality badge is borderless at rest
  (border + tint appear only on hover); the seek bar's rail
  thickens 4 → 6 px on hover and the thumb gains an `accent-
  glow` ring; the play control is a flat icon — no disc, no
  fill, no halo behind it — matching prev / next / shuffle /
  repeat. The fallback rendered when no track is loaded uses
  the same flat-icon style at 45 % opacity.
- **Modernised transport icon set**. Play / Pause / Prev /
  Next / Shuffle / Repeat / Repeat-one / Queue / Volume / Plus
  redrawn from scratch. Filled glyphs (play, prev, next) get
  rounded edges; outlines (shuffle, repeat) bumped to
  `stroke-width: 2.2` so they read at the same density as the
  filled siblings; the queue glyph is now three lines + a
  music note (Apple Music–style) instead of the dated
  list-with-arrow.
- **Native dialogs replaced by in-app modals**. The two
  `window.prompt("Playlist name")` calls (Sidebar's "+" button
  and the dedicated Playlists view) are replaced by a new
  `<NamePromptDialog>` component: glass card with fade + scale
  in, accent-coloured Create button (disabled while the field
  is empty), Esc / Enter shortcuts. Keeps Qobee's visual
  language consistent with the rest of the UI.
- **Sidebar "new playlist" button**. Replaced the rotating
  text `+` (which produced red/blue chromatic fringes on
  Windows WebView2) with a real SVG plus glyph and a clean
  scale + soft background pulse on hover.
- **Boot flash eliminated**. The main window is now created
  with `visible: false` + `backgroundColor: #0a0c10`, and the
  front-end calls a new `show_main_window` Tauri command
  inside two chained `requestAnimationFrame`s once Svelte has
  committed its first paint. No more white flash + position
  jump on launch.
- **Accent legibility floor**. `accent.svelte.ts` now
  post-processes the cover-derived swatch through an
  `ensureLegible` helper: if the picked colour falls below
  `0.42` Rec. 709 luminance, it's lightened toward white in
  25 % steps until it crosses the threshold. Hue is preserved.
  Resolves the "black PlayButton with bluish halo" seen on
  moody / near-black album art (`mercurial`-style covers).
- **Cursor on the title-bar settings icon**. Was inheriting
  the system-control `cursor: default`; now `cursor: pointer`
  to match every other interactive button.
- **`.play-pill` rename in `ArtistDetail` / `FavoritesView`**.
  The "Shuffle Play" pill was colliding with the global
  `.play-btn` rule from `play-button.css` (which forces a 36 px
  round disc). Renamed to break the collision; the pill now
  renders correctly with its icon + label inline.

### Fixed

- **Webview blank screen on first run**. Vite's prod build
  resolved Svelte 5's SSR entry instead of the browser entry
  because `resolve.conditions` was scoped to `mode === "test"`
  only. The bundle was a fraction of its real size (30 KB vs
  266 KB) and `mount()` threw `lifecycle_function_unavailable`
  silently. Pinned `conditions: ["browser"]` for every Vite
  mode.
- **Immersive Now Playing**: the dark veil over the blurred
  cover was too transparent on near-white artwork, leaving the
  title unreadable. The radial veil now ramps from 0.4 at the
  centre to 0.88 at the edges (was 0.18 → 0.78), with a
  matching top-to-bottom wash.
- **Title legibility**: the layered `text-shadow` on the title
  produced a faint embossed double under each letter. The
  shadow is now a single soft `0 1px 12px rgba(0, 0, 0, 0.5)`
  on each line of the title block.
- **Play button styling**: the chunky frosted disc behind the
  play icon is gone. Prev / play / next render as flat icons
  in a row, the play icon is just larger (36 px vs 22 px) and
  fully opaque, with a subtle scale-on-hover. No background,
  border, glow or backdrop-blur on the primary controls.

### Infrastructure

- The release workflow drops the dead `bundle.macOS.infoPlist`
  field that Tauri CLI 2.4.0 rejected (`Info.plist` is now
  picked up automatically from `src-tauri/Info.plist`), so the
  hand-built `scripts/build-dev.ps1` stops failing on a config
  validation error before reaching the Rust build.



Two big themes for this release:

- **Now Playing redesign**. The fullscreen view ditches the flat
  blurred-cover background for an `AnimatedAmbientBackground` that
  drifts and rotates two saturated copies of the cover, with the
  motion entirely independent of pause/play so the field never
  resets when you tap pause. The cluster of pill-shaped controls
  becomes a single frosted-glass play disc surrounded by minimalist
  icon hit areas, the progress bar lives in its own row with
  inline timestamps, and the icon set (play / pause / prev / next /
  heart / quote / compress) is redrawn with rounded geometry. A
  new "Background" picker in Settings → Appearance ships four
  shell-wide palettes (Dark neutral / Deep black / Soft gray /
  Indigo tint), all of which now drive a unified `--bg-shell`
  variable so the title bar, sidebar, content area and player bar
  read as one continuous surface. The hairline progress strip
  pinned to the top of the player bar is gone — replaced by a
  centered seek row inside the bar — and the only remaining
  divider is a faint right border on the sidebar.

- **Google Drive backend**. A new `qobee-drive` crate implements
  the OAuth Desktop / PKCE flow, a folder picker, the indexer that
  pulls head + tail of each remote audio file to extract tags via
  `lofty`, a `MediaSource` that serves Symphonia from HTTP
  byte-range reads (with an LRU page cache), and two-way favorite
  sync via a `qobee-state.json` file written into the source
  folder. Tokens live in the OS keychain (Credential Manager on
  Windows, Keychain on macOS, Secret Service on Linux); the user
  brings their own OAuth client id + secret, so nothing about the
  library transits through a shared third party. Settings → Library
  gains a "Remote sources" sub-section with a Connect / Sync /
  Remove triad per source. Local tracks and Drive tracks share the
  same player surface — the engine dispatches via a custom
  `drv://<source_id>/<file_id>` URI scheme.

The release also lays the groundwork for a macOS build: a
`macos-latest` job in the release workflow produces a universal
`.dmg` (Apple Silicon + Intel via `lipo`) on every `v*.*.*` tag,
and the CI workflow runs the same matrix so cross-platform
regressions surface on PRs rather than at release time.

## [0.4.1] - 2026-05-26

A maintenance release that scrubs every lint, restores `cargo doc`
to a clean state and tightens the CI gate. No behavioural changes
to the audio path or the user interface — defaults, key list and
runtime characteristics are identical to 0.4.0.

### Fixed

- **Clippy regression in the test suite**: `audio_settings_properties`
  used the literal `3.14` to build an out-of-range `resampler_quality`
  value, which clippy now rejects under `approx_constant`. Replaced
  with `2.5_f32` so the test still exercises the rejection path
  without tripping the lint.
- **`cargo doc` warnings**: 16 broken intra-doc links and one bare
  URL fixed across `qobee-engine` (`diagnostic/mod.rs`, `dsd/dop.rs`,
  `dsp/convolver.rs`, `backend_cpal_shared.rs`) and `qobee-app`
  (`commands.rs`, `discord.rs`, `windows_integration.rs`,
  `cover_host.rs`). The Windows integration module-level doc was
  rewritten to match the actual `register_integration` /
  `unregister_integration` surface; references to functions that no
  longer exist (`register_context_menu`, `apply_jump_list`, etc.)
  were removed.
- **Discord worker comment indentation** in `discord.rs` was off by
  two columns and visually attached to the wrong variable; restored
  the intended indentation so it documents `published` rather than
  `active`.
- **Vite chunking hints** at build time about
  `@tauri-apps/api/event` and `lib/queuePopover.svelte` being both
  statically and dynamically imported. Both imports in `App.svelte`
  are now static so the deep-link `listen` and the queue popover
  toggle resolve immediately at startup. Bundle size drops from
  231.21 kB to 229.72 kB (gzip 70.20 → 69.52 kB) and there is no
  more micro-pause when a `qobee://queue` deep link fires.

### Changed

- **Rust style cleanup**: ~50 clippy warnings cleared across the
  workspace. Highlights:
  - `field_reassign_with_default` rewritten as struct init with
    `..Default::default()` (cleaner, less mutable state).
  - `derivable_impls` collapsed to `#[derive(Default)]` where
    appropriate.
  - `manual_clamp` → `.clamp(FLOOR, 0.0)` in `volume.rs`.
  - `manual_div_ceil` → `.div_ceil()` in `dsd/dsf.rs`.
  - Various `needless_return`, `redundant_closure`,
    `manual_split_once`, `repeat_n`, `manual_range_contains`
    fixes auto-applied by `cargo clippy --fix`.
  - Convoluted `paths.is_empty().then(|| ()).map(|_| ()).is_some()`
    expression in the deep-link parser collapsed to a plain
    `if/else`.
  - Identity `bit_depth.map(|b| b)` removed in `library/scan.rs`.
  - Targeted `#[allow(clippy::needless_range_loop)]` and
    `#[allow(clippy::too_many_arguments)]` on hot audio paths
    where parallel frame/channel indexing or the existing
    argument list reads more clearly than the suggested
    refactor.
- **Whitespace pass**: `cargo fmt --all` applied to the entire
  workspace. ~30 files reformatted in `crates/library` and
  `src-tauri`, no semantic change.

### CI

- `.github/workflows/ci.yml` now runs `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets --no-deps -- -D warnings`
  and `cargo test --workspace` in addition to the existing
  `cargo check`. The `dtolnay/rust-toolchain@stable` step now
  installs the `rustfmt` and `clippy` components explicitly.
  Together these gates catch the regression that 0.4.0 shipped
  with (the clippy error above was latent because the previous
  CI only ran `cargo check`).

[0.4.2]: https://github.com/qobee/qobee/releases/tag/v0.4.2
[0.4.1]: https://github.com/qobee/qobee/releases/tag/v0.4.1
[0.4.0]: https://github.com/qobee/qobee/releases/tag/v0.4.0

## [0.4.0] - 2026-05-26

A polish-focused release: every shell surface gets a refresh and
Qobee finally feels at home on the Windows desktop. The audio
engine itself didn't move — defaults, key list and behaviour are
identical to 0.3.0 — so this is a safe upgrade that brings UX and
integration up to the quality of the audio chain.

Highlights:

- A cinematic fullscreen Now Playing that fills the viewport
  without scrollbars.
- A resizable, collapsible sidebar like Apple Music / Arc.
- A two-tier Audio settings panel that hides the 12 expert knobs
  behind an *Advanced* disclosure.
- Real Windows desktop integration (system tray, single-instance,
  audio file & folder context menus, `qobee://` protocol,
  autostart) — every hook opt-in, every registry write scoped to
  HKCU.

### Added

- **Resizable, collapsible sidebar** with a drag handle on the
  right edge. Below 140 px the layout snaps to a 64 px icon-only
  rail; double-click the handle to toggle. Width and collapsed
  state are persisted in `localStorage`.
- **Cinematic fullscreen Now Playing**: blurred cover background +
  dark veil, single vertical column with `clamp()`-based spacing
  that adapts to viewport height, glass-pill primary controls, a
  premium progress bar with discrete accent glow, and a lyrics
  toggle that swaps the cover for a glass panel without leaving
  the layout.
- **Two-tier Audio settings**: the panel now leads with the
  essentials (limiter on/off, crossfeed, convolver, balance,
  bit-perfect badge) and hides the 12 expert knobs (RG headroom,
  dither profile, limiter ceiling/look-ahead, volume curve/floor,
  resampler quality, custom crossfeed, trim, null-test) behind a
  collapsed *Advanced* section. The advanced toggle is persisted
  per user.
- **Settings page reorganised as collapsible cards** (Library,
  Playback, Audio, Windows Integration, Appearance, Equalizer,
  Integrations, Advanced). All closed by default so the page is no
  longer overwhelming on first visit.
- **Windows desktop integration** — every item opt-in from
  `Settings → Windows Integration`, all writes scoped to `HKCU`:
  - AppUserModelID `app.qobee.player` set on the running process.
  - System tray icon with right-click menu (show/focus, play /
    pause, prev / next, library, settings, quit) and left-click
    focus.
  - Close-to-tray + start-minimized toggles.
  - Autostart entry under `Run\Qobee`.
  - `qobee://` protocol handler with `play`, `enqueue`,
    `play-next`, `play-folder`, `enqueue-folder`,
    `import-folder`, `scan-folder`, `library`, `settings`,
    `home`, `queue` actions.
  - Audio file context menu: `Play in Qobee`, `Add to Qobee
    queue`, `Play next`, `Import to library`. Soft handler via
    `OpenWithProgids` on the 10 supported extensions; never
    replaces the default audio app.
  - Folder context menu: `Play folder`, `Add folder to queue`,
    `Scan folder`, `Import folder` on `Directory`,
    `Directory\Background`, `Drive`.
- **Single-instance + argument forwarding** via
  `tauri-plugin-single-instance`: a second `qobee.exe` invocation
  (Explorer file double-click, jump list, deep link, CLI) sends
  its argv to the running window and exits.
- **CLI argument parser** (`--play`, `--enqueue`, `--play-next`,
  `--play-folder`, `--enqueue-folder`, `--scan-folder`,
  `--import-folder`, `--open-library`, `--open-settings`,
  `--minimized`, plus bare paths). Same parser handles deep links.
- **NSIS installer hook** (`src-tauri/installer/qobee.nsh`) that
  registers the protocol + AUMID on install and cleans every
  registry key on uninstall.
- **Library helpers** `Library::find_track_id_by_path` and
  `Library::tracks_in_folder` so the integration layer can resolve
  paths sent by Explorer / deep links without leaking SQL into the
  Tauri layer.
- New Tauri commands `get_windows_integration_status`,
  `set_windows_integration`, `dispatch_app_command`.

### Changed

- **Player bar refresh**: tighter `auto / minmax(220px, 1.4fr) /
  auto` grid that stays usable when the sidebar is wide, badge
  qualité in a neutral pill instead of accent-soft, volume slider
  without a permanent percentage label (visible in the tooltip).
  The badge truncates with ellipsis under heavy compression, and
  the bar uses `min-width: 0; overflow: hidden` so nothing
  overflows the right edge of the window.
- **Title bar in two columns** (`1fr / auto`): the searchbar takes
  the available space, the settings and window controls sit on
  the right with consistent 46 px hitboxes; close button uses the
  Windows-standard red `#c42b1c` hover.
- **Sidebar polish**: 36 px rows (was 40), 13 px / 500 typography,
  active state shifts from a heavy `accent-soft` fill to a 2 px
  accent indicator with a soft glow, sidebar background shares
  `--bg-0` with the rest of the shell so it no longer detaches
  from the title bar.
- **Card / album-grid polish**: 8 px internal padding with a
  `rgba(255,255,255,0.04)` hover surface, drop shadow that grows
  on hover, play overlay with `accent-glow`. Typography hierarchy
  reset (28 px page titles, 14 px section headers, 13 px album
  titles, 11 px metadata).
- **Scrollbar** redrawn as a 8 px discreet pill
  (`rgba(255,255,255,0.06)` → `0.14` on hover) on both webkit and
  Firefox.
- **`BitPerfectHealth` panel**: the alarming "Chaîne modifiée"
  badge becomes "DSP actif" with a friendly one-line explanation;
  the engine's bullet-point messages are hidden behind an
  *"Afficher les détails techniques"* disclosure.
- **Removed mini-player button** from the player bar (was a
  source of clutter; the mini-player is still reachable through
  the existing `toggle_mini_player` Tauri command).

### Fixed

- The fullscreen Now Playing view no longer scrolls or shows a
  stray scrollbar — `100dvw / 100dvh` + `overflow: hidden`, with
  every block sized via `clamp()` so the layout fits any viewport
  down to ~600×600.
- Tauri `beforeDevCommand` resolved `../ui` from the workspace
  cargo root, which pointed to a non-existent `Apps/ui` folder
  and broke `cargo tauri dev`. Path is now `ui` (relative to the
  workspace cargo root).
- Search bar focus state no longer flashes the accent ring or
  resizes the icon — the rendering stays neutral while typing.


## [0.3.0] - 2026-05-26

A fidelity-focused expansion of the audio chain: every stage from
the decoder to the device is now configurable from a new
**Settings → Audio** panel, surfaced honestly through a
**Bit-Perfect Health** badge, and verifiable end-to-end through a
**null-test diagnostic**.

### Changed defaults — sonic impact

The following defaults change the way Qobee sounds versus 0.2.0
and earlier. The legacy behaviour stays available, just not as the
default:

- `audio.dither_profile = shaped_f_weighted` (was: TPDF white
  triangular noise). Set `audio.dither_profile = tpdf` to restore
  the previous behaviour.
- `audio.peak_limiter_mode = lookahead_limiter` (was: a soft-clip
  applied unconditionally before the device, or no limiter
  depending on the chain). Set `audio.peak_limiter_mode = off` for
  no limiter, or `soft_clip` for the legacy soft-clipper.
- `audio.volume_curve = logarithmic` (was: `quadratic`). Set
  `audio.volume_curve = quadratic` to restore the previous slider
  taper.

These defaults are applied automatically on the first launch after
upgrade; they only affect playback rendering, not the stored audio
files.

### Added

- **Settings → Audio** panel exposing every stage of the new DSP
  chain (ReplayGain peak protection, dither profile, peak limiter,
  volume curve, crossfeed, resampler quality, convolver, balance,
  per-channel trim). Backed by 19 persisted `audio.*` keys
  documented in the README.
- **ReplayGain peak protection** with a configurable safety
  headroom (`audio.rg_peak_protection`,
  `audio.rg_safety_headroom_db`). Reads
  `REPLAYGAIN_TRACK_PEAK` / `_ALBUM_PEAK` tags during scan and
  reports the actually applied attenuation in `PlayerState`.
- **Dither stage** with three profiles — TPDF, shaped HP, and the
  new default shaped F-weighted — exact bypass when output is
  > 16 bits.
- **Peak limiter** with three modes — `off`, `soft_clip`, and the
  new default `lookahead_limiter` (true-peak look-ahead with
  configurable ceiling, look-ahead window and release).
- **Logarithmic volume curve** with a configurable floor
  (`audio.volume_floor_db`); `quadratic` remains available.
- **Bit-Perfect Health badge** in the player bar that lists every
  factor breaking bit-perfect (non-native sample rate, channel
  upmix, non-unity volume, EQ on, ReplayGain attenuation, dither
  active, limiter biting) instead of a binary flag.
- **Crossfeed** for headphones, Bauer-style, with `bauer`,
  `bauer_strong` and `custom` presets plus tunable inter-aural
  delay and low-pass cutoff.
- **DSD / DoP** playback: native decoding of DSF and DFF files,
  with DSD-over-PCM packing for WASAPI Exclusive devices that
  accept it.
- **FFT-partitioned IR convolver** with a Settings → Audio file
  picker for the impulse response and a per-IR gain trim.
- **Stereo balance and per-channel trim** (`audio.balance`,
  `audio.trim_db_per_channel`) for asymmetric listening setups.
- **Null-test diagnostic** in Settings → Diagnostics that plays a
  reference signal, captures via loopback or pre-render, and
  reports peak/RMS difference plus a `BitPerfect / Modified /
  Inconclusive` verdict.
- New Tauri commands: `get_audio_setting`, `set_audio_setting`,
  `list_audio_settings`, `reset_audio_settings`,
  `get_bit_perfect_health`, `get_device_mix_format`,
  `open_windows_sound_settings`, `load_convolver_ir`,
  `unload_convolver_ir`, `get_convolver_status`, `run_null_test`,
  `cancel_null_test`.

### Changed

- The audio engine now runs a single deterministic DSP pipeline
  shared by both Shared and Exclusive backends; bypass conditions
  for each stage are explicit and reflected in the Bit-Perfect
  Health badge.
- Out-of-range `audio.*` values are clamped and rewritten at boot;
  missing keys are created with their default.

### Performance

Mini-bench `bench_dsp_chain_cost` (`cargo test --release -p
qobee-engine --lib -- --ignored bench_dsp_chain_cost --nocapture`,
5 s 192 kHz stereo, 2048-frame blocks):

- all-off (limiter off, no crossfeed, no convolver, no dither):
  0.14 ms wall, ≈ 0 ms/block, RT factor ≈ 35 000×.
- defaults (lookahead limiter, shaped F-weighted dither): 313.8 ms
  wall, 0.67 ms/block, RT factor 15.9×.
- all-on (defaults + crossfeed BauerStrong + convolver 65 536-tap
  IR + 16-bit dither): 688.3 ms wall, 1.47 ms/block, RT factor
  7.3×. Convolver dominates (`O(N log N)` per partition) as
  expected; lookahead limiter adds a ring-buffer per channel,
  crossfeed is two biquads + a delay line per side.
- Property 5 (R3.4, true-peak ≤ ceiling) and `r3_4` stress run at
  `PROPTEST_CASES=200`, both green across two consecutive runs.
- Release binary `target/release/qobee-app.exe` measures 20.17 MB
  (no comparable 0.2.0 baseline recorded locally; tracked
  forward).

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

[0.3.0]: https://github.com/qobee/qobee/releases/tag/v0.3.0
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
