<script lang="ts">
  import { fly } from "svelte/transition";
  import { contextMenu, type ContextMenuEntry } from "../lib/contextMenu.svelte";

  // Bind the menu element so we can clamp its position to the
  // viewport: anchoring to the raw mouse coords near the right/bottom
  // edge would otherwise overflow the window.
  let menuEl: HTMLDivElement | null = $state(null);

  // Once the menu is open, measure it on the next frame and adjust
  // x/y so the whole panel stays on screen.
  $effect(() => {
    if (!contextMenu.state.open || !menuEl) return;
    const rect = menuEl.getBoundingClientRect();
    const margin = 8;
    let x = contextMenu.state.x;
    let y = contextMenu.state.y;
    if (x + rect.width + margin > window.innerWidth) {
      x = Math.max(margin, window.innerWidth - rect.width - margin);
    }
    if (y + rect.height + margin > window.innerHeight) {
      y = Math.max(margin, window.innerHeight - rect.height - margin);
    }
    if (x !== contextMenu.state.x || y !== contextMenu.state.y) {
      contextMenu.state = { ...contextMenu.state, x, y };
    }
  });

  async function pick(entry: ContextMenuEntry): Promise<void> {
    if (entry === "separator") return;
    if (entry.disabled) return;
    contextMenu.close();
    try {
      await entry.onSelect();
    } catch (e) {
      console.error("context menu action failed", e);
    }
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key === "Escape") contextMenu.close();
  }
</script>

<svelte:window onkeydown={onKey} />

{#if contextMenu.state.open}
  <!-- Full-viewport scrim that swallows the next click anywhere
       outside the menu. We listen on mousedown rather than click so
       the menu disappears immediately when the user starts a new
       interaction. -->
  <button
    class="scrim"
    onmousedown={() => contextMenu.close()}
    oncontextmenu={(e) => {
      e.preventDefault();
      contextMenu.close();
    }}
    aria-label="Close context menu"
  ></button>

  <div
    class="menu"
    role="menu"
    bind:this={menuEl}
    style="left: {contextMenu.state.x}px; top: {contextMenu.state.y}px;"
    transition:fly={{ y: -4, duration: 120 }}
  >
    {#each contextMenu.state.items as entry, i (i)}
      {#if entry === "separator"}
        <div class="sep" role="separator"></div>
      {:else}
        <button
          class="item"
          class:danger={entry.danger}
          disabled={entry.disabled}
          onclick={() => pick(entry)}
          role="menuitem"
        >
          {entry.label}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: transparent;
    border: none;
    cursor: default;
    z-index: 999;
    /* Reset default button styling so the scrim is invisible. */
    padding: 0;
    margin: 0;
  }
  .menu {
    position: fixed;
    z-index: 1000;
    min-width: 200px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-m, 8px);
    box-shadow: 0 18px 40px -10px rgba(0, 0, 0, 0.55),
      0 4px 12px -4px rgba(0, 0, 0, 0.5);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .item {
    background: transparent;
    border: none;
    color: var(--fg-0);
    text-align: left;
    padding: 7px 12px;
    border-radius: var(--radius-s, 6px);
    cursor: pointer;
    font-size: 13px;
    line-height: 1.2;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .item:hover:not(:disabled) {
    background: var(--accent-soft, rgba(0, 122, 255, 0.18));
    color: var(--fg-0);
  }
  .item:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .item.danger {
    color: var(--danger, #e85a5a);
  }
  .item.danger:hover:not(:disabled) {
    background: var(--danger);
    color: #fff;
  }
  .sep {
    height: 1px;
    background: var(--border);
    margin: 4px 2px;
  }
</style>
