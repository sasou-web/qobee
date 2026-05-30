<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { emitTo } from "@tauri-apps/api/event";
  import {
    nextTrack,
    prevTrack,
    seek,
    type Track,
    getTrack,
    toggleMiniPlayer,
  } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import { formatDuration } from "../lib/format";
  import { PLAYER_LABELS } from "../lib/labels";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";
  import PlayButton from "./PlayButton.svelte";

  // The MiniPlayer runs in its own webview window, so it has its
  // own Svelte runtime. We must wire the player events ourselves
  // (the main App's `app.wire()` lives in a separate JS context).
  // `app.wire()` is idempotent — calling it here is safe.
  let track = $state<Track | null>(null);
  let lastResolvedId = "";

  $effect(() => {
    const sid = app.player.current_track_id;
    if (!sid) {
      track = null;
      lastResolvedId = "";
      return;
    }
    if (sid === lastResolvedId) return;
    lastResolvedId = sid;
    const id = Number(sid);
    if (!Number.isFinite(id)) return;
    void getTrack(id)
      .then((t) => {
        if (lastResolvedId === sid) track = t;
      })
      .catch(() => {
        // best-effort
      });
  });

  let pos = $derived(app.player.position_seconds);
  let dur = $derived(Math.max(app.player.duration_seconds, track?.duration_seconds ?? 0));
  let pct = $derived(dur > 0 ? Math.min(100, (pos / dur) * 100) : 0);

  let alwaysOnTop = $state(true);
  async function toggleAlwaysOnTop(): Promise<void> {
    alwaysOnTop = !alwaysOnTop;
    try {
      await getCurrentWindow().setAlwaysOnTop(alwaysOnTop);
    } catch (e) {
      console.error(e);
    }
  }

  async function restoreMain(): Promise<void> {
    try {
      await toggleMiniPlayer(false);
    } catch (e) {
      console.error(e);
    }
  }

  // Open the dedicated queue view (R3.5). The MiniPlayer runs in its
  // own webview window, so `app.setView` here would only mutate the
  // mini's local store. Instead we restore the main window and ask it
  // to navigate via the same `deep-link:navigate` bus the tray uses;
  // App.svelte's listener routes "queue" to `app.setView("queue")`.
  async function openQueue(): Promise<void> {
    try {
      await toggleMiniPlayer(false);
      await emitTo("main", "deep-link:navigate", "queue");
    } catch (e) {
      console.error(e);
    }
  }

  function onProgressClick(e: MouseEvent): void {
    const target = e.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    if (dur <= 0) return;
    void seek(ratio * dur);
  }

  onMount(async () => {
    // Wire player state + events into the local app store so this
    // window updates live like the main app.
    try {
      await app.wire();
    } catch (e) {
      console.error("mini player: app.wire failed", e);
    }
    try {
      await getCurrentWindow().setAlwaysOnTop(true);
    } catch {
      // ignore
    }
  });
  onDestroy(() => {});
</script>

<div class="root" data-tauri-drag-region>
  <!-- Top bar with always-on-top + restore controls. The drag region
       is on the empty area; the buttons are explicitly no-drag. -->
  <div class="topbar" data-tauri-drag-region>
    <div class="spacer" data-tauri-drag-region></div>
    <button
      class="wctrl"
      onclick={openQueue}
      title={PLAYER_LABELS.queue.label}
      aria-label={PLAYER_LABELS.queue.label}
    >
      <Icon name="queue" size={16} />
    </button>
    <button
      class="wctrl"
      class:active={alwaysOnTop}
      onclick={toggleAlwaysOnTop}
      aria-pressed={alwaysOnTop}
      title={alwaysOnTop ? "Disable always on top" : "Always on top"}
      aria-label="Always on top"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M12 2 L12 22" />
        <path d="M5 9 L12 2 L19 9" />
      </svg>
    </button>
    <button
      class="wctrl restore"
      onclick={restoreMain}
      title="Restore main window"
      aria-label="Restore main window"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <polyline points="15 3 21 3 21 9" />
        <polyline points="9 21 3 21 3 15" />
        <line x1="21" y1="3" x2="14" y2="10" />
        <line x1="3" y1="21" x2="10" y2="14" />
      </svg>
    </button>
  </div>

  {#if track}
    <div class="body">
      <div class="cover">
        <Cover coverKey={track.cover_key} size={96} title={track.title} />
      </div>

      <div class="meta">
        <p class="title" title={track.title}>{track.title}</p>
        <p class="artist" title={track.artist}>{track.artist}</p>

        <button
          class="progress-row"
          type="button"
          onclick={onProgressClick}
          aria-label="Seek"
          title="Click to seek"
        >
          <div class="progress" aria-hidden="true">
            <span class="bar" style="width: {pct}%"></span>
          </div>
          <div class="times">
            <span>{formatDuration(pos)}</span>
            <span>{formatDuration(dur)}</span>
          </div>
        </button>

        <div class="controls">
          <button
            class="ghost"
            onclick={() => void prevTrack()}
            aria-label={PLAYER_LABELS.previous.label}
            title={PLAYER_LABELS.previous.label}
          >
            <Icon name="prev" size={18} />
          </button>
          <PlayButton
            target={{ kind: "track", id: track.id }}
            size="md"
            class="mini-play"
          />
          <button
            class="ghost"
            onclick={() => void nextTrack()}
            aria-label={PLAYER_LABELS.next.label}
            title={PLAYER_LABELS.next.label}
          >
            <Icon name="next" size={18} />
          </button>
        </div>
      </div>
    </div>
  {:else}
    <div class="empty">
      <p>Nothing playing</p>
      <p class="hint">Pick a track in the main window to see it here.</p>
    </div>
  {/if}
</div>

<style>
  :global(html, body) {
    background: var(--bg-1);
    height: 100%;
    margin: 0;
    overflow: hidden;
  }
  :global(#app) {
    height: 100%;
  }
  .root {
    position: relative;
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg-1);
    color: var(--fg-0);
    -webkit-user-select: none;
    user-select: none;
    overflow: hidden;
  }
  .topbar {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    height: 32px;
    padding: 4px 6px 0;
    flex: 0 0 32px;
    gap: 2px;
  }
  .spacer {
    flex: 1;
    height: 100%;
  }
  .wctrl {
    width: 32px;
    height: 28px;
    background: transparent;
    border: none;
    color: var(--fg-2);
    border-radius: var(--radius-sm);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    -webkit-app-region: no-drag;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .wctrl:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .wctrl.active {
    color: var(--accent);
  }
  .wctrl.restore:hover {
    background: var(--bg-3);
  }

  .body {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 12px;
    padding: 4px 12px 12px;
    flex: 1;
    min-height: 0;
  }
  .cover {
    width: 96px;
    height: 96px;
    border-radius: var(--radius-md);
    overflow: hidden;
    background: var(--bg-2);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);
  }
  .meta {
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    min-width: 0;
    gap: 4px;
  }
  .title {
    margin: 0;
    font-weight: 600;
    font-size: 14px;
    line-height: 1.25;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .artist {
    margin: 0;
    color: var(--fg-2);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .progress-row {
    background: transparent;
    border: none;
    padding: 0;
    margin: 0;
    cursor: pointer;
    display: block;
    width: 100%;
    text-align: left;
  }
  .progress {
    height: 4px;
    background: var(--bg-3);
    border-radius: 2px;
    overflow: hidden;
    transition: height var(--dur-fast) var(--ease-out);
  }
  .progress-row:hover .progress {
    height: 5px;
  }
  .bar {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.18s linear;
  }
  .times {
    display: flex;
    justify-content: space-between;
    font-size: 10px;
    color: var(--fg-3);
    margin-top: 3px;
    font-variant-numeric: tabular-nums;
  }
  .controls {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    margin-top: 2px;
  }
  .controls button {
    background: transparent;
    border: none;
    color: var(--fg-1);
    width: 32px;
    height: 32px;
    border-radius: 999px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    -webkit-app-region: no-drag;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out);
  }
  .controls .ghost:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .controls .ghost:active {
    transform: scale(0.95);
  }
  /* The PlayButton sized "md" is 36px which sits well between the
     32px ghost siblings. Use the named class to bump elevation
     slightly so it feels like the primary action. */
  :global(.controls .mini-play) {
    width: 38px;
    height: 38px;
    --pb-size: 38px;
  }

  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    padding: 8px;
    text-align: center;
  }
  .empty p {
    margin: 0;
    color: var(--fg-1);
    font-size: 13px;
    font-weight: 600;
  }
  .empty .hint {
    color: var(--fg-3);
    font-size: 11px;
    font-weight: 400;
  }
</style>
