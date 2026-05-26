<script lang="ts">
  // Settings → Windows Integration.
  //
  // Each toggle round-trips through `set_windows_integration` so
  // the Rust side can persist the flag *and* register / unregister
  // the matching shell hook (registry key, autostart entry, etc.)
  // in the same atomic call.
  //
  // Disabled wholesale on non-Windows hosts via `platform_supported`.

  import { onMount } from "svelte";
  import {
    getWindowsIntegrationStatus,
    setWindowsIntegration,
    type WindowsIntegrationStatus,
  } from "../lib/api";

  let status = $state<WindowsIntegrationStatus | null>(null);
  let busyKey = $state<string | null>(null);
  let lastError = $state<string | null>(null);

  onMount(async () => {
    try {
      status = await getWindowsIntegrationStatus();
    } catch (e) {
      lastError = String(e);
    }
  });

  async function toggle<K extends keyof WindowsIntegrationStatus>(
    key: K,
    value: boolean,
  ) {
    if (!status) return;
    busyKey = String(key);
    lastError = null;
    try {
      const update = { [key]: value } as Record<string, unknown>;
      const next = await setWindowsIntegration(update);
      status = next;
    } catch (e) {
      lastError = String(e);
    } finally {
      busyKey = null;
    }
  }
</script>

<div class="panel">
  {#if !status}
    <p class="hint">Loading integration status…</p>
  {:else if !status.platform_supported}
    <p class="hint subtle">
      Windows integrations are unavailable on this platform. Tray
      icon, jump list, file/folder context menus and the
      <code>qobee://</code> protocol are Windows-only for now.
    </p>
  {:else}
    {#if lastError}
      <div class="error">{lastError}</div>
    {/if}

    <h4>Startup</h4>
    <label class="row toggle">
      <input
        type="checkbox"
        checked={status.autostart}
        disabled={busyKey === "autostart"}
        onchange={(e) =>
          toggle("autostart", (e.target as HTMLInputElement).checked)}
      />
      <span>
        <strong>Launch on Windows startup</strong>
        <em>Adds a "Qobee" entry under HKCU\…\Run.</em>
      </span>
    </label>
    <label class="row toggle">
      <input
        type="checkbox"
        checked={status.start_minimized}
        disabled={busyKey === "start_minimized"}
        onchange={(e) =>
          toggle("start_minimized", (e.target as HTMLInputElement).checked)}
      />
      <span>
        <strong>Start minimized</strong>
        <em>Open hidden in the tray; useful with autostart.</em>
      </span>
    </label>

    <h4>Tray &amp; window</h4>
    <label class="row toggle">
      <input
        type="checkbox"
        checked={status.show_tray_icon}
        disabled={busyKey === "show_tray_icon"}
        onchange={(e) =>
          toggle("show_tray_icon", (e.target as HTMLInputElement).checked)}
      />
      <span>
        <strong>Show tray icon</strong>
        <em>Right-click the tray for play / pause / next / library.</em>
      </span>
    </label>
    <label class="row toggle">
      <input
        type="checkbox"
        checked={status.minimize_to_tray_on_close}
        disabled={busyKey === "minimize_to_tray_on_close"}
        onchange={(e) =>
          toggle(
            "minimize_to_tray_on_close",
            (e.target as HTMLInputElement).checked,
          )}
      />
      <span>
        <strong>Minimize to tray on close</strong>
        <em>Closing the window keeps Qobee playing in the background.</em>
      </span>
    </label>

    <h4>Shell integration</h4>
    <label class="row toggle">
      <input
        type="checkbox"
        checked={status.audio_file_context_menu}
        disabled={busyKey === "audio_file_context_menu"}
        onchange={(e) =>
          toggle(
            "audio_file_context_menu",
            (e.target as HTMLInputElement).checked,
          )}
      />
      <span>
        <strong>Audio file right-click menu</strong>
        <em>
          Adds "Play in Qobee", "Add to Qobee queue", "Play next",
          "Import to library" to .mp3 / .flac / .wav and friends.
          Never replaces your default audio app.
        </em>
      </span>
    </label>
    <label class="row toggle">
      <input
        type="checkbox"
        checked={status.folder_context_menu}
        disabled={busyKey === "folder_context_menu"}
        onchange={(e) =>
          toggle(
            "folder_context_menu",
            (e.target as HTMLInputElement).checked,
          )}
      />
      <span>
        <strong>Folder right-click menu</strong>
        <em>
          Adds "Play folder", "Add folder to queue", "Scan folder",
          "Import folder" to any directory or drive.
        </em>
      </span>
    </label>
    <label class="row toggle">
      <input
        type="checkbox"
        checked={status.protocol_handler}
        disabled={busyKey === "protocol_handler"}
        onchange={(e) =>
          toggle("protocol_handler", (e.target as HTMLInputElement).checked)}
      />
      <span>
        <strong>Enable <code>qobee://</code> links</strong>
        <em>
          Lets browsers and other apps trigger
          <code>qobee://play?path=…</code> /
          <code>qobee://library</code> / <code>qobee://settings</code>.
        </em>
      </span>
    </label>
    <label class="row toggle">
      <input
        type="checkbox"
        checked={status.jump_list}
        disabled={busyKey === "jump_list"}
        onchange={(e) =>
          toggle("jump_list", (e.target as HTMLInputElement).checked)}
      />
      <span>
        <strong>Taskbar jump list</strong>
        <em>
          Right-click the Qobee icon in the taskbar for quick access
          to library, settings and last-played items.
        </em>
      </span>
    </label>

    <p class="hint subtle">
      AppUserModelID:
      <code>{status.app_user_model_id}</code>. Each integration writes
      to <code>HKCU\Software\Classes</code> only — Qobee never modifies
      machine-wide associations and never claims to be your default
      audio player.
    </p>
  {/if}
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h4 {
    margin: 12px 0 4px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-2);
  }
  .row.toggle {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 10px 12px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-s);
    cursor: pointer;
  }
  .row.toggle:hover {
    background: var(--bg-3);
  }
  .row.toggle input {
    margin-top: 2px;
    accent-color: var(--accent);
    width: 16px;
    height: 16px;
    flex-shrink: 0;
  }
  .row.toggle span {
    display: flex;
    flex-direction: column;
    gap: 2px;
    color: var(--fg-1);
    font-size: 13px;
  }
  .row.toggle strong {
    color: var(--fg-0);
    font-weight: 500;
  }
  .row.toggle em {
    font-style: normal;
    color: var(--fg-2);
    font-size: 12px;
    line-height: 1.5;
  }
  .row.toggle code {
    background: var(--bg-3);
    padding: 1px 5px;
    border-radius: 3px;
    font-size: 11px;
  }
  .hint {
    color: var(--fg-2);
    font-size: 12px;
    margin: 4px 0 0;
    line-height: 1.5;
  }
  .hint.subtle {
    color: var(--fg-3);
    margin-top: 12px;
  }
  .error {
    color: #f87171;
    font-size: 12px;
    padding: 8px 10px;
    background: rgba(248, 113, 113, 0.08);
    border: 1px solid rgba(248, 113, 113, 0.35);
    border-radius: var(--radius-s);
  }
</style>
