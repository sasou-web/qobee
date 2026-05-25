<script lang="ts">
  import { fly } from "svelte/transition";
  import { flip } from "svelte/animate";
  import {
    getEndless,
    getQueue,
    getTracks,
    queueJumpTo,
    queueMove,
    queueRemoveAt,
    setEndless,
    type QueueView as QueueViewT,
    type Track,
  } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import { formatDuration } from "../lib/format";
  import { queuePopover } from "../lib/queuePopover.svelte";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";
  import { openTrackMenu } from "../lib/trackMenu";

  let queue = $state<QueueViewT>({ items: [], cursor: null });
  let trackMap = $state<Map<number, Track>>(new Map());
  let loading = $state<boolean>(false);
  let endless = $state<boolean>(true);

  // Drag state for reorder. Same flow as the dedicated view.
  let dragFromIdx = $state<number | null>(null);
  let dragOverIdx = $state<number | null>(null);

  // The popover element so click-outside can ignore clicks landing
  // inside it (the trigger button explicitly toggles the store, so
  // its own click never reaches us first).
  let popEl: HTMLDivElement | null = $state(null);

  async function load(): Promise<void> {
    loading = true;
    try {
      queue = await getQueue();
      const missing = queue.items.filter((id) => !trackMap.has(id));
      if (missing.length > 0) {
        const fetched = await getTracks(missing);
        const next = new Map(trackMap);
        for (const t of fetched) next.set(t.id, t);
        trackMap = next;
      }
    } catch (e) {
      app.lastError = String(e);
    } finally {
      loading = false;
    }
  }

  async function refreshEndless(): Promise<void> {
    try {
      endless = await getEndless();
    } catch {
      // best-effort
    }
  }

  async function toggleEndless(): Promise<void> {
    const next = !endless;
    endless = next;
    try {
      await setEndless(next);
    } catch (e) {
      app.lastError = String(e);
      endless = !next;
    }
  }

  // Reload whenever the popover opens or the playing track changes
  // (covers auto-advance, manual next, play_next inserts).
  $effect(() => {
    if (queuePopover.open) {
      void load();
      void refreshEndless();
    }
  });
  $effect(() => {
    void app.player.current_track_id;
    if (queuePopover.open) void load();
  });

  // Items decorated with index + flags so the template stays clean.
  const decorated = $derived.by(() =>
    queue.items.map((id, idx) => ({
      idx,
      track: trackMap.get(id) ?? null,
      isCurrent: queue.cursor === idx,
      isPast: queue.cursor !== null && idx < queue.cursor,
    }))
  );

  const upcomingDuration = $derived.by(() => {
    if (queue.cursor === null) return 0;
    return queue.items
      .slice(queue.cursor + 1)
      .map((id) => trackMap.get(id)?.duration_seconds ?? 0)
      .reduce((a, b) => a + b, 0);
  });

  async function handleJump(idx: number): Promise<void> {
    try {
      await queueJumpTo(idx);
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function handleRemove(idx: number, e: Event): Promise<void> {
    e.stopPropagation();
    try {
      await queueRemoveAt(idx);
      await load();
    } catch (err) {
      app.lastError = String(err);
    }
  }

  function onDragStart(idx: number, e: DragEvent): void {
    dragFromIdx = idx;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", String(idx));
    }
  }

  function onDragOver(idx: number, e: DragEvent): void {
    if (dragFromIdx === null) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    dragOverIdx = idx;
  }

  async function onDrop(idx: number, e: DragEvent): Promise<void> {
    e.preventDefault();
    if (dragFromIdx === null) return;
    const from = dragFromIdx;
    dragFromIdx = null;
    dragOverIdx = null;
    if (from === idx) return;
    try {
      await queueMove(from, idx);
      await load();
    } catch (err) {
      app.lastError = String(err);
    }
  }

  function onDragEnd(): void {
    dragFromIdx = null;
    dragOverIdx = null;
  }

  // Close on click outside / Escape. The trigger button stops
  // propagation on its own click so it doesn't immediately reopen
  // through this listener.
  function onWindowClick(e: MouseEvent): void {
    if (!queuePopover.open) return;
    if (popEl && popEl.contains(e.target as Node)) return;
    queuePopover.close();
  }
  function onKey(e: KeyboardEvent): void {
    if (queuePopover.open && e.key === "Escape") {
      queuePopover.close();
    }
  }
</script>

<svelte:window onmousedown={onWindowClick} onkeydown={onKey} />

{#if queuePopover.open}
  <div
    class="popover"
    bind:this={popEl}
    role="dialog"
    aria-label="Up next"
    transition:fly={{ y: 10, duration: 180 }}
  >
    <header class="head">
      <div class="head-text">
        <h2>Up next</h2>
        <p class="sub">
          {#if queue.items.length === 0}
            Nothing in the queue
          {:else}
            {queue.items.length} track{queue.items.length === 1 ? "" : "s"}
            {#if upcomingDuration > 0}
              · {formatDuration(upcomingDuration)} remaining
            {/if}
          {/if}
        </p>
      </div>

      <div class="head-actions">
        <!-- Endless mode: when on, the player picks a random album
             after the queue ends (both on auto-advance and manual
             next), keeping the listening session going. -->
        <button
          class="icon-btn"
          class:on={endless}
          onclick={toggleEndless}
          title={endless ? "Endless mode: on" : "Endless mode: off"}
          aria-label={endless ? "Disable endless mode" : "Enable endless mode"}
          aria-pressed={endless}
        >
          <Icon name="infinity" size={16} />
        </button>
        <button class="close" onclick={() => queuePopover.close()} aria-label="Close">
          ×
        </button>
      </div>
    </header>

    <div class="body">
      {#if loading && queue.items.length === 0}
        <p class="state">Loading…</p>
      {:else if queue.items.length === 0}
        <div class="empty">
          <p class="empty-line">Queue is empty.</p>
          <p class="empty-hint">
            Use <strong>Add to queue</strong> from a right-click menu.
          </p>
        </div>
      {:else}
        <ul class="rows">
          {#each decorated as row (row.track?.id ?? row.idx)}
            {@const t = row.track}
            <li
              class:current={row.isCurrent}
              class:past={row.isPast}
              class:drop-above={dragOverIdx === row.idx &&
                dragFromIdx !== null &&
                dragFromIdx > row.idx}
              class:drop-below={dragOverIdx === row.idx &&
                dragFromIdx !== null &&
                dragFromIdx < row.idx}
              class:dragging={dragFromIdx === row.idx}
              animate:flip={{ duration: 200 }}
              ondragover={(e) => onDragOver(row.idx, e)}
              ondrop={(e) => onDrop(row.idx, e)}
            >
              <button
                class="grip"
                draggable="true"
                ondragstart={(e) => onDragStart(row.idx, e)}
                ondragend={onDragEnd}
                aria-label="Drag to reorder"
                tabindex="-1"
              >
                <Icon name="drag" size={12} />
              </button>

              <button
                class="row-body"
                onclick={() => (row.isCurrent ? null : handleJump(row.idx))}
                ondblclick={() => handleJump(row.idx)}
                oncontextmenu={(e) => t && openTrackMenu(e, t)}
                disabled={row.isCurrent}
              >
                <div class="thumb">
                  {#if t}
                    <Cover coverKey={t.cover_key} size={36} title={t.title} />
                  {:else}
                    <div class="thumb-placeholder">?</div>
                  {/if}
                  {#if row.isCurrent && app.player.status === "playing"}
                    <span class="row-eq" aria-hidden="true">
                      <span></span><span></span><span></span>
                    </span>
                  {/if}
                </div>

                <div class="text">
                  <div class="title">{t?.title ?? "Unknown"}</div>
                  <div class="artist">{t?.artist ?? ""}</div>
                </div>

                <div class="duration">
                  {t ? formatDuration(t.duration_seconds) : "—"}
                </div>
              </button>

              <button
                class="remove"
                onclick={(e) => handleRemove(row.idx, e)}
                aria-label="Remove from queue"
                title="Remove"
              >
                <Icon name="trash" size={12} />
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </div>
{/if}

<style>
  .popover {
    position: fixed;
    /* Anchored above the PlayerBar (which is `var(--player-height)`
       tall) and aligned to the right edge with a margin matching the
       bar's content padding. */
    bottom: calc(var(--player-height) + 8px);
    right: 16px;
    width: min(420px, calc(100% - 32px));
    max-height: 65vh;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: 0 30px 60px -12px rgba(0, 0, 0, 0.6),
      0 4px 18px -4px rgba(0, 0, 0, 0.4);
    z-index: 1050;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    /* The popover sits over the content layer; backdrop blur softens
       the visual transition since this lives just above the bar. */
    backdrop-filter: blur(20px);
  }

  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
    padding: 12px 14px 10px;
    border-bottom: 1px solid var(--border);
  }
  .head-text {
    min-width: 0;
    flex: 1;
  }
  .head-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }
  .icon-btn {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: transparent;
    border: none;
    color: var(--fg-2);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .icon-btn:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .icon-btn.on {
    background: var(--accent-soft);
    color: var(--accent);
  }
  h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }
  .sub {
    margin: 2px 0 0;
    color: var(--fg-2);
    font-size: 11px;
  }
  .close {
    background: transparent;
    border: none;
    color: var(--fg-2);
    width: 24px;
    height: 24px;
    border-radius: 6px;
    font-size: 18px;
    line-height: 1;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0;
  }
  .close:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }

  .body {
    overflow-y: auto;
    padding: 6px;
  }
  .state {
    margin: 16px;
    color: var(--fg-2);
  }
  .empty {
    text-align: center;
    padding: 30px 16px;
    color: var(--fg-2);
  }
  .empty-line {
    color: var(--fg-1);
    font-size: 13px;
    margin: 0 0 4px;
  }
  .empty-hint {
    margin: 0;
    font-size: 12px;
  }

  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  li {
    position: relative;
    display: grid;
    grid-template-columns: 22px 1fr 28px;
    align-items: center;
    gap: 2px;
    padding: 2px 4px;
    border-radius: var(--radius-md);
    transition: background var(--dur-fast) var(--ease-out);
  }
  li:hover {
    background: var(--bg-2);
  }
  li.current {
    background: var(--accent-soft);
  }
  li.past {
    opacity: 0.45;
  }
  li.dragging {
    opacity: 0.4;
  }
  li.drop-above::before,
  li.drop-below::after {
    content: "";
    position: absolute;
    left: 6px;
    right: 6px;
    height: 2px;
    border-radius: 1px;
    background: var(--accent);
    box-shadow: 0 0 8px 0 var(--accent-glow);
  }
  li.drop-above::before { top: -1px; }
  li.drop-below::after { bottom: -1px; }

  .grip {
    width: 20px;
    height: 32px;
    background: transparent;
    border: none;
    color: var(--fg-3, var(--fg-2));
    cursor: grab;
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    padding: 0;
    transition: opacity var(--dur-fast) var(--ease-out);
  }
  li:hover .grip {
    opacity: 1;
  }
  .grip:active {
    cursor: grabbing;
  }

  .row-body {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 10px;
    width: 100%;
    background: transparent;
    border: none;
    color: var(--fg-1);
    cursor: pointer;
    padding: 4px 6px;
    text-align: left;
    border-radius: var(--radius-md);
    min-width: 0;
  }
  .row-body:hover {
    background: transparent;
  }
  .row-body:disabled {
    cursor: default;
  }
  .thumb {
    position: relative;
    flex-shrink: 0;
  }
  .thumb-placeholder {
    width: 36px;
    height: 36px;
    border-radius: var(--radius-md);
    background: var(--bg-2);
    color: var(--fg-2);
    display: grid;
    place-items: center;
    font-size: 14px;
    font-weight: 600;
  }
  .row-eq {
    position: absolute;
    right: 3px;
    bottom: 3px;
    display: inline-flex;
    align-items: flex-end;
    gap: 1.5px;
    height: 10px;
    padding: 1px 2px;
    background: rgba(0, 0, 0, 0.55);
    border-radius: 2px;
  }
  .row-eq > span {
    width: 1.5px;
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

  .text {
    min-width: 0;
  }
  .title {
    color: var(--fg-0);
    font-weight: 500;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  li.current .title {
    color: var(--accent);
  }
  .artist {
    color: var(--fg-2);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .duration {
    color: var(--fg-2);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  .remove {
    width: 22px;
    height: 22px;
    background: transparent;
    border: none;
    color: var(--fg-3, var(--fg-2));
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-s);
    opacity: 0;
    padding: 0;
    transition: opacity var(--dur-fast) var(--ease-out),
      background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  li:hover .remove {
    opacity: 1;
  }
  .remove:hover {
    background: var(--danger);
    color: #fff;
  }
</style>
