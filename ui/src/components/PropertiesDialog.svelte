<script lang="ts">
  import { fade, scale } from "svelte/transition";
  import { coverUrl } from "../lib/api";
  import {
    propertiesDialog,
    type PropertiesTab,
  } from "../lib/propertiesDialog.svelte";
  import {
    extractPalette,
    swatchToCss,
    swatchTextColor,
    type Swatch,
  } from "../lib/palette";
  import Cover from "./Cover.svelte";

  type Tab = { id: PropertiesTab; label: string };
  const TABS: Tab[] = [
    { id: "details", label: "Details" },
    { id: "artists", label: "Artists" },
    { id: "artwork", label: "Artwork" },
  ];

  // Cache the palette per cover key so re-opening the same dialog
  // doesn't re-decode the image. Cleared on close to keep memory
  // bounded.
  let palette = $state<Swatch[] | null>(null);
  let paletteError = $state<string | null>(null);
  let paletteLoading = $state<boolean>(false);
  let paletteForKey = $state<string | null>(null);

  function onKey(e: KeyboardEvent): void {
    if (e.key === "Escape") propertiesDialog.close();
  }

  // Re-run extraction whenever the dialog opens with a fresh cover or
  // the user switches to the Artwork tab for the first time. Doing the
  // work lazily keeps Details/Artists fast.
  $effect(() => {
    const s = propertiesDialog.state;
    if (!s.open || !s.payload) {
      palette = null;
      paletteError = null;
      paletteForKey = null;
      paletteLoading = false;
      return;
    }
    if (s.tab !== "artwork") return;
    const key = s.payload.coverKey;
    if (!key) {
      palette = [];
      return;
    }
    if (paletteForKey === key) return; // already extracted
    const url = coverUrl(key);
    if (!url) {
      palette = [];
      return;
    }
    paletteLoading = true;
    paletteError = null;
    paletteForKey = key;
    extractPalette(url, 5)
      .then((p) => {
        palette = p;
      })
      .catch((e) => {
        paletteError = String(e);
        palette = [];
      })
      .finally(() => (paletteLoading = false));
  });

  async function copy(text: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // best-effort: ignore failures (no Clipboard API in some
      // restricted webviews).
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if propertiesDialog.state.open && propertiesDialog.state.payload}
  {@const p = propertiesDialog.state.payload}
  {@const tab = propertiesDialog.state.tab}
  <div
    class="overlay"
    role="dialog"
    aria-modal="true"
    aria-label={p.title}
    transition:fade={{ duration: 120 }}
  >
    <button
      class="dismiss"
      onclick={() => propertiesDialog.close()}
      aria-label="Close"
    ></button>

    <div class="card" transition:scale={{ duration: 160, start: 0.96 }}>
      <header class="head">
        <Cover coverKey={p.coverKey} size={56} title={p.title} />
        <div class="head-text">
          <h2>{p.title}</h2>
          {#if p.subtitle}
            <p class="sub">{p.subtitle}</p>
          {/if}
        </div>
        <button class="close" onclick={() => propertiesDialog.close()} aria-label="Close">
          ×
        </button>
      </header>

      <div class="tabs" role="tablist">
        {#each TABS as t (t.id)}
          <button
            class="tab"
            class:active={tab === t.id}
            onclick={() => propertiesDialog.setTab(t.id)}
            role="tab"
            aria-selected={tab === t.id}
          >
            {t.label}
          </button>
        {/each}
      </div>

      <div class="body" role="tabpanel">
        {#if tab === "details"}
          {#if p.audioTraits.length > 0}
            <h3>Audio</h3>
            <dl class="fields">
              {#each p.audioTraits as f (f.label)}
                <div class="row">
                  <dt>{f.label}</dt>
                  <dd class:mono={f.monospace} title={f.value}>{f.value}</dd>
                </div>
              {/each}
            </dl>
          {/if}

          {#if p.details.length > 0}
            <h3>Information</h3>
            <dl class="fields">
              {#each p.details as f (f.label)}
                <div class="row">
                  <dt>{f.label}</dt>
                  {#if f.monospace}
                    <dd class="mono">
                      <button
                        type="button"
                        class="copyable"
                        title="Click to copy"
                        onclick={() => copy(f.value)}
                      >
                        {f.value}
                      </button>
                    </dd>
                  {:else}
                    <dd title={f.value}>{f.value}</dd>
                  {/if}
                </div>
              {/each}
            </dl>
          {/if}

          {#if p.audioTraits.length === 0 && p.details.length === 0}
            <p class="empty">No metadata available.</p>
          {/if}
        {:else if tab === "artists"}
          {#if p.artists.length === 0}
            <p class="empty">No artist information.</p>
          {:else}
            <ul class="artists">
              {#each p.artists as a, i (`${a.role}:${a.name}:${i}`)}
                <li>
                  {#if a.onSelect}
                    <button
                      class="artist-row"
                      onclick={() => {
                        a.onSelect?.();
                        propertiesDialog.close();
                      }}
                    >
                      <span class="artist-name">{a.name}</span>
                      <span class="artist-role">{a.role}</span>
                      <span class="chev" aria-hidden="true">→</span>
                    </button>
                  {:else}
                    <div class="artist-row plain">
                      <span class="artist-name">{a.name}</span>
                      <span class="artist-role">{a.role}</span>
                    </div>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        {:else if tab === "artwork"}
          {#if !p.coverKey}
            <p class="empty">No artwork embedded.</p>
          {:else}
            <div class="artwork">
              <Cover coverKey={p.coverKey} size={320} title={p.title} />
              <div class="palette">
                <div class="palette-head">
                  <span>Color palette</span>
                  {#if paletteLoading}
                    <span class="muted">extracting…</span>
                  {/if}
                </div>
                {#if paletteError}
                  <p class="empty">Couldn't read this artwork's pixels.</p>
                {:else if !palette || palette.length === 0}
                  <p class="muted">No dominant colors found.</p>
                {:else}
                  <div class="swatches">
                    {#each palette as s, i (i)}
                      {@const css = swatchToCss(s)}
                      <button
                        class="swatch"
                        style="background: {css}; color: {swatchTextColor(s)};"
                        onclick={() => copy(css)}
                        title="Click to copy"
                      >
                        <span class="hex">
                          #{s.r.toString(16).padStart(2, "0")}{s.g
                            .toString(16)
                            .padStart(2, "0")}{s.b.toString(16).padStart(2, "0")}
                        </span>
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        {/if}
      </div>
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
    width: min(560px, 100%);
    max-height: 80vh;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-l, 12px);
    box-shadow: 0 30px 60px -12px rgba(0, 0, 0, 0.65);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .head {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 14px;
    padding: 16px 18px;
    border-bottom: 1px solid var(--border);
  }
  .head-text {
    min-width: 0;
  }
  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 700;
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    margin: 2px 0 0;
    color: var(--fg-2);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .close {
    background: transparent;
    border: none;
    color: var(--fg-2);
    font-size: 22px;
    line-height: 1;
    width: 28px;
    height: 28px;
    border-radius: 6px;
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
  .tabs {
    display: flex;
    gap: 4px;
    padding: 8px 12px 0;
    border-bottom: 1px solid var(--border);
  }
  .tab {
    background: transparent;
    border: none;
    color: var(--fg-2);
    padding: 8px 14px;
    font-size: 13px;
    cursor: pointer;
    border-radius: 8px 8px 0 0;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
    position: relative;
    bottom: -1px;
  }
  .tab:hover:not(.active) {
    color: var(--fg-0);
    background: var(--bg-2);
  }
  .tab.active {
    color: var(--fg-0);
    background: var(--bg-2);
    /* Pseudo-line under the active tab masks the bottom border so it
       reads like a folder tab. */
    box-shadow: inset 0 -1px 0 0 var(--bg-2);
  }
  .body {
    padding: 16px 18px 18px;
    overflow-y: auto;
  }
  h3 {
    margin: 0 0 8px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-2);
  }
  h3:not(:first-child) {
    margin-top: 18px;
  }
  .fields {
    margin: 0;
    display: flex;
    flex-direction: column;
  }
  .row {
    display: grid;
    grid-template-columns: 130px 1fr;
    gap: 12px;
    align-items: baseline;
    padding: 6px 0;
    border-bottom: 1px solid var(--border);
  }
  .row:last-child {
    border-bottom: none;
  }
  dt {
    color: var(--fg-2);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    margin: 0;
  }
  dd {
    margin: 0;
    color: var(--fg-0);
    font-size: 13px;
    overflow-wrap: anywhere;
  }
  dd.mono {
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }
  dd .copyable {
    background: transparent;
    border: none;
    color: inherit;
    font: inherit;
    padding: 0;
    text-align: left;
    cursor: copy;
    overflow-wrap: anywhere;
    white-space: normal;
  }
  dd .copyable:hover {
    color: var(--accent);
  }
  .empty,
  .muted {
    color: var(--fg-2);
    font-size: 13px;
  }

  .artists {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .artist-row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    align-items: center;
    gap: 12px;
    width: 100%;
    background: var(--bg-2);
    border: 1px solid var(--border);
    color: var(--fg-0);
    padding: 10px 14px;
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    transition: background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .artist-row.plain {
    cursor: default;
  }
  .artist-row:hover:not(.plain) {
    background: var(--bg-3);
    border-color: var(--accent-soft, rgba(0, 122, 255, 0.3));
  }
  .artist-name {
    font-weight: 500;
  }
  .artist-role {
    color: var(--fg-2);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .chev {
    color: var(--fg-2);
    transition: transform var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .artist-row:hover:not(.plain) .chev {
    color: var(--accent);
    transform: translateX(2px);
  }

  .artwork {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
  }
  .palette {
    width: 100%;
  }
  .palette-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 8px;
    color: var(--fg-2);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  .swatches {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    gap: 8px;
  }
  .swatch {
    aspect-ratio: 1 / 1;
    border: none;
    border-radius: 12px;
    cursor: pointer;
    display: flex;
    align-items: flex-end;
    justify-content: center;
    padding: 6px;
    transition: transform var(--dur-base) var(--ease-spring);
    box-shadow: 0 1px 0 0 rgba(255, 255, 255, 0.04) inset,
      0 6px 14px -6px rgba(0, 0, 0, 0.45);
  }
  .swatch:hover {
    transform: translateY(-2px);
  }
  .hex {
    font-family: ui-monospace, monospace;
    font-size: 10px;
    font-weight: 600;
    background: rgba(0, 0, 0, 0.3);
    padding: 2px 6px;
    border-radius: 999px;
    backdrop-filter: blur(4px);
    opacity: 0;
    transition: opacity var(--dur-fast) var(--ease-out);
  }
  .swatch:hover .hex {
    opacity: 1;
  }
</style>
