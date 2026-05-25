<script lang="ts">
  import {
    getPlaylist,
    playPlaylistFromTrack,
    removeFromPlaylist,
    type PlaylistDetail as PlaylistDetailT,
    type Track,
  } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import { formatDuration, formatQuality } from "../lib/format";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";

  let detail = $state<PlaylistDetailT | null>(null);
  let loading = $state<boolean>(false);

  async function load(): Promise<void> {
    const id = app.selectedPlaylistId;
    if (id === null) {
      detail = null;
      return;
    }
    loading = true;
    try {
      detail = await getPlaylist(id);
    } catch (e) {
      app.lastError = String(e);
      detail = null;
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void app.selectedPlaylistId;
    void load();
  });

  async function handlePlay(track: Track): Promise<void> {
    if (app.selectedPlaylistId === null) return;
    try {
      await playPlaylistFromTrack(app.selectedPlaylistId, track.id);
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function handleRemove(position: number): Promise<void> {
    if (app.selectedPlaylistId === null) return;
    try {
      await removeFromPlaylist(app.selectedPlaylistId, position);
      await load();
      await app.refreshPlaylists();
    } catch (e) {
      app.lastError = String(e);
    }
  }
</script>

<button class="back" onclick={() => app.goBack()}>← Back</button>

{#if loading}
  <p class="state">Loading playlist…</p>
{:else if !detail}
  <p class="state">Playlist not found.</p>
{:else}
  <header class="header">
    <Cover coverKey={detail.playlist.cover_key} size={180} title={detail.playlist.name} />
    <div class="info">
      <div class="kind">Playlist</div>
      <h1>{detail.playlist.name}</h1>
      <div class="sub">{detail.playlist.track_count} tracks</div>
    </div>
  </header>

  {#if detail.tracks.length === 0}
    <p class="state">
      This playlist is empty. Add tracks from any album or search result.
    </p>
  {:else}
    <table class="tracks">
      <colgroup>
        <col style="width: 40px;" />
        <col />
        <col style="width: 140px;" />
        <col style="width: 80px;" />
        <col style="width: 80px;" />
      </colgroup>
      <thead>
        <tr>
          <th>#</th>
          <th>Title</th>
          <th>Quality</th>
          <th>Duration</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        {#each detail.tracks as t, i (i)}
          <tr class:current={app.player.current_track_id === String(t.id)}>
            <td class="num">{i + 1}</td>
            <td>
              <div class="title">{t.title}</div>
              <div class="artist">{t.artist} · {t.album}</div>
            </td>
            <td class="quality">{formatQuality(t.sample_rate, t.bit_depth)}</td>
            <td class="duration">{formatDuration(t.duration_seconds)}</td>
            <td class="actions">
              <button onclick={() => handlePlay(t)} aria-label="Play">
                <Icon name="play" />
              </button>
              <button class="ghost" onclick={() => handleRemove(i)} aria-label="Remove">✕</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
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
    font-size: 13px;
  }
  .state {
    color: var(--fg-2);
  }
  .tracks {
    width: 100%;
    border-collapse: collapse;
    margin-top: 8px;
  }
  thead th {
    text-align: left;
    color: var(--fg-2);
    font-weight: 500;
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    padding: 10px 8px;
    border-bottom: 1px solid var(--border);
  }
  tbody td {
    padding: 8px;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }
  tbody tr:hover {
    background: var(--bg-2);
  }
  tbody tr.current {
    background: var(--accent-soft);
  }
  .num {
    color: var(--fg-2);
    text-align: right;
    padding-right: 14px;
  }
  .title {
    color: var(--fg-0);
    font-weight: 500;
  }
  .artist {
    color: var(--fg-2);
    font-size: 12px;
  }
  .quality,
  .duration {
    color: var(--fg-1);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .actions {
    text-align: right;
    display: flex;
    justify-content: flex-end;
    gap: 4px;
  }
  .actions button {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--accent);
    background: transparent;
    border: none;
    cursor: pointer;
  }
  .actions button:hover {
    background: var(--accent-soft);
  }
  .actions .ghost {
    color: var(--fg-2);
  }
  .actions .ghost:hover {
    background: var(--danger);
    color: #fff;
  }
</style>
