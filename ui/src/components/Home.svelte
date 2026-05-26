<script lang="ts">
  import { fly } from "svelte/transition";
  import { app } from "../lib/stores.svelte";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";
  import Marquee from "./Marquee.svelte";
  import { openAlbumMenu } from "../lib/trackMenu";

  // Cap stagger so a long list doesn't dribble in over a full second.
  function stagger(i: number): number {
    return Math.min(i * 28, 280);
  }
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
      <div class="grid">
        {#each app.recentAlbums as alb, i (alb.id)}
          <button
            class="album-card lift marquee-host"
            onclick={() => app.selectAlbum(alb.id)}
            oncontextmenu={(e) => openAlbumMenu(e, alb)}
            in:fly|global={{ y: 10, duration: 260, delay: stagger(i) }}
          >
            <div class="album-thumb">
              <Cover coverKey={alb.cover_key} size={160} title={alb.title} />
              <span class="play-overlay" aria-hidden="true">
                <Icon name="play" size={20} />
              </span>
            </div>
            <Marquee class="t" text={alb.title} />
            <Marquee class="a" text={alb.artist} />
          </button>
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
      <div class="genres">
        {#each app.genres.slice(0, 8) as g, i (g.name)}
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
      <div class="artists-grid">
        {#each app.recentArtists as ar, i (ar.id)}
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

  /* Albums grid (recently played albums). */
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
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
    width: 40px;
    height: 40px;
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
    box-shadow: 0 8px 20px -4px rgba(0, 0, 0, 0.55),
      0 0 20px var(--accent-glow);
    pointer-events: none;
  }
  .album-card:hover .play-overlay {
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
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
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
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
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
