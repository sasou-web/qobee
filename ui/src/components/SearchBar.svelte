<script lang="ts">
  import { fade } from "svelte/transition";
  import { app } from "../lib/stores.svelte";
  import Icon from "./Icon.svelte";

  function onInput(e: Event): void {
    const v = (e.target as HTMLInputElement).value;
    app.searchQuery = v;
    if (v.trim().length > 0 && app.selectedView !== "search") {
      app.setView("search");
    }
  }

  function onFocus(): void {
    if (app.selectedView !== "search") {
      app.setView("search");
    }
  }
</script>

<div class="search">
  <span class="icon"><Icon name="search" size={16} /></span>
  <input
    type="search"
    placeholder="Search tracks, albums, artists…"
    value={app.searchQuery}
    oninput={onInput}
    onfocus={onFocus}
  />
  {#if app.searchQuery.length > 0}
    <button
      class="clear"
      onclick={() => (app.searchQuery = "")}
      aria-label="Clear"
      transition:fade={{ duration: 120 }}
    >✕</button>
  {/if}
</div>

<style>
  .search {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    background: var(--bg-2);
    border: 1px solid transparent;
    border-radius: var(--radius-pill);
    height: var(--searchbar-height);
    padding: 0 var(--space-4);
    color: var(--fg-2);
    transition: background var(--dur-base) var(--ease-out),
      border-color var(--dur-base) var(--ease-out),
      box-shadow var(--dur-base) var(--ease-out),
      color var(--dur-base) var(--ease-out);
  }
  .search:hover {
    background: var(--bg-3);
    color: var(--fg-1);
  }
  .search:focus-within {
    background: var(--bg-2);
    border-color: var(--accent);
    color: var(--fg-1);
    box-shadow: 0 0 0 3px var(--accent-glow);
  }
  .icon {
    display: inline-flex;
    transition: transform var(--dur-base) var(--ease-spring),
      color var(--dur-base) var(--ease-out);
  }
  .search:focus-within .icon {
    color: var(--accent);
    transform: scale(1.1);
  }
  input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    color: var(--fg-0);
    font-size: 14px;
    height: 100%;
  }
  input::placeholder {
    color: var(--fg-2);
  }
  /* Hide the native clear (×) on Chromium; we render our own. */
  input[type="search"]::-webkit-search-cancel-button {
    -webkit-appearance: none;
    appearance: none;
  }
  .clear {
    background: transparent;
    border: none;
    color: var(--fg-2);
    cursor: pointer;
    font-size: 12px;
    padding: 2px 6px;
    border-radius: 50%;
    width: 22px;
    height: 22px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .clear:hover {
    color: var(--fg-0);
    background: var(--bg-3);
  }
</style>
