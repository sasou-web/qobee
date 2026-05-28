<script lang="ts">
  // Modal dialog used wherever the app needs the user to type a
  // single short string (playlist name today, possibly more later).
  //
  // Replaces `window.prompt()` calls so the input lives inside the
  // app's visual language (dark glass card, accent button, Esc to
  // dismiss, Enter to submit) rather than the WebView's native
  // chrome (`tauri.localhost says…` headerbar, blue OS button).
  //
  // Controlled component: the parent owns the `open` state and
  // resolves a `Promise<string | null>` via the `onsubmit` callback.
  // The dialog never mutates global state on its own.

  import { onMount, tick } from "svelte";
  import { fade, scale } from "svelte/transition";
  import { quintOut } from "svelte/easing";

  interface Props {
    /** Whether the dialog is rendered. Parent toggles this. */
    open: boolean;
    /** Headline shown at the top of the card. */
    title: string;
    /** Field label rendered above the input. */
    label?: string;
    /** Placeholder shown inside the empty input. */
    placeholder?: string;
    /** Pre-filled value. Useful for "Rename" flows. */
    initial?: string;
    /** Submit button label. Defaults to "Create". */
    submitLabel?: string;
    /**
     * Called when the user accepts the dialog. The string is
     * already trimmed; the dialog only fires this if the trimmed
     * value is non-empty.
     */
    onsubmit: (value: string) => void;
    /** Called when the user dismisses the dialog (Esc / backdrop / Cancel). */
    oncancel: () => void;
  }

  let {
    open,
    title,
    label = "Name",
    placeholder = "",
    initial = "",
    submitLabel = "Create",
    onsubmit,
    oncancel,
  }: Props = $props();

  let value = $state(initial);
  let inputEl = $state<HTMLInputElement | null>(null);

  // Reset the field whenever the dialog re-opens. We don't want the
  // previous (already-submitted) value bleeding into the next
  // invocation. Focus the field on the next tick so the transition
  // has time to mount the DOM node.
  $effect(() => {
    if (open) {
      value = initial;
      void tick().then(() => inputEl?.focus());
    }
  });

  function submit(): void {
    const trimmed = value.trim();
    if (!trimmed) return;
    onsubmit(trimmed);
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === "Escape") {
      e.preventDefault();
      oncancel();
    } else if (e.key === "Enter") {
      e.preventDefault();
      submit();
    }
  }

  onMount(() => {
    // Capture-phase keydown so the dialog beats any global shortcut
    // listener (the player binds Space, /, etc. on window).
    window.addEventListener("keydown", onKeydown, { capture: true });
    return () => {
      window.removeEventListener("keydown", onKeydown, { capture: true });
    };
  });
</script>

{#if open}
  <div
    class="overlay"
    role="dialog"
    aria-modal="true"
    aria-labelledby="name-prompt-title"
    in:fade={{ duration: 140 }}
    out:fade={{ duration: 100 }}
  >
    <button
      class="backdrop"
      type="button"
      aria-label="Cancel"
      onclick={oncancel}
    ></button>

    <div
      class="card"
      in:scale={{ start: 0.96, duration: 200, easing: quintOut }}
      out:scale={{ start: 0.98, duration: 120 }}
    >
      <h2 id="name-prompt-title">{title}</h2>

      <label class="field">
        <span>{label}</span>
        <input
          bind:this={inputEl}
          bind:value
          type="text"
          {placeholder}
          autocomplete="off"
          spellcheck="false"
        />
      </label>

      <div class="actions">
        <button class="ghost" type="button" onclick={oncancel}>Cancel</button>
        <button
          class="primary"
          type="button"
          onclick={submit}
          disabled={!value.trim()}
        >
          {submitLabel}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 1100;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .backdrop {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);
    border: none;
    cursor: default;
    padding: 0;
    margin: 0;
  }

  .card {
    position: relative;
    z-index: 2;
    width: min(440px, 100%);
    background: linear-gradient(
      180deg,
      rgba(20, 20, 24, 0.96) 0%,
      rgba(12, 12, 16, 0.96) 100%
    );
    backdrop-filter: blur(20px) saturate(140%);
    -webkit-backdrop-filter: blur(20px) saturate(140%);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 14px;
    box-shadow:
      0 30px 80px -20px rgba(0, 0, 0, 0.7),
      inset 0 1px 0 rgba(255, 255, 255, 0.06);
    padding: 22px 22px 18px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--fg-0);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .field span {
    color: var(--fg-2);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .field input {
    background: rgba(255, 255, 255, 0.04);
    color: var(--fg-0);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    padding: 9px 12px;
    font-size: 14px;
    font-family: inherit;
    transition:
      border-color var(--dur-fast) var(--ease-out),
      background var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out);
  }
  .field input:focus {
    outline: none;
    border-color: var(--accent);
    background: rgba(255, 255, 255, 0.08);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .field input::placeholder {
    color: var(--fg-3, var(--fg-2));
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
  .actions button {
    height: 34px;
    padding: 0 16px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    background: transparent;
    color: var(--fg-1);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out);
  }
  .actions .ghost:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--fg-0);
  }
  .actions .primary {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }
  .actions .primary:hover:not(:disabled) {
    filter: brightness(1.08);
    transform: translateY(-1px);
  }
  .actions .primary:active:not(:disabled) {
    transform: translateY(0);
  }
  .actions .primary:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
</style>
