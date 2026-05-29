<script lang="ts">
  import { onDestroy } from "svelte";
  import { fade } from "svelte/transition";
  import { getLyrics, type Lyrics } from "../lib/api";
  import { app } from "../lib/stores.svelte";

  interface Props {
    /// Track id to fetch lyrics for. Pass `null` to clear.
    trackId: number | null;
    /// Visual variant:
    ///  - "panel": compact side panel (default), used inside the
    ///    fullscreen hero square and anywhere a small lyrics box is
    ///    embedded.
    ///  - "stage": large, centred karaoke layout that fills its
    ///    parent — big active line, dimmed neighbours, generous line
    ///    wrapping. Used by the fullscreen karaoke mode.
    variant?: "panel" | "stage";
  }
  let { trackId, variant = "panel" }: Props = $props();

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

  // Auto-scroll to the active line. `block: center` keeps the sung
  // line vertically centred in both variants.
  let listEl = $state<HTMLDivElement | null>(null);
  $effect(() => {
    if (activeIdx < 0 || !listEl) return;
    const el = listEl.querySelector<HTMLElement>(`[data-idx="${activeIdx}"]`);
    if (el) {
      el.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  });

  onDestroy(() => {});
</script>

<div class="lyrics" class:stage={variant === "stage"} in:fade={{ duration: 200 }}>
  {#if loading}
    <p class="state">Chargement des paroles…</p>
  {:else if error}
    <p class="state error">Paroles indisponibles : {error}</p>
  {:else if !lyrics || (lyrics.synced === null && lyrics.unsynced === null)}
    <p class="state subtle">
      Aucune parole trouvée. Dépose un fichier <code>.lrc</code> à côté du
      morceau ou intègre les paroles dans les tags pour les voir ici.
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
    <p class="state subtle">Aucune parole trouvée.</p>
  {/if}
</div>

<style>
  .lyrics {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 0; /* allow the flex child to shrink so .list scrolls */
    max-width: 560px;
    margin: 0 auto;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  /* Karaoke stage: no width cap, fill the parent. */
  .lyrics.stage {
    max-width: min(900px, 92vw);
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
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    /* Top/bottom padding centres the first and last lines; sized
       relative to the list's own box rather than the viewport so it
       never pushes content off-screen in the small panel. */
    padding: 45% 8px;
    scroll-behavior: smooth;
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
    /* Long lines wrap instead of spilling past the edges. */
    overflow-wrap: break-word;
    word-break: break-word;
    hyphens: auto;
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

  /* ---- Stage (karaoke) sizing ---- */
  .lyrics.stage .list {
    padding: 38dvh 12px;
  }
  .lyrics.stage .line {
    padding: 10px 16px;
    font-size: clamp(20px, 3.4dvh, 34px);
    line-height: 1.3;
    color: var(--fg-3);
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .lyrics.stage .line.past {
    color: var(--fg-3);
    opacity: 0.55;
  }
  .lyrics.stage .line.active {
    color: var(--fg-0);
    font-weight: 800;
    font-size: clamp(26px, 4.6dvh, 46px);
    transform: scale(1.02);
    text-shadow: 0 2px 24px rgba(0, 0, 0, 0.5);
    filter: drop-shadow(0 0 18px var(--accent-glow));
  }

  .plain {
    margin: 0;
    padding: 16px 12px;
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    font-family: var(--font-sans);
    font-size: 15px;
    line-height: 1.65;
    white-space: pre-wrap;
    overflow-wrap: break-word;
    color: var(--fg-1);
  }
  .lyrics.stage .plain {
    text-align: center;
    font-size: clamp(16px, 2.4dvh, 22px);
    line-height: 1.8;
    color: var(--fg-1);
    padding: 8dvh 12px;
  }
</style>
