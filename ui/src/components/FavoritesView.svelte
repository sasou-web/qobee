<script lang="ts">
  import { fly } from "svelte/transition";
  import { listFavorites, playTracks, type Track } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import { formatDuration, formatQuality } from "../lib/format";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";
  import Marquee from "./Marquee.svelte";
  import { openTrackMenu } from "../lib/trackMenu";

  let tracks = $state<Track[]>([]);
  let loading = $state<boolean>(true);

  async function load(): Promise<void> {
    loading = true;
    try {
      tracks = await listFavorites();
    } catch (e) {
      app.lastError = String(e);
      tracks = [];
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void app.selectedView;
    if (app.selectedView === "favorites") void load();
  });

  async function handlePlay(idx: number): Promise<void> {
    if (tracks.length === 0) return;
    try {
      await playTracks(
        tracks.map((t) => t.id),
        idx
      );
    } catch (e) {
      app.lastError = String(e);
    }
  }

  // Total duration of the favorites collection — gives the page a
  // small "feel" of weight, like the Albums header.
  const totalDuration = $derived(
    tracks.reduce((acc, t) => acc + t.duration_seconds, 0)
  );
</script>

<header class="header" in:fly={{ y: 8, duration: 240 }}>
  <div class="hero">
    <div class="emoji" aria-hidden="true">
      <Icon name="heart-filled" size={42} />
    </div>
    <div class="info">
      <div class="kind">Library</div>
      <h1>Favorites</h1>
      <div class="sub">
        {#if loading}
          Loading…
        {:else}
          {tracks.length} track{tracks.length === 1 ? "" : "s"}
          {#if tracks.length > 0}
            · {formatDuration(totalDuration)}
          {/if}
        {/if}
      </div>
      {#if tracks.length > 0}
        <div class="actions">
          <button class="play-pill" onclick={() => handlePlay(0)} aria-label="Play favorites">
            <Icon name="play" size={16} />
            <span>Play</span>
          </button>
        </div>
      {/if}
    </div>
  </div>
</header>

{#if loading}
  <p class="state">Loading favorites…</p>
{:else if tracks.length === 0}
  <div class="empty">
    <p class="empty-line">No favorites yet.</p>
    <p class="empty-hint">
      Right-click a track and pick <strong>Add to favorites</strong> to bookmark it here.
    </p>
  </div>
{:else}
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
      {#each tracks as t, i (t.id)}
        {@const isCurrent = app.player.current_track_id === String(t.id)}
        {@const isPlaying = isCurrent && app.player.status === "playing"}
        <tr
          class:current={isCurrent}
          onclick={() => handlePlay(i)}
          ondblclick={() => handlePlay(i)}
          oncontextmenu={(e) => openTrackMenu(e, t)}
          role="button"
          tabindex="0"
          onkeydown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              handlePlay(i);
            }
          }}
          in:fly|global={{ y: 6, duration: 200, delay: Math.min(i * 8, 160) }}
        >
          <td class="num">
            <span class="num-text" class:hidden={isPlaying}>{i + 1}</span>
            {#if isPlaying}
              <span class="row-eq" aria-hidden="true">
                <span></span><span></span><span></span>
              </span>
            {/if}
          </td>
          <td class="title-cell">
            <div class="title-row">
              <Cover coverKey={t.cover_key} size={36} title={t.title} />
              <div class="text">
                <Marquee class="title" text={t.title} />
                <Marquee class="artist" text={t.artist} />
              </div>
            </div>
          </td>
          <td class="album">{t.album}</td>
          <td class="quality">{formatQuality(t.sample_rate, t.bit_depth)}</td>
          <td class="duration">{formatDuration(t.duration_seconds)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

<style>
  .header {
    margin-bottom: var(--space-6);
  }
  .hero {
    display: flex;
    align-items: flex-end;
    gap: 22px;
  }
  .emoji {
    width: 140px;
    height: 140px;
    border-radius: var(--radius-lg);
    background: linear-gradient(135deg, #ff5470, #c83a85);
    display: grid;
    place-items: center;
    color: #fff;
    box-shadow: 0 18px 36px -12px rgba(200, 58, 133, 0.6);
    flex-shrink: 0;
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
    font-size: 36px;
    margin: 0;
    font-weight: 700;
    letter-spacing: -0.015em;
  }
  .sub {
    color: var(--fg-1);
    font-size: 14px;
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
    padding: 10px var(--space-5);
    border-radius: var(--radius-pill);
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
    box-shadow: 0 6px 18px -6px var(--accent-glow);
    transition: transform var(--dur-base) var(--ease-spring),
      box-shadow var(--dur-base) var(--ease-out);
  }
  .play-pill:hover {
    transform: scale(1.04);
    box-shadow: 0 10px 24px -6px var(--accent-glow);
  }
  .play-pill:active {
    transform: scale(0.96);
  }
  .state {
    color: var(--fg-2);
  }
  .empty {
    text-align: center;
    padding: 60px 20px;
    color: var(--fg-2);
  }
  .empty-line {
    font-size: 16px;
    color: var(--fg-1);
    margin: 0 0 var(--space-2);
  }
  .empty-hint {
    margin: 0;
    font-size: 13px;
  }

  .tracks {
    width: 100%;
    table-layout: fixed;
    border-collapse: collapse;
    margin-top: var(--space-2);
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
  tbody tr.current {
    background: var(--accent-soft);
    box-shadow: inset 3px 0 0 0 var(--accent);
  }
  tbody tr.current .title-row :global(.title) {
    color: var(--accent);
    font-weight: 600;
  }

  .num {
    position: relative;
    color: var(--fg-2);
    text-align: right;
    padding-right: 14px;
  }
  .num-text.hidden {
    visibility: hidden;
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

  .title-cell {
    min-width: 0;
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-width: 0;
  }
  .title-row .text {
    min-width: 0;
    flex: 1;
  }
  .title-row :global(.title) {
    color: var(--fg-0);
    font-weight: 500;
  }
  .title-row :global(.artist) {
    color: var(--fg-2);
    font-size: 12px;
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
