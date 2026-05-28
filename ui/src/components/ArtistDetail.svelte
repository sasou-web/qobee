<script lang="ts">
  import {
    getArtistDetail,
    playTracks,
    trackIdsByArtist,
    type AlbumWithKind,
    type ArtistDetail as ArtistDetailT,
  } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";
  import PlayButton from "./PlayButton.svelte";
  import { formatDuration } from "../lib/format";
  import { openAlbumMenu } from "../lib/trackMenu";

  let detail = $state<ArtistDetailT | null>(null);
  let loading = $state<boolean>(false);
  let playing = $state<boolean>(false);

  $effect(() => {
    const name = app.selectedArtist;
    if (!name) {
      detail = null;
      return;
    }
    loading = true;
    getArtistDetail(name)
      .then((d) => (detail = d))
      .catch((e) => {
        app.lastError = String(e);
        detail = null;
      })
      .finally(() => (loading = false));
  });

  function group(items: AlbumWithKind[], kind: string): AlbumWithKind[] {
    return items.filter((a) => a.kind === kind);
  }

  /** In-place Fisher-Yates so we don't mutate the input array. */
  function shuffle<T>(arr: T[]): T[] {
    const out = arr.slice();
    for (let i = out.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      const a = out[i] as T;
      const b = out[j] as T;
      out[i] = b;
      out[j] = a;
    }
    return out;
  }

  /** Play a shuffled selection of the artist's whole catalog. */
  async function handleShuffleArtist(): Promise<void> {
    if (!detail || playing) return;
    playing = true;
    try {
      const ids = await trackIdsByArtist(detail.name);
      if (ids.length === 0) {
        app.lastError = "No tracks found for this artist.";
        return;
      }
      await playTracks(shuffle(ids), 0);
    } catch (e) {
      app.lastError = String(e);
    } finally {
      playing = false;
    }
  }
</script>

<button class="back" onclick={() => app.goBack()}>← Back</button>

{#if loading}
  <p class="state">Loading…</p>
{:else if !detail}
  <p class="state">Artist not found.</p>
{:else}
  <header class="head">
    <div class="info">
      <h1>{detail.name}</h1>
      <p class="sub">
        {detail.track_count} tracks · {detail.albums.length} releases
      </p>
      <div class="actions">
        <button
          class="play-pill"
          onclick={handleShuffleArtist}
          disabled={playing || detail.track_count === 0}
          aria-label="Shuffle play artist"
          title="Shuffle play this artist's catalog"
        >
          <Icon name="shuffle" size={14} />
          <span>Shuffle Play</span>
        </button>
      </div>
    </div>
  </header>

  {#each [{ kind: "album", label: "Albums" }, { kind: "ep", label: "EPs" }, { kind: "single", label: "Singles" }] as section (section.kind)}
    {@const items = group(detail.albums, section.kind)}
    {#if items.length > 0}
      <section class="block">
        <h2>{section.label}</h2>
        <div class="grid" class:single-row={section.kind === "single"}>
          {#each items as a (a.album.id)}
            {@const cardSize = section.kind === "single" ? 110 : 160}
            <div
              class="card lift marquee-host"
              class:single={section.kind === "single"}
              role="button"
              tabindex="0"
              onclick={() => app.selectAlbum(a.album.id)}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  app.selectAlbum(a.album.id);
                }
              }}
              oncontextmenu={(e) => openAlbumMenu(e, a.album)}
              style:width={`${cardSize}px`}
            >
              <div class="thumb">
                <Cover
                  coverKey={a.album.cover_key}
                  size={cardSize}
                  title={a.album.title}
                />
                <span class="play-overlay">
                  <PlayButton
                    target={{ kind: "album", id: a.album.id }}
                    size={section.kind === "single" ? "sm" : "md"}
                  />
                </span>
              </div>
              <div class="t">{a.album.title}</div>
              <div class="m">
                {a.album.year ?? ""}{a.album.year ? " · " : ""}
                {a.album.track_count} track{a.album.track_count === 1 ? "" : "s"}
                · {formatDuration(a.duration_seconds)}
              </div>
            </div>
          {/each}
        </div>
      </section>
    {/if}
  {/each}
{/if}

<style>
  .back {
    background: transparent;
    border: none;
    color: var(--fg-2);
    cursor: pointer;
    margin-bottom: 12px;
    padding: 6px 10px;
    border-radius: var(--radius-s);
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .back:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  h1 {
    font-size: 30px;
    margin: 4px 0;
    font-weight: 700;
    letter-spacing: -0.015em;
  }
  .sub {
    color: var(--fg-2);
    font-size: 13px;
    margin: 0 0 24px;
  }
  .head {
    margin-bottom: var(--space-6);
  }
  .info {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .info .sub {
    margin: 0;
  }
  .actions {
    display: flex;
    gap: var(--space-3);
    margin-top: var(--space-4);
  }
  .play-pill {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    background: var(--accent);
    color: #fff;
    border: none;
    padding: 9px var(--space-5);
    border-radius: var(--radius-pill);
    font-weight: 600;
    font-size: 13px;
    white-space: nowrap;
    cursor: pointer;
    box-shadow: 0 6px 18px -6px var(--accent-glow);
    transition: transform var(--dur-base) var(--ease-spring),
      box-shadow var(--dur-base) var(--ease-out),
      background var(--dur-fast) var(--ease-out);
  }
  .play-pill :global(svg) {
    flex-shrink: 0;
  }
  .play-pill:hover:not(:disabled) {
    transform: scale(1.04);
    box-shadow: 0 10px 24px -6px var(--accent-glow);
  }
  .play-pill:active:not(:disabled) {
    transform: scale(0.96);
  }
  .play-pill:disabled {
    opacity: 0.5;
    cursor: not-allowed;
    box-shadow: none;
  }
  .state {
    color: var(--fg-2);
  }
  .block {
    margin-bottom: 28px;
  }
  h2 {
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--fg-2);
    margin: 0 0 12px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
    gap: var(--space-6);
    justify-items: start;
  }
  .grid.single-row {
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: var(--space-5);
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
    color: var(--fg-1);
    transition: transform var(--dur-base) var(--ease-out);
  }
  .card:hover {
    background: transparent;
  }
  .card:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
    border-radius: var(--radius-lg);
  }
  .thumb {
    position: relative;
    border-radius: var(--radius-lg);
    overflow: hidden;
    /* Hug the cover so the play overlay anchors on the artwork
       rather than floating in the empty grid cell. */
    width: fit-content;
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
  /* Singles have a smaller cover, so the overlay sits closer to
     the corner. The PlayButton size prop already shrinks the disc. */
  .card.single .play-overlay {
    right: 6px;
    bottom: 6px;
  }
  .t {
    color: var(--fg-0);
    font-weight: 500;
    margin-top: var(--space-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .card:hover .t {
    color: var(--accent);
  }
  .m {
    color: var(--fg-2);
    font-size: 11px;
    margin-top: var(--space-1);
  }
</style>
