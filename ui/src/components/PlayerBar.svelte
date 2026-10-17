<script lang="ts">
  import { onMount } from "svelte";
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
  import { playerStore } from "../lib/playerStore.svelte";
  import { settings } from "../lib/settings.svelte";
  import { formatDuration, formatQuality } from "../lib/format";
  import { PLAYER_LABELS } from "../lib/labels";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";
  import PlayButton from "./PlayButton.svelte";
  import SleepTimer from "./SleepTimer.svelte";
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

  // Mute state, Kopuz-style: the mute button remembers the volume it
  // muted from and restores it on un-mute. `isMuted` tracks whether
  // the slider currently sits at zero.
  let isMuted = $derived(playerStore.volume <= 0);
  let volumeBeforeMute = $state<number>(1);

  async function refreshNowPlaying(): Promise<void> {
    const id = playerStore.currentTrackId;
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
    void playerStore.currentTrackId;
    void refreshNowPlaying();
  });

  $effect(() => {
    void refreshDevices();
  });

  let isPlaying = $derived(playerStore.status === "playing");
  // While the user is dragging the seek bar we drive the fill + thumb
  // from a local preview (`seekPreviewPct`) so they track the pointer
  // in real time. Without this the native thumb jumps to the cursor
  // instantly while the purple fill waits for the engine to echo a
  // `player:position` event back — the "fill lags the thumb" bug.
  let seeking = $state<boolean>(false);
  let seekPreviewPct = $state<number>(0);
  let progressPct = $derived.by(() => {
    if (seeking) return seekPreviewPct;
    const dur = playerStore.durationSeconds;
    if (dur <= 0) return 0;
    return Math.min(100, (playerStore.positionSeconds / dur) * 100);
  });
  let volumePct = $derived(Math.round(playerStore.volume * 100));
  let remainingSeconds = $derived(
    Math.max(0, playerStore.durationSeconds - playerStore.positionSeconds)
  );

  // Live drag: keep the fill + thumb glued to the pointer by feeding
  // a local preview, and don't hit the engine on every input tick.
  function onSeekInput(e: Event): void {
    seeking = true;
    seekPreviewPct = Number((e.target as HTMLInputElement).value);
  }

  // Release (pointer up / keyboard commit): seek the engine once to
  // the final value, then drop back to the store-driven position.
  async function onSeekCommit(e: Event): Promise<void> {
    const value = Number((e.target as HTMLInputElement).value);
    seekPreviewPct = value;
    const dur = playerStore.durationSeconds;
    if (dur > 0) {
      // Optimistically move the store so the fill stays put through
      // the round-trip instead of snapping back to the old position.
      playerStore.positionMs = (value / 100) * dur * 1000;
      try {
        await seek((value / 100) * dur);
      } catch (err) {
        app.lastError = String(err);
      }
    }
    seeking = false;
  }

  // Volume drag. Fully custom slider (no native <input>): we compute
  // the value from the pointer X relative to the track width, update
  // the store optimistically so the fill + thumb track the pointer in
  // real time, and debounce the settings write so we don't hit SQLite
  // on every move.
  let pendingVolumePersist: ReturnType<typeof setTimeout> | null = null;
  let volTrackEl = $state<HTMLElement | null>(null);
  let volDragging = $state<boolean>(false);

  function persistVolumeSoon(value: number): void {
    if (pendingVolumePersist) clearTimeout(pendingVolumePersist);
    pendingVolumePersist = setTimeout(() => {
      void settings.set("defaultVolume", value);
    }, 250);
  }

  async function applyVolume(value: number): Promise<void> {
    const v = Math.min(1, Math.max(0, value));
    playerStore.volume = v;
    if (v > 0) volumeBeforeMute = v;
    try {
      await setVolume(v);
      persistVolumeSoon(v);
    } catch (err) {
      app.lastError = String(err);
    }
  }

  /** Map a pointer clientX onto a 0..1 volume from the track geometry. */
  function volumeFromPointer(clientX: number): number {
    if (!volTrackEl) return playerStore.volume;
    const rect = volTrackEl.getBoundingClientRect();
    if (rect.width <= 0) return playerStore.volume;
    return (clientX - rect.left) / rect.width;
  }

  function onVolPointerDown(e: PointerEvent): void {
    e.preventDefault();
    volDragging = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    void applyVolume(volumeFromPointer(e.clientX));
  }

  function onVolPointerMove(e: PointerEvent): void {
    if (!volDragging) return;
    void applyVolume(volumeFromPointer(e.clientX));
  }

  function onVolPointerUp(e: PointerEvent): void {
    if (!volDragging) return;
    volDragging = false;
    try {
      (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }
  }

  function onVolKeydown(e: KeyboardEvent): void {
    let next = playerStore.volume;
    if (e.key === "ArrowRight" || e.key === "ArrowUp") next += 0.05;
    else if (e.key === "ArrowLeft" || e.key === "ArrowDown") next -= 0.05;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = 1;
    else return;
    e.preventDefault();
    void applyVolume(next);
  }

  // Mute / un-mute toggle (Kopuz-style). Muting remembers the current
  // level; un-muting restores it (falling back to 1.0 if we muted from
  // an already-silent slider).
  async function toggleMute(): Promise<void> {
    if (playerStore.volume > 0) {
      volumeBeforeMute = playerStore.volume;
      await applyVolume(0);
    } else {
      await applyVolume(volumeBeforeMute > 0 ? volumeBeforeMute : 1);
    }
  }

  // Mouse-wheel volume. Steps by a fixed ±0.02 (2%) per notch, clamped
  // to [0, 1]. The simplified engine maps the slider linearly to gain.
  async function handleVolumeWheel(e: WheelEvent): Promise<void> {
    e.preventDefault();
    const direction = e.deltaY > 0 ? -1 : 1;
    const next = Math.min(1, Math.max(0, playerStore.volume + direction * 0.02));
    if (next === playerStore.volume) return;
    await applyVolume(next);
  }

  // --- Relayout on resize / restore (R1.5) ----------------------------------
  // When the window goes hidden/minimised → restored/maximised the
  // WebView can lag a frame before it reflows. We schedule a single
  // `requestAnimationFrame` that forces a synchronous reflow of the
  // Player_Zone (this footer) and, via the document body, the album /
  // artist headers — so everything re-flows well under 500 ms without
  // leaving content overflowing or popping an unexpected scrollbar.
  // Lightweight pattern, mirrored from `Sidebar.svelte`'s rAF guard.
  let barEl = $state<HTMLElement | null>(null);
  let relayoutScheduled = false;

  function scheduleRelayout(): void {
    if (relayoutScheduled) return;
    relayoutScheduled = true;
    requestAnimationFrame(() => {
      relayoutScheduled = false;
      // Reading layout properties forces the engine to recompute the
      // box model now (a forced synchronous reflow) rather than on a
      // later, possibly delayed, frame. We touch the footer and the
      // document body so both the Player_Zone and the page headers
      // re-measure against the new viewport size.
      void barEl?.offsetHeight;
      void document.body.offsetWidth;
    });
  }

  onMount(() => {
    // `resize` covers maximise / restore drags; `focus` catches the
    // hidden→restored transition (minimise to tray then reopen) where
    // a resize event may not fire.
    window.addEventListener("resize", scheduleRelayout);
    window.addEventListener("focus", scheduleRelayout);
    // A ResizeObserver on the footer itself nudges a relayout when the
    // bar's own box changes (e.g. sidebar width change reflows the
    // grid column the bar lives in).
    const ro = new ResizeObserver(scheduleRelayout);
    if (barEl) ro.observe(barEl);
    return () => {
      window.removeEventListener("resize", scheduleRelayout);
      window.removeEventListener("focus", scheduleRelayout);
      ro.disconnect();
    };
  });
</script>

<footer class="bar player-zone" bind:this={barEl}>
  <!-- Top progress strip: a thin seek bar with the elapsed time on
       its left and the remaining time on its right. Styled to feel
       like a single horizontal unit (no full-width bleed). -->
  <div class="seek-row">
    <span class="seek-time tabular">{formatDuration(playerStore.positionSeconds)}</span>
    <div class="seek-track">
      <input
        class="seek"
        type="range"
        min="0"
        max="100"
        step="0.1"
        value={progressPct}
        oninput={onSeekInput}
        onchange={onSeekCommit}
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
        aria-pressed={shuffleOn}
        aria-label={shuffleOn ? "Shuffle on" : "Shuffle off"}
        title={shuffleOn ? "Shuffle on" : "Shuffle off"}
      >
        <Icon name="shuffle" size={15} />
      </button>
      <button
        class="ctrl"
        onclick={() => prevTrack()}
        aria-label={PLAYER_LABELS.previous.label}
        title={PLAYER_LABELS.previous.label}
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
      <button class="ctrl" onclick={() => nextTrack()} aria-label={PLAYER_LABELS.next.label} title={PLAYER_LABELS.next.label}>
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
        title={PLAYER_LABELS.immersive.label}
        aria-label={PLAYER_LABELS.immersive.label}
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
        aria-pressed={nowFavorite}
        aria-label={nowFavorite ? PLAYER_LABELS.unfavorite.label : PLAYER_LABELS.favorite.label}
        title={nowFavorite ? PLAYER_LABELS.unfavorite.label : PLAYER_LABELS.favorite.label}
      >
        <Icon name={nowFavorite ? "heart-filled" : "heart"} size={15} />
      </button>
    </div>

    <!-- Right: format badges, queue, volume. -->
    <div class="right">
      <span class="badge quality" title={formatQuality(playerStore.sampleRate, playerStore.bitDepth)}>
        {formatQuality(playerStore.sampleRate, playerStore.bitDepth)}
      </span>

      <button
        class="ctrl"
        class:active={queuePopover.open}
        onmousedown={(e) => e.stopPropagation()}
        onclick={() => queuePopover.toggle()}
        aria-pressed={queuePopover.open}
        aria-label={PLAYER_LABELS.queue.label}
        title={PLAYER_LABELS.queue.label}
      >
        <Icon name="queue" size={15} />
      </button>

      <SleepTimer />

      <div class="volume" onwheel={handleVolumeWheel} role="group" aria-label="Volume">
        <button
          class="vol-btn"
          onclick={toggleMute}
          aria-label={isMuted ? "Réactiver le son" : "Couper le son"}
          title={isMuted ? "Réactiver le son" : "Couper le son"}
        >
          <Icon name={isMuted ? "volume-mute" : "volume"} size={14} />
        </button>
        <div
          class="vol-track"
          class:dragging={volDragging}
          bind:this={volTrackEl}
          onpointerdown={onVolPointerDown}
          onpointermove={onVolPointerMove}
          onpointerup={onVolPointerUp}
          onpointercancel={onVolPointerUp}
          onkeydown={onVolKeydown}
          role="slider"
          tabindex="0"
          aria-label="Volume"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={volumePct}
          aria-valuetext={`${volumePct}%`}
          title={`Volume: ${volumePct}%`}
        >
          <div class="vol-fill" style:width={`${volumePct}%`}></div>
          <span class="vol-thumb" style:left={`${volumePct}%`}></span>
        </div>
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
    width: 124px;
    min-width: 96px;
    color: var(--fg-2);
    padding-left: 4px;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .volume:hover {
    color: var(--fg-1);
  }
  /* Mute / un-mute button — flat icon that brightens on hover. */
  .vol-btn {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--fg-2);
    cursor: pointer;
    border-radius: 50%;
    transition:
      color var(--dur-fast) var(--ease-out),
      background var(--dur-fast) var(--ease-out);
  }
  .vol-btn:hover {
    color: var(--fg-0);
    background: rgba(255, 255, 255, 0.06);
  }

  /* Fully-custom volume slider (no native <input>): a div track with a
     fill + thumb both driven by the same `volumePct`, so they always
     stay aligned. Grey at rest, accent on hover; thumb only on hover. */
  .vol-track {
    position: relative;
    flex: 1 1 auto;
    min-width: 60px;
    height: 14px;
    display: flex;
    align-items: center;
    cursor: pointer;
    touch-action: none;
  }
  .vol-track::before {
    /* the rail */
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    transform: translateY(-50%);
    height: 4px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.12);
  }
  .vol-fill {
    position: absolute;
    left: 0;
    top: 50%;
    transform: translateY(-50%);
    height: 4px;
    border-radius: 999px;
    background: var(--fg-1);
    pointer-events: none;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .volume:hover .vol-fill,
  .vol-track.dragging .vol-fill {
    background: var(--accent);
  }
  .vol-thumb {
    position: absolute;
    top: 50%;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    background: var(--fg-0);
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.5);
    transform: translate(-50%, -50%);
    pointer-events: none;
    opacity: 0;
    transition: opacity var(--dur-fast) var(--ease-out);
  }
  .volume:hover .vol-thumb,
  .vol-track.dragging .vol-thumb {
    opacity: 1;
  }
  .vol-track:focus-visible {
    outline: none;
  }
  .vol-track:focus-visible .vol-thumb {
    opacity: 1;
    box-shadow: 0 0 0 4px var(--accent-glow);
  }
</style>
