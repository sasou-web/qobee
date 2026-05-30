<script lang="ts">
  // TrayNotice — bandeau persistant de première fermeture en zone de
  // notification (R7).
  //
  // Le backend (`src-tauri/src/lib.rs`, hook `CloseRequested`) émet
  // l'événement `tray:first-close-notice` UNE SEULE fois : la première
  // fois que la fenêtre principale est masquée en zone de notification
  // sous un `close_behavior` qui garde l'app vivante. La persistance de
  // l'« affiché une fois » (`windows.tray_notice_shown`) est entièrement
  // gérée côté backend — ce composant se contente de réagir à
  // l'événement et n'écrit JAMAIS `close_behavior` (R7.4).
  //
  // À la réception : on affiche le bandeau « Qobee continue dans la zone
  // de notification » (R7.1) avec un bouton « Quitter complètement » qui
  // appelle `quitApp()` (commande `quit_app`, R7.2). Une croix permet de
  // masquer l'avis. Le listener vit au niveau du shell (App.svelte), donc
  // il est toujours actif quand l'événement survient.

  import { onMount } from "svelte";
  import { fly } from "svelte/transition";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { quitApp } from "../lib/api";
  import { announce } from "../lib/a11y";

  const MESSAGE = "Qobee continue dans la zone de notification";

  let visible = $state(false);
  let quitting = $state(false);

  function show() {
    // Idempotent côté UI : le backend ne ré-émet pas l'événement, mais
    // on évite tout double-affichage si l'événement arrivait deux fois.
    if (visible) return;
    visible = true;
    // Annonce non intrusive pour les lecteurs d'écran (R2.9) : le
    // message apparaît sans voler le focus clavier.
    announce(MESSAGE);
  }

  function dismiss() {
    visible = false;
  }

  async function quit() {
    if (quitting) return;
    quitting = true;
    try {
      await quitApp();
    } catch {
      // Hors Tauri (vite preview) ou échec de la commande : on laisse
      // le bandeau ouvert pour que l'utilisateur puisse réessayer.
      quitting = false;
    }
  }

  onMount(() => {
    let unlisten: UnlistenFn | null = null;
    void listen<unknown>("tray:first-close-notice", () => {
      show();
    }).then((un) => {
      unlisten = un;
    });
    return () => {
      unlisten?.();
    };
  });
</script>

{#if visible}
  <div
    class="tray-notice"
    role="status"
    aria-live="polite"
    aria-label={MESSAGE}
    transition:fly={{ y: 16, duration: 220 }}
  >
    <span class="dot" aria-hidden="true"></span>
    <span class="msg">{MESSAGE}</span>
    <div class="actions">
      <button
        type="button"
        class="quit"
        onclick={quit}
        disabled={quitting}
        aria-label="Quitter complètement"
        title="Quitter complètement"
      >
        Quitter complètement
      </button>
      <button
        type="button"
        class="close"
        onclick={dismiss}
        aria-label="Fermer l'avis"
        title="Fermer"
      >
        <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
          <path
            d="M3 3l8 8M11 3l-8 8"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
          />
        </svg>
      </button>
    </div>
  </div>
{/if}

<style>
  .tray-notice {
    position: fixed;
    left: 50%;
    bottom: calc(var(--player-zone-h, 72px) + 16px);
    transform: translateX(-50%);
    z-index: 1100;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    max-width: min(560px, calc(100vw - 32px));
    background: var(--bg-1);
    color: var(--fg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    font-size: 13px;
    box-shadow: var(--shadow-2);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--accent);
  }
  .msg {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
  }
  .quit {
    background: var(--accent-soft);
    border: 1px solid var(--accent);
    color: var(--fg-0);
    border-radius: var(--radius-s);
    padding: 6px 12px;
    font-size: 12px;
    white-space: nowrap;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .quit:hover:not(:disabled) {
    background: var(--accent-glow);
  }
  .quit:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .close {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    color: var(--fg-2);
    border: 1px solid transparent;
    border-radius: var(--radius-s);
    cursor: pointer;
  }
  .close:hover {
    color: var(--fg-0);
    background: var(--bg-2);
  }
</style>
