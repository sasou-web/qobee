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
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: var(--radius-pill);
    height: var(--searchbar-height);
    padding: 0 var(--space-3);
    color: var(--fg-2);
    transition: background var(--dur-base) var(--ease-out),
      border-color var(--dur-base) var(--ease-out),
      color var(--dur-base) var(--ease-out);
  }
  .search:hover {
    background: rgba(255, 255, 255, 0.06);
    border-color: rgba(255, 255, 255, 0.1);
    color: var(--fg-1);
  }
  /* Focus state stays neutral on purpose: no accent ring or border
     change so the header doesn't flash when the user types. */
  .search:focus-within {
    background: rgba(255, 255, 255, 0.06);
    border-color: rgba(255, 255, 255, 0.12);
    color: var(--fg-1);
  }
  .icon {
    display: inline-flex;
  }
  input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    color: var(--fg-0);
    font-size: 13px;
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
