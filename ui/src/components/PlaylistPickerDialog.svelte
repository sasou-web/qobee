<script lang="ts">
  import { fade, scale } from "svelte/transition";
  import { addToPlaylist, createPlaylist } from "../lib/api";
  import { playlistPicker } from "../lib/playlistPicker.svelte";
  import { app } from "../lib/stores.svelte";
  import { toasts } from "../lib/toasts.svelte";

  // Local "create playlist" form state. Visible only when the user
  // clicks "+ New playlist" — keeping the picker compact by default.
  let creating = $state<boolean>(false);
  let newName = $state<string>("");
  let busy = $state<boolean>(false);

  function onKey(e: KeyboardEvent): void {
    if (e.key === "Escape") close();
  }

  function close(): void {
    creating = false;
    newName = "";
    playlistPicker.close();
  }

  async function pick(playlistId: number, playlistName: string): Promise<void> {
    if (busy) return;
    busy = true;
    try {
      // `addToPlaylist` resolves with the number of tracks actually
      // persisted (R3.2). Only confirm success when at least one row
      // was written; a zero count means nothing was persisted, so we
      // surface an error instead of a misleading success (R3.3).
      const n = await addToPlaylist(playlistId, playlistPicker.state.trackIds);
      await app.refreshPlaylists();
      if (n > 0) {
        toasts.success(`Ajouté à «${playlistName}»`);
        close();
      } else {
        toasts.error(`Impossible d'ajouter à «${playlistName}» : aucun titre n'a été enregistré.`);
      }
    } catch (e) {
      app.lastError = String(e);
      toasts.error(`Impossible d'ajouter à «${playlistName}» : ${String(e)}`);
    } finally {
      busy = false;
    }
  }

  async function handleCreate(): Promise<void> {
    const name = newName.trim();
    if (!name) return;
    busy = true;
    try {
      const created = await createPlaylist(name);
      const n = await addToPlaylist(created.id, playlistPicker.state.trackIds);
      await app.refreshPlaylists();
      if (n > 0) {
        toasts.success(`Ajouté à «${name}»`);
        close();
      } else {
        toasts.error(`Impossible d'ajouter à «${name}» : aucun titre n'a été enregistré.`);
      }
    } catch (e) {
      app.lastError = String(e);
      toasts.error(`Impossible d'ajouter à «${name}» : ${String(e)}`);
    } finally {
      busy = false;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if playlistPicker.state.open}
  <div
    class="overlay"
    role="dialog"
    aria-modal="true"
    aria-label="Add to playlist"
    transition:fade={{ duration: 120 }}
  >
    <button class="dismiss" onclick={close} aria-label="Close"></button>

    <div class="card" transition:scale={{ duration: 160, start: 0.96 }}>
      <header>
        <h2>Add to playlist</h2>
        <p class="sub">
          {playlistPicker.state.trackIds.length} track{playlistPicker.state.trackIds.length === 1 ? "" : "s"} selected
        </p>
      </header>

      {#if app.playlists.length === 0 && !creating}
        <p class="empty">No playlist yet. Create one below.</p>
      {:else if !creating}
        <ul class="list">
          {#each app.playlists as pl (pl.id)}
            <li>
              <button class="row" onclick={() => pick(pl.id, pl.name)} disabled={busy}>
                <span class="name">{pl.name}</span>
                <span class="count">{pl.track_count} tracks</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}

      {#if creating}
        <div class="create">
          <input
            type="text"
            placeholder="New playlist name"
            bind:value={newName}
            onkeydown={(e) => {
              if (e.key === "Enter") void handleCreate();
            }}
          />
          <div class="create-actions">
            <button onclick={() => { creating = false; newName = ""; }} disabled={busy}>
              Cancel
            </button>
            <button class="primary" onclick={handleCreate} disabled={busy || !newName.trim()}>
              Create &amp; add
            </button>
          </div>
        </div>
      {:else}
        <div class="actions">
          <button onclick={() => (creating = true)} disabled={busy}>+ Nouvelle playlist</button>
          <button onclick={close} disabled={busy}>Cancel</button>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    z-index: 1100;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .dismiss {
    position: absolute;
    inset: 0;
    background: transparent;
    border: none;
    cursor: default;
  }
  .card {
    position: relative;
    z-index: 1;
    width: min(440px, 100%);
    max-height: 70vh;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-l, 12px);
    box-shadow: 0 30px 60px -12px rgba(0, 0, 0, 0.65);
    padding: 20px 22px 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    overflow: hidden;
  }
  h2 {
    margin: 0;
    font-size: 17px;
    font-weight: 700;
  }
  .sub {
    margin: 4px 0 0;
    color: var(--fg-2);
    font-size: 12px;
  }
  .empty {
    color: var(--fg-2);
    font-size: 13px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    max-height: 340px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-s, 6px);
    padding: 8px 12px;
    color: var(--fg-0);
    cursor: pointer;
    text-align: left;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .row:hover:not(:disabled) {
    background: var(--bg-2);
  }
  .name {
    font-weight: 500;
  }
  .count {
    color: var(--fg-2);
    font-size: 12px;
  }
  .create {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .create input {
    background: var(--bg-2);
    border: 1px solid var(--border);
    color: var(--fg-0);
    border-radius: var(--radius-s, 6px);
    padding: 8px 10px;
    font-size: 13px;
  }
  .create input:focus {
    outline: 2px solid var(--accent);
    outline-offset: 0;
  }
  .create-actions,
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .actions {
    justify-content: space-between;
  }
  .actions button,
  .create-actions button {
    background: var(--bg-2);
    border: 1px solid var(--border);
    color: var(--fg-0);
    border-radius: var(--radius-s, 6px);
    padding: 6px 14px;
    font-size: 13px;
    cursor: pointer;
  }
  .actions button:hover:not(:disabled),
  .create-actions button:hover:not(:disabled) {
    background: var(--bg-3);
  }
  .primary {
    background: var(--accent) !important;
    color: #fff !important;
    border-color: var(--accent) !important;
  }
  .primary:hover:not(:disabled) {
    filter: brightness(1.08);
  }
</style>
