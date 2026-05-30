<script lang="ts">
  import { fly } from "svelte/transition";
  import { app } from "../lib/stores.svelte";
  import {
    ALBUM_SORT_OPTIONS,
    loadAlbumSort,
    saveAlbumSort,
    sortAlbums,
    type AlbumSortField,
    type SortDirection,
  } from "../lib/sort";
  import Cover from "./Cover.svelte";
  import Marquee from "./Marquee.svelte";
  import PlayButton from "./PlayButton.svelte";
  import SortMenu from "./SortMenu.svelte";
  import { openAlbumMenu } from "../lib/trackMenu";

  function stagger(i: number): number {
    return Math.min(i * 18, 240);
  }

  // Persisted sort state. Loaded once on mount; every change goes back
  // to localStorage so the user's choice survives reloads.
  const initial = loadAlbumSort();
  let sortField = $state<AlbumSortField>(initial.field);
  let sortDirection = $state<SortDirection>(initial.direction);

  const sortedAlbums = $derived(
    sortAlbums(app.albums, sortField, sortDirection)
  );

  function setField(f: AlbumSortField): void {
    sortField = f;
    saveAlbumSort({ field: sortField, direction: sortDirection });
  }
  function setDirection(d: SortDirection): void {
    sortDirection = d;
    saveAlbumSort({ field: sortField, direction: sortDirection });
  }
</script>

{#if app.albums.length === 0}
  <div class="empty">
    <h2>No albums yet</h2>
    <p>Click <strong>Scan folder</strong> in the sidebar to index a music folder.</p>
  </div>
{:else}
  <header class="header" in:fly={{ y: 8, duration: 240 }}>
    <h1 class="title">Albums</h1>
    <SortMenu
      options={ALBUM_SORT_OPTIONS}
      field={sortField}
      direction={sortDirection}
      onfield={setField}
      ondirection={setDirection}
    />
  </header>
  <div class="grid">
    {#each sortedAlbums as album, i (album.id)}
      <div
        class="card lift marquee-host"
        role="button"
        tabindex="0"
        aria-label={`Ouvrir l'album « ${album.title} » — ${album.artist}`}
        onclick={() => app.selectAlbum(album.id)}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            app.selectAlbum(album.id);
          }
        }}
        oncontextmenu={(e) => openAlbumMenu(e, album)}
        in:fly|global={{ y: 10, duration: 260, delay: stagger(i) }}
      >
        <div class="thumb">
          <Cover coverKey={album.cover_key} size={180} title={album.title} />
          <span class="play-overlay">
            <PlayButton target={{ kind: "album", id: album.id }} size="md" />
          </span>
        </div>
        <div class="meta">
          <Marquee class="t" text={album.title} />
          <Marquee
            class="a"
            text={`${album.artist}${album.year ? ` · ${album.year}` : ""}`}
          />
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    margin: 0 0 var(--space-6);
  }
  .title {
    font-size: 28px;
    font-weight: 700;
    letter-spacing: -0.02em;
    margin: 0;
    color: var(--fg-0);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: var(--space-5) var(--space-4);
    justify-items: start;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-2);
    background: transparent;
    border: none;
    text-align: left;
    cursor: pointer;
    width: calc(180px + var(--space-2) * 2);
    border-radius: var(--radius-md);
    transition: background var(--dur-base) var(--ease-out);
  }
  .card:hover {
    background: rgba(255, 255, 255, 0.04);
  }
  .card:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 0;
  }
  .thumb {
    position: relative;
    border-radius: var(--radius-md);
    overflow: hidden;
    width: fit-content;
    box-shadow: 0 6px 16px -8px rgba(0, 0, 0, 0.5);
    transition: box-shadow var(--dur-base) var(--ease-out);
  }
  .card:hover .thumb {
    box-shadow: 0 14px 32px -10px rgba(0, 0, 0, 0.7);
  }
  .play-overlay {
    position: absolute;
    right: 10px;
    bottom: 10px;
    opacity: 0;
    transform: translateY(8px) scale(0.8);
    transition: opacity var(--dur-base) var(--ease-out),
      transform var(--dur-base) var(--ease-spring);
  }
  .card:hover .play-overlay,
  .card:focus-within .play-overlay {
    opacity: 1;
    transform: translateY(0) scale(1);
  }
  .meta {
    padding: 0;
    width: 100%;
    min-width: 0;
  }
  /* :global because Marquee receives the class via prop. */
  .meta :global(.t) {
    font-weight: 600;
    font-size: 13px;
    color: var(--fg-0);
    margin-top: 0;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .card:hover :global(.t) {
    color: var(--accent);
  }
  .meta :global(.a) {
    font-size: 11px;
    color: var(--fg-2);
    margin-top: 2px;
  }
  .empty {
    padding: 60px 0;
    color: var(--fg-2);
    max-width: 520px;
  }
  .empty h2 {
    font-size: 20px;
    color: var(--fg-1);
    margin: 0 0 var(--space-2);
  }
  .empty p {
    margin: 0;
  }
</style>
