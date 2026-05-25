<script lang="ts">
  import { fly } from "svelte/transition";
  import { app } from "../lib/stores.svelte";
  import {
    GENRE_SORT_OPTIONS,
    loadGenreSort,
    saveGenreSort,
    sortGenres,
    type GenreSortField,
    type SortDirection,
  } from "../lib/sort";
  import Cover from "./Cover.svelte";
  import Marquee from "./Marquee.svelte";
  import SortMenu from "./SortMenu.svelte";

  const initial = loadGenreSort();
  let sortField = $state<GenreSortField>(initial.field);
  let sortDirection = $state<SortDirection>(initial.direction);

  const sortedGenres = $derived(
    sortGenres(app.genres, sortField, sortDirection)
  );

  function setField(f: GenreSortField): void {
    sortField = f;
    saveGenreSort({ field: sortField, direction: sortDirection });
  }
  function setDirection(d: SortDirection): void {
    sortDirection = d;
    saveGenreSort({ field: sortField, direction: sortDirection });
  }

  function stagger(i: number): number {
    return Math.min(i * 18, 240);
  }
</script>

<section>
  <header class="header">
    <h1>Genres</h1>
    {#if app.genres.length > 0}
      <SortMenu
        options={GENRE_SORT_OPTIONS}
        field={sortField}
        direction={sortDirection}
        onfield={setField}
        ondirection={setDirection}
      />
    {/if}
  </header>

  {#if app.genres.length === 0}
    <p class="empty">
      No genre tags found in the library. Re-scan with files that include a
      genre tag, or add tags through your tag editor.
    </p>
  {:else}
    <div class="grid">
      {#each sortedGenres as g, i (g.name)}
        <button
          class="card lift marquee-host"
          onclick={() => app.selectGenre(g.name)}
          in:fly|global={{ y: 10, duration: 260, delay: stagger(i) }}
        >
          <div class="thumb">
            <Cover coverKey={g.cover_key} size={140} title={g.name} />
          </div>
          <div class="meta">
            <Marquee class="t" text={g.name} />
            <div class="c">{g.track_count} tracks</div>
          </div>
        </button>
      {/each}
    </div>
  {/if}
</section>

<style>
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    margin: 4px 0 var(--space-6);
  }
  h1 {
    font-size: 24px;
    font-weight: 700;
    letter-spacing: -0.01em;
    margin: 0;
  }
  .empty {
    color: var(--fg-2);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
    gap: var(--space-6);
    /* Anchor each card to the start of its cell so a half-empty
       grid row doesn't stretch the card over the empty space. */
    justify-items: start;
  }
  .card {
    background: transparent;
    border: none;
    color: var(--fg-1);
    cursor: pointer;
    padding: 0;
    border-radius: var(--radius-lg);
    text-align: left;
    /* Pin to cover width so the meta block can ellipsis/marquee
       within the card. */
    width: 140px;
    transition: transform var(--dur-base) var(--ease-out);
  }
  .card:hover {
    background: transparent;
  }
  .thumb {
    border-radius: var(--radius-lg);
    overflow: hidden;
    width: fit-content;
  }
  .meta {
    width: 100%;
    min-width: 0;
  }
  .meta :global(.t) {
    color: var(--fg-0);
    font-weight: 500;
    margin-top: var(--space-3);
    transition: color var(--dur-fast) var(--ease-out);
  }
  .card:hover :global(.t) {
    color: var(--accent);
  }
  .c {
    color: var(--fg-2);
    font-size: 12px;
    margin-top: var(--space-1);
  }
</style>
