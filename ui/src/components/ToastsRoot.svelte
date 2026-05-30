<script lang="ts">
  import { fly, fade } from "svelte/transition";
  import { toasts } from "../lib/toasts.svelte";
  import { announce } from "../lib/a11y";

  // Mirror every newly pushed toast into the shared ARIA live region
  // (R2.9) so NVDA reads it out without moving focus. Toast ids are
  // assigned monotonically by the store, so we only have to remember
  // the highest id we've already announced and replay the tail when
  // the stack grows. The visible `.root` below is itself a
  // `role="status"` region for redundancy; `announce` additionally
  // forces re-reading of repeated identical messages that an
  // unchanged DOM node would otherwise be ignored by screen readers.
  let lastAnnouncedId = $state(0);
  $effect(() => {
    for (const t of toasts.items) {
      if (t.id > lastAnnouncedId) {
        announce(t.message);
        lastAnnouncedId = t.id;
      }
    }
  });
</script>

<!-- Top-right stack of stacked, transient notifications. Each toast
     is a button so the user can dismiss it manually, but pointer
     events are also disabled on the container so the surrounding UI
     remains clickable through the empty space between toasts. The
     stack is an ARIA live region (R2.9) announced politely. -->
<div class="root" role="status" aria-live="polite" aria-atomic="true">
  {#each toasts.items as t (t.id)}
    <button
      class="toast"
      data-kind={t.kind}
      onclick={() => toasts.dismiss(t.id)}
      in:fly={{ x: 24, duration: 220 }}
      out:fade={{ duration: 160 }}
      title="Dismiss"
    >
      <span class="dot" aria-hidden="true"></span>
      <span class="msg">{t.message}</span>
    </button>
  {/each}
</div>

<style>
  .root {
    position: fixed;
    top: calc(var(--titlebar-height) + 12px);
    right: 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 1000;
    pointer-events: none;
    max-width: 360px;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: 10px;
    background: var(--bg-1);
    color: var(--fg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 10px 14px;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
    transition: transform var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .toast:hover {
    transform: translateY(-1px);
    border-color: var(--accent-soft);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .toast[data-kind="info"] .dot {
    background: var(--accent);
  }
  .toast[data-kind="success"] .dot {
    background: #4ade80;
  }
  .toast[data-kind="warn"] .dot {
    background: #facc15;
  }
  .toast[data-kind="error"] .dot {
    background: var(--danger);
  }
  .toast[data-kind="error"] {
    border-color: var(--danger);
  }
  .msg {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }
</style>
