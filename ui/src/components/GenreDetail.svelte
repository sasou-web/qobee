<script lang="ts">
  import { fly } from "svelte/transition";
  import { listAlbumsByGenre, type Album } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import Cover from "./Cover.svelte";
  import Icon from "./Icon.svelte";
  import Marquee from "./Marquee.svelte";
  import { openAlbumMenu } from "../lib/trackMenu";

  let albums = $state<Album[]>([]);
  let loading = $state<boolean>(false);

  $effect(() => {
    const g = app.selectedGenre;
    if (!g) {
      albums = [];
      return;
    }
    loading = true;
    listAlbumsByGenre(g)
      .then((a) => (albums = a))
      .catch((e) => {
        app.lastError = String(e);
        albums = [];
      })
      .finally(() => (loading = false));
  });

  function stagger(i: number): number {
    return Math.min(i * 18, 240);
  }
</script>

<button class="back" onclick={() => app.goBack()}>← Back</button>

{#if !app.selectedGenre}
  <p class="empty">No genre selected.</p>
{:else}
  <h1>{app.selectedGenre}</h1>
  {#if loading}
    <p class="empty">Loading…</p>
  {:else if albums.length === 0}
    <p class="empty">No albums in this genre.</p>
  {:else}
    <div class="grid">
      {#each albums as alb, i (alb.id)}
        <button
          class="card lift marquee-host"
          onclick={() => app.selectAlbum(alb.id)}
          oncontextmenu={(e) => openAlbumMenu(e, alb)}
          in:fly|global={{ y: 10, duration: 260, delay: stagger(i) }}
        >
          <div class="thumb">
            <Cover coverKey={alb.cover_key} size={160} title={alb.title} />
            <span class="play-overlay" aria-hidden="true">
              <Icon name="play" size={20} />
            </span>
          </div>
          <div class="meta">
            <Marquee class="t" text={alb.title} />
            <Marquee class="a" text={alb.artist} />
          </div>
        </button>
      {/each}
    </div>
  {/if}
{/if}

<style>
  .back {
    background: transparent;
    border: none;
    color: var(--fg-2);
    cursor: pointer;
    margin-bottom: 12px;
    padding: 6px 10px;
    border-radius: var(--radius-s);
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .back:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  h1 {
    font-size: 30px;
    margin: 4px 0 var(--space-4);
    font-weight: 700;
    letter-spacing: -0.015em;
  }
  .empty {
    color: var(--fg-2);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: var(--space-6);
    justify-items: start;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    background: transparent;
    border: none;
    color: var(--fg-1);
    cursor: pointer;
    padding: 0;
    border-radius: var(--radius-lg);
    text-align: left;
    width: 160px;
    transition: transform var(--dur-base) var(--ease-out);
  }
  .card:hover {
    background: transparent;
  }
  .thumb {
    position: relative;
    border-radius: var(--radius-lg);
    overflow: hidden;
    width: fit-content;
  }
  .play-overlay {
    position: absolute;
    right: 12px;
    bottom: 12px;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transform: translateY(8px) scale(0.8);
    transition: opacity var(--dur-base) var(--ease-out),
      transform var(--dur-base) var(--ease-spring);
    box-shadow: 0 10px 24px -6px rgba(0, 0, 0, 0.55);
    pointer-events: none;
  }
  .card:hover .play-overlay {
    opacity: 1;
    transform: translateY(0) scale(1);
  }
  .meta {
    width: 100%;
    min-width: 0;
  }
  .meta :global(.t) {
    color: var(--fg-0);
    font-weight: 500;
    margin-top: var(--space-1);
    transition: color var(--dur-fast) var(--ease-out);
  }
  .card:hover :global(.t) {
    color: var(--accent);
  }
  .meta :global(.a) {
    color: var(--fg-2);
    font-size: 12px;
    margin-top: var(--space-1);
  }
</style>
