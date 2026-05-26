<script lang="ts">
  // Cinematic fullscreen Now Playing.
  //
  // Layout principles:
  //   * The overlay strictly fills the viewport (100dvw × 100dvh)
  //     with overflow:hidden so nothing ever scrolls or pokes a
  //     scrollbar.
  //   * Background = the current cover, blurred and darkened so the
  //     foreground stays legible whatever the artwork.
  //   * One vertical column, gap-driven rhythm — cover, text,
  //     progress, controls — all centred horizontally and vertically.
  //   * Lyrics live behind a side panel that slides in over the
  //     cover. The lyrics button sits with the secondary controls,
  //     never floating on top of the artwork.

  import { fade, fly } from "svelte/transition";
  import {
    coverUrl,
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
  import LyricsPanel from "./LyricsPanel.svelte";

  interface Props {
    track: Track | null;
  }
  let { track }: Props = $props();

  let isPlaying = $derived(app.player.status === "playing");
  let pos = $derived(app.player.position_seconds);
  let dur = $derived(
    Math.max(app.player.duration_seconds, track?.duration_seconds ?? 0),
  );
  let pct = $derived(dur > 0 ? Math.min(100, (pos / dur) * 100) : 0);

  // Cover URL fed straight to the background. Falls back to a flat
  // accent gradient when no artwork is available so the layout still
  // looks intentional.
  let bgUrl = $derived(coverUrl(track?.cover_key ?? null));

  let nowFavorite = $state(false);
  let lastResolvedId = "";
  let showLyrics = $state(false);

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
    const ratio = Math.min(
      1,
      Math.max(0, (e.clientX - rect.left) / rect.width),
    );
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
  <!-- Cinematic background: blurred cover image + dark overlay. -->
  <div
    class="bg-art"
    style:background-image={bgUrl ? `url("${bgUrl}")` : ""}
    aria-hidden="true"
  ></div>
  <div class="bg-veil" aria-hidden="true"></div>

  <!-- Fullscreen exit button — top-right, glass pill. -->
  <button
    class="exit-btn"
    onclick={close}
    aria-label="Close Now Playing"
    title="Close (Esc)"
  >
    <Icon name="compress" size={16} />
  </button>

  {#if track}
    <div class="stage">
      <!-- Hero zone: either the artwork or the lyrics panel. The
           swap is animated so the user keeps a sense of place. -->
      <div class="hero">
        {#if showLyrics}
          <div class="lyrics-frame" in:fly={{ y: 12, duration: 240 }}>
            <LyricsPanel trackId={track.id} />
          </div>
        {:else}
          <div class="cover-frame" in:fade={{ duration: 200 }}>
            <Cover coverKey={track.cover_key} size={520} title={track.title} />
          </div>
        {/if}
      </div>

      <div class="text">
        <h1 class="title" title={track.title}>{track.title}</h1>
        <p class="artist" title={track.artist}>{track.artist}</p>
        {#if track.album}
          <p class="album" title={track.album}>{track.album}</p>
        {/if}
      </div>

      <!-- Premium progress bar: thicker rail, soft accent glow at
           the played edge, bigger thumb on hover. -->
      <div class="progress-wrap">
        <div
          class="progress"
          role="slider"
          tabindex="0"
          aria-valuemin="0"
          aria-valuemax={Math.round(dur)}
          aria-valuenow={Math.round(pos)}
          aria-label="Seek"
          onclick={onProgressClick}
          onkeydown={(e) => {
            if (e.key === "ArrowRight") void seek(Math.min(dur, pos + 5));
            else if (e.key === "ArrowLeft") void seek(Math.max(0, pos - 5));
          }}
          style:--pct={`${pct}%`}
        >
          <span class="rail" aria-hidden="true"></span>
          <span class="fill" aria-hidden="true"></span>
          <span class="thumb" aria-hidden="true"></span>
        </div>
        <div class="times">
          <span>{formatDuration(pos)}</span>
          <span>{formatDuration(dur)}</span>
        </div>
      </div>

      <!-- Primary controls: prev / play / next, sized as a triad. -->
      <div class="controls primary">
        <button
          class="ctrl"
          onclick={() => void prevTrack()}
          aria-label="Previous"
          title="Previous"
        >
          <Icon name="prev" size={22} />
        </button>
        <button
          class="ctrl play"
          onclick={togglePlayPause}
          aria-label={isPlaying ? "Pause" : "Play"}
          title={isPlaying ? "Pause" : "Play"}
        >
          <Icon name={isPlaying ? "pause" : "play"} size={28} />
        </button>
        <button
          class="ctrl"
          onclick={() => void nextTrack()}
          aria-label="Next"
          title="Next"
        >
          <Icon name="next" size={22} />
        </button>
      </div>

      <!-- Secondary actions: favorite + lyrics toggle, never on
           top of the artwork. -->
      <div class="controls secondary">
        <button
          class="ctrl small"
          class:active={nowFavorite}
          onclick={toggleFavorite}
          aria-label={nowFavorite ? "Remove from favorites" : "Add to favorites"}
          title={nowFavorite ? "Remove from favorites" : "Add to favorites"}
        >
          <Icon
            name={nowFavorite ? "heart-filled" : "heart"}
            size={16}
          />
        </button>
        <button
          class="ctrl small"
          class:active={showLyrics}
          onclick={() => (showLyrics = !showLyrics)}
          aria-label={showLyrics ? "Hide lyrics" : "Show lyrics"}
          title={showLyrics ? "Hide lyrics" : "Show lyrics"}
        >
          <Icon name="quote" size={15} />
        </button>
      </div>
    </div>
  {:else}
    <p class="empty">Nothing playing.</p>
  {/if}
</div>

<style>
  /* ============================================================ */
  /* Layout — strictly fills the viewport, never scrolls.          */
  /* ============================================================ */
  .overlay {
    position: fixed;
    inset: 0;
    width: 100vw;
    height: 100dvh;
    background: var(--bg-0);
    z-index: 800;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    isolation: isolate;
  }

  /* ============================================================ */
  /* Cinematic background — blurred cover + dark veil.             */
  /* ============================================================ */
  .bg-art {
    position: absolute;
    inset: -10%;
    background-size: cover;
    background-position: center;
    background-color: var(--accent-soft);
    filter: blur(80px) saturate(140%);
    transform: scale(1.15);
    opacity: 0.55;
    z-index: 0;
    pointer-events: none;
  }
  .bg-veil {
    position: absolute;
    inset: 0;
    background:
      radial-gradient(
        circle at 50% 30%,
        rgba(0, 0, 0, 0.25),
        rgba(0, 0, 0, 0.7) 70%,
        rgba(0, 0, 0, 0.85) 100%
      ),
      linear-gradient(
        180deg,
        rgba(0, 0, 0, 0.15) 0%,
        rgba(0, 0, 0, 0.55) 100%
      );
    z-index: 1;
    pointer-events: none;
  }

  /* ============================================================ */
  /* Stage — single vertical column, perfectly centred.            */
  /* ============================================================ */
  .stage {
    position: relative;
    z-index: 2;
    display: flex;
    flex-direction: column;
    align-items: center;
    /* Generous gaps that grow with viewport size; fixed minimum
       keeps a tight rhythm on small windows. */
    gap: clamp(14px, 2.4dvh, 26px);
    width: min(640px, 92vw);
    max-height: 100dvh;
    padding: clamp(24px, 4dvh, 48px) 24px;
    box-sizing: border-box;
  }

  /* Exit button — glass pill, top-right, never overlaps the cover. */
  .exit-btn {
    position: absolute;
    top: max(env(safe-area-inset-top), 16px);
    right: 16px;
    z-index: 5;
    width: 36px;
    height: 36px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
    backdrop-filter: blur(12px) saturate(140%);
    -webkit-backdrop-filter: blur(12px) saturate(140%);
    border: 1px solid rgba(255, 255, 255, 0.12);
    color: var(--fg-0);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out);
  }
  .exit-btn:hover {
    background: rgba(255, 255, 255, 0.14);
    border-color: rgba(255, 255, 255, 0.2);
    transform: scale(1.05);
  }
  .exit-btn:active {
    transform: scale(0.96);
  }

  /* ============================================================ */
  /* Hero — cover or lyrics, same footprint so swap is stable.     */
  /* ============================================================ */
  .hero {
    position: relative;
    width: min(45dvh, 380px);
    aspect-ratio: 1 / 1;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .cover-frame {
    width: 100%;
    height: 100%;
    border-radius: 18px;
    overflow: hidden;
    box-shadow:
      0 30px 80px -20px rgba(0, 0, 0, 0.7),
      0 8px 24px -8px rgba(0, 0, 0, 0.5),
      0 0 0 1px rgba(255, 255, 255, 0.04);
  }
  .cover-frame :global(.cover) {
    width: 100% !important;
    height: 100% !important;
    border-radius: 18px;
    box-shadow: none;
  }
  .lyrics-frame {
    width: 100%;
    height: 100%;
    background: rgba(0, 0, 0, 0.35);
    backdrop-filter: blur(20px) saturate(140%);
    -webkit-backdrop-filter: blur(20px) saturate(140%);
    border-radius: 18px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }

  /* ============================================================ */
  /* Title block.                                                  */
  /* ============================================================ */
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
    font-size: clamp(20px, 3.2dvh, 26px);
    font-weight: 700;
    letter-spacing: -0.015em;
    color: var(--fg-0);
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
    text-shadow: 0 2px 12px rgba(0, 0, 0, 0.4);
  }
  .artist {
    font-size: clamp(13px, 2dvh, 15px);
    color: var(--fg-1);
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .album {
    font-size: clamp(11px, 1.6dvh, 12px);
    color: var(--fg-2);
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  /* ============================================================ */
  /* Progress bar — premium rendering with discrete glow.          */
  /* ============================================================ */
  .progress-wrap {
    width: 100%;
    max-width: 480px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .progress {
    position: relative;
    height: 18px;
    cursor: pointer;
    outline: none;
    --pct: 0%;
  }
  .progress:focus-visible .rail {
    box-shadow: 0 0 0 2px var(--accent);
  }
  .rail {
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    height: 4px;
    transform: translateY(-50%);
    background: rgba(255, 255, 255, 0.12);
    border-radius: 999px;
    transition: height var(--dur-fast) var(--ease-out);
  }
  .fill {
    position: absolute;
    left: 0;
    top: 50%;
    height: 4px;
    width: var(--pct);
    transform: translateY(-50%);
    background: linear-gradient(
      90deg,
      var(--accent),
      color-mix(in srgb, var(--accent) 80%, white)
    );
    border-radius: 999px;
    box-shadow: 0 0 12px var(--accent-glow);
    transition: height var(--dur-fast) var(--ease-out);
  }
  .thumb {
    position: absolute;
    left: var(--pct);
    top: 50%;
    width: 12px;
    height: 12px;
    transform: translate(-50%, -50%) scale(0);
    background: var(--fg-0);
    border-radius: 50%;
    box-shadow:
      0 2px 6px rgba(0, 0, 0, 0.4),
      0 0 0 4px var(--accent-glow);
    transition: transform var(--dur-base) var(--ease-spring);
    pointer-events: none;
  }
  .progress:hover .rail,
  .progress:hover .fill {
    height: 6px;
  }
  .progress:hover .thumb,
  .progress:focus-visible .thumb {
    transform: translate(-50%, -50%) scale(1);
  }
  .times {
    display: flex;
    justify-content: space-between;
    color: var(--fg-2);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.04em;
  }

  /* ============================================================ */
  /* Controls — primary triad, secondary row underneath.           */
  /* ============================================================ */
  .controls {
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .controls.primary {
    gap: 18px;
  }
  .controls.secondary {
    gap: 12px;
  }
  .ctrl {
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.1);
    color: var(--fg-0);
    width: 52px;
    height: 52px;
    border-radius: 999px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    backdrop-filter: blur(10px);
    -webkit-backdrop-filter: blur(10px);
    transition: background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .ctrl:hover {
    background: rgba(255, 255, 255, 0.12);
    border-color: rgba(255, 255, 255, 0.2);
    transform: translateY(-1px);
  }
  .ctrl:active {
    transform: scale(0.95);
  }
  .ctrl.play {
    width: 68px;
    height: 68px;
    background: var(--accent);
    border-color: transparent;
    color: #fff;
    box-shadow:
      0 8px 28px -8px var(--accent-glow),
      0 4px 12px rgba(0, 0, 0, 0.35);
  }
  .ctrl.play:hover {
    filter: brightness(1.08);
    transform: translateY(-1px) scale(1.02);
  }
  .ctrl.small {
    width: 38px;
    height: 38px;
    background: transparent;
    border-color: rgba(255, 255, 255, 0.12);
    color: var(--fg-1);
  }
  .ctrl.small:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--fg-0);
  }
  .ctrl.small.active {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }
  .empty {
    color: var(--fg-2);
    z-index: 2;
    position: relative;
  }

  /* ============================================================ */
  /* Reduced-motion: keep everything static, just fade.            */
  /* ============================================================ */
  @media (prefers-reduced-motion: reduce) {
    .ctrl,
    .ctrl.play,
    .exit-btn,
    .progress .rail,
    .progress .fill,
    .progress .thumb {
      transition: none;
    }
  }
</style>
