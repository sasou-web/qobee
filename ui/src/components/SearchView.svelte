<script lang="ts">
  import { fly } from "svelte/transition";
  import {
    playTracks,
    search,
    type SearchResults,
  } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import { formatDuration, formatQuality } from "../lib/format";
  import Cover from "./Cover.svelte";
  import Marquee from "./Marquee.svelte";
  import { openAlbumMenu, openTrackMenu } from "../lib/trackMenu";

  let results = $state<SearchResults | null>(null);
  let loading = $state<boolean>(false);
  let lastQuery = "";
  let debounceHandle: ReturnType<typeof setTimeout> | null = null;

  function runSearch(query: string): void {
    if (query.trim().length === 0) {
      results = null;
      lastQuery = "";
      return;
    }
    if (query === lastQuery) return;
    lastQuery = query;
    loading = true;
    search(query, 50)
      .then((r) => {
        // Discard stale results.
        if (r.query === lastQuery.trim()) {
          results = r;
        }
      })
      .catch((e) => {
        app.lastError = String(e);
        results = null;
      })
      .finally(() => (loading = false));
  }

  $effect(() => {
    const q = app.searchQuery;
    if (debounceHandle !== null) clearTimeout(debounceHandle);
    debounceHandle = setTimeout(() => runSearch(q), 180);
  });

  async function playFromList(idx: number): Promise<void> {
    if (!results) return;
    const ids = results.tracks.map((t) => t.id);
    try {
      await playTracks(ids, idx);
    } catch (e) {
      app.lastError = String(e);
    }
  }

  function stagger(i: number): number {
    return Math.min(i * 18, 240);
  }
</script>

<section>
  <h1>Search</h1>

  {#if app.searchQuery.trim().length === 0}
    <p class="hint">Type something above to search your library.</p>
  {:else if loading && !results}
    <p class="hint">Searching…</p>
  {:else if !results || (results.tracks.length === 0 && results.albums.length === 0 && results.artists.length === 0)}
    <p class="hint">No results for "{app.searchQuery}".</p>
  {:else}
    {#if results.albums.length > 0}
      <div class="block">
        <h2>Albums</h2>
        <div class="grid">
          {#each results.albums as alb, i (alb.id)}
            <button
              class="album-card lift marquee-host"
              onclick={() => app.selectAlbum(alb.id)}
              oncontextmenu={(e) => openAlbumMenu(e, alb)}
              in:fly|global={{ y: 10, duration: 260, delay: stagger(i) }}
            >
              <div class="thumb">
                <Cover coverKey={alb.cover_key} size={140} title={alb.title} />
              </div>
              <div class="meta">
                <Marquee class="t" text={alb.title} />
                <Marquee class="a" text={alb.artist} />
              </div>
            </button>
          {/each}
        </div>
      </div>
    {/if}

    {#if results.artists.length > 0}
      <div class="block">
        <h2>Artists</h2>
        <ul class="artists">
          {#each results.artists as ar (ar.id)}
            <li>
              <button onclick={() => app.selectArtist(ar.name)}>
                <span class="name">{ar.name}</span>
                <span class="muted">{ar.album_count} albums · {ar.track_count} tracks</span>
              </button>
            </li>
          {/each}
        </ul>
      </div>
    {/if}

    {#if results.tracks.length > 0}
      <div class="block">
        <h2>Tracks</h2>
        <table class="tracks">
          <colgroup>
            <col class="col-num" />
            <col />
            <col class="col-album" />
            <col class="col-quality" />
            <col class="col-duration" />
          </colgroup>
          <thead>
            <tr>
              <th>#</th>
              <th>Title</th>
              <th>Album</th>
              <th>Quality</th>
              <th>Duration</th>
            </tr>
          </thead>
          <tbody>
            {#each results.tracks as t, i (t.id)}
              {@const isCurrent = app.player.current_track_id === String(t.id)}
              {@const isPlaying = isCurrent && app.player.status === "playing"}
              <tr
                class:current={isCurrent}
                onclick={() => playFromList(i)}
                ondblclick={() => playFromList(i)}
                oncontextmenu={(e) => openTrackMenu(e, t)}
                role="button"
                tabindex="0"
                onkeydown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    playFromList(i);
                  }
                }}
              >
                <td class="num">
                  <span class="num-text" class:hidden={isPlaying}>{i + 1}</span>
                  {#if isPlaying}
                    <span class="row-eq" aria-hidden="true">
                      <span></span><span></span><span></span>
                    </span>
                  {/if}
                </td>
                <td>
                  <div class="title">{t.title}</div>
                  <div class="artist">{t.artist}</div>
                </td>
                <td class="album">{t.album}</td>
                <td class="quality">{formatQuality(t.sample_rate, t.bit_depth)}</td>
                <td class="duration">{formatDuration(t.duration_seconds)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</section>

<style>
  h1 {
    font-size: 24px;
    font-weight: 700;
    letter-spacing: -0.01em;
    margin: 4px 0 var(--space-6);
  }
  h2 {
    font-size: 14px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-2);
    margin: 0 0 var(--space-3);
  }
  .hint {
    color: var(--fg-2);
  }
  .block {
    margin-bottom: var(--space-7);
  }

  /* Album cards: same pattern as Home / AlbumGrid / GenreDetail. */
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
    gap: var(--space-6);
    justify-items: start;
  }
  .album-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    background: transparent;
    border: none;
    color: var(--fg-1);
    cursor: pointer;
    padding: 0;
    border-radius: var(--radius-lg);
    text-align: left;
    width: 140px;
    transition: transform var(--dur-base) var(--ease-out);
  }
  .album-card:hover {
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
    margin-top: var(--space-1);
    transition: color var(--dur-fast) var(--ease-out);
  }
  .album-card:hover :global(.t) {
    color: var(--accent);
  }
  .meta :global(.a) {
    color: var(--fg-2);
    font-size: 12px;
    margin-top: var(--space-1);
  }

  /* Artists list. */
  .artists {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: var(--space-1);
  }
  .artists button {
    display: flex;
    flex-direction: column;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    cursor: pointer;
    color: var(--fg-0);
    padding: var(--space-2);
    border-radius: var(--radius-sm);
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .artists button:hover {
    background: var(--bg-2);
  }
  .artists .name {
    font-weight: 500;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .artists button:hover .name {
    color: var(--accent);
  }
  .muted {
    color: var(--fg-2);
    font-size: 11px;
  }

  /* Tracks table — mirrors TrackList.svelte for consistency. */
  .tracks {
    width: 100%;
    table-layout: fixed;
    border-collapse: collapse;
  }
  .col-num { width: 40px; }
  .col-album { width: 220px; }
  .col-quality { width: 140px; }
  .col-duration { width: 80px; }

  thead th {
    text-align: left;
    color: var(--fg-2);
    font-weight: 500;
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    padding: var(--space-3) var(--space-2);
    border-bottom: 1px solid var(--border);
  }
  tbody td {
    height: 56px;
    padding: var(--space-2);
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
    overflow: hidden;
    transition: color var(--dur-fast) var(--ease-out);
  }
  tbody tr {
    cursor: pointer;
    transition: background var(--dur-base) var(--ease-out);
  }
  tbody tr:hover {
    background: var(--bg-2);
  }
  tbody tr:hover .title {
    color: var(--fg-0);
  }
  tbody tr.current {
    background: var(--accent-soft);
    box-shadow: inset 3px 0 0 0 var(--accent);
    animation: row-in 240ms var(--ease-spring);
  }
  tbody tr.current .title {
    color: var(--accent);
    font-weight: 600;
  }
  @keyframes row-in {
    from { box-shadow: inset 0 0 0 0 var(--accent); }
    to { box-shadow: inset 3px 0 0 0 var(--accent); }
  }

  .num {
    position: relative;
    color: var(--fg-2);
    text-align: right;
    padding-right: 14px;
  }
  .num-text {
    transition: opacity var(--dur-fast) var(--ease-out);
  }
  .num-text.hidden {
    visibility: hidden;
    opacity: 0;
  }
  .row-eq {
    position: absolute;
    right: 14px;
    top: 50%;
    transform: translateY(-50%);
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    height: 14px;
    width: 14px;
    pointer-events: none;
  }
  .row-eq > span {
    width: 2px;
    background: var(--accent);
    border-radius: 1px;
    transform-origin: bottom;
    animation: row-bar 900ms var(--ease-in-out) infinite;
  }
  .row-eq > span:nth-child(1) { height: 60%; animation-delay: 0ms; }
  .row-eq > span:nth-child(2) { height: 100%; animation-delay: 180ms; }
  .row-eq > span:nth-child(3) { height: 75%; animation-delay: 360ms; }
  @keyframes row-bar {
    0%, 100% { transform: scaleY(0.4); }
    50% { transform: scaleY(1); }
  }

  .title {
    color: var(--fg-1);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .artist {
    color: var(--fg-2);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .album,
  .quality,
  .duration {
    color: var(--fg-1);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
