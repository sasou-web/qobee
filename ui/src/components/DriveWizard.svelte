<script lang="ts">
  // Three-step wizard that walks the user through connecting a
  // Google Drive folder to Qobee.
  //
  // 1. **Setup**: explain how to create a Google Cloud OAuth
  //    Desktop client, ask for the client id + secret, plus a
  //    display name for this source.
  // 2. **Authorize**: kick off the desktop OAuth flow. We open the
  //    consent screen in the system browser and poll the loopback
  //    listener for the redirect.
  // 3. **Done**: show which Google account got connected and
  //    close.
  //
  // The Rust side does the heavy lifting (loopback listener, PKCE,
  // token persistence in the OS keychain). The UI just orchestrates
  // the user flow and shows progress.
  //
  // No accent color anywhere — keeps the wizard at home in the
  // dark glass shell we just built.

  import { fade, fly } from "svelte/transition";
  import {
    driveIndex,
    driveListFolder,
    driveOAuthCancel,
    driveOAuthStart,
    driveOAuthWait,
    driveSetFolder,
    type DriveListItem,
  } from "../lib/api";
  import Icon from "./Icon.svelte";

  interface Props {
    open: boolean;
    onclose: () => void;
    onsuccess: () => void;
  }

  let { open, onclose, onsuccess }: Props = $props();

  type Step = "setup" | "auth" | "pick" | "indexing" | "done" | "error";

  let step = $state<Step>("setup");
  let name = $state("My Drive");
  let clientId = $state("");
  let clientSecret = $state("");
  let sessionId: string | null = null;
  let authUrl = $state("");
  let busy = $state(false);
  let errorMessage = $state<string | null>(null);
  let connectedEmail = $state<string | null>(null);

  // PR3: folder picker + indexing state.
  let createdSourceId = $state<number | null>(null);
  let folderItems = $state<DriveListItem[]>([]);
  let folderStack = $state<{ id: string; name: string }[]>([
    { id: "", name: "My Drive" },
  ]);
  let pickedFolder = $state<{ id: string; name: string } | null>(null);
  let indexingProgress = $state<{ visited: number; indexed: number; current: string }>({
    visited: 0,
    indexed: 0,
    current: "",
  });
  let indexResult = $state<{ visited: number; indexed: number; errors: string[] } | null>(null);

  function reset(): void {
    step = "setup";
    name = "My Drive";
    clientId = "";
    clientSecret = "";
    sessionId = null;
    authUrl = "";
    busy = false;
    errorMessage = null;
    connectedEmail = null;
    createdSourceId = null;
    folderItems = [];
    folderStack = [{ id: "", name: "My Drive" }];
    pickedFolder = null;
    indexingProgress = { visited: 0, indexed: 0, current: "" };
    indexResult = null;
  }

  async function close(): Promise<void> {
    if (sessionId) {
      await driveOAuthCancel(sessionId).catch(() => {
        // best-effort
      });
      sessionId = null;
    }
    reset();
    onclose();
  }

  function copyAuthUrl(): void {
    if (!authUrl) return;
    void navigator.clipboard?.writeText(authUrl);
  }

  async function startAuth(): Promise<void> {
    if (busy) return;
    if (!clientId.trim() || !clientSecret.trim() || !name.trim()) {
      errorMessage = "Fill in every field before continuing.";
      return;
    }
    busy = true;
    errorMessage = null;
    try {
      const r = await driveOAuthStart(clientId.trim(), clientSecret.trim());
      sessionId = r.session_id;
      authUrl = r.auth_url;
      step = "auth";
    } catch (e) {
      errorMessage = String(e);
      step = "error";
    } finally {
      busy = false;
    }
  }

  async function waitForRedirect(): Promise<void> {
    if (!sessionId) return;
    busy = true;
    errorMessage = null;
    try {
      const r = await driveOAuthWait(sessionId, name.trim());
      sessionId = null;
      connectedEmail = r.email ?? r.display_name ?? "Connected";
      createdSourceId = r.source_id;
      step = "pick";
      // Eagerly load the root folder so the user lands on a
      // populated picker.
      void loadFolder("");
    } catch (e) {
      errorMessage = String(e);
      step = "error";
    } finally {
      busy = false;
    }
  }

  async function loadFolder(folderId: string): Promise<void> {
    if (createdSourceId === null) return;
    busy = true;
    try {
      folderItems = await driveListFolder(createdSourceId, folderId);
    } catch (e) {
      errorMessage = String(e);
    } finally {
      busy = false;
    }
  }

  async function enterFolder(item: DriveListItem): Promise<void> {
    folderStack = [...folderStack, { id: item.id, name: item.name }];
    await loadFolder(item.id);
  }

  async function popFolder(): Promise<void> {
    if (folderStack.length <= 1) return;
    folderStack = folderStack.slice(0, -1);
    const top = folderStack[folderStack.length - 1];
    if (!top) return;
    await loadFolder(top.id);
  }

  async function pickCurrentFolder(): Promise<void> {
    if (createdSourceId === null) return;
    const top = folderStack[folderStack.length - 1];
    if (!top) return;
    pickedFolder = top;
    busy = true;
    errorMessage = null;
    try {
      await driveSetFolder(createdSourceId, top.id || "root", top.name);
      step = "indexing";
      void runIndex();
    } catch (e) {
      errorMessage = String(e);
      step = "error";
    } finally {
      busy = false;
    }
  }

  async function runIndex(): Promise<void> {
    if (createdSourceId === null) return;
    busy = true;
    try {
      const r = await driveIndex(createdSourceId);
      indexResult = {
        visited: r.files_visited,
        indexed: r.files_indexed,
        errors: r.errors,
      };
      step = "done";
      onsuccess();
    } catch (e) {
      errorMessage = String(e);
      step = "error";
    } finally {
      busy = false;
    }
  }

  // Auto-start the redirect listener when we enter the auth step.
  $effect(() => {
    if (step === "auth" && sessionId && !busy) {
      void waitForRedirect();
    }
  });
</script>

{#if open}
  <div class="overlay" in:fade={{ duration: 180 }} out:fade={{ duration: 140 }}>
    <button
      type="button"
      class="backdrop"
      aria-label="Close"
      onclick={close}
    ></button>
    <div class="dialog" in:fly={{ y: 12, duration: 200 }}>
      <header class="header">
        <h2>Connect Google Drive</h2>
        <button class="close" onclick={close} aria-label="Close">
          <Icon name="compress" size={14} />
        </button>
      </header>

      {#if step === "setup"}
        <div class="body" in:fade={{ duration: 160 }}>
          <p class="lead">
            Qobee uses your own Google OAuth client so nothing about
            your library transits through a third party. Setup is
            one-off.
          </p>

          <ol class="steps">
            <li>
              Go to <a
                href="https://console.cloud.google.com/apis/credentials"
                target="_blank"
                rel="noopener noreferrer">Google Cloud → Credentials</a
              >.
            </li>
            <li>Create an OAuth 2.0 Client ID, type <strong>Desktop app</strong>.</li>
            <li>
              Enable the <a
                href="https://console.cloud.google.com/apis/library/drive.googleapis.com"
                target="_blank"
                rel="noopener noreferrer">Google Drive API</a
              > on the same project.
            </li>
            <li>Paste the Client ID and Client Secret below.</li>
          </ol>

          <div class="field">
            <label for="drv-name">Source name</label>
            <input
              id="drv-name"
              type="text"
              bind:value={name}
              placeholder="My Drive"
            />
          </div>
          <div class="field">
            <label for="drv-cid">Client ID</label>
            <input
              id="drv-cid"
              type="text"
              bind:value={clientId}
              placeholder="000000000000-xxxxx.apps.googleusercontent.com"
              autocomplete="off"
              spellcheck="false"
            />
          </div>
          <div class="field">
            <label for="drv-csec">Client Secret</label>
            <input
              id="drv-csec"
              type="password"
              bind:value={clientSecret}
              placeholder="GOCSPX-..."
              autocomplete="off"
              spellcheck="false"
            />
          </div>

          {#if errorMessage}
            <p class="error">{errorMessage}</p>
          {/if}

          <div class="actions">
            <button class="ghost" onclick={close}>Cancel</button>
            <button class="primary" onclick={startAuth} disabled={busy}>
              {busy ? "Starting…" : "Authorize in browser"}
            </button>
          </div>
        </div>
      {:else if step === "auth"}
        <div class="body" in:fade={{ duration: 160 }}>
          <p class="lead">
            Your browser should have opened the Google consent screen.
            Sign in and click <strong>Allow</strong>; Qobee will pick up the result automatically.
          </p>
          <div class="auth-card">
            <span class="spinner" aria-hidden="true"></span>
            <span>Waiting for the redirect…</span>
          </div>
          <p class="hint">
            Browser didn't open? Copy the link below and paste it manually.
          </p>
          <div class="auth-link">
            <input
              readonly
              value={authUrl}
              onclick={(e) => (e.target as HTMLInputElement).select()}
            />
            <button class="ghost" onclick={copyAuthUrl}>Copy</button>
          </div>
          <div class="actions">
            <button class="ghost" onclick={close}>Cancel</button>
          </div>
        </div>
      {:else if step === "pick"}
        <div class="body" in:fade={{ duration: 160 }}>
          <p class="lead">
            Pick the folder Qobee should index. Click a folder to enter it,
            then "Use this folder" once you're inside the right place.
          </p>
          <div class="breadcrumb">
            {#each folderStack as f, i (`${f.id}-${i}`)}
              {#if i > 0}<span class="sep">/</span>{/if}
              <span>{f.name}</span>
            {/each}
          </div>
          <div class="picker">
            {#if folderStack.length > 1}
              <button class="picker-row up" onclick={popFolder}>
                <span class="picker-icon">↩</span>
                <span class="picker-name">..</span>
              </button>
            {/if}
            {#if busy}
              <div class="auth-card">
                <span class="spinner" aria-hidden="true"></span>
                <span>Loading…</span>
              </div>
            {:else if folderItems.filter((i) => i.is_folder).length === 0}
              <p class="empty">No subfolder here.</p>
            {:else}
              {#each folderItems.filter((i) => i.is_folder) as item (item.id)}
                <button
                  class="picker-row"
                  onclick={() => enterFolder(item)}
                  title={item.name}
                >
                  <span class="picker-icon">📁</span>
                  <span class="picker-name">{item.name}</span>
                </button>
              {/each}
            {/if}
          </div>
          <p class="hint">
            Audio files in this folder: {folderItems.filter((i) => !i.is_folder).length}
          </p>
          {#if errorMessage}
            <p class="error">{errorMessage}</p>
          {/if}
          <div class="actions">
            <button class="ghost" onclick={close}>Skip</button>
            <button class="primary" onclick={pickCurrentFolder} disabled={busy}>
              Use this folder
            </button>
          </div>
        </div>
      {:else if step === "indexing"}
        <div class="body" in:fade={{ duration: 160 }}>
          <p class="lead">Indexing your library…</p>
          <div class="auth-card">
            <span class="spinner" aria-hidden="true"></span>
            <div>
              <div>{indexingProgress.indexed} / {indexingProgress.visited} indexed</div>
              {#if indexingProgress.current}
                <div class="subtle">{indexingProgress.current}</div>
              {/if}
            </div>
          </div>
          <p class="hint">
            We download a small head + tail of each file to read its tags.
            Streaming playback ships in the next build; for now indexed
            files appear in your library but cannot be played yet.
          </p>
        </div>
      {:else if step === "done"}
        <div class="body" in:fade={{ duration: 160 }}>
          <p class="lead">Done.</p>
          <div class="success-card">
            <span class="dot"></span>
            <div>
              <div class="strong">{connectedEmail}</div>
              <div class="subtle">
                {#if indexResult}
                  Indexed {indexResult.indexed} / {indexResult.visited} audio files in
                  "{pickedFolder?.name ?? name}".
                  {#if indexResult.errors.length > 0}
                    {indexResult.errors.length} error{indexResult.errors.length > 1 ? "s" : ""}.
                  {/if}
                {:else}
                  Source "{name}" is connected.
                {/if}
              </div>
            </div>
          </div>
          <div class="actions">
            <button class="primary" onclick={close}>Close</button>
          </div>
        </div>
      {:else}
        <div class="body" in:fade={{ duration: 160 }}>
          <p class="lead">Authorization failed.</p>
          {#if errorMessage}
            <pre class="error-block">{errorMessage}</pre>
          {/if}
          <div class="actions">
            <button class="ghost" onclick={close}>Close</button>
            <button
              class="primary"
              onclick={() => {
                step = "setup";
                errorMessage = null;
              }}>Try again</button
            >
          </div>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 900;
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
    width: min(520px, 100%);
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
    gap: 14px;
  }
  .lead {
    margin: 0;
    color: var(--fg-1);
    line-height: 1.5;
    font-size: 13px;
  }
  .steps {
    margin: 0;
    padding-left: 18px;
    color: var(--fg-2);
    font-size: 12px;
    line-height: 1.65;
  }
  .steps a {
    color: var(--fg-0);
    text-decoration: underline;
    text-decoration-color: rgba(255, 255, 255, 0.3);
    text-underline-offset: 2px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .field label {
    font-size: 11px;
    font-weight: 600;
    color: var(--fg-2);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .field input {
    background: rgba(255, 255, 255, 0.04);
    color: var(--fg-0);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: 13px;
    font-family: ui-monospace, monospace;
    transition: border-color 120ms ease, background 120ms ease;
  }
  .field input:focus {
    outline: none;
    border-color: rgba(255, 255, 255, 0.28);
    background: rgba(255, 255, 255, 0.08);
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
  .actions button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .auth-card,
  .success-card {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 10px;
    color: var(--fg-1);
    font-size: 13px;
  }
  .success-card .strong {
    font-weight: 600;
    color: var(--fg-0);
  }
  .success-card .subtle {
    font-size: 11px;
    color: var(--fg-2);
    margin-top: 2px;
  }
  .success-card .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #4ed1a1;
    box-shadow: 0 0 10px #4ed1a1aa;
  }
  .spinner {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.16);
    border-top-color: var(--fg-0);
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .hint {
    font-size: 11px;
    color: var(--fg-2);
    margin: 0;
  }
  .auth-link {
    display: flex;
    gap: 6px;
  }
  .auth-link input {
    flex: 1;
    background: rgba(255, 255, 255, 0.04);
    color: var(--fg-1);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 8px;
    padding: 6px 8px;
    font-size: 11px;
    font-family: ui-monospace, monospace;
  }
  .auth-link button {
    height: 30px;
    padding: 0 10px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--fg-1);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    font-size: 11px;
    cursor: pointer;
    transition: background 120ms ease;
  }
  .auth-link button:hover {
    background: rgba(255, 255, 255, 0.12);
    color: var(--fg-0);
  }
  .error {
    margin: 0;
    color: #ef9aa6;
    font-size: 12px;
  }
  .error-block {
    margin: 0;
    padding: 10px 12px;
    background: rgba(239, 93, 111, 0.06);
    border: 1px solid rgba(239, 93, 111, 0.25);
    border-radius: 8px;
    color: #ef9aa6;
    font-size: 11px;
    font-family: ui-monospace, monospace;
    max-height: 160px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
  }

  /* Folder picker */
  .breadcrumb {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    color: var(--fg-2);
    font-size: 11px;
    padding: 4px 0 0;
  }
  .breadcrumb .sep {
    color: var(--fg-3);
  }
  .picker {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 240px;
    overflow-y: auto;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 8px;
    padding: 4px;
    background: rgba(0, 0, 0, 0.2);
  }
  .picker-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border: none;
    background: transparent;
    color: var(--fg-1);
    font-size: 12px;
    text-align: left;
    border-radius: 6px;
    cursor: pointer;
    transition: background 120ms ease, color 120ms ease;
  }
  .picker-row:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--fg-0);
  }
  .picker-row.up {
    color: var(--fg-2);
  }
  .picker-icon {
    width: 16px;
    text-align: center;
    flex-shrink: 0;
  }
  .picker-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    padding: 10px;
    color: var(--fg-2);
    font-size: 11px;
    margin: 0;
    text-align: center;
  }
</style>
