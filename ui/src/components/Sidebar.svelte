<script lang="ts">
  import { createPlaylist } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import Icon from "./Icon.svelte";

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

<aside>
  <!-- Top drag-cap covers the titlebar row so users can drag the
       window from anywhere along the top edge, including the sidebar
       column. Buttons below are interactive and never trigger drag. -->
  <div class="drag-cap" data-tauri-drag-region></div>

  <nav class="primary">
    <button
      class:active={app.selectedView === "home"}
      onclick={() => app.setView("home")}
    >
      <span class="indicator"></span>
      <Icon name="home" /> Home
    </button>
    <button
      class:active={app.selectedView === "search"}
      onclick={() => app.setView("search")}
    >
      <span class="indicator"></span>
      <Icon name="search" /> Search
    </button>
    <button
      class:active={app.selectedView === "albums" || app.selectedView === "album-detail"}
      onclick={() => app.setView("albums")}
    >
      <span class="indicator"></span>
      <Icon name="albums" /> Albums
    </button>
    <button
      class:active={app.selectedView === "artists" || app.selectedView === "artist-detail"}
      onclick={() => app.setView("artists")}
    >
      <span class="indicator"></span>
      <Icon name="artists" /> Artists
    </button>
    <button
      class:active={app.selectedView === "genres" || app.selectedView === "genre-detail"}
      onclick={() => app.setView("genres")}
    >
      <span class="indicator"></span>
      <Icon name="genres" /> Genres
    </button>
    <button
      class:active={app.selectedView === "favorites"}
      onclick={() => app.setView("favorites")}
    >
      <span class="indicator"></span>
      <Icon name="heart" /> Favorites
    </button>
    <button
      class:active={app.selectedView === "settings"}
      onclick={() => app.setView("settings")}
    >
      <span class="indicator"></span>
      <Icon name="settings" /> Settings
    </button>
  </nav>

  <div class="section-header">
    <span>Playlists</span>
    <button class="ghost" onclick={handleNewPlaylist} title="New playlist">+</button>
  </div>

  <nav class="playlists">
    <button
      class="all"
      class:active={app.selectedView === "playlists"}
      onclick={() => app.setView("playlists")}
    >
      <span class="indicator"></span>
      <Icon name="playlist" /> All playlists
    </button>
    {#each app.playlists.slice(0, 12) as pl (pl.id)}
      <button
        class:active={app.selectedView === "playlist-detail" && app.selectedPlaylistId === pl.id}
        onclick={() => app.selectPlaylist(pl.id)}
        title={pl.name}
      >
        <span class="indicator"></span>
        <span class="name">{pl.name}</span>
        <span class="count">{pl.track_count}</span>
      </button>
    {/each}
  </nav>

  <div class="spacer"></div>
</aside>

<style>
  aside {
    display: flex;
    flex-direction: column;
    background: var(--bg-1);
    border-right: 1px solid var(--border);
    /* Top padding matches the breathing room above the search pill
       in the titlebar ((48 - 32) / 2 = 8px) so the first nav button
       sits on the same horizontal axis as the searchbar. */
    padding: var(--space-2) var(--space-2) var(--space-4);
    gap: var(--space-1);
    grid-column: 1;
    grid-row: 1 / 3;
    overflow-y: auto;
    width: var(--sidebar-width);
  }
  /* Slim drag strip across the top of the sidebar so users can still
     pick up the window from this column. Buttons below stay clickable
     because Tauri's drag logic only fires when the event target itself
     carries data-tauri-drag-region. */
  .drag-cap {
    height: var(--space-2);
    margin: calc(var(--space-2) * -1) calc(var(--space-2) * -1) 0;
    flex: 0 0 var(--space-2);
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  nav button {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    text-align: left;
    height: 40px;
    padding: 0 var(--space-3);
    color: var(--fg-1);
    border-radius: var(--radius-sm);
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out);
  }
  nav button :global(svg) {
    transition: transform var(--dur-base) var(--ease-spring),
      color var(--dur-fast) var(--ease-out);
  }
  nav button:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  nav button:hover :global(svg) {
    transform: scale(1.08);
  }
  nav button:active:not(:disabled) {
    transform: scale(0.98);
  }
  nav button.active {
    background: var(--accent-soft);
    color: var(--fg-0);
  }
  nav button.active :global(svg) {
    color: var(--accent);
  }
  /* Animated left indicator. Hidden by default, slides in & grows on
     active. Pure CSS, no layout shift since the bar is positioned. */
  .indicator {
    position: absolute;
    left: 2px;
    top: 50%;
    width: 3px;
    height: 0;
    border-radius: 2px;
    background: var(--accent);
    transform: translateY(-50%);
    transition: height var(--dur-base) var(--ease-spring),
      opacity var(--dur-fast) var(--ease-out);
    opacity: 0;
    pointer-events: none;
  }
  nav button.active .indicator {
    height: 60%;
    opacity: 1;
  }
  nav button:hover:not(.active) .indicator {
    height: 30%;
    opacity: 0.5;
  }
  nav.playlists button {
    font-size: 13px;
    height: 32px;
    padding: 0 var(--space-3);
  }
  nav.playlists button .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  nav.playlists button .count {
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
    color: var(--fg-2);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    margin: var(--space-5) var(--space-3) var(--space-2);
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
</style>
