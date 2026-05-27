<script lang="ts">
  // Resizable + collapsible sidebar.
  //
  // Behaviour modelled on Apple Music / Arc / Zen browsers:
  //
  //   * Right edge is a 4 px drag handle. Dragging it horizontally
  //     resizes the sidebar between 64 px (icon-only) and 320 px.
  //     Below the snap threshold (140 px) the layout collapses to
  //     icon-only.
  //
  //   * The collapse button at the bottom toggles between the last
  //     expanded width and the icon-only state, so a quick click
  //     restores whatever width the user had set.
  //
  //   * The width and the collapsed flag are persisted in
  //     localStorage so the layout survives reloads.
  //
  // The whole thing drives a single CSS variable (`--sidebar-width`)
  // on the host so the App grid template reacts naturally.

  import { onMount, onDestroy } from "svelte";
  import { createPlaylist } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import Icon from "./Icon.svelte";

  // ---- Constants -----------------------------------------------------------

  const COLLAPSED_WIDTH = 64;
  const MIN_EXPANDED = 180;
  const MAX_EXPANDED = 320;
  const DEFAULT_WIDTH = 220;
  /** Below this drag value the sidebar snaps to collapsed. */
  const SNAP_THRESHOLD = 140;

  const STORAGE_KEY_WIDTH = "qobee.sidebar.width";
  const STORAGE_KEY_COLLAPSED = "qobee.sidebar.collapsed";

  // ---- State ---------------------------------------------------------------

  let collapsed = $state(false);
  /** Width remembered for "expand back" — only updated when not collapsed. */
  let expandedWidth = $state(DEFAULT_WIDTH);
  /** Effective width fed to CSS — collapsed → COLLAPSED_WIDTH. */
  let effectiveWidth = $derived(collapsed ? COLLAPSED_WIDTH : expandedWidth);

  let dragging = $state(false);
  /** Last pointer X seen, sampled at most once per animation frame. */
  let pendingX: number | null = null;
  let rafScheduled = false;

  // Restore from localStorage on mount.
  onMount(() => {
    try {
      const w = Number(localStorage.getItem(STORAGE_KEY_WIDTH));
      if (Number.isFinite(w) && w >= MIN_EXPANDED && w <= MAX_EXPANDED) {
        expandedWidth = w;
      }
      collapsed = localStorage.getItem(STORAGE_KEY_COLLAPSED) === "1";
    } catch {
      /* localStorage unavailable: default values stay. */
    }
  });

  // Push the current width as a CSS var on the document so the App
  // grid template (`grid-template-columns: var(--sidebar-width) 1fr`)
  // reacts in real time without piping props through the App shell.
  $effect(() => {
    document.documentElement.style.setProperty(
      "--sidebar-width",
      `${effectiveWidth}px`,
    );
  });

  function persist() {
    try {
      localStorage.setItem(STORAGE_KEY_WIDTH, String(expandedWidth));
      localStorage.setItem(STORAGE_KEY_COLLAPSED, collapsed ? "1" : "0");
    } catch {
      /* best effort */
    }
  }

  // ---- Drag-to-resize ------------------------------------------------------

  function onResizeStart(e: PointerEvent) {
    e.preventDefault();
    dragging = true;
    // Disable transitions during drag for a 1:1 feel. The variable is
    // read by both the aside (width) and the App shell
    // (grid-template-columns).
    document.documentElement.style.setProperty(
      "--sidebar-resize-transition",
      "none",
    );
    document.body.style.cursor = "ew-resize";
    window.addEventListener("pointermove", onResizeMove);
    window.addEventListener("pointerup", onResizeEnd, { once: true });
  }

  function onResizeMove(e: PointerEvent) {
    pendingX = e.clientX;
    if (rafScheduled) return;
    rafScheduled = true;
    requestAnimationFrame(applyPendingWidth);
  }

  function applyPendingWidth() {
    rafScheduled = false;
    if (pendingX === null) return;
    // Distance from the left edge of the window — the sidebar is
    // anchored at column 0 so this is the desired width.
    const next = Math.max(0, pendingX);
    pendingX = null;
    if (next < SNAP_THRESHOLD) {
      collapsed = true;
      return;
    }
    collapsed = false;
    expandedWidth = Math.min(MAX_EXPANDED, Math.max(MIN_EXPANDED, next));
  }

  function onResizeEnd() {
    dragging = false;
    document.documentElement.style.removeProperty(
      "--sidebar-resize-transition",
    );
    document.body.style.cursor = "";
    window.removeEventListener("pointermove", onResizeMove);
    persist();
  }

  // Double-click on the handle = quick toggle between collapsed and
  // the last expanded width.
  function onHandleDouble() {
    collapsed = !collapsed;
    persist();
  }

  onDestroy(() => {
    window.removeEventListener("pointermove", onResizeMove);
  });

  // ---- Existing actions ----------------------------------------------------

  async function handleNewPlaylist(): Promise<void> {
    const name = window.prompt("Playlist name");
    if (!name || !name.trim()) return;
    try {
      const created = await createPlaylist(name.trim());
      await app.refreshPlaylists();
      app.selectPlaylist(created.id);
    } catch (e) {
      app.lastError = String(e);
    }
  }
</script>

<aside
  class:collapsed
  class:dragging
  aria-label="Navigation"
>
  <!-- Top drag-cap covers the titlebar row so users can drag the
       window from anywhere along the top edge, including the sidebar
       column. Buttons below are interactive and never trigger drag. -->
  <div class="drag-cap" data-tauri-drag-region></div>

  <nav class="primary">
    <button
      class:active={app.selectedView === "home"}
      onclick={() => app.setView("home")}
      title="Home"
    >
      <span class="indicator"></span>
      <Icon name="home" />
      <span class="label">Home</span>
    </button>
    <button
      class:active={app.selectedView === "search"}
      onclick={() => app.setView("search")}
      title="Search"
    >
      <span class="indicator"></span>
      <Icon name="search" />
      <span class="label">Search</span>
    </button>
    <button
      class:active={app.selectedView === "albums" || app.selectedView === "album-detail"}
      onclick={() => app.setView("albums")}
      title="Albums"
    >
      <span class="indicator"></span>
      <Icon name="albums" />
      <span class="label">Albums</span>
    </button>
    <button
      class:active={app.selectedView === "artists" || app.selectedView === "artist-detail"}
      onclick={() => app.setView("artists")}
      title="Artists"
    >
      <span class="indicator"></span>
      <Icon name="artists" />
      <span class="label">Artists</span>
    </button>
    <button
      class:active={app.selectedView === "genres" || app.selectedView === "genre-detail"}
      onclick={() => app.setView("genres")}
      title="Genres"
    >
      <span class="indicator"></span>
      <Icon name="genres" />
      <span class="label">Genres</span>
    </button>
    <button
      class:active={app.selectedView === "favorites"}
      onclick={() => app.setView("favorites")}
      title="Favorites"
    >
      <span class="indicator"></span>
      <Icon name="heart" />
      <span class="label">Favorites</span>
    </button>
  </nav>

  <div class="section-header">
    <span class="label">Playlists</span>
    <button
      class="ghost"
      onclick={handleNewPlaylist}
      title="New playlist"
    >+</button>
  </div>

  <nav class="playlists">
    <button
      class="all"
      class:active={app.selectedView === "playlists"}
      onclick={() => app.setView("playlists")}
      title="All playlists"
    >
      <span class="indicator"></span>
      <Icon name="playlist" />
      <span class="label">All playlists</span>
    </button>
    {#each app.playlists.slice(0, 12) as pl (pl.id)}
      <button
        class:active={app.selectedView === "playlist-detail" && app.selectedPlaylistId === pl.id}
        onclick={() => app.selectPlaylist(pl.id)}
        title={pl.name}
      >
        <span class="indicator"></span>
        <Icon name="playlist" />
        <span class="label name">{pl.name}</span>
        <span class="count label">{pl.track_count}</span>
      </button>
    {/each}
  </nav>

  <div class="spacer"></div>

  <!-- Right-edge drag handle. 6 px wide invisible bar that captures
       pointer events for resize. Drag below the snap threshold to
       collapse, drag back to expand. Double-click to toggle. -->
  <div
    class="resize-handle"
    role="separator"
    aria-label="Resize sidebar"
    aria-orientation="vertical"
    onpointerdown={onResizeStart}
    ondblclick={onHandleDouble}
  ></div>
</aside>

<style>
  aside {
    display: flex;
    flex-direction: column;
    background: var(--bg-shell);
    /* Hairline divider between the sidebar and the content area.
       Kept very low alpha so the shell still reads as unified, but
       the eye can pick out the boundary without effort. */
    border-right: 1px solid rgba(255, 255, 255, 0.05);
    padding: var(--space-2) var(--space-3) var(--space-3);
    gap: 2px;
    grid-column: 1;
    grid-row: 1 / 4;
    overflow-y: auto;
    overflow-x: hidden;
    width: var(--sidebar-width);
    position: relative;
    transition: var(
      --sidebar-resize-transition,
      width 220ms cubic-bezier(0.32, 0.72, 0, 1)
    );
  }
  /* Top drag strip covers the titlebar slice above the sidebar so
     users can grab the window from this column too. */
  .drag-cap {
    height: calc(var(--titlebar-height) - var(--space-2));
    margin: calc(var(--space-2) * -1) calc(var(--space-3) * -1) 0;
    flex: 0 0 calc(var(--titlebar-height) - var(--space-2));
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  nav button {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    text-align: left;
    height: 36px;
    padding: 0 var(--space-3);
    color: var(--fg-2);
    font-size: 13px;
    font-weight: 500;
    border-radius: var(--radius-md);
    overflow: hidden;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  nav button :global(svg) {
    flex-shrink: 0;
    color: var(--fg-2);
    transition: color var(--dur-fast) var(--ease-out);
  }
  nav button:hover {
    background: rgba(255, 255, 255, 0.04);
    color: var(--fg-0);
  }
  nav button:hover :global(svg) {
    color: var(--fg-0);
  }
  nav button:active:not(:disabled) {
    background: rgba(255, 255, 255, 0.06);
  }
  nav button.active {
    background: rgba(255, 255, 255, 0.05);
    color: var(--fg-0);
  }
  nav button.active :global(svg) {
    color: var(--accent);
  }
  /* Animated left indicator — thin accent bar instead of the heavy
     accent-soft fill the active state used to carry. */
  .indicator {
    position: absolute;
    left: 4px;
    top: 50%;
    width: 2px;
    height: 0;
    border-radius: 999px;
    background: var(--accent);
    transform: translateY(-50%);
    box-shadow: 0 0 6px var(--accent-glow);
    transition: height var(--dur-base) var(--ease-spring),
      opacity var(--dur-fast) var(--ease-out);
    opacity: 0;
    pointer-events: none;
  }
  nav button.active .indicator {
    height: 18px;
    opacity: 1;
  }

  /* Labels are visible only when expanded. Collapsing hides them
     instantly so they don't poke through the narrow column. */
  .label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition: opacity var(--dur-fast) var(--ease-out);
  }
  aside.collapsed nav button {
    justify-content: center;
    padding: 0;
    gap: 0;
  }
  aside.collapsed .label {
    display: none;
  }

  nav.playlists button {
    font-size: 12px;
    height: 30px;
    padding: 0 var(--space-3);
  }
  nav.playlists button .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  nav.playlists button .count {
    flex: 0 0 auto;
    color: var(--fg-2);
    font-size: 11px;
    transition: color var(--dur-fast) var(--ease-out);
  }
  nav.playlists button:hover .count {
    color: var(--fg-1);
  }
  nav.playlists .all {
    color: var(--fg-2);
    font-style: italic;
  }
  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    color: var(--fg-3);
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    margin: var(--space-5) var(--space-3) var(--space-2);
    height: 16px;
  }
  aside.collapsed .section-header {
    margin: var(--space-3) 0;
    justify-content: center;
  }
  aside.collapsed .section-header .label {
    display: none;
  }
  .ghost {
    background: transparent;
    border: none;
    color: var(--fg-2);
    cursor: pointer;
    padding: 0 6px;
    font-size: 16px;
    line-height: 1;
    transition: color var(--dur-fast) var(--ease-out),
      transform var(--dur-base) var(--ease-spring);
  }
  .ghost:hover {
    color: var(--accent);
    transform: rotate(90deg);
  }
  .spacer {
    flex: 1;
  }

  /* --- Resize handle --- */
  .resize-handle {
    position: absolute;
    top: 0;
    right: -2px;
    bottom: 0;
    width: 6px;
    cursor: ew-resize;
    z-index: 5;
    background: transparent;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .resize-handle::after {
    /* Visible accent strip on hover, like Arc / Cider. */
    content: "";
    position: absolute;
    top: 0;
    right: 2px;
    width: 2px;
    height: 100%;
    background: transparent;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .resize-handle:hover::after,
  aside.dragging .resize-handle::after {
    background: var(--accent);
  }
  /* While dragging we suppress text selection across the whole app so
     the cursor sticks to ew-resize. */
  aside.dragging {
    user-select: none;
  }
</style>
