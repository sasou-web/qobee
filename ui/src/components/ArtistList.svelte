<script lang="ts">
  import { fly } from "svelte/transition";
  import { app } from "../lib/stores.svelte";
  import {
    ARTIST_SORT_OPTIONS,
    loadArtistSort,
    saveArtistSort,
    sortArtists,
    type ArtistSortField,
    type SortDirection,
  } from "../lib/sort";
  import Cover from "./Cover.svelte";
  import SortMenu from "./SortMenu.svelte";

  // Persisted sort state. Same pattern as the Albums tab so users get
  // a consistent feel across the library.
  const initial = loadArtistSort();
  let sortField = $state<ArtistSortField>(initial.field);
  let sortDirection = $state<SortDirection>(initial.direction);

  const sortedArtists = $derived(
    sortArtists(app.artists, sortField, sortDirection)
  );

  function setField(f: ArtistSortField): void {
    sortField = f;
    saveArtistSort({ field: sortField, direction: sortDirection });
  }
  function setDirection(d: SortDirection): void {
    sortDirection = d;
    saveArtistSort({ field: sortField, direction: sortDirection });
  }

  // Cap the entry stagger so a 500-artist library doesn't dribble in
  // for ten seconds.
  function stagger(i: number): number {
    return Math.min(i * 14, 240);
  }
</script>

{#if app.artists.length === 0}
  <div class="empty">
    <h2>No artists yet</h2>
    <p>Scan a folder from the sidebar to populate your library.</p>
  </div>
{:else}
  <header class="header" in:fly={{ y: 8, duration: 240 }}>
    <h1 class="title">Artists</h1>
    <SortMenu
      options={ARTIST_SORT_OPTIONS}
      field={sortField}
      direction={sortDirection}
      onfield={setField}
      ondirection={setDirection}
    />
  </header>

  <div class="grid">
    {#each sortedArtists as artist, i (artist.id)}
      <button
        class="card lift"
        onclick={() => app.selectArtist(artist.name)}
        in:fly|global={{ y: 10, duration: 260, delay: stagger(i) }}
      >
        <div class="avatar">
          <Cover coverKey={artist.cover_key} size={160} title={artist.name} />
        </div>
        <div class="meta">
          <div class="name">{artist.name}</div>
          <div class="sub">
            {artist.album_count} album{artist.album_count === 1 ? "" : "s"}
            · {artist.track_count} track{artist.track_count === 1 ? "" : "s"}
          </div>
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
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
    gap: var(--space-6);
    justify-items: center;
  }
  .card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-3);
    padding: 0;
    background: transparent;
    border: none;
    color: var(--fg-1);
    cursor: pointer;
    width: 160px;
    text-align: center;
    transition: transform var(--dur-base) var(--ease-out);
  }
  .card:hover {
    background: transparent;
  }
  /* Force the artist cover to render as a circle. The Cover component
     uses a square box internally; clipping it here gives the typical
     "artist portrait" feel without modifying the shared component. */
  .avatar :global(.cover) {
    border-radius: 50%;
  }
  .meta {
    width: 100%;
    min-width: 0;
  }
  .name {
    color: var(--fg-0);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .card:hover .name {
    color: var(--accent);
  }
  .sub {
    color: var(--fg-2);
    font-size: 12px;
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
