<script lang="ts">
  // Page/panneau dédié de la file d'attente (R3.5, R3.6, R3.7).
  //
  // Atteignable depuis la Sidebar (entrée « File d'attente ») et depuis
  // le Mini_Player. Le `QueuePopover` reste l'accès rapide ancré
  // au-dessus du PlayerBar ; ce composant est la vue complète, rendue
  // comme une route à part entière par `App.svelte` (`selectedView ===
  // "queue"`).
  //
  // Le flux de données (charge `getQueue()` + résolution des pistes via
  // `getTracks`, saut, retrait, réorganisation par drag) reprend
  // exactement celui de `QueuePopover.svelte` pour rester cohérent.

  import { flip } from "svelte/animate";
  import {
    getQueue,
    getTracks,
    queueJumpTo,
    queueMove,
    queueRemoveAt,
    clearQueue,
    type QueueView as QueueViewT,
    type Track,
  } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import { formatDuration } from "../lib/format";
  import { activateOnKey, announce } from "../lib/a11y";
  import { PLAYER_LABELS } from "../lib/labels";
  import { openTrackMenu } from "../lib/trackMenu";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";

  let queue = $state<QueueViewT>({ items: [], cursor: null });
  let trackMap = $state<Map<number, Track>>(new Map());
  let loading = $state<boolean>(false);

  // Drag state for reorder — same flow as the popover.
  let dragFromIdx = $state<number | null>(null);
  let dragOverIdx = $state<number | null>(null);

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

  // Reload on first render and whenever the playing track changes
  // (covers auto-advance, manual next, play_next inserts).
  $effect(() => {
    void app.player.current_track_id;
    void load();
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

  // « Vider la file » (R3.7). The currently playing track keeps
  // playing — queue and playback are decoupled. We announce the
  // result via the shared ARIA live region (R2.9).
  async function handleClear(): Promise<void> {
    if (queue.items.length === 0) return;
    try {
      await clearQueue();
      await load();
      announce("File d'attente vidée");
    } catch (e) {
      app.lastError = String(e);
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
</script>

<section class="queue-panel" aria-label="File d'attente">
  <header class="head">
    <div class="head-text">
      <h1>{PLAYER_LABELS.queue.label}</h1>
      <p class="sub">
        {#if queue.items.length === 0}
          File vide
        {:else}
          {queue.items.length} titre{queue.items.length === 1 ? "" : "s"}
          {#if upcomingDuration > 0}
            · {formatDuration(upcomingDuration)} restant
          {/if}
        {/if}
      </p>
    </div>

    <button
      class="clear-btn"
      onclick={handleClear}
      disabled={queue.items.length === 0}
      title="Vider la file"
      aria-label="Vider la file"
    >
      <Icon name="trash" size={14} />
      <span>Vider la file</span>
    </button>
  </header>

  {#if loading && queue.items.length === 0}
    <p class="state">Chargement…</p>
  {:else if queue.items.length === 0}
    <div class="empty" role="status">
      <p class="empty-line">Votre file est vide.</p>
      <p class="empty-hint">
        Ajoutez des titres via le menu d'une piste → « Ajouter à la file ».
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
            aria-label="Glisser pour réorganiser"
            tabindex="-1"
          >
            <Icon name="drag" size={13} />
          </button>

          <!-- Row body: a non-native clickable element made keyboard
               activatable via `activateOnKey` (R2.8). Enter/Espace
               jumps to the track just like a click. The action wires
               the keydown handler + focusability at runtime, which
               svelte-check can't see statically — hence the ignore. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="row-body"
            class:is-current={row.isCurrent}
            role="button"
            tabindex={row.isCurrent ? -1 : 0}
            aria-label={t
              ? `Lire « ${t.title} » — ${t.artist}`
              : "Piste inconnue"}
            aria-current={row.isCurrent ? "true" : undefined}
            use:activateOnKey={() => (row.isCurrent ? undefined : handleJump(row.idx))}
            onclick={() => (row.isCurrent ? null : handleJump(row.idx))}
            ondblclick={() => handleJump(row.idx)}
            oncontextmenu={(e) => t && openTrackMenu(e, t)}
          >
            <div class="thumb">
              {#if t}
                <Cover coverKey={t.cover_key} size={44} title={t.title} />
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
              <div class="title">{t?.title ?? "Inconnu"}</div>
              <div class="artist">{t?.artist ?? ""}</div>
            </div>

            <div class="duration">
              {t ? formatDuration(t.duration_seconds) : "—"}
            </div>
          </div>

          <button
            class="remove"
            onclick={(e) => handleRemove(row.idx, e)}
            aria-label="Retirer de la file"
            title="Retirer"
          >
            <Icon name="trash" size={13} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .queue-panel {
    max-width: 880px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
    margin-bottom: var(--space-2);
  }
  .head-text {
    min-width: 0;
  }
  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 700;
    letter-spacing: -0.02em;
    color: var(--fg-0);
  }
  .sub {
    margin: 4px 0 0;
    color: var(--fg-2);
    font-size: 13px;
  }

  .clear-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    height: 34px;
    padding: 0 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-1);
    color: var(--fg-1);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .clear-btn:hover:not(:disabled) {
    background: var(--danger);
    border-color: var(--danger);
    color: #fff;
  }
  .clear-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .state {
    margin: var(--space-5) 0;
    color: var(--fg-2);
  }
  .empty {
    text-align: center;
    padding: 60px 16px;
    color: var(--fg-2);
  }
  .empty-line {
    color: var(--fg-1);
    font-size: 16px;
    font-weight: 600;
    margin: 0 0 8px;
  }
  .empty-hint {
    margin: 0;
    font-size: 13px;
  }

  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  li {
    position: relative;
    display: grid;
    grid-template-columns: 26px 1fr 34px;
    align-items: center;
    gap: 4px;
    padding: 4px 6px;
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
    width: 24px;
    height: 40px;
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
    gap: 12px;
    width: 100%;
    background: transparent;
    border: none;
    color: var(--fg-1);
    cursor: pointer;
    padding: 6px 8px;
    text-align: left;
    border-radius: var(--radius-md);
    min-width: 0;
  }
  .row-body.is-current {
    cursor: default;
  }
  .row-body:focus-visible {
    outline: 2px solid var(--focus-ring, #8ab4ff);
    outline-offset: 2px;
  }
  .thumb {
    position: relative;
    flex-shrink: 0;
  }
  .thumb-placeholder {
    width: 44px;
    height: 44px;
    border-radius: var(--radius-md);
    background: var(--bg-2);
    color: var(--fg-2);
    display: grid;
    place-items: center;
    font-size: 16px;
    font-weight: 600;
  }
  .row-eq {
    position: absolute;
    right: 3px;
    bottom: 3px;
    display: inline-flex;
    align-items: flex-end;
    gap: 1.5px;
    height: 12px;
    padding: 1px 2px;
    background: rgba(0, 0, 0, 0.55);
    border-radius: 2px;
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

  .text {
    min-width: 0;
  }
  .title {
    color: var(--fg-0);
    font-weight: 500;
    font-size: 14px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  li.current .title {
    color: var(--accent);
  }
  .artist {
    color: var(--fg-2);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .duration {
    color: var(--fg-2);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  .remove {
    width: 28px;
    height: 28px;
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
