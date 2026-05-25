<script lang="ts">
  import { onDestroy } from "svelte";
  import { fade } from "svelte/transition";
  import { getLyrics, type Lyrics } from "../lib/api";
  import { app } from "../lib/stores.svelte";

  interface Props {
    /// Track id to fetch lyrics for. Pass `null` to clear.
    trackId: number | null;
  }
  let { trackId }: Props = $props();

  let lyrics = $state<Lyrics | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let lastLoadedId = $state<number | null>(null);

  // Reload whenever the track id changes.
  $effect(() => {
    const id = trackId;
    if (id === null || id === undefined) {
      lyrics = null;
      lastLoadedId = null;
      return;
    }
    if (id === lastLoadedId) return;
    lastLoadedId = id;
    loading = true;
    error = null;
    void getLyrics(id)
      .then((res) => {
        if (lastLoadedId !== id) return; // raced with a newer fetch
        lyrics = res;
        loading = false;
      })
      .catch((e) => {
        if (lastLoadedId !== id) return;
        error = String(e);
        loading = false;
      });
  });

  // Active line index for synced lyrics: latest line whose timestamp
  // is less than or equal to the current player position.
  let positionMs = $derived(Math.floor(app.player.position_seconds * 1000));
  let activeIdx = $derived.by(() => {
    if (!lyrics?.synced || lyrics.synced.length === 0) return -1;
    const lines = lyrics.synced;
    // Linear scan is fine; lyric files are typically <500 lines.
    let best = -1;
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      if (line && line.ms <= positionMs) best = i;
      else break;
    }
    return best;
  });

  // Auto-scroll to the active line.
  let listEl = $state<HTMLDivElement | null>(null);
  $effect(() => {
    if (activeIdx < 0 || !listEl) return;
    const el = listEl.querySelector<HTMLElement>(`[data-idx="${activeIdx}"]`);
    if (el) {
      el.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  });

  // No-op cleanup; effect handles cancellation via the lastLoadedId
  // guard.
  onDestroy(() => {});
</script>

<div class="lyrics" in:fade={{ duration: 200 }}>
  {#if loading}
    <p class="state">Loading lyrics…</p>
  {:else if error}
    <p class="state error">Could not load lyrics: {error}</p>
  {:else if !lyrics || (lyrics.synced === null && lyrics.unsynced === null)}
    <p class="state subtle">
      No lyrics found. Drop a <code>.lrc</code> file next to the audio file
      or embed lyrics in the file's tags to see them here.
    </p>
  {:else if lyrics.synced && lyrics.synced.length > 0}
    <div class="list" bind:this={listEl}>
      {#each lyrics.synced as line, i (i)}
        <div
          class="line"
          class:active={i === activeIdx}
          class:past={i < activeIdx}
          data-idx={i}
        >
          {line.text || "♪"}
        </div>
      {/each}
    </div>
  {:else if lyrics.unsynced}
    <pre class="plain">{lyrics.unsynced}</pre>
  {:else}
    <p class="state subtle">No lyrics found.</p>
  {/if}
</div>

<style>
  .lyrics {
    position: relative;
    width: 100%;
    max-width: 560px;
    height: 100%;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  .state {
    text-align: center;
    color: var(--fg-2);
    padding: 40px 16px;
    font-size: 14px;
    line-height: 1.6;
  }
  .state.subtle code {
    background: var(--bg-2);
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 12px;
  }
  .state.error {
    color: var(--danger);
  }
  .list {
    overflow-y: auto;
    padding: 30vh 8px;
    scroll-behavior: smooth;
    /* Hide scrollbar; the auto-scroll keeps the active line centered. */
    scrollbar-width: none;
  }
  .list::-webkit-scrollbar {
    display: none;
  }
  .line {
    padding: 8px 12px;
    font-size: 18px;
    line-height: 1.45;
    color: var(--fg-2);
    text-align: center;
    transition: color var(--dur-base) var(--ease-out),
      transform var(--dur-base) var(--ease-out),
      filter var(--dur-base) var(--ease-out);
  }
  .line.past {
    color: var(--fg-3);
  }
  .line.active {
    color: var(--fg-0);
    font-weight: 600;
    font-size: 20px;
    transform: scale(1.05);
    filter: drop-shadow(0 0 12px var(--accent-glow));
  }
  .plain {
    margin: 0;
    padding: 16px 12px;
    overflow-y: auto;
    font-family: var(--font-sans);
    font-size: 15px;
    line-height: 1.65;
    white-space: pre-wrap;
    color: var(--fg-1);
  }
</style>
