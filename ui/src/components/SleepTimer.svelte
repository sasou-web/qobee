<script lang="ts">
  // Sleep-timer control: a small moon button in the player bar that
  // opens a popover of presets (15/30/45/60/90 min), an "end of
  // track" option, and an "off" entry. When a wall-clock timer is
  // armed the button shows the remaining minutes and polls the
  // backend once a second to keep the countdown fresh.

  import { onDestroy } from "svelte";
  import { fly } from "svelte/transition";
  import {
    getSleepTimer,
    setSleepAfterTrack,
    setSleepTimer,
    type SleepTimerState,
  } from "../lib/api";
  import { toasts } from "../lib/toasts.svelte";
  import Icon from "./Icon.svelte";

  let open = $state(false);
  let timer = $state<SleepTimerState>({ remaining_secs: null, stop_after_track: false });

  const PRESETS = [15, 30, 45, 60, 90];

  let active = $derived(timer.remaining_secs !== null || timer.stop_after_track);
  let label = $derived.by(() => {
    if (timer.remaining_secs !== null) {
      const m = Math.ceil(timer.remaining_secs / 60);
      return `${m} min`;
    }
    if (timer.stop_after_track) return "Fin";
    return "";
  });

  // Poll the backend once a second so the countdown stays in sync
  // even though the actual deadline lives in Rust.
  const poll = window.setInterval(() => {
    void refresh();
  }, 1000);
  onDestroy(() => window.clearInterval(poll));

  async function refresh(): Promise<void> {
    try {
      timer = await getSleepTimer();
    } catch {
      // best-effort; running outside Tauri (vite preview) etc.
    }
  }
  void refresh();

  async function arm(minutes: number): Promise<void> {
    try {
      await setSleepTimer(minutes);
      await refresh();
      toasts.info(`Minuteur réglé sur ${minutes} min`);
    } catch (e) {
      toasts.error(`Minuteur impossible : ${e}`);
    }
    open = false;
  }

  async function armEndOfTrack(): Promise<void> {
    try {
      await setSleepAfterTrack(true);
      await refresh();
      toasts.info("Arrêt à la fin du morceau");
    } catch (e) {
      toasts.error(`Minuteur impossible : ${e}`);
    }
    open = false;
  }

  async function disarm(): Promise<void> {
    try {
      await setSleepTimer(0);
      await setSleepAfterTrack(false);
      await refresh();
      toasts.info("Minuteur annulé");
    } catch (e) {
      toasts.error(`${e}`);
    }
    open = false;
  }
</script>

<div class="sleep-wrap">
  <button
    class="ctrl"
    class:active
    onmousedown={(e) => e.stopPropagation()}
    onclick={() => (open = !open)}
    aria-label="Minuteur de mise en veille"
    aria-expanded={open}
    title={active ? `Minuteur : ${label}` : "Minuteur de mise en veille"}
  >
    <Icon name="moon" size={15} />
    {#if active}<span class="pill">{label}</span>{/if}
  </button>

  {#if open}
    <!-- Click-away backdrop. -->
    <button
      class="backdrop"
      aria-label="Fermer le minuteur"
      onclick={() => (open = false)}
    ></button>
    <div class="popover" transition:fly={{ y: 6, duration: 160 }}>
      <p class="head">Minuteur</p>
      {#each PRESETS as m (m)}
        <button class="row" onclick={() => arm(m)}>{m} minutes</button>
      {/each}
      <button class="row" onclick={armEndOfTrack}>Fin du morceau</button>
      {#if active}
        <div class="sep"></div>
        <button class="row danger" onclick={disarm}>Annuler</button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .sleep-wrap {
    position: relative;
    display: inline-flex;
  }
  .ctrl {
    background: transparent;
    border: none;
    color: var(--fg-2);
    height: 30px;
    padding: 0 8px;
    border-radius: 8px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    transition: color var(--dur-fast) var(--ease-out),
      background var(--dur-fast) var(--ease-out);
  }
  .ctrl:hover {
    color: var(--fg-0);
    background: rgba(255, 255, 255, 0.05);
  }
  .ctrl.active {
    color: var(--accent);
  }
  .pill {
    font-size: 11px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.02em;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    background: transparent;
    border: none;
    cursor: default;
    z-index: 60;
  }
  .popover {
    position: absolute;
    bottom: calc(100% + 8px);
    right: 0;
    z-index: 61;
    min-width: 168px;
    padding: 6px;
    background: var(--bg-2);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 12px;
    box-shadow: 0 12px 36px rgba(0, 0, 0, 0.45);
  }
  .head {
    margin: 4px 8px 6px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--fg-3);
  }
  .row {
    display: block;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    border: none;
    background: transparent;
    color: var(--fg-1);
    font-size: 13px;
    border-radius: 8px;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .row:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--fg-0);
  }
  .row.danger {
    color: var(--danger);
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: rgba(255, 255, 255, 0.08);
  }
</style>
