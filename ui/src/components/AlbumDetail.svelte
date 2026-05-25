<script lang="ts">
  import {
    albumSizeBytes,
    getAlbum,
    playAlbumFromTrack,
    type AlbumDetail as AlbumDetailT,
  } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";
  import TrackList from "./TrackList.svelte";
  import { formatDuration } from "../lib/format";

  let detail = $state<AlbumDetailT | null>(null);
  let loading = $state<boolean>(false);
  let sizeBytes = $state<number | null>(null);

  $effect(() => {
    const id = app.selectedAlbumId;
    if (id === null) {
      detail = null;
      sizeBytes = null;
      return;
    }
    loading = true;
    getAlbum(id)
      .then((d) => (detail = d))
      .catch((e) => {
        app.lastError = String(e);
        detail = null;
      })
      .finally(() => (loading = false));
    albumSizeBytes(id)
      .then((b) => (sizeBytes = b))
      .catch(() => (sizeBytes = null));
  });

  function formatBytes(bytes: number): string {
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    if (bytes < 1024 * 1024 * 1024)
      return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }

  async function handlePlayAlbum(): Promise<void> {
    if (!detail || detail.tracks.length === 0) return;
    const first = detail.tracks[0];
    if (!first) return;
    try {
      // Queue the whole album from the first track. Lets the engine
      // pick up shuffle/repeat state without us re-implementing it.
      await playAlbumFromTrack(detail.album.id, first.id);
    } catch (e) {
      app.lastError = String(e);
    }
  }
</script>

<button class="back" onclick={() => app.goBack()}>← Back</button>

{#if loading}
  <p class="state">Loading album…</p>
{:else if !detail}
  <p class="state">Album not found.</p>
{:else}
  {@const album = detail.album}
  <header class="header">
    <Cover coverKey={album.cover_key} size={220} title={album.title} />
    <div class="info">
      <div class="kind">Album</div>
      <h1>{album.title}</h1>
      <div class="sub">
        <button class="artist-link" onclick={() => app.selectArtist(album.artist)}>
          {album.artist}
        </button>
        {#if album.year}
          · <span>{album.year}</span>
        {/if}
        · <span>{album.track_count} tracks</span>
        · <span>{formatDuration(album.total_duration_seconds)}</span>
        {#if sizeBytes !== null && sizeBytes > 0}
          · <span>{formatBytes(sizeBytes)}</span>
        {/if}
      </div>
      <div class="actions">
        <button
          class="play-btn"
          onclick={handlePlayAlbum}
          disabled={detail.tracks.length === 0}
          aria-label="Play album"
        >
          <Icon name="play" size={16} />
          <span>Play</span>
        </button>
      </div>
    </div>
  </header>

  <TrackList tracks={detail.tracks} albumId={album.id} />
{/if}

<style>
  .back {
    background: transparent;
    border: none;
    color: var(--fg-2);
    cursor: pointer;
    margin-bottom: 16px;
    padding: 6px 10px;
    border-radius: var(--radius-s);
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .back:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .header {
    display: flex;
    gap: 22px;
    align-items: flex-end;
    margin-bottom: 28px;
  }
  .info {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-bottom: 6px;
    min-width: 0;
  }
  .kind {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    color: var(--fg-2);
  }
  h1 {
    font-size: 30px;
    margin: 0;
    font-weight: 700;
    letter-spacing: -0.015em;
  }
  .sub {
    color: var(--fg-1);
    font-size: 14px;
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
  }
  .artist-link {
    background: transparent;
    border: none;
    padding: 0;
    color: var(--fg-0);
    font-weight: 500;
    cursor: pointer;
    font-size: inherit;
  }
  .artist-link:hover {
    color: var(--accent);
    text-decoration: underline;
  }
  .actions {
    display: flex;
    gap: var(--space-3);
    margin-top: var(--space-4);
  }
  .play-btn {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    background: var(--accent);
    color: #fff;
    border: none;
    padding: 10px var(--space-5);
    border-radius: var(--radius-pill);
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
    box-shadow: 0 6px 18px -6px var(--accent-glow);
    transition: transform var(--dur-base) var(--ease-spring),
      box-shadow var(--dur-base) var(--ease-out),
      background var(--dur-fast) var(--ease-out);
  }
  .play-btn:hover {
    transform: scale(1.04);
    box-shadow: 0 10px 24px -6px var(--accent-glow);
    background: var(--accent);
  }
  .play-btn:active:not(:disabled) {
    transform: scale(0.96);
  }
  .play-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
    box-shadow: none;
  }
  .state {
    color: var(--fg-2);
  }
</style>
