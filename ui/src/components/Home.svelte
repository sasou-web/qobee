<script lang="ts">
  import { fly } from "svelte/transition";
  import { app } from "../lib/stores.svelte";
  import Cover from "./Cover.svelte";
  import PlayButton from "./PlayButton.svelte";
  import Marquee from "./Marquee.svelte";
  import { openAlbumMenu } from "../lib/trackMenu";

  // Cap stagger so a long list doesn't dribble in over a full second.
  function stagger(i: number): number {
    return Math.min(i * 28, 280);
  }

  // We render exactly two full rows for each section. The number of
  // columns depends on the available container width, which itself
  // depends on the sidebar collapsed/expanded state. We observe the
  // grid element and recompute the column count whenever it resizes.
  //
  // Each block has its own minimum item width so the 3 grids can
  // breathe independently (albums use 180px, genres 180px, artist
  // avatars 140px — same values that drove the original
  // `minmax(_, 1fr)` grid).
  function columnsFor(width: number, minItemPx: number, gapPx: number): number {
    if (width <= 0) return 1;
    // Mirrors CSS `repeat(auto-fill, minmax(<min>, 1fr))`: the number
    // of columns that fit if every item is at its minimum size, with
    // (cols - 1) gaps between them. Floor + max(1, …) guards against
    // narrow containers.
    const cols = Math.floor((width + gapPx) / (minItemPx + gapPx));
    return Math.max(1, cols);
  }

  let albumsGridEl = $state<HTMLDivElement | null>(null);
  let genresGridEl = $state<HTMLDivElement | null>(null);
  let artistsGridEl = $state<HTMLDivElement | null>(null);

  let albumsCols = $state(1);
  let genresCols = $state(1);
  let artistsCols = $state(1);

  // Bind a ResizeObserver to a grid element and feed its width into a
  // `setCols` callback. Returns a cleanup. Done as a regular function
  // (not a $effect) so each block can pass its own min-item / gap.
  function observeCols(
    el: HTMLDivElement | null,
    minItemPx: number,
    gapPx: number,
    setCols: (n: number) => void,
  ): () => void {
    if (!el) return () => {};
    const compute = () => setCols(columnsFor(el.clientWidth, minItemPx, gapPx));
    compute();
    const ro = new ResizeObserver(compute);
    ro.observe(el);
    return () => ro.disconnect();
  }

  $effect(() => observeCols(albumsGridEl, 180, 16, (n) => (albumsCols = n)));
  $effect(() => observeCols(genresGridEl, 180, 12, (n) => (genresCols = n)));
  $effect(() =>
    observeCols(artistsGridEl, 140, 24, (n) => (artistsCols = n)),
  );

  // Two full rows per block. We pick the column count so both
  // rows are full: the natural number of columns the container
  // can fit, capped by `ceil(N / 2)` so we never leave a half-
  // empty bottom row. The remaining tail (when N is odd or
  // exceeds 2 × cols) is dropped from the home page; "See all"
  // still lists everything.
  function fitTwoRows<T>(all: T[], naturalCols: number): { cols: number; visible: T[] } {
    if (all.length === 0) return { cols: 1, visible: [] };
    const cols = Math.max(1, Math.min(naturalCols, Math.ceil(all.length / 2)));
    return { cols, visible: all.slice(0, cols * 2) };
  }

  let albumsLayout = $derived(fitTwoRows(app.recentAlbums, albumsCols));
  let genresLayout = $derived(fitTwoRows(app.genres, genresCols));
  let artistsLayout = $derived(fitTwoRows(app.recentArtists, artistsCols));

  let visibleAlbums = $derived(albumsLayout.visible);
  let visibleGenres = $derived(genresLayout.visible);
  let visibleArtists = $derived(artistsLayout.visible);

  // Force the grid to render exactly the chosen number of columns
  // — `auto-fill` would otherwise still create as many tracks as
  // the container can fit, and the slice would not align with the
  // visible row break.
  let albumsGridStyle = $derived(`--cols: ${albumsLayout.cols}`);
  let genresGridStyle = $derived(`--cols: ${genresLayout.cols}`);
  let artistsGridStyle = $derived(`--cols: ${artistsLayout.cols}`);
</script>

<section class="home">
  <h1 in:fly={{ y: 8, duration: 240 }}>Home</h1>

  <div class="block">
    <div class="block-header">
      <h2>Recently played albums</h2>
    </div>
    {#if app.recentAlbums.length === 0}
      <p class="empty muted">No albums in history yet.</p>
    {:else}
      <div class="grid" bind:this={albumsGridEl} style={albumsGridStyle}>
        {#each visibleAlbums as alb, i (alb.id)}
          <!-- Card is a div + role=button so we can nest a real
               <PlayButton> inside without producing invalid
               <button>-in-<button>. Keyboard support mirrors the
               native button: Enter / Space activate selection. -->
          <div
            class="album-card lift marquee-host"
            role="button"
            tabindex="0"
            aria-label={`Ouvrir l'album « ${alb.title} » — ${alb.artist}`}
            onclick={() => app.selectAlbum(alb.id)}
            onkeydown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                app.selectAlbum(alb.id);
              }
            }}
            oncontextmenu={(e) => openAlbumMenu(e, alb)}
            in:fly|global={{ y: 10, duration: 260, delay: stagger(i) }}
          >
            <div class="album-thumb">
              <Cover coverKey={alb.cover_key} size={160} title={alb.title} />
              <span class="play-overlay">
                <PlayButton target={{ kind: "album", id: alb.id }} size="md" />
              </span>
            </div>
            <Marquee class="t" text={alb.title} />
            <Marquee class="a" text={alb.artist} />
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <div class="block">
    <div class="block-header">
      <h2>Genres</h2>
      <button class="link" onclick={() => app.setView("genres")}>See all</button>
    </div>
    {#if app.genres.length === 0}
      <p class="empty muted">No genres detected. Make sure your files have a genre tag.</p>
    {:else}
      <div class="genres" bind:this={genresGridEl} style={genresGridStyle}>
        {#each visibleGenres as g, i (g.name)}
          <button
            class="genre-chip"
            onclick={() => app.selectGenre(g.name)}
            in:fly|global={{ y: 6, duration: 220, delay: stagger(i) }}
          >
            <Cover coverKey={g.cover_key} size={64} title={g.name} />
            <div>
              <div class="g-name">{g.name}</div>
              <div class="g-count">{g.track_count} tracks</div>
            </div>
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <div class="block">
    <div class="block-header">
      <h2>Recently played artists</h2>
      <button class="link" onclick={() => app.setView("artists")}>See all</button>
    </div>
    {#if app.recentArtists.length === 0}
      <p class="empty muted">No artist history yet.</p>
    {:else}
      <div class="artists-grid" bind:this={artistsGridEl} style={artistsGridStyle}>
        {#each visibleArtists as ar, i (ar.id)}
          <button
            class="artist-card lift"
            onclick={() => app.selectArtist(ar.name)}
            in:fly|global={{ y: 10, duration: 260, delay: stagger(i) }}
          >
            <div class="artist-avatar">
              <Cover coverKey={ar.cover_key} size={120} title={ar.name} />
            </div>
            <div class="artist-name">{ar.name}</div>
            <div class="artist-sub">
              {ar.album_count} album{ar.album_count === 1 ? "" : "s"}
              · {ar.track_count} track{ar.track_count === 1 ? "" : "s"}
            </div>
          </button>
        {/each}
      </div>
    {/if}
  </div>
</section>

<style>
  .home {
    display: flex;
    flex-direction: column;
    gap: var(--space-7);
  }
  h1 {
    font-size: 28px;
    font-weight: 700;
    letter-spacing: -0.02em;
    margin: 0 0 var(--space-2);
    color: var(--fg-0);
  }
  h2 {
    font-size: 14px;
    font-weight: 600;
    margin: 0;
    color: var(--fg-1);
    letter-spacing: -0.005em;
  }
  .block-header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: var(--space-4);
  }
  .muted {
    color: var(--fg-2);
    font-size: 12px;
  }
  .empty {
    color: var(--fg-2);
    font-size: 13px;
  }
  .link {
    background: transparent;
    border: none;
    color: var(--fg-2);
    cursor: pointer;
    font-size: 12px;
    font-weight: 500;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .link:hover {
    color: var(--accent);
  }

  /* Albums grid (recently played albums). The number of columns
     is set inline via `--cols` (computed in script: natural fit
     capped so two rows always fill). */
  .grid {
    display: grid;
    grid-template-columns: repeat(var(--cols, 5), 1fr);
    gap: var(--space-5) var(--space-4);
    justify-items: start;
  }
  .album-card {
    background: transparent;
    border: none;
    text-align: left;
    color: var(--fg-1);
    cursor: pointer;
    padding: var(--space-2);
    border-radius: var(--radius-md);
    width: calc(160px + var(--space-2) * 2);
    transition: background var(--dur-base) var(--ease-out);
  }
  .album-card:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .album-card:hover {
    background: rgba(255, 255, 255, 0.04);
  }
  .album-thumb {
    position: relative;
    border-radius: var(--radius-md);
    overflow: hidden;
    width: fit-content;
    box-shadow: 0 6px 16px -8px rgba(0, 0, 0, 0.5);
    transition: box-shadow var(--dur-base) var(--ease-out);
  }
  .album-card:hover .album-thumb {
    box-shadow: 0 12px 28px -10px rgba(0, 0, 0, 0.7);
  }
  .play-overlay {
    position: absolute;
    right: 10px;
    bottom: 10px;
    opacity: 0;
    transform: translateY(8px) scale(0.8);
    transition: opacity var(--dur-base) var(--ease-out),
      transform var(--dur-base) var(--ease-spring);
    /* Pointer events auto so the inner PlayButton receives clicks
       but the wrapper itself doesn't enlarge the hit area beyond
       the visible disc. */
  }
  .album-card:hover .play-overlay,
  .album-card:focus-within .play-overlay {
    opacity: 1;
    transform: translateY(0) scale(1);
  }
  .album-card :global(.t) {
    color: var(--fg-0);
    font-weight: 600;
    font-size: 13px;
    margin-top: var(--space-3);
    transition: color var(--dur-fast) var(--ease-out);
  }
  .album-card:hover :global(.t) {
    color: var(--accent);
  }
  .album-card :global(.a) {
    color: var(--fg-2);
    font-size: 11px;
    margin-top: 2px;
  }

  /* Genre chips. */
  .genres {
    display: grid;
    grid-template-columns: repeat(var(--cols, 5), 1fr);
    gap: var(--space-3);
  }
  .genre-chip {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2);
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
    color: var(--fg-1);
    text-align: left;
    transition: background var(--dur-base) var(--ease-out),
      border-color var(--dur-base) var(--ease-out),
      transform var(--dur-base) var(--ease-out);
  }
  .genre-chip:hover {
    background: var(--bg-3);
    border-color: var(--accent-soft);
    transform: translateY(-2px);
  }
  .g-name {
    color: var(--fg-0);
    font-weight: 500;
  }
  .g-count {
    color: var(--fg-2);
    font-size: 11px;
  }

  /* Recently played artists. Avatar circles, like ArtistList but
     compact (120px) so a row fits more entries. */
  .artists-grid {
    display: grid;
    grid-template-columns: repeat(var(--cols, 6), 1fr);
    gap: var(--space-6);
    justify-items: center;
  }
  .artist-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    padding: 0;
    background: transparent;
    border: none;
    color: var(--fg-1);
    cursor: pointer;
    width: 130px;
    text-align: center;
    transition: transform var(--dur-base) var(--ease-out);
  }
  .artist-card:hover {
    background: transparent;
  }
  /* Force the cover into a perfect circle. The Cover component itself
     is square; clipping here keeps the global component reusable. */
  .artist-avatar :global(.cover) {
    border-radius: 50%;
  }
  .artist-name {
    color: var(--fg-0);
    font-weight: 600;
    font-size: 13px;
    margin-top: var(--space-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .artist-card:hover .artist-name {
    color: var(--accent);
  }
  .artist-sub {
    color: var(--fg-2);
    font-size: 11px;
  }
</style>
