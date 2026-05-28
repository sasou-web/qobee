<script lang="ts">
  import {
    addFavorite,
    albumIdForTrack,
    getRepeatMode,
    getShuffle,
    getTrack,
    isFavorite,
    nextTrack,
    prevTrack,
    removeFavorite,
    seek,
    setRepeatMode,
    setShuffle,
    setVolume,
    type RepeatMode,
    type Track,
  } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import { settings } from "../lib/settings.svelte";
  import { volumeSettings } from "../lib/audioSettings.svelte";
  import { stepDb } from "../lib/volumeFormat";
  import { formatDuration, formatOutputMode, formatQuality } from "../lib/format";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";
  import PlayButton from "./PlayButton.svelte";
  import { openTrackMenu } from "../lib/trackMenu";
  import { queuePopover } from "../lib/queuePopover.svelte";
  import { nowPlayingFullscreen } from "../lib/nowPlayingFullscreen.svelte";

  // Resolved metadata for the currently playing track. Cached so the
  // bar doesn't blink while a new track is being looked up.
  let nowTitle = $state<string>("");
  let nowArtist = $state<string>("");
  let nowAlbum = $state<string>("");
  let nowAlbumId = $state<number | null>(null);
  let nowCoverKey = $state<string | null>(null);
  let nowTrack = $state<Track | null>(null);
  let nowFavorite = $state<boolean>(false);
  let lastResolvedId = "";

  // Playback flags pulled from the engine on mount and after every
  // toggle. We cache locally so the icon flips immediately on click,
  // then rebroadcasts the truth from the backend.
  let shuffleOn = $state<boolean>(false);
  let repeatMode = $state<RepeatMode>("off");

  async function refreshNowPlaying(): Promise<void> {
    const id = app.player.current_track_id;
    if (id === null) {
      nowTitle = "";
      nowArtist = "";
      nowAlbum = "";
      nowAlbumId = null;
      nowCoverKey = null;
      nowTrack = null;
      nowFavorite = false;
      lastResolvedId = "";
      return;
    }
    if (id === lastResolvedId) return;
    lastResolvedId = id;
    const numId = Number(id);
    if (!Number.isFinite(numId)) return;
    try {
      const t = await getTrack(numId);
      if (t && String(t.id) === id) {
        nowTitle = t.title;
        nowArtist = t.artist;
        nowAlbum = t.album;
        nowCoverKey = t.cover_key;
        nowTrack = t;
        // These two are independent best-effort lookups: albumIdForTrack
        // round-trips to SQLite, isFavorite is a tiny COUNT. Failing
        // either is fine — we just lose the affordance.
        const [albumId, fav] = await Promise.all([
          albumIdForTrack(t.id).catch(() => null),
          isFavorite(t.id).catch(() => false),
        ]);
        // Make sure the user hasn't skipped to another track while we
        // were resolving — discard stale results.
        if (lastResolvedId === id) {
          nowAlbumId = albumId;
          nowFavorite = fav;
        }
      }
    } catch {
      // best-effort
    }
  }

  function goToAlbum(): void {
    if (nowAlbumId !== null) app.selectAlbum(nowAlbumId);
  }

  function goToArtist(): void {
    if (nowArtist) app.selectArtist(nowArtist);
  }

  function onNowContext(event: MouseEvent): void {
    if (!nowTrack) {
      event.preventDefault();
      return;
    }
    openTrackMenu(event, nowTrack);
  }

  async function refreshDevices(): Promise<void> {
    try {
      shuffleOn = await getShuffle();
      repeatMode = await getRepeatMode();
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function toggleShuffle(): Promise<void> {
    const v = !shuffleOn;
    shuffleOn = v;
    try {
      await setShuffle(v);
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function cycleRepeat(): Promise<void> {
    const next: RepeatMode =
      repeatMode === "off" ? "queue" : repeatMode === "queue" ? "track" : "off";
    repeatMode = next;
    try {
      await setRepeatMode(next);
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function toggleFavorite(): Promise<void> {
    if (!nowTrack) return;
    const next = !nowFavorite;
    nowFavorite = next;
    try {
      if (next) await addFavorite(nowTrack.id);
      else await removeFavorite(nowTrack.id);
    } catch (e) {
      app.lastError = String(e);
      // Revert on failure so the heart matches reality.
      nowFavorite = !next;
    }
  }

  $effect(() => {
    void app.player.current_track_id;
    void refreshNowPlaying();
  });

  $effect(() => {
    void refreshDevices();
  });

  let isPlaying = $derived(app.player.status === "playing");
  let progressPct = $derived.by(() => {
    const dur = app.player.duration_seconds;
    if (dur <= 0) return 0;
    return Math.min(100, (app.player.position_seconds / dur) * 100);
  });
  let volumePct = $derived(Math.round(app.player.volume * 100));
  let remainingSeconds = $derived(
    Math.max(0, app.player.duration_seconds - app.player.position_seconds)
  );

  // Lazily warm the cached `audio.volume_curve` / `audio.volume_floor_db`
  // pair so wheel scrolls behave correctly. The store returns spec
  // defaults synchronously while the load is in flight.
  void volumeSettings.ensureLoaded();

  async function handleSeek(e: Event): Promise<void> {
    const value = Number((e.target as HTMLInputElement).value);
    const dur = app.player.duration_seconds;
    if (dur <= 0) return;
    try {
      await seek((value / 100) * dur);
    } catch (err) {
      app.lastError = String(err);
    }
  }

  async function handleVolume(e: Event): Promise<void> {
    const value = Number((e.target as HTMLInputElement).value) / 100;
    try {
      await setVolume(value);
      await settings.set("defaultVolume", value);
    } catch (err) {
      app.lastError = String(err);
    }
  }

  // Mouse-wheel volume. Steps by ±2 dB per notch (R4.6) by deferring
  // to `stepDb` so the slider's audible delta stays uniform across
  // the curve regardless of the user's selected `audio.volume_curve`.
  let pendingVolumePersist: ReturnType<typeof setTimeout> | null = null;
  async function handleVolumeWheel(e: WheelEvent): Promise<void> {
    e.preventDefault();
    const direction = e.deltaY > 0 ? -1 : 1;
    const next = stepDb(
      app.player.volume,
      direction * 2.0,
      volumeSettings.curve,
      volumeSettings.floorDb
    );
    if (next === app.player.volume) return;
    try {
      await setVolume(next);
      app.player = { ...app.player, volume: next };
      if (pendingVolumePersist) clearTimeout(pendingVolumePersist);
      pendingVolumePersist = setTimeout(() => {
        void settings.set("defaultVolume", next);
      }, 250);
    } catch (err) {
      app.lastError = String(err);
    }
  }
</script>

<footer class="bar">
  <!-- Top progress strip: a thin seek bar with the elapsed time on
       its left and the remaining time on its right. Styled to feel
       like a single horizontal unit (no full-width bleed). -->
  <div class="seek-row">
    <span class="seek-time tabular">{formatDuration(app.player.position_seconds)}</span>
    <div class="seek-track">
      <input
        class="seek"
        type="range"
        min="0"
        max="100"
        step="0.1"
        value={progressPct}
        oninput={handleSeek}
        style:--range-fill={`${progressPct}%`}
        aria-label="Seek"
      />
    </div>
    <span class="seek-time tabular">−{formatDuration(remainingSeconds)}</span>
  </div>

  <div class="grid">
    <!-- Left: transport + secondary actions. -->
    <div class="left">
      <button
        class="ctrl"
        class:active={shuffleOn}
        onclick={toggleShuffle}
        aria-label={shuffleOn ? "Shuffle on" : "Shuffle off"}
        title={shuffleOn ? "Shuffle on" : "Shuffle off"}
      >
        <Icon name="shuffle" size={15} />
      </button>
      <button
        class="ctrl"
        onclick={() => prevTrack()}
        aria-label="Previous"
      >
        <Icon name="prev" size={18} />
      </button>
      {#if nowTrack}
        <PlayButton
          target={{ kind: "track", id: nowTrack.id }}
          size="md"
          class="ctrl-play"
        />
      {:else}
        <!-- No track loaded — render a disabled placeholder that
             matches the button's footprint so the row layout
             doesn't shift when a track gets queued. -->
        <button
          class="ctrl play"
          disabled
          aria-label="Play / pause"
          title="Nothing to play"
        >
          <Icon name="play" size={22} />
        </button>
      {/if}
      <button class="ctrl" onclick={() => nextTrack()} aria-label="Next">
        <Icon name="next" size={18} />
      </button>
      <button
        class="ctrl"
        class:active={repeatMode !== "off"}
        onclick={cycleRepeat}
        aria-label={`Repeat: ${repeatMode}`}
        title={`Repeat: ${repeatMode}`}
      >
        <Icon name={repeatMode === "track" ? "repeat-one" : "repeat"} size={15} />
      </button>
    </div>

    <!-- Center: now-playing card. The whole card opens the
         fullscreen Now Playing view; album / artist links and the
         heart stay independently clickable thanks to event
         stoppers. -->
    <div
      class="center"
      class:has-track={nowTitle.length > 0}
      class:playing={isPlaying}
      oncontextmenu={onNowContext}
      role="presentation"
    >
      <button
        class="card-hit"
        onclick={() => nowPlayingFullscreen.toggle()}
        title={nowTitle ? "Open Now Playing (F)" : "Now Playing"}
        aria-label="Open Now Playing"
        type="button"
      ></button>
      <div class="cover-wrap">
        <Cover coverKey={nowCoverKey} size={48} title={nowTitle} />
        {#if isPlaying}
          <div class="eq" aria-hidden="true">
            <span></span><span></span><span></span>
          </div>
        {/if}
      </div>
      <div class="text">
        <div class="title" title={nowTitle}>{nowTitle || "—"}</div>
        <div class="meta">
          {#if nowAlbum}
            <button
              class="link"
              onclick={(e) => {
                e.stopPropagation();
                goToAlbum();
              }}
              disabled={nowAlbumId === null}
              title={nowAlbumId === null ? nowAlbum : `Open ${nowAlbum}`}
            >
              {nowAlbum}
            </button>
          {/if}
          {#if nowAlbum && nowArtist}
            <span class="dot" aria-hidden="true">·</span>
          {/if}
          {#if nowArtist}
            <button
              class="link"
              onclick={(e) => {
                e.stopPropagation();
                goToArtist();
              }}
              title={`Open ${nowArtist}`}
            >
              {nowArtist}
            </button>
          {/if}
          {#if !nowAlbum && !nowArtist}
            <span class="muted">—</span>
          {/if}
        </div>
      </div>
      <button
        class="heart"
        class:filled={nowFavorite}
        onclick={(e) => {
          e.stopPropagation();
          void toggleFavorite();
        }}
        disabled={!nowTrack}
        aria-label={nowFavorite ? "Remove from favorites" : "Add to favorites"}
        title={nowFavorite ? "Remove from favorites" : "Add to favorites"}
      >
        <Icon name={nowFavorite ? "heart-filled" : "heart"} size={15} />
      </button>
    </div>

    <!-- Right: format badges, queue, volume. -->
    <div class="right">
      <span class="badge quality" title={`${formatOutputMode(app.player.output_mode)} · ${formatQuality(app.player.sample_rate, app.player.bit_depth)}`}>
        {formatQuality(app.player.sample_rate, app.player.bit_depth)}
      </span>

      <button
        class="ctrl"
        class:active={queuePopover.open}
        onmousedown={(e) => e.stopPropagation()}
        onclick={() => queuePopover.toggle()}
        aria-label="Show queue"
        title="Up next"
      >
        <Icon name="queue" size={15} />
      </button>

      <div class="volume" onwheel={handleVolumeWheel} role="group" aria-label="Volume">
        <Icon name="volume" size={13} />
        <input
          type="range"
          min="0"
          max="100"
          step="1"
          value={app.player.volume * 100}
          oninput={handleVolume}
          style:--range-fill={`${volumePct}%`}
          aria-label="Volume"
          title={`Volume: ${volumePct}%`}
        />
      </div>
    </div>
  </div>
</footer>

<style>
  .bar {
    grid-column: 2 / -1;
    grid-row: 3;
    position: relative;
    /* The bar stacks: a slim seek row on top, then the controls
       grid. We tighten vertical padding so the strip doesn't claim
       more screen than it needs while staying comfortable to
       click. No top border — the bar is read as continuous with
       the content area thanks to the shared --bg-shell. */
    height: auto;
    min-height: var(--player-height);
    background: var(--bg-shell);
    border-top: none;
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow: hidden;
  }

  /* Seek row — pinned above the controls. Horizontal padding
     deliberately a touch wider than the grid below so the
     timestamps and rail tuck inside the visible bar without
     hugging its edges. The vertical padding gives the rail air
     above and a hair below the controls grid. */
  .seek-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 18px 2px;
    width: 100%;
    box-sizing: border-box;
  }
  .seek-time {
    color: var(--fg-3, var(--fg-2));
    font-size: 10px;
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.02em;
    flex: 0 0 auto;
    min-width: 32px;
    text-align: center;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .seek-row:hover .seek-time {
    color: var(--fg-1);
  }
  .seek-track {
    flex: 1 1 auto;
    position: relative;
    height: 14px;
    display: flex;
    align-items: center;
  }
  .seek {
    width: 100%;
    height: 14px;
    margin: 0;
    padding: 0;
    background: transparent;
    cursor: pointer;
    --range-fill: 0%;
  }
  .seek::-webkit-slider-runnable-track {
    height: 4px;
    border-radius: 999px;
    background: linear-gradient(
      to right,
      var(--accent) 0%,
      var(--accent) var(--range-fill),
      rgba(255, 255, 255, 0.08) var(--range-fill),
      rgba(255, 255, 255, 0.08) 100%
    );
    transition:
      height var(--dur-fast) var(--ease-out),
      background var(--dur-fast) var(--ease-out);
  }
  .seek-row:hover .seek::-webkit-slider-runnable-track {
    height: 6px;
    background: linear-gradient(
      to right,
      var(--accent) 0%,
      var(--accent) var(--range-fill),
      rgba(255, 255, 255, 0.14) var(--range-fill),
      rgba(255, 255, 255, 0.14) 100%
    );
  }
  .seek::-moz-range-track {
    height: 4px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
  }
  .seek::-moz-range-progress {
    height: 4px;
    border-radius: 999px;
    background: var(--accent);
  }
  .seek::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 12px;
    height: 12px;
    margin-top: -4px;
    border-radius: 50%;
    background: var(--fg-0);
    box-shadow:
      0 2px 6px rgba(0, 0, 0, 0.45),
      0 0 0 0 var(--accent-glow);
    opacity: 0;
    transform: scale(0.6);
    transition:
      opacity var(--dur-fast) var(--ease-out),
      transform var(--dur-base) var(--ease-spring),
      box-shadow var(--dur-base) var(--ease-out);
  }
  .seek-row:hover .seek::-webkit-slider-thumb {
    opacity: 1;
    transform: scale(1);
    box-shadow:
      0 2px 6px rgba(0, 0, 0, 0.45),
      0 0 0 4px var(--accent-glow);
  }

  .grid {
    flex: 1 1 auto;
    min-height: 60px;
    display: grid;
    grid-template-columns: auto minmax(220px, 1.4fr) auto;
    align-items: center;
    gap: var(--space-4);
    padding: 4px var(--space-4) 8px;
    min-width: 0;
  }
  .left,
  .center,
  .right {
    min-width: 0;
  }

  /* --- shared button --- */
  .ctrl {
    position: relative;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--fg-1);
    cursor: pointer;
    padding: 0;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out),
      transform var(--dur-base) var(--ease-spring);
  }
  .ctrl:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--fg-0);
  }
  .ctrl:active:not(:disabled) {
    transform: scale(0.92);
  }
  .ctrl.active {
    color: var(--accent);
  }
  .ctrl.active::after {
    /* Small accent dot under the icon — clearer "this is on"
       affordance than tinting the icon alone, and lighter than a
       full background pill. */
    content: "";
    position: absolute;
    bottom: 3px;
    left: 50%;
    transform: translateX(-50%);
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--accent);
  }
  .ctrl.active:hover {
    background: rgba(255, 255, 255, 0.06);
  }
  /* Disabled placeholder rendered when no track is loaded yet.
     Matches the flat-icon style of the live PlayButton: no disc,
     no shadow, just a slightly bigger icon than its siblings. The
     opacity is dialled down via :disabled so users see "nothing
     to play here" without the placeholder competing visually
     with the active controls. */
  .ctrl.play {
    width: 36px;
    height: 36px;
    background: transparent;
    color: var(--fg-0);
    box-shadow: none;
  }
  .ctrl.play:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .ctrl.play:hover:not(:disabled) {
    transform: scale(1.06);
  }
  .ctrl.play:active:not(:disabled) {
    transform: scale(0.94);
  }

  /* Shared PlayButton sized to fit the transport row. We override
     the PlayButton color tokens here so the player-bar play
     control is a flat icon — no disc, no fill — matching the
     prev/next/shuffle/repeat siblings. The icon is just larger.
     Background, shadow, and hover halo are removed; only the
     icon scales on hover to confirm the hit. */
  :global(.left .ctrl-play) {
    --pb-size: 36px;
    --pb-icon: 22px;
    --pb-bg: transparent;
    --pb-fg: var(--fg-0);
    --pb-shadow: none;
  }
  /* Kill the global hover halo (`box-shadow: var(--accent-glow)`)
     so a transparent disc doesn't ghost behind the icon. */
  :global(.left .ctrl-play:hover:not(:disabled)) {
    box-shadow: none;
    transform: scale(1.06);
  }
  :global(.left .ctrl-play:active:not(:disabled)) {
    transform: scale(0.94);
  }
  :global(.left .ctrl-play.state-error) {
    --pb-bg: transparent;
    --pb-fg: var(--danger, #c0392b);
  }

  /* --- left zone --- */
  .left {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .tabular {
    font-variant-numeric: tabular-nums;
  }

  /* --- center zone --- */
  .center {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-width: 0;
    padding: 6px 10px 6px 6px;
    border-radius: var(--radius-md);
    background: rgba(255, 255, 255, 0.02);
    border: 1px solid rgba(255, 255, 255, 0.06);
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out);
  }
  .center.has-track:hover {
    background: rgba(255, 255, 255, 0.05);
    border-color: rgba(255, 255, 255, 0.1);
    box-shadow: 0 4px 14px -8px rgba(0, 0, 0, 0.5);
  }
  /* Full-card clickable hit area sitting behind every visual
     element. The cover, title, links and heart all sit on top of it
     via z-index; clicks on those propagate normally and are stopped
     by the inner buttons when needed. */
  .card-hit {
    position: absolute;
    inset: 0;
    background: transparent;
    border: none;
    border-radius: inherit;
    padding: 0;
    margin: 0;
    cursor: pointer;
    z-index: 0;
  }
  .card-hit:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .cover-wrap {
    position: relative;
    flex-shrink: 0;
    pointer-events: none;
    z-index: 1;
  }
  .cover-wrap :global(.cover) {
    border-radius: var(--radius-s);
  }
  .center.playing .cover-wrap {
    animation: breathe 4s var(--ease-in-out) infinite;
  }
  @keyframes breathe {
    0%, 100% { transform: scale(1); }
    50% { transform: scale(1.03); }
  }
  .eq {
    position: absolute;
    inset: auto 3px 3px auto;
    display: flex;
    align-items: flex-end;
    gap: 1.5px;
    height: 11px;
    padding: 1.5px 2px;
    background: rgba(0, 0, 0, 0.55);
    border-radius: 3px;
    backdrop-filter: blur(4px);
  }
  .eq span {
    width: 1.5px;
    background: var(--accent);
    border-radius: 1px;
    transform-origin: bottom;
    animation: bar 900ms var(--ease-in-out) infinite;
  }
  .eq span:nth-child(1) { animation-delay: 0ms; height: 60%; }
  .eq span:nth-child(2) { animation-delay: 180ms; height: 100%; }
  .eq span:nth-child(3) { animation-delay: 360ms; height: 75%; }
  @keyframes bar {
    0%, 100% { transform: scaleY(0.4); }
    50% { transform: scaleY(1); }
  }
  .center .text {
    flex: 1 1 auto;
    min-width: 0;
    max-width: 360px;
    pointer-events: none;
    z-index: 1;
  }
  .title {
    color: var(--fg-0);
    font-weight: 600;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    margin-top: 1px;
    color: var(--fg-2);
    font-size: 11px;
    display: flex;
    align-items: baseline;
    gap: 4px;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }
  .meta .dot {
    color: var(--fg-3, var(--fg-2));
    flex-shrink: 0;
  }
  .meta .muted {
    color: var(--fg-3, var(--fg-2));
  }
  .link {
    background: transparent;
    border: none;
    padding: 0;
    margin: 0;
    color: var(--fg-2);
    cursor: pointer;
    font: inherit;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    pointer-events: auto;
    position: relative;
    z-index: 2;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .link:hover:not(:disabled) {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .link:disabled {
    cursor: default;
  }
  .heart {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--fg-3);
    cursor: pointer;
    padding: 0;
    flex-shrink: 0;
    margin-left: 4px;
    pointer-events: auto;
    position: relative;
    z-index: 2;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out),
      transform var(--dur-base) var(--ease-spring);
  }
  .heart:hover:not(:disabled) {
    background: rgba(255, 95, 126, 0.1);
    color: #ff5f7e;
  }
  .heart:active:not(:disabled) {
    transform: scale(0.85);
  }
  .heart.filled {
    color: #ff5f7e;
  }
  .heart:disabled {
    opacity: 0.3;
    cursor: default;
  }

  /* --- right zone --- */
  .right {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    justify-content: flex-end;
    min-width: 0;
  }
  .badge {
    font-size: 10px;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    padding: 0 8px;
    height: 22px;
    display: inline-flex;
    align-items: center;
    border-radius: 4px;
    background: transparent;
    border: 1px solid transparent;
    color: var(--fg-3, var(--fg-2));
    white-space: nowrap;
    min-width: 0;
    max-width: 160px;
    overflow: hidden;
    text-overflow: ellipsis;
    transition:
      color var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out),
      background var(--dur-fast) var(--ease-out);
  }
  .badge.quality:hover {
    color: var(--fg-1);
    border-color: rgba(255, 255, 255, 0.08);
    background: rgba(255, 255, 255, 0.03);
  }
  .volume {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 120px;
    min-width: 90px;
    color: var(--fg-2);
    padding-left: 4px;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .volume:hover {
    color: var(--fg-1);
  }
  .volume :global(svg) {
    flex-shrink: 0;
    opacity: 0.7;
  }
  .volume input[type="range"] {
    height: 14px;
    flex: 1 1 auto;
    min-width: 60px;
  }
</style>
