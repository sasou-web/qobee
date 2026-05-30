<script lang="ts">
  import { onMount } from "svelte";
  import { fly } from "svelte/transition";
  import { listen } from "@tauri-apps/api/event";
  import { invoke } from "@tauri-apps/api/core";
  import { app } from "./lib/stores.svelte";
  import { playerStore } from "./lib/playerStore.svelte";
  import type { PlayerEventDto } from "./lib/playerEvent";
  import { settings } from "./lib/settings.svelte";
  import { accent } from "./lib/accent.svelte";
  import { discordPresence } from "./lib/discordPresence";
  import { nowPlayingFullscreen } from "./lib/nowPlayingFullscreen.svelte";
  import { installKeyboardShortcuts } from "./lib/keyboardShortcuts";
  import { installFolderDrop } from "./lib/dropToScan";
  import { checkForUpdates } from "./lib/updater";
  import { restoreSession, saveSession } from "./lib/api";
  import {
    setMediaMetadata,
    setMediaPlaybackState,
    setMediaPositionState,
    ensureMediaSession,
  } from "./lib/mediaSession";
  import { getTrack, type Track } from "./lib/api";

  import Sidebar from "./components/Sidebar.svelte";
  import AlbumGrid from "./components/AlbumGrid.svelte";
  import ArtistList from "./components/ArtistList.svelte";
  import AlbumDetail from "./components/AlbumDetail.svelte";
  import PlayerBar from "./components/PlayerBar.svelte";
  import Home from "./components/Home.svelte";
  import GenreList from "./components/GenreList.svelte";
  import GenreDetail from "./components/GenreDetail.svelte";
  import PlaylistList from "./components/PlaylistList.svelte";
  import PlaylistDetail from "./components/PlaylistDetail.svelte";
  import SearchView from "./components/SearchView.svelte";
  import Settings from "./components/Settings.svelte";
  import ArtistDetail from "./components/ArtistDetail.svelte";
  import TitleBar from "./components/TitleBar.svelte";
  import FavoritesView from "./components/FavoritesView.svelte";
  import ContextMenuRoot from "./components/ContextMenuRoot.svelte";
  import PropertiesDialog from "./components/PropertiesDialog.svelte";
  import PlaylistPickerDialog from "./components/PlaylistPickerDialog.svelte";
  import QueuePopover from "./components/QueuePopover.svelte";
  import QueuePanel from "./components/QueuePanel.svelte";
  import ToastsRoot from "./components/ToastsRoot.svelte";
  import TrayNotice from "./components/TrayNotice.svelte";
  import Welcome from "./components/Welcome.svelte";
  import NowPlayingFullscreen from "./components/NowPlayingFullscreen.svelte";
  import { setEqGains, setOutputDevice, setVolume } from "./lib/api";

  // Element-bound ref so we can scroll the content area back to the
  // top whenever the user navigates to a new view. Without this, an
  // album reached by scrolling all the way down would render the next
  // album halfway through the page, hiding its cover/title/play
  // button.
  let mainEl: HTMLElement | null = $state(null);
  let shellEl: HTMLElement | null = $state(null);

  // Cache of the currently playing track so MediaSession + accent can
  // both react to track changes without each component re-resolving
  // the same id. Updated every time `app.player.current_track_id`
  // changes.
  let nowTrack = $state<Track | null>(null);
  let nowTrackId = "";

  onMount(() => {
    // Suppress the native browser context menu app-wide. We use a
    // custom one for music actions; the default menu (Reload, Inspect…)
    // doesn't make sense in a desktop player. Registered synchronously
    // so onMount honors the cleanup return value (an async function
    // would return a Promise that onMount can't dispose of).
    const onContext = (e: MouseEvent) => e.preventDefault();
    window.addEventListener("contextmenu", onContext);

    // Hook OS media keys / Bluetooth controls / Windows quick
    // controls etc. Idempotent — safe to call before any track is
    // playing; setMediaMetadata fills the data later.
    ensureMediaSession();

    // Bind the accent driver to the shell so it can mutate
    // --accent / --accent-soft / --accent-glow at the root level.
    accent.bind(shellEl);

    // App-level keyboard shortcuts: Space, ←/→, n/p, F, Esc, Ctrl+F.
    const cleanupKeys = installKeyboardShortcuts();

    // Drop a folder anywhere on the window to add it to the library.
    // The unlisten is async so we have to track it separately.
    let cleanupDrop: (() => void) | null = null;
    void installFolderDrop()
      .then((fn) => {
        cleanupDrop = fn;
      })
      .catch(() => {
        // best-effort; drag-drop is a nice-to-have, not critical.
      });

    // Listen for navigation requests forwarded by the tray menu /
    // CLI args / `qobee://` deep links so the user lands on the
    // requested view when the OS asks for it.
    let cleanupDeepLink: (() => void) | null = null;
    void listen<string>("deep-link:navigate", (e) => {
      switch (e.payload) {
        case "home":
          app.setView("home");
          break;
        case "library":
        case "albums":
          app.setView("albums");
          break;
        case "settings":
          app.setView("settings");
          break;
        case "queue":
          // The queue is now a dedicated route (QueuePanel). Navigate
          // to it instead of the quick-access popover so a request
          // coming from the tray / mini player lands on the full view.
          app.setView("queue");
          break;
        default:
          break;
      }
    }).then((un) => {
      cleanupDeepLink = un;
    });

    // R8 transport bus: a single listener on `player:event` feeds
    // the reactive `playerStore`. Registered here in App.svelte —
    // the root is never unmounted (Qobee uses a home-grown router
    // on top of App.svelte, not SvelteKit) so the listener
    // survives every page transition. Doing this in any child
    // page would re-register on each visit and either leak
    // listeners or miss events between mounts.
    let cleanupPlayerEvent: (() => void) | null = null;
    void listen<PlayerEventDto>("player:event", (e) => {
      playerStore.apply(e.payload);
    }).then((un) => {
      cleanupPlayerEvent = un;
    });

    void (async () => {
      await settings.load();
      await app.wire();
      app.scanRunning = false;
      await app.refreshAll();
      try {
        await setVolume(settings.values.defaultVolume);
        if (settings.values.outputDeviceId) {
          await setOutputDevice(settings.values.outputDeviceId);
        }
        const eq = settings.values.eqEnabled
          ? settings.values.eqGains
          : [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        await setEqGains(eq);
      } catch {
        // best-effort
      }
      // Boot Discord Rich Presence after settings have loaded so we
      // honor the user's enable/disable preference. The worker
      // itself reconnects silently if Discord starts later.
      try {
        await discordPresence.setClientId(settings.values.discordClientId);
        await discordPresence.setCoverUploadEnabled(
          settings.values.discordCoverUpload
        );
      } catch {
        // ignored
      }
      if (settings.values.discordRichPresence) {
        await discordPresence.init();
      } else {
        await discordPresence.setEnabled(false);
      }

      // Silently check GitHub for a newer release once the app is
      // up and running. Stays quiet when already current or offline;
      // surfaces a toast + auto-installs when an update is found.
      void checkForUpdates({ silent: true });

      // Restore the previous session (queue + position), paused, so
      // the user picks up where they left off without audio starting
      // unprompted. Best-effort; a fresh install simply has nothing
      // to restore.
      try {
        await restoreSession();
      } catch {
        // ignored
      }
    })();

    // R7 — reveal the window once Svelte has mounted and the first
    // paint is done. We create the window with `visible: false` in
    // `tauri.conf.json` so the user never sees the WebView's default
    // white background flash before the dark theme paints. Two
    // chained `requestAnimationFrame`s give Svelte time to commit
    // the DOM and the browser time to actually paint it.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        void invoke("show_main_window").catch(() => {
          // If the command fails (e.g. running outside Tauri during
          // `vite preview`) the window is already visible. No-op.
        });
      });
    });

    // Persist the session (queue + position) every 10 s so a crash
    // or a forced quit still leaves a recent resume point. Cheap: a
    // single settings-row write, skipped when the queue is empty.
    const sessionSaver = window.setInterval(() => {
      void saveSession().catch(() => {});
    }, 10000);

    return () => {
      window.removeEventListener("contextmenu", onContext);
      cleanupKeys();
      if (cleanupDrop) cleanupDrop();
      if (cleanupDeepLink) cleanupDeepLink();
      if (cleanupPlayerEvent) cleanupPlayerEvent();
      window.clearInterval(sessionSaver);
      void saveSession().catch(() => {});
    };
  });

  // Compose a key per "logical view" so detail pages animate on entry too.
  let viewKey = $derived(
    `${app.selectedView}:${app.selectedAlbumId ?? ""}:${app.selectedGenre ?? ""}:${app.selectedArtist ?? ""}:${app.selectedPlaylistId ?? ""}`
  );

  // Reset scroll position whenever the logical view changes. Without
  // this the new view inherits the previous scroll position; a user
  // scrolled near the bottom of the album grid would land in the
  // middle of an album page and miss the cover.
  $effect(() => {
    viewKey;
    const main = mainEl;
    if (!main) return;
    main.scrollTo({ top: 0, behavior: "auto" });
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        main.scrollTo({ top: 0, behavior: "auto" });
      });
    });
  });

  // Resolve the currently playing track (full record) whenever the id
  // changes. We do this once, here, rather than per consumer so the
  // accent driver, media session and any future panel share the same
  // lookup.
  $effect(() => {
    const id = app.player.current_track_id;
    if (!id) {
      nowTrack = null;
      nowTrackId = "";
      void accent.update(null);
      setMediaMetadata(null);
      return;
    }
    if (id === nowTrackId) return;
    nowTrackId = id;
    const numId = Number(id);
    if (!Number.isFinite(numId)) return;
    void getTrack(numId).then((t) => {
      if (!t || String(t.id) !== nowTrackId) return;
      nowTrack = t;
      void accent.update(t.cover_key);
      setMediaMetadata(t);
    });
  });

  // Mirror the user setting into the accent driver.
  $effect(() => {
    accent.setEnabled(settings.values.accentFollowsCover);
    if (settings.values.accentFollowsCover && nowTrack) {
      void accent.update(nowTrack.cover_key);
    }
  });

  // Mirror engine state into the OS-level "is currently playing" flag.
  $effect(() => {
    const status = app.player.status;
    if (status === "playing") setMediaPlaybackState("playing");
    else if (status === "paused") setMediaPlaybackState("paused");
    else if (status === "idle" || status === "stopped") setMediaPlaybackState("none");
  });

  // Push position updates so OS-level scrubbers progress in sync.
  $effect(() => {
    setMediaPositionState(
      app.player.duration_seconds,
      app.player.position_seconds
    );
  });

  // -------------------------------------------------------------------------
  // Discord Rich Presence
  // -------------------------------------------------------------------------

  $effect(() => {
    void discordPresence.setEnabled(settings.values.discordRichPresence);
    if (settings.values.discordRichPresence) {
      void discordPresence.init();
    }
  });

  $effect(() => {
    void discordPresence.setClientId(settings.values.discordClientId);
  });

  // RGPD: opt-in cover upload. Off by default; on means uploads to
  // litterbox.catbox.moe are allowed so Discord can render the
  // local cover art.
  $effect(() => {
    void discordPresence.setCoverUploadEnabled(
      settings.values.discordCoverUpload
    );
  });

  $effect(() => {
    if (!settings.values.discordRichPresence) return;
    const status = app.player.status;
    const paused = status === "paused";
    if (status === "idle" || status === "stopped" || status === "errored") {
      void discordPresence.clearPresence();
      return;
    }
    if (!nowTrack) return;
    void discordPresence.updateTrack(
      nowTrack,
      app.player.position_seconds,
      paused
    );
  });

  $effect(() => {
    if (!settings.values.discordRichPresence) return;
    const status = app.player.status;
    if (status === "playing") void discordPresence.setPaused(false);
    else if (status === "paused") void discordPresence.setPaused(true);
  });

  // Show the Welcome screen until at least one library root is
  // configured. We delay the decision until rootsChecked is true so
  // the regular UI doesn't briefly flash empty before Welcome takes
  // over on a fresh install.
  let showWelcome = $derived(app.rootsChecked && !app.hasLibraryRoots);
</script>

<div class="shell" bind:this={shellEl}>
  <TitleBar />

  {#if !showWelcome}
    <Sidebar />
  {/if}

  <main
    class="content"
    class:welcome-mode={showWelcome}
    bind:this={mainEl}
  >
    {#if app.scanRunning}
      <div class="scan-banner" transition:fly={{ y: -8, duration: 200 }}>
        <span class="scan-dot"></span>
        <strong>Scanning library…</strong>
        {#if app.scan}
          <span>
            {app.scan.files_indexed} indexed / {app.scan.files_visited} files
          </span>
          {#if app.scan.current}
            <span class="current">{app.scan.current}</span>
          {/if}
        {/if}
      </div>
    {/if}

    {#if app.lastError}
      <button
        class="error-banner"
        onclick={() => (app.lastError = null)}
        transition:fly={{ y: -8, duration: 200 }}
      >
        {app.lastError}
      </button>
    {/if}

    {#if showWelcome}
      <Welcome />
    {:else}
      {#key viewKey}
        <div class="view" in:fly={{ y: 8, duration: 220 }}>
          {#if app.selectedView === "home"}
            <Home />
          {:else if app.selectedView === "albums"}
            <AlbumGrid />
          {:else if app.selectedView === "artists"}
            <ArtistList />
          {:else if app.selectedView === "artist-detail"}
            <ArtistDetail />
          {:else if app.selectedView === "album-detail"}
            <AlbumDetail />
          {:else if app.selectedView === "genres"}
            <GenreList />
          {:else if app.selectedView === "genre-detail"}
            <GenreDetail />
          {:else if app.selectedView === "playlists"}
            <PlaylistList />
          {:else if app.selectedView === "playlist-detail"}
            <PlaylistDetail />
          {:else if app.selectedView === "search"}
            <SearchView />
          {:else if app.selectedView === "favorites"}
            <FavoritesView />
          {:else if app.selectedView === "queue"}
            <QueuePanel />
          {:else if app.selectedView === "settings"}
            <Settings />
          {:else}
            <Home />
          {/if}
        </div>
      {/key}
    {/if}
  </main>

  {#if !showWelcome}
    <PlayerBar />
  {/if}
</div>

<ContextMenuRoot />
<PropertiesDialog />
<PlaylistPickerDialog />
<QueuePopover />
<ToastsRoot />
<TrayNotice />

{#if nowPlayingFullscreen.open}
  <NowPlayingFullscreen track={nowTrack} />
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: var(--sidebar-width) 1fr;
    grid-template-rows: var(--titlebar-height) 1fr auto;
    height: 100%;
    background: var(--bg-shell);
    transition: var(
      --sidebar-resize-transition,
      grid-template-columns 220ms cubic-bezier(0.32, 0.72, 0, 1)
    );
  }
  .content {
    grid-column: 2;
    grid-row: 2;
    overflow-y: auto;
    overflow-x: hidden;
    background: var(--bg-shell);
    padding: var(--content-padding-top) var(--content-padding-x)
      var(--content-padding-bottom);
    scroll-behavior: auto;
  }
  /* When the Welcome screen is showing, the sidebar and the player
     bar are hidden, so the main content takes the whole viewport
     below the title bar. */
  .content.welcome-mode {
    grid-column: 1 / -1;
    padding-top: calc(var(--titlebar-height) + 24px);
  }
  .view {
    will-change: transform, opacity;
  }
  .scan-banner {
    display: flex;
    gap: var(--space-3);
    align-items: center;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    margin-bottom: var(--space-4);
    font-size: 13px;
    color: var(--fg-1);
  }
  .scan-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 0 0 var(--accent-glow);
    animation: pulse 1.6s var(--ease-in-out) infinite;
  }
  .scan-banner .current {
    color: var(--fg-2);
    font-family: ui-monospace, monospace;
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }
  .error-banner {
    display: block;
    width: 100%;
    text-align: left;
    background: var(--danger);
    color: #fff;
    padding: var(--space-3) var(--space-4);
    border: none;
    border-radius: var(--radius-md);
    margin-bottom: var(--space-4);
    cursor: pointer;
    font-size: 13px;
  }
  @keyframes pulse {
    0%, 100% {
      box-shadow: 0 0 0 0 var(--accent-glow);
      transform: scale(1);
    }
    50% {
      box-shadow: 0 0 0 6px transparent;
      transform: scale(1.15);
    }
  }
</style>
