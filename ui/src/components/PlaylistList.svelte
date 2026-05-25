<script lang="ts">
  import { createPlaylist, deletePlaylist } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import Cover from "./Cover.svelte";

  async function handleNew(): Promise<void> {
    const name = window.prompt("Playlist name");
    if (!name || !name.trim()) return;
    try {
      const created = await createPlaylist(name.trim());
      await app.refreshPlaylists();
      app.selectPlaylist(created.id);
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function handleDelete(id: number, name: string): Promise<void> {
    if (!window.confirm(`Delete playlist "${name}"?`)) return;
    try {
      await deletePlaylist(id);
      await app.refreshPlaylists();
    } catch (e) {
      app.lastError = String(e);
    }
  }
</script>

<section>
  <header class="header">
    <h1>Playlists</h1>
    <button class="new" onclick={handleNew}>+ New playlist</button>
  </header>

  {#if app.playlists.length === 0}
    <p class="empty">
      You don't have any playlist yet. Create one to start organizing your music.
    </p>
  {:else}
    <ul class="list">
      {#each app.playlists as pl (pl.id)}
        <li>
          <button class="card" onclick={() => app.selectPlaylist(pl.id)}>
            <Cover coverKey={pl.cover_key} size={64} title={pl.name} />
            <div class="meta">
              <div class="name">{pl.name}</div>
              <div class="sub">{pl.track_count} tracks</div>
            </div>
          </button>
          <button
            class="del"
            onclick={(e) => {
              e.stopPropagation();
              handleDelete(pl.id, pl.name);
            }}
            aria-label="Delete playlist"
          >
            ✕
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--space-4);
    margin: 4px 0 var(--space-6);
  }
  h1 {
    font-size: 24px;
    font-weight: 700;
    letter-spacing: -0.01em;
    margin: 0;
  }
  .new {
    border: 1px solid var(--border);
    color: var(--fg-0);
    padding: 6px 12px;
    border-radius: 8px;
    cursor: pointer;
    background: transparent;
  }
  .new:hover {
    background: var(--bg-2);
  }
  .empty {
    color: var(--fg-2);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 6px;
  }
  li {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .card {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 12px;
    background: transparent;
    border: none;
    color: var(--fg-1);
    padding: 8px;
    border-radius: 10px;
    cursor: pointer;
    text-align: left;
  }
  .card:hover {
    background: var(--bg-2);
  }
  .name {
    color: var(--fg-0);
    font-weight: 500;
  }
  .sub {
    color: var(--fg-2);
    font-size: 12px;
  }
  .del {
    background: transparent;
    border: none;
    color: var(--fg-2);
    cursor: pointer;
    padding: 6px 8px;
    border-radius: 6px;
  }
  .del:hover {
    background: var(--danger);
    color: #fff;
  }
</style>
