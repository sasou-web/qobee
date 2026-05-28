<script lang="ts">
  // R5.2 — shown when `drive_oauth_*` rejects with `kind ===
  // "access_denied"`. The component is a *passive* error view: it
  // never re-invokes the Drive flow on its own, leaving that to
  // the parent (`DriveWizard`). A `Réessayer` button calls back
  // into the parent so the wizard can replay step "setup" with
  // the same form state.
  //
  // The "Comment configurer Google Drive ?" link opens the in-repo
  // `docs/google-cloud-setup.md` (FR) walkthrough in the user's
  // default browser via the `shell_open` Tauri command (R5.2 / R5.5).
  //
  // We keep the look in line with `DriveWizard.svelte`: same dark
  // glass shell, no accent colors.

  import { fade, fly } from "svelte/transition";
  import { shellOpen } from "../lib/api";
  import Icon from "./Icon.svelte";
  import { GOOGLE_CLOUD_SETUP_DOC_URL } from "../lib/driveDocs";

  interface Props {
    /** Raw `reason` string from `DriveCommandError::AccessDenied`.
     *  Surfaced verbatim in a `<pre>` block so the user can copy it
     *  when reporting an issue. May be empty / opaque; the prose
     *  body covers the most common causes regardless. */
    reason: string;
    /** Re-run `drive_connect` (i.e. replay the OAuth wizard from
     *  step 1). The parent wizard owns the actual OAuth call so
     *  this component stays decoupled from the Tauri command
     *  surface beyond the docs link. */
    onretry: () => void;
    /** Dismiss the error screen and close the wizard. */
    onclose: () => void;
  }

  let { reason, onretry, onclose }: Props = $props();

  async function openSetupDoc(event: MouseEvent): Promise<void> {
    // Prevent the webview from following the `<a href>` directly:
    // we want the user's default browser, not an in-app load that
    // would either fail (no http handler) or navigate the webview
    // away from Qobee. `shell_open` is a thin Tauri wrapper around
    // `webbrowser::open` (validates http/https and opens system
    // browser).
    event.preventDefault();
    try {
      await shellOpen(GOOGLE_CLOUD_SETUP_DOC_URL);
    } catch (e) {
      // Best-effort fallback: if the backend command is unavailable
      // for some reason, let the link follow its `target="_blank"`.
      window.open(GOOGLE_CLOUD_SETUP_DOC_URL, "_blank", "noopener,noreferrer");
      // eslint-disable-next-line no-console
      console.warn("shell_open failed, used window.open fallback:", e);
    }
  }
</script>

<div class="overlay" in:fade={{ duration: 180 }} out:fade={{ duration: 140 }}>
  <button
    type="button"
    class="backdrop"
    aria-label="Fermer"
    onclick={onclose}
  ></button>
  <div class="dialog" in:fly={{ y: 12, duration: 200 }} role="alertdialog" aria-labelledby="drv-err-title">
    <header class="header">
      <h2 id="drv-err-title">Connexion refusée</h2>
      <button class="close" onclick={onclose} aria-label="Fermer">
        <Icon name="compress" size={14} />
      </button>
    </header>

    <div class="body">
      <p class="lead">
        Google a refusé d'autoriser Qobee à accéder à ton Drive. C'est
        presque toujours une question de configuration côté Google
        Cloud Console — pas un problème côté Qobee.
      </p>

      <p class="hint">
        Causes les plus fréquentes :
      </p>
      <ul class="causes">
        <li>
          <strong>L'application n'est pas vérifiée par Google</strong> et
          ton compte n'est pas listé en <em>test users</em>. Tant que le
          projet OAuth reste en mode <code>Testing</code>, seuls les
          comptes ajoutés explicitement peuvent passer le consentement.
        </li>
        <li>
          <strong>Tu as refusé un ou plusieurs scopes</strong> sur l'écran
          de consentement. Qobee a besoin de
          <code>drive.readonly</code> et
          <code>drive.metadata.readonly</code> (lecture seule).
        </li>
        <li>
          <strong>Une politique d'admin Google Workspace</strong> bloque
          l'application pour ton compte. Le code Google associé est
          <code>admin_policy_enforced</code>.
        </li>
        <li>
          Le client OAuth est d'un mauvais type (Web app, Android…).
          Pour Qobee, il doit être de type <strong>Desktop app</strong>.
        </li>
      </ul>

      {#if reason}
        <details class="details">
          <summary>Détails techniques</summary>
          <pre class="reason">{reason}</pre>
        </details>
      {/if}

      <p class="hint doc-link">
        <a
          href={GOOGLE_CLOUD_SETUP_DOC_URL}
          target="_blank"
          rel="noopener noreferrer"
          onclick={openSetupDoc}
        >
          Comment configurer Google Drive ? →
        </a>
      </p>

      <div class="actions">
        <button class="ghost" onclick={onclose}>Annuler</button>
        <button class="primary" onclick={onretry}>Réessayer</button>
      </div>
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 920;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    pointer-events: auto;
  }
  .backdrop {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);
    border: none;
    cursor: default;
  }
  .dialog {
    position: relative;
    z-index: 2;
    width: min(540px, 100%);
    max-height: 88dvh;
    overflow-y: auto;
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
  }
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 18px 22px 8px;
  }
  .header h2 {
    font-size: 16px;
    font-weight: 600;
    margin: 0;
    color: var(--fg-0);
    letter-spacing: -0.01em;
  }
  .close {
    width: 28px;
    height: 28px;
    border-radius: 999px;
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.1);
    color: var(--fg-2);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: background 120ms ease, color 120ms ease;
  }
  .close:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--fg-0);
  }
  .body {
    padding: 4px 22px 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .lead {
    margin: 0;
    color: var(--fg-1);
    line-height: 1.5;
    font-size: 13px;
  }
  .hint {
    margin: 0;
    color: var(--fg-2);
    font-size: 12px;
    line-height: 1.5;
  }
  .causes {
    margin: 0;
    padding-left: 18px;
    color: var(--fg-2);
    font-size: 12px;
    line-height: 1.6;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .causes strong {
    color: var(--fg-1);
    font-weight: 600;
  }
  .causes code {
    font-family: ui-monospace, monospace;
    font-size: 11px;
    color: var(--fg-1);
    background: rgba(255, 255, 255, 0.06);
    border-radius: 4px;
    padding: 1px 4px;
  }
  .details {
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 8px;
    padding: 6px 10px;
  }
  .details summary {
    cursor: pointer;
    color: var(--fg-2);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-weight: 600;
    user-select: none;
  }
  .details summary::marker {
    color: var(--fg-3);
  }
  .reason {
    margin: 8px 0 0;
    padding: 8px 10px;
    background: rgba(239, 93, 111, 0.06);
    border: 1px solid rgba(239, 93, 111, 0.25);
    border-radius: 6px;
    color: #ef9aa6;
    font-size: 11px;
    font-family: ui-monospace, monospace;
    max-height: 140px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .doc-link {
    margin-top: 4px;
  }
  .doc-link a {
    color: var(--fg-0);
    text-decoration: underline;
    text-decoration-color: rgba(255, 255, 255, 0.3);
    text-underline-offset: 2px;
    font-size: 12px;
  }
  .doc-link a:hover {
    text-decoration-color: rgba(255, 255, 255, 0.7);
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 6px;
  }
  .actions button {
    height: 32px;
    padding: 0 14px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    background: transparent;
    color: var(--fg-1);
    font-size: 12px;
    cursor: pointer;
    transition: background 120ms ease, color 120ms ease,
      border-color 120ms ease;
  }
  .actions .ghost:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--fg-0);
  }
  .actions .primary {
    background: rgba(255, 255, 255, 0.12);
    color: var(--fg-0);
    border-color: rgba(255, 255, 255, 0.18);
  }
  .actions .primary:hover {
    background: rgba(255, 255, 255, 0.18);
    border-color: rgba(255, 255, 255, 0.26);
  }
</style>
