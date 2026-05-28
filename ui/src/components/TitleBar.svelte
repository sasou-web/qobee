<script lang="ts">
  // Custom Windows 11-style frameless title bar.
  //
  // The whole bar (minus the search field and the window controls)
  // carries `data-tauri-drag-region`. Tauri's webview turns that into
  // a native window drag, including double-click-to-maximize, so we
  // get the OS behavior for free without any JS plumbing.

  import { onMount, onDestroy } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { app } from "../lib/stores.svelte";
  import SearchBar from "./SearchBar.svelte";
  import Icon from "./Icon.svelte";

  let isMaximized = $state(false);
  let unlisten: (() => void) | null = null;

  let isSettings = $derived(app.selectedView === "settings");

  function openSettings(): void {
    app.setView("settings");
  }

  async function syncMaximized(): Promise<void> {
    try {
      isMaximized = await getCurrentWindow().isMaximized();
    } catch {
      isMaximized = false;
    }
  }

  async function minimize(): Promise<void> {
    try {
      await getCurrentWindow().minimize();
    } catch {
      // ignore
    }
  }

  async function toggleMaximize(): Promise<void> {
    try {
      await getCurrentWindow().toggleMaximize();
      // toggleMaximize resolves before Windows finishes the animation;
      // re-check shortly after to flip the icon.
      setTimeout(syncMaximized, 60);
    } catch {
      // ignore
    }
  }

  async function close(): Promise<void> {
    try {
      await getCurrentWindow().close();
    } catch {
      // ignore
    }
  }

  onMount(async () => {
    await syncMaximized();
    try {
      const win = getCurrentWindow();
      const off = await win.onResized(() => {
        void syncMaximized();
      });
      unlisten = off;
    } catch {
      // ignore — listener is best-effort
    }
  });

  onDestroy(() => {
    if (unlisten) unlisten();
  });
</script>

<header class="titlebar" data-tauri-drag-region>
  <div class="center" data-tauri-drag-region>
    <!-- SearchBar deliberately has no drag attribute: clicks stay
         interactive and never bubble up as a window drag. -->
    <SearchBar />
  </div>

  <div class="right" data-tauri-drag-region>
    <button
      class="ctrl settings-btn"
      class:active={isSettings}
      type="button"
      aria-label="Settings"
      title="Settings"
      onclick={openSettings}
    >
      <Icon name="settings" size={14} />
    </button>
    <div class="controls">
      <button
        class="ctrl"
        type="button"
        aria-label="Minimize"
        title="Minimize"
        onclick={minimize}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M0 5 H10" stroke="currentColor" stroke-width="1" />
        </svg>
      </button>

      <button
        class="ctrl"
        type="button"
        aria-label={isMaximized ? "Restore" : "Maximize"}
        title={isMaximized ? "Restore" : "Maximize"}
        onclick={toggleMaximize}
      >
        {#if isMaximized}
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <path
              d="M2.5 0.5 H9.5 V7.5 M0.5 2.5 H7.5 V9.5 H0.5 Z"
              fill="none"
              stroke="currentColor"
              stroke-width="1"
            />
          </svg>
        {:else}
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <rect
              x="0.5"
              y="0.5"
              width="9"
              height="9"
              fill="none"
              stroke="currentColor"
              stroke-width="1"
            />
          </svg>
        {/if}
      </button>

      <button
        class="ctrl close"
        type="button"
        aria-label="Close"
        title="Close"
        onclick={close}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path
            d="M0.5 0.5 L9.5 9.5 M9.5 0.5 L0.5 9.5"
            stroke="currentColor"
            stroke-width="1"
          />
        </svg>
      </button>
    </div>
  </div>
</header>

<style>
  .titlebar {
    grid-column: 2;
    grid-row: 1;
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    position: relative;
    height: var(--titlebar-height);
    background: var(--bg-shell);
    border-bottom: none;
    -webkit-user-select: none;
    user-select: none;
    z-index: 10;
  }

  .center {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    padding: 0 var(--space-4);
    min-width: 0;
  }
  .center :global(.search) {
    width: 100%;
    max-width: 480px;
  }

  .right {
    display: flex;
    align-items: center;
    height: 100%;
    background: transparent;
  }
  .controls {
    display: flex;
    height: 100%;
  }

  .ctrl {
    width: 46px;
    height: 100%;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: 0;
    padding: 0;
    color: var(--fg-2);
    cursor: default;
    transition: background 100ms ease, color 100ms ease;
    -webkit-app-region: no-drag;
  }
  .ctrl:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--fg-0);
  }
  .ctrl:active {
    background: rgba(255, 255, 255, 0.04);
  }
  .ctrl.close:hover {
    background: #c42b1c;
    color: #ffffff;
  }
  .ctrl.close:active {
    background: #a82319;
    color: #ffffff;
  }
  .ctrl.settings-btn {
    width: 38px;
    margin-right: var(--space-2);
    border-radius: 999px;
    height: 28px;
    align-self: center;
    color: var(--fg-2);
    cursor: pointer;
  }
  .ctrl.settings-btn:hover {
    color: var(--fg-0);
    background: rgba(255, 255, 255, 0.08);
  }
  .ctrl.settings-btn.active {
    color: var(--accent);
    background: var(--accent-soft);
  }
  .ctrl:focus {
    outline: none;
  }
  .ctrl:focus-visible {
    outline: 1px solid var(--accent);
    outline-offset: -2px;
  }
</style>
