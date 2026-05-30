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
  import { toasts } from "../lib/toasts.svelte";
  import { PLAYER_LABELS } from "../lib/labels";
  import { formatDuration } from "../lib/format";
  import AnimatedAmbientBackground from "./AnimatedAmbientBackground.svelte";
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

  let nowFavorite = $state(false);
  let lastResolvedId = "";
  // Lyrics display mode:
  //  - "off": show the cover (default).
  //  - "panel": small lyrics box inside the hero square.
  //  - "karaoke": full-stage synced karaoke layout.
  let lyricsMode = $state<"off" | "panel" | "karaoke">("off");
  let showLyrics = $derived(lyricsMode !== "off");

  function cycleLyrics(): void {
    lyricsMode =
      lyricsMode === "off" ? "panel" : lyricsMode === "panel" ? "karaoke" : "off";
  }

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
        toasts.info("Retiré des favoris");
      } else {
        await addFavorite(track.id);
        nowFavorite = true;
        toasts.success("Ajouté aux favoris");
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

  // R4.5 — "Up next" / queue action from the immersive view. Leave the
  // overlay and navigate to the dedicated queue view so the user can
  // manage what plays next.
  function openQueue(): void {
    nowPlayingFullscreen.hide();
    app.setView("queue");
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
  <!-- Premium ambient background — layered radial blobs, vignette,
       grain. Drives subtle motion off `isPlaying`. -->
  <AnimatedAmbientBackground
    coverKey={track?.cover_key ?? null}
    {isPlaying}
  />

  <!-- Fullscreen exit button — top-right, glass pill. Pulled further
       in from the corner (top/right ≥ 24px + horizontal offset) so it
       doesn't sit under the Windows close button (R4.3). Carries an
       explicit FR label + a visible "Esc" hint (R4.2). -->
  <button
    class="exit-btn"
    onclick={close}
    aria-label={PLAYER_LABELS.exitImmersive.label}
    title={`${PLAYER_LABELS.exitImmersive.label} (${PLAYER_LABELS.exitImmersive.shortcut})`}
  >
    <Icon name="compress" size={14} />
    <span class="exit-label">{PLAYER_LABELS.exitImmersive.label}</span>
    <kbd class="exit-kbd">{PLAYER_LABELS.exitImmersive.shortcut}</kbd>
  </button>

  {#if track}
    <div class="stage" class:karaoke={lyricsMode === "karaoke"}>
      {#if lyricsMode === "karaoke"}
        <!-- Full-stage karaoke: the synced lyrics fill the viewport,
             with a compact track header pinned at the top. -->
        <div class="karaoke-header">
          <h1 class="k-title" title={track.title}>{track.title}</h1>
          <p class="k-artist" title={track.artist}>{track.artist}</p>
        </div>
        <div class="karaoke-body" in:fade={{ duration: 220 }}>
          <LyricsPanel trackId={track.id} variant="stage" />
        </div>
      {:else}
        <!-- Hero zone: either the artwork or the lyrics panel. The
             swap is animated so the user keeps a sense of place. -->
        <div class="hero">
          {#if lyricsMode === "panel"}
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
      {/if}

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

      <!-- Primary controls: prev / play / next, flat icons in a row.
           The play icon is just larger; no frosted disc around it. -->
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
          <Icon name={isPlaying ? "pause" : "play"} size={36} />
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

      <!-- Secondary actions: favorite + queue + lyrics toggle, never on
           top of the artwork. -->
      <div class="controls secondary">
        <button
          class="ctrl small"
          class:active={nowFavorite}
          onclick={toggleFavorite}
          aria-pressed={nowFavorite}
          aria-label={nowFavorite ? PLAYER_LABELS.unfavorite.label : PLAYER_LABELS.favorite.label}
          title={nowFavorite ? PLAYER_LABELS.unfavorite.label : PLAYER_LABELS.favorite.label}
        >
          <Icon
            name={nowFavorite ? "heart-filled" : "heart"}
            size={15}
          />
        </button>
        <button
          class="ctrl small"
          onclick={openQueue}
          aria-label={PLAYER_LABELS.queue.label}
          title={PLAYER_LABELS.queue.label}
        >
          <Icon name="queue" size={15} />
        </button>
        <button
          class="ctrl small"
          class:active={showLyrics}
          onclick={cycleLyrics}
          aria-pressed={showLyrics}
          aria-label="Toggle lyrics"
          title={lyricsMode === "off"
            ? "Paroles"
            : lyricsMode === "panel"
              ? "Paroles plein écran (karaoké)"
              : "Masquer les paroles"}
        >
          <Icon name="quote" size={14} />
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
  /*                                                                */
  /* The immersive view is intentionally always cinematic-dark,    */
  /* regardless of the app's light/dark theme. We override the     */
  /* foreground / background variables locally so all child        */
  /* components (Cover, LyricsPanel, etc.) inherit a dark palette  */
  /* and the white-on-translucent decorations stay legible.        */
  /* ============================================================ */
  .overlay {
    position: fixed;
    inset: 0;
    width: 100vw;
    height: 100dvh;
    background: #07090c;
    z-index: 800;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    isolation: isolate;

    /* Force a dark palette inside the overlay so light theme
       doesn't flip the title/artist into dark text against a
       bright blurred cover. */
    --bg-0: #0e0e10;
    --bg-1: #16161a;
    --bg-2: #1d1d22;
    --bg-3: #27272d;
    --fg-0: #f4f4f6;
    --fg-1: #c8c8d0;
    --fg-2: #a4a4ae;
    --fg-3: #6c6c76;
    color-scheme: dark;
  }

  /* ============================================================ */
  /* Background — see AnimatedAmbientBackground.svelte. The                */
  /* component is positioned absolutely inside this overlay and  */
  /* paints behind everything via z-index 0.                      */
  /* ============================================================ */

  /* ============================================================ */
  /* Stage — single vertical column, perfectly centred.            */
  /* ============================================================ */
  .stage {
    position: relative;
    z-index: 2;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    /* Generous gaps that grow with viewport size; fixed minimum
       keeps a tight rhythm on small windows. */
    gap: clamp(14px, 2.4dvh, 22px);
    width: min(560px, 88vw);
    max-height: 100dvh;
    padding: clamp(20px, 4dvh, 40px) 24px;
    box-sizing: border-box;
  }

  /* ============================================================ */
  /* Karaoke mode — the synced lyrics take the whole stage. The    */
  /* progress bar + controls stay pinned at the bottom; a compact  */
  /* track header sits at the top. Width opens up to ~900px so     */
  /* long lines breathe instead of wrapping awkwardly.             */
  /* ============================================================ */
  .stage.karaoke {
    width: min(900px, 94vw);
    justify-content: space-between;
    gap: clamp(10px, 1.6dvh, 18px);
  }
  .karaoke-header {
    flex: 0 0 auto;
    text-align: center;
    width: 100%;
    padding-top: clamp(8px, 2dvh, 24px);
  }
  .k-title {
    font-size: clamp(16px, 2.4dvh, 22px);
    font-weight: 700;
    letter-spacing: -0.02em;
    color: var(--fg-0);
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
    text-shadow: 0 1px 12px rgba(0, 0, 0, 0.5);
  }
  .k-artist {
    font-size: clamp(11px, 1.6dvh, 13px);
    font-weight: 500;
    color: var(--fg-2);
    margin: 4px 0 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
    text-shadow: 0 1px 8px rgba(0, 0, 0, 0.45);
  }
  .karaoke-body {
    flex: 1 1 auto;
    min-height: 0; /* let the inner list scroll instead of overflowing */
    width: 100%;
    display: flex;
    /* Soft top/bottom fade so lines melt into the background as they
       scroll past the active region. Kept shallow (8%) so the first
       and last visible lines stay readable rather than dissolving. */
    -webkit-mask-image: linear-gradient(
      to bottom,
      transparent 0%,
      #000 8%,
      #000 92%,
      transparent 100%
    );
    mask-image: linear-gradient(
      to bottom,
      transparent 0%,
      #000 8%,
      #000 92%,
      transparent 100%
    );
  }

  /* Exit button — glass pill, top-right, never overlaps the cover.
     Override the global focus-visible rule with an inner accent
     ring so the focus state looks at home on the translucent pill. */
  .exit-btn {
    position: absolute;
    top: max(env(safe-area-inset-top), 24px);
    right: 40px;
    z-index: 5;
    height: 32px;
    padding: 0 12px;
    gap: 8px;
    border-radius: 999px;
    background: linear-gradient(
      180deg,
      rgba(255, 255, 255, 0.1) 0%,
      rgba(255, 255, 255, 0.03) 100%
    );
    backdrop-filter: blur(20px) saturate(160%);
    -webkit-backdrop-filter: blur(20px) saturate(160%);
    border: 1px solid rgba(255, 255, 255, 0.12);
    color: var(--fg-1);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    font-weight: 500;
    box-shadow:
      0 4px 14px rgba(0, 0, 0, 0.32),
      inset 0 1px 0 rgba(255, 255, 255, 0.16);
    transition: background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out);
  }
  .exit-label {
    white-space: nowrap;
    letter-spacing: 0.01em;
  }
  .exit-kbd {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 10px;
    line-height: 1;
    padding: 3px 6px;
    border-radius: 5px;
    background: rgba(255, 255, 255, 0.1);
    border: 1px solid rgba(255, 255, 255, 0.18);
    color: var(--fg-0);
    box-shadow: inset 0 -1px 0 rgba(0, 0, 0, 0.25);
  }
  .exit-btn:hover {
    background: linear-gradient(
      180deg,
      rgba(255, 255, 255, 0.18) 0%,
      rgba(255, 255, 255, 0.06) 100%
    );
    border-color: rgba(255, 255, 255, 0.22);
    color: var(--fg-0);
    transform: scale(1.05);
  }
  .exit-btn:active {
    transform: scale(0.96);
  }
  .exit-btn:focus-visible {
    outline: none;
    box-shadow:
      0 6px 18px rgba(0, 0, 0, 0.35),
      inset 0 1px 0 rgba(255, 255, 255, 0.18),
      0 0 0 2px rgba(255, 255, 255, 0.18);
  }

  /* ============================================================ */
  /* Hero — cover or lyrics, same footprint so swap is stable.     */
  /* ============================================================ */
  .hero {
    position: relative;
    width: min(42dvh, 360px);
    aspect-ratio: 1 / 1;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .cover-frame {
    position: relative;
    z-index: 1;
    width: 100%;
    height: 100%;
    border-radius: 22px;
    overflow: hidden;
    box-shadow:
      0 40px 100px -25px rgba(0, 0, 0, 0.85),
      0 16px 40px -12px rgba(0, 0, 0, 0.6),
      0 0 0 1px rgba(255, 255, 255, 0.08),
      inset 0 1px 0 rgba(255, 255, 255, 0.08);
  }
  .cover-frame :global(.cover) {
    width: 100% !important;
    height: 100% !important;
    border-radius: 22px;
    box-shadow: none;
  }
  .lyrics-frame {
    position: relative;
    z-index: 1;
    width: 100%;
    height: 100%;
    background: linear-gradient(
      180deg,
      rgba(255, 255, 255, 0.05) 0%,
      rgba(0, 0, 0, 0.45) 100%
    );
    backdrop-filter: blur(28px) saturate(160%);
    -webkit-backdrop-filter: blur(28px) saturate(160%);
    border-radius: 22px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.1),
      0 20px 60px -20px rgba(0, 0, 0, 0.6);
    overflow: hidden;
  }

  /* ============================================================ */
  /* Title block — hero typography. The title sets the rhythm,     */
  /* artist and album step down in size and weight.                */
  /* ============================================================ */
  .text {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 6px;
    width: 100%;
    min-width: 0;
  }
  .title {
    font-size: clamp(20px, 3.2dvh, 26px);
    font-weight: 700;
    letter-spacing: -0.02em;
    line-height: 1.15;
    color: var(--fg-0);
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
    text-shadow: 0 1px 12px rgba(0, 0, 0, 0.5);
  }
  .artist {
    font-size: clamp(13px, 1.9dvh, 14px);
    font-weight: 500;
    color: var(--fg-1);
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
    text-shadow: 0 1px 8px rgba(0, 0, 0, 0.45);
  }
  .album {
    font-size: clamp(9px, 1.2dvh, 10px);
    font-weight: 600;
    color: var(--fg-2);
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
    text-transform: uppercase;
    letter-spacing: 0.16em;
    text-shadow: 0 1px 6px rgba(0, 0, 0, 0.45);
  }

  /* ============================================================ */
  /* Progress bar — slim glass tube with accent fill.              */
  /* ============================================================ */
  .progress-wrap {
    width: 100%;
    max-width: 480px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .progress {
    position: relative;
    height: 18px;
    cursor: pointer;
    outline: none;
    --pct: 0%;
  }
  /* Keyboard focus indicator for the seek slider (R2.2). The base
     rule drops the native outline for mouse use; on keyboard focus
     we draw an accent ring around the rail so the control stays
     visibly focused. */
  .progress:focus-visible {
    outline: 2px solid var(--accent, #8ab4ff);
    outline-offset: 4px;
    border-radius: 999px;
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
    box-shadow:
      inset 0 1px 1px rgba(0, 0, 0, 0.4),
      inset 0 -1px 0 rgba(255, 255, 255, 0.06);
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
      rgba(255, 255, 255, 0.85),
      rgba(255, 255, 255, 1)
    );
    border-radius: 999px;
    box-shadow:
      0 0 10px rgba(255, 255, 255, 0.3),
      inset 0 1px 0 rgba(255, 255, 255, 0.4);
    transition: height var(--dur-fast) var(--ease-out);
  }
  .thumb {
    position: absolute;
    left: var(--pct);
    top: 50%;
    width: 12px;
    height: 12px;
    transform: translate(-50%, -50%) scale(0);
    background: linear-gradient(
      180deg,
      #ffffff 0%,
      #e6e6ea 100%
    );
    border-radius: 50%;
    box-shadow:
      0 2px 8px rgba(0, 0, 0, 0.5),
      0 0 0 4px rgba(255, 255, 255, 0.18);
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
  .progress:focus-visible .rail {
    box-shadow:
      inset 0 1px 1px rgba(0, 0, 0, 0.4),
      inset 0 -1px 0 rgba(255, 255, 255, 0.06),
      0 0 0 2px rgba(255, 255, 255, 0.2);
  }
  .times {
    display: flex;
    justify-content: space-between;
    color: var(--fg-2);
    font-size: 11px;
    font-weight: 500;
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.04em;
    text-shadow: 0 1px 4px rgba(0, 0, 0, 0.5);
  }

  /* ============================================================ */
  /* Controls — flat, icon-first. Reference: Apple Music / Spotify */
  /* style — no glass discs, just icons in a row. The play icon   */
  /* is bigger and brighter than prev/next; no surrounding pill.  */
  /* ============================================================ */
  .controls {
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .controls.primary {
    gap: 36px;
  }
  .controls.secondary {
    gap: 20px;
    margin-top: 6px;
  }

  /* Icon-only button. Hit area is generous, but visually it is
     just the icon — no background, no border, no shadow. */
  .ctrl {
    background: transparent;
    border: none;
    color: var(--fg-1);
    width: 44px;
    height: 44px;
    border-radius: 999px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    transition: color var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out),
      opacity var(--dur-fast) var(--ease-out);
    opacity: 0.85;
  }
  .ctrl:hover {
    color: var(--fg-0);
    opacity: 1;
  }
  .ctrl:active {
    transform: scale(0.92);
  }
  .ctrl:focus-visible {
    outline: none;
    color: var(--fg-0);
    box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.16);
  }

  /* Play button — same flat treatment, just a larger icon and
     fully white. The size of the icon is what makes it the
     primary action; no disc, no glow. */
  .ctrl.play {
    width: 56px;
    height: 56px;
    color: var(--fg-0);
    opacity: 1;
  }
  .ctrl.play:hover {
    transform: scale(1.06);
  }
  .ctrl.play:active {
    transform: scale(0.96);
  }

  /* Small actions (favorite, lyrics) — same icon-only style at
     smaller scale. */
  .ctrl.small {
    width: 32px;
    height: 32px;
    color: var(--fg-2);
    opacity: 0.8;
  }
  .ctrl.small:hover {
    color: var(--fg-0);
    opacity: 1;
  }
  .ctrl.small.active {
    color: var(--fg-0);
    opacity: 1;
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
