<script lang="ts">
  // Convolver IR picker (R9 — UI surface).
  //
  // Surfaces three controls:
  //   1. Activé toggle bound to `audio.convolver_enabled`.
  //   2. File picker (Tauri dialog plugin) that calls
  //      `loadConvolverIr(path)` and persists `audio.convolver_ir_path`.
  //   3. Slider for `audio.convolver_gain_db` in [-24, 0] dB,
  //      step 0.5.
  //
  // Plus a status block showing whether an IR is loaded, the
  // current path, and the reported latency in milliseconds.

  import { onMount } from "svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import {
    getAudioSetting,
    setAudioSetting,
    getConvolverStatus,
    loadConvolverIr,
    unloadConvolverIr,
    type ConvolverStatus,
  } from "../../lib/api";

  let enabled = $state(false);
  let gainDb = $state(-6);
  let irPath = $state<string | null>(null);
  let status = $state<ConvolverStatus | null>(null);
  let lastError = $state<string | null>(null);
  let loading = $state(false);

  async function refreshStatus() {
    try {
      status = await getConvolverStatus();
      irPath = status.ir_path;
    } catch (e) {
      // Status is best-effort; surface but don't block the UI.
      console.warn("getConvolverStatus failed", e);
    }
  }

  onMount(async () => {
    const [en, gain, path] = await Promise.all([
      getAudioSetting<boolean>("audio.convolver_enabled"),
      getAudioSetting<number>("audio.convolver_gain_db"),
      getAudioSetting<string | null>("audio.convolver_ir_path"),
    ]);
    if (typeof en === "boolean") enabled = en;
    if (typeof gain === "number" && Number.isFinite(gain)) gainDb = gain;
    if (typeof path === "string" && path.length > 0) irPath = path;
    await refreshStatus();
  });

  async function onToggleEnabled() {
    try {
      await setAudioSetting<boolean>("audio.convolver_enabled", enabled);
      lastError = null;
      await refreshStatus();
    } catch (e) {
      lastError = String(e);
    }
  }

  async function onPickFile() {
    try {
      const picked = await openDialog({
        multiple: false,
        directory: false,
        filters: [{ name: "WAV", extensions: ["wav"] }],
      });
      if (!picked || typeof picked !== "string") return;
      loading = true;
      lastError = null;
      await loadConvolverIr(picked);
      await setAudioSetting<string>("audio.convolver_ir_path", picked);
      irPath = picked;
      await refreshStatus();
    } catch (e) {
      lastError = String(e);
    } finally {
      loading = false;
    }
  }

  async function onUnload() {
    try {
      loading = true;
      lastError = null;
      await unloadConvolverIr();
      await setAudioSetting<string>("audio.convolver_ir_path", "");
      irPath = null;
      await refreshStatus();
    } catch (e) {
      lastError = String(e);
    } finally {
      loading = false;
    }
  }

  let gainTimer: ReturnType<typeof setTimeout> | null = null;
  function onGainInput() {
    if (gainTimer) clearTimeout(gainTimer);
    gainTimer = setTimeout(async () => {
      try {
        await setAudioSetting<number>("audio.convolver_gain_db", gainDb);
        lastError = null;
      } catch (e) {
        lastError = String(e);
      }
    }, 80);
  }

  let stateLabel = $derived.by(() => {
    if (lastError) return `Erreur : ${lastError}`;
    if (loading) return "Chargement…";
    if (status?.ir_len && status.ir_len > 0) return "Chargé";
    return "Non chargé";
  });

  let stateClass = $derived(
    lastError ? "err" : status?.ir_len ? "ok" : "idle"
  );
</script>

<div class="convolver-panel">
  <header class="row">
    <label class="toggle">
      <input type="checkbox" bind:checked={enabled} onchange={onToggleEnabled} />
      <span>Activé</span>
    </label>
    <span class="badge {stateClass}">{stateLabel}</span>
  </header>

  <div class="file-row">
    <button type="button" class="btn" onclick={onPickFile} disabled={loading}>
      Choisir un fichier WAV…
    </button>
    {#if irPath}
      <button type="button" class="btn ghost" onclick={onUnload} disabled={loading}>
        Décharger
      </button>
    {/if}
  </div>

  {#if irPath}
    <div class="path" title={irPath}>{irPath}</div>
  {/if}

  <label class="slider">
    <span class="slider-label">
      Gain compensatoire
      <em>{gainDb.toFixed(1)} dB</em>
    </span>
    <input
      type="range"
      min="-24"
      max="0"
      step="0.5"
      bind:value={gainDb}
      oninput={onGainInput}
    />
  </label>

  <dl class="grid">
    <div>
      <dt>État</dt>
      <dd>{stateLabel}</dd>
    </div>
    <div>
      <dt>Latence</dt>
      <dd>
        {status && status.latency_ms > 0
          ? `${status.latency_ms.toFixed(2)} ms`
          : "—"}
      </dd>
    </div>
  </dl>
</div>

<style>
  .convolver-panel {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 12px 14px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-m);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    font-size: 13px;
    color: var(--fg-1);
  }
  .toggle input {
    accent-color: var(--accent);
    width: 16px;
    height: 16px;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    padding: 4px 10px;
    border-radius: 999px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-weight: 600;
    border: 1px solid var(--border);
    background: var(--bg-3);
  }
  .badge.ok {
    color: #4ade80;
    border-color: rgba(74, 222, 128, 0.35);
    background: rgba(74, 222, 128, 0.08);
  }
  .badge.err {
    color: #f87171;
    border-color: rgba(248, 113, 113, 0.35);
    background: rgba(248, 113, 113, 0.08);
  }
  .badge.idle {
    color: var(--fg-2);
  }
  .file-row {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .btn {
    background: var(--bg-3);
    color: var(--fg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-s);
    padding: 6px 12px;
    font-size: 12px;
    cursor: pointer;
  }
  .btn:hover:not(:disabled) {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn.ghost {
    background: transparent;
  }
  .path {
    font-size: 11px;
    color: var(--fg-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono, monospace);
  }
  .slider {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .slider-label {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    font-size: 12px;
    color: var(--fg-1);
  }
  .slider-label em {
    font-style: normal;
    color: var(--fg-2);
    font-variant-numeric: tabular-nums;
  }
  .slider input[type="range"] {
    width: 100%;
    accent-color: var(--accent);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px 16px;
    margin: 0;
  }
  .grid div {
    display: flex;
    flex-direction: column;
  }
  .grid dt {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-2);
  }
  .grid dd {
    margin: 2px 0 0;
    color: var(--fg-0);
    font-variant-numeric: tabular-nums;
  }
</style>
