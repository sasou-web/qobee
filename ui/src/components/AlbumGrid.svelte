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
  import Icon from "./Icon.svelte";
  import Marquee from "./Marquee.svelte";
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
      <button
        class="card lift marquee-host"
        onclick={() => app.selectAlbum(album.id)}
        oncontextmenu={(e) => openAlbumMenu(e, album)}
        in:fly|global={{ y: 10, duration: 260, delay: stagger(i) }}
      >
        <div class="thumb">
          <Cover coverKey={album.cover_key} size={180} title={album.title} />
          <span class="play-overlay" aria-hidden="true">
            <Icon name="play" size={20} />
          </span>
        </div>
        <div class="meta">
          <Marquee class="t" text={album.title} />
          <Marquee
            class="a"
            text={`${album.artist}${album.year ? ` · ${album.year}` : ""}`}
          />
        </div>
      </button>
    {/each}
  </div>
{/if}

<style>
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    margin: 4px 0 var(--space-6);
  }
  .title {
    font-size: 24px;
    font-weight: 700;
    letter-spacing: -0.01em;
    margin: 0;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: var(--space-6);
    justify-items: start;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: 0;
    background: transparent;
    border: none;
    text-align: left;
    cursor: pointer;
    /* Card stays exactly the size of the cover; meta block uses the
       same width so long titles get an ellipsis (and marquee on
       hover) instead of overflowing into neighbours. */
    width: 180px;
    transition: transform var(--dur-base) var(--ease-out);
  }
  .card:hover {
    background: transparent;
  }
  .thumb {
    position: relative;
    border-radius: var(--radius-lg);
    overflow: hidden;
    /* Same fix as Home.svelte: the wrapper must match the cover's
       intrinsic size so the overlay button stays anchored on it. */
    width: fit-content;
  }
  .play-overlay {
    position: absolute;
    right: 12px;
    bottom: 12px;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transform: translateY(8px) scale(0.8);
    transition: opacity var(--dur-base) var(--ease-out),
      transform var(--dur-base) var(--ease-spring);
    box-shadow: 0 10px 24px -6px rgba(0, 0, 0, 0.55);
    pointer-events: none;
  }
  .card:hover .play-overlay {
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
    color: var(--fg-0);
    margin-top: var(--space-3);
    transition: color var(--dur-fast) var(--ease-out);
  }
  .card:hover :global(.t) {
    color: var(--accent);
  }
  .meta :global(.a) {
    font-size: 12px;
    color: var(--fg-2);
    margin-top: var(--space-1);
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
