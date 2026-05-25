<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import {
    isFavorite,
    nextTrack,
    pause,
    prevTrack,
    resume,
    seek,
    addFavorite,
    removeFavorite,
    type Track,
  } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import { nowPlayingFullscreen } from "../lib/nowPlayingFullscreen.svelte";
  import { formatDuration } from "../lib/format";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";

  interface Props {
    track: Track | null;
  }
  let { track }: Props = $props();

  let isPlaying = $derived(app.player.status === "playing");
  let pos = $derived(app.player.position_seconds);
  let dur = $derived(Math.max(app.player.duration_seconds, track?.duration_seconds ?? 0));
  let pct = $derived(dur > 0 ? Math.min(100, (pos / dur) * 100) : 0);

  let nowFavorite = $state(false);
  let lastResolvedId = "";

  $effect(() => {
    const id = track?.id;
    if (id === undefined) {
      nowFavorite = false;
      lastResolvedId = "";
      return;
    }
    const sid = String(id);
    if (sid === lastResolvedId) return;
    lastResolvedId = sid;
    void isFavorite(id)
      .then((fav) => {
        if (lastResolvedId === sid) nowFavorite = fav;
      })
      .catch(() => {
        // best-effort
      });
  });

  async function togglePlayPause(): Promise<void> {
    try {
      if (isPlaying) await pause();
      else await resume();
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function toggleFavorite(): Promise<void> {
    if (!track) return;
    try {
      if (nowFavorite) {
        await removeFavorite(track.id);
        nowFavorite = false;
      } else {
        await addFavorite(track.id);
        nowFavorite = true;
      }
    } catch (e) {
      app.lastError = String(e);
    }
  }

  function onProgressClick(e: MouseEvent): void {
    const target = e.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    if (dur <= 0) return;
    void seek(ratio * dur);
  }

  function close(): void {
    nowPlayingFullscreen.hide();
  }
</script>

<div
  class="overlay"
  in:fade={{ duration: 220 }}
  out:fade={{ duration: 200 }}
  role="dialog"
  aria-modal="true"
  aria-label="Now Playing"
>
  <button
    class="close"
    onclick={close}
    aria-label="Close Now Playing"
    title="Esc to close"
  >
    <Icon name="compress" size={18} />
  </button>

  {#if track}
    <div class="content" in:fly={{ y: 20, duration: 320 }}>
      <div class="cover-frame">
        <Cover coverKey={track.cover_key} size={420} title={track.title} />
      </div>

      <div class="text">
        <h1 class="title" title={track.title}>{track.title}</h1>
        <p class="artist" title={track.artist}>{track.artist}</p>
        {#if track.album}
          <p class="album" title={track.album}>{track.album}</p>
        {/if}
      </div>

      <div class="progress-wrap">
        <div
          class="progress"
          role="slider"
          tabindex="0"
          aria-valuemin="0"
          aria-valuemax={Math.round(dur)}
          aria-valuenow={Math.round(pos)}
          onclick={onProgressClick}
          onkeydown={(e) => {
            if (e.key === "ArrowRight") void seek(Math.min(dur, pos + 5));
            else if (e.key === "ArrowLeft") void seek(Math.max(0, pos - 5));
          }}
        >
          <span class="bar" style="width: {pct}%"></span>
        </div>
        <div class="times">
          <span>{formatDuration(pos)}</span>
          <span>{formatDuration(dur)}</span>
        </div>
      </div>

      <div class="controls">
        <button class="ghost" onclick={() => void prevTrack()} aria-label="Previous">
          <Icon name="prev" size={22} />
        </button>
        <button class="play" onclick={togglePlayPause} aria-label={isPlaying ? "Pause" : "Play"}>
          {#if isPlaying}
            <Icon name="pause" size={28} />
          {:else}
            <Icon name="play" size={28} />
          {/if}
        </button>
        <button class="ghost" onclick={() => void nextTrack()} aria-label="Next">
          <Icon name="next" size={22} />
        </button>
        <button
          class="ghost fav"
          class:active={nowFavorite}
          onclick={toggleFavorite}
          aria-label={nowFavorite ? "Remove from favorites" : "Add to favorites"}
        >
          {#if nowFavorite}
            <Icon name="heart-filled" size={20} />
          {:else}
            <Icon name="heart" size={20} />
          {/if}
        </button>
      </div>
    </div>
  {:else}
    <p class="empty">Nothing playing.</p>
  {/if}

  <!-- Background tinted by the same accent the rest of the app
       follows from the cover (set by accent.svelte.ts). -->
  <div class="bg" aria-hidden="true"></div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: var(--bg-0);
    z-index: 800;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }
  .bg {
    position: absolute;
    inset: -10%;
    pointer-events: none;
    background:
      radial-gradient(circle at 30% 20%, var(--accent-glow), transparent 55%),
      radial-gradient(circle at 70% 80%, var(--accent-soft), transparent 60%);
    filter: blur(40px);
    opacity: 0.9;
    z-index: 0;
  }
  .content {
    position: relative;
    z-index: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 24px;
    padding: 24px;
    max-width: 540px;
    width: 100%;
  }
  .close {
    position: absolute;
    top: calc(var(--titlebar-height) + 12px);
    right: 12px;
    z-index: 2;
    background: var(--bg-1);
    border: 1px solid var(--border);
    color: var(--fg-1);
    width: 36px;
    height: 36px;
    border-radius: 999px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .close:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .cover-frame {
    width: 100%;
    max-width: 420px;
    aspect-ratio: 1 / 1;
    border-radius: var(--radius-lg);
    overflow: hidden;
    box-shadow: 0 30px 60px rgba(0, 0, 0, 0.4);
  }
  .cover-frame :global(.cover) {
    width: 100%;
    height: 100%;
  }
  .text {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 4px;
    width: 100%;
    min-width: 0;
  }
  .title {
    font-size: 28px;
    font-weight: 700;
    letter-spacing: -0.01em;
    color: var(--fg-0);
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .artist {
    font-size: 16px;
    color: var(--fg-1);
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .album {
    font-size: 13px;
    color: var(--fg-2);
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .progress-wrap {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .progress {
    height: 6px;
    background: var(--bg-2);
    border-radius: 999px;
    cursor: pointer;
    outline: none;
    overflow: hidden;
  }
  .progress:focus-visible {
    box-shadow: 0 0 0 2px var(--accent);
  }
  .bar {
    display: block;
    height: 100%;
    background: var(--accent);
    border-radius: 999px;
    transition: width 120ms linear;
  }
  .times {
    display: flex;
    justify-content: space-between;
    color: var(--fg-2);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .controls button {
    border-radius: 999px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .ghost {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-1);
    width: 44px;
    height: 44px;
  }
  .ghost:hover {
    background: var(--bg-2);
    color: var(--fg-0);
    transform: translateY(-1px);
  }
  .ghost.fav.active {
    color: var(--accent);
    border-color: var(--accent);
  }
  .play {
    background: var(--accent);
    border: none;
    color: #fff;
    width: 56px;
    height: 56px;
  }
  .play:hover {
    filter: brightness(1.1);
    transform: translateY(-1px);
  }
  .empty {
    color: var(--fg-2);
    z-index: 1;
  }
</style>
