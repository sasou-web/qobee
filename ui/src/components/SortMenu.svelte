<script lang="ts" generics="K extends string">
  // A popover sort menu, shaped like the one in the design reference:
  // a vertical list of sort *fields*, then a "Sorting Order" section
  // with Ascending / Descending. The active field is rendered with the
  // accent background; the active order is marked with a check.
  //
  // Keeping it generic over `K` lets the same component drive both the
  // Albums tab (Date Added / Name / Artist / …) and the Genres tab
  // (Name / Track Count) without rewriting state plumbing.

  import { onMount } from "svelte";
  import { fly } from "svelte/transition";
  import type { SortDirection } from "../lib/sort";

  interface Option<KK extends string> {
    key: KK;
    label: string;
  }

  interface Props {
    options: Array<Option<K>>;
    field: K;
    direction: SortDirection;
    onfield: (field: K) => void;
    ondirection: (dir: SortDirection) => void;
    /** Optional label rendered on the trigger button. Defaults to "Sort". */
    label?: string;
  }

  let { options, field, direction, onfield, ondirection, label = "Sort" }: Props = $props();

  let open = $state(false);
  let triggerEl: HTMLButtonElement | undefined = $state();
  let menuEl: HTMLDivElement | undefined = $state();

  const activeLabel = $derived(
    options.find((o) => o.key === field)?.label ?? label
  );

  function toggle(): void {
    open = !open;
  }

  function close(): void {
    open = false;
  }

  function pickField(k: K): void {
    onfield(k);
    close();
  }

  function pickDirection(d: SortDirection): void {
    ondirection(d);
    close();
  }

  function onDocClick(e: MouseEvent): void {
    if (!open) return;
    const t = e.target as Node | null;
    if (!t) return;
    if (menuEl && menuEl.contains(t)) return;
    if (triggerEl && triggerEl.contains(t)) return;
    close();
  }

  function onKey(e: KeyboardEvent): void {
    if (open && e.key === "Escape") {
      close();
      triggerEl?.focus();
    }
  }

  onMount(() => {
    document.addEventListener("click", onDocClick, true);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("click", onDocClick, true);
      document.removeEventListener("keydown", onKey);
    };
  });
</script>

<div class="wrap">
  <button
    bind:this={triggerEl}
    class="trigger"
    type="button"
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={toggle}
  >
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M3 6h13" />
      <path d="M3 12h9" />
      <path d="M3 18h5" />
      <path d="M17 4v16" />
      <path d="m13 16 4 4 4-4" />
    </svg>
    <span class="trigger-label">{activeLabel}</span>
    <svg class="chev" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="m6 9 6 6 6-6" />
    </svg>
  </button>

  {#if open}
    <div
      bind:this={menuEl}
      class="menu"
      role="menu"
      transition:fly={{ y: -4, duration: 140 }}
    >
      {#each options as opt (opt.key)}
        <button
          class="row field"
          class:active={opt.key === field}
          role="menuitemradio"
          aria-checked={opt.key === field}
          onclick={() => pickField(opt.key)}
        >
          <span class="row-label">{opt.label}</span>
        </button>
      {/each}

      <div class="divider"></div>
      <div class="section-label">Sorting Order</div>

      <button
        class="row order"
        role="menuitemradio"
        aria-checked={direction === "asc"}
        onclick={() => pickDirection("asc")}
      >
        <span class="row-label">Ascending</span>
        {#if direction === "asc"}
          <svg class="check" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M5 12l5 5 9-11" />
          </svg>
        {/if}
      </button>

      <button
        class="row order"
        role="menuitemradio"
        aria-checked={direction === "desc"}
        onclick={() => pickDirection("desc")}
      >
        <span class="row-label">Descending</span>
        {#if direction === "desc"}
          <svg class="check" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M5 12l5 5 9-11" />
          </svg>
        {/if}
      </button>
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
    display: inline-block;
  }

  .trigger {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    padding: 6px 10px 6px 10px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    color: var(--fg-1);
    font-size: 13px;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .trigger:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .trigger[aria-expanded="true"] {
    border-color: var(--accent);
    color: var(--fg-0);
  }
  .trigger-label {
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chev {
    color: var(--fg-2);
  }

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 30;
    min-width: 240px;
    padding: 6px;
    background: #1a1a1d;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lift);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 10px 14px;
    background: transparent;
    border: none;
    border-radius: var(--radius-md);
    color: var(--fg-0);
    font-size: 14px;
    text-align: left;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .row:hover {
    background: var(--bg-3);
  }
  .row.active {
    background: var(--accent);
    color: #fff;
  }
  .row.active:hover {
    background: var(--accent);
  }
  .row-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .check {
    color: var(--accent);
    flex: 0 0 auto;
  }

  .divider {
    height: 1px;
    background: var(--border);
    margin: 6px 4px 4px;
  }
  .section-label {
    padding: 6px 14px;
    color: var(--fg-3);
    font-size: 13px;
  }
</style>
