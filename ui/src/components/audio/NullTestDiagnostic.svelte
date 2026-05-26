
<script lang="ts">
  // Null-test diagnostic panel (R11 — UI surface).
  //
  // Two-button shell: "Lancer le diagnostic" runs the orchestrator
  // (Loopback in Shared, Pre-Render in Exclusive); "Annuler" trips
  // the cancellation flag observed by the in-flight run. Result
  // block renders a tri-state badge (vert / ambre / gris) plus the
  // raw stats (peak / RMS dB diff, sample mismatch count, aligned
  // frames) and the captured-via mode.

  import {
    runNullTest,
    cancelNullTest,
    type NullTestReport,
    type NullTestConclusion,
    type NullTestSource,
  } from "../../lib/api";

  let running = $state(false);
  let report = $state<NullTestReport | null>(null);
  let lastError = $state<string | null>(null);

  async function onRun() {
    running = true;
    lastError = null;
    report = null;
    try {
      report = await runNullTest();
    } catch (e) {
      lastError = String(e);
    } finally {
      running = false;
    }
  }

  async function onCancel() {
    try {
      await cancelNullTest();
    } catch (e) {
      lastError = String(e);
    }
  }

  function conclusionLabel(c: NullTestConclusion): string {
    switch (c) {
      case "bit_perfect":
        return "Bit-perfect";
      case "modified":
        return "Modifié";
      case "inconclusive":
      default:
        return "Inconclu";
    }
  }

  function conclusionClass(c: NullTestConclusion): string {
    switch (c) {
      case "bit_perfect":
        return "ok";
      case "modified":
        return "warn";
      case "inconclusive":
      default:
        return "idle";
    }
  }

  function sourceLabel(s: NullTestSource): string {
    return s === "loopback" ? "Loopback (Shared)" : "Pre-Render (Exclusive)";
  }

  function formatDb(v: number): string {
    if (!Number.isFinite(v)) return "−∞ dBFS";
    return `${v.toFixed(1)} dBFS`;
  }
</script>

<div class="null-test-panel">
  <div class="actions">
    <button type="button" class="btn primary" onclick={onRun} disabled={running}>
      {#if running}
        <span class="spinner" aria-hidden="true"></span>
        Diagnostic en cours…
      {:else}
        Lancer le diagnostic
      {/if}
    </button>
    <button type="button" class="btn ghost" onclick={onCancel} disabled={!running}>
      Annuler
    </button>
  </div>

  {#if lastError}
    <div class="error">{lastError}</div>
  {/if}

  {#if report}
    <div class="result">
      <header class="row">
        <span class="badge {conclusionClass(report.conclusion)}">
          {conclusionLabel(report.conclusion)}
        </span>
        <span class="mode-badge">{sourceLabel(report.captured_via)}</span>
      </header>

      <dl class="grid">
        <div>
          <dt>Diff. crête</dt>
          <dd>{formatDb(report.peak_diff_dbfs)}</dd>
        </div>
        <div>
          <dt>Diff. RMS</dt>
          <dd>{formatDb(report.rms_diff_dbfs)}</dd>
        </div>
        <div>
          <dt>Échantillons différents</dt>
          <dd>{report.samples_diff_count.toLocaleString("fr-FR")}</dd>
        </div>
        <div>
          <dt>Trames alignées</dt>
          <dd>{report.aligned_frames.toLocaleString("fr-FR")}</dd>
        </div>
      </dl>

      {#if report.error}
        <div class="error">Erreur : {report.error}</div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .null-test-panel {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 12px 14px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-m);
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
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
  .btn.primary {
    background: var(--accent);
    color: var(--bg-0);
    border-color: var(--accent);
  }
  .btn.primary:hover:not(:disabled) {
    filter: brightness(1.05);
    color: var(--bg-0);
  }
  .btn.ghost {
    background: transparent;
  }
  .spinner {
    width: 12px;
    height: 12px;
    border: 2px solid currentColor;
    border-right-color: transparent;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
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
  .badge.warn {
    color: #f59e0b;
    border-color: rgba(245, 158, 11, 0.35);
    background: rgba(245, 158, 11, 0.08);
  }
  .badge.idle {
    color: var(--fg-2);
  }
  .mode-badge {
    display: inline-flex;
    align-items: center;
    padding: 4px 10px;
    border-radius: var(--radius-s);
    font-size: 11px;
    color: var(--fg-2);
    background: var(--bg-3);
    border: 1px solid var(--border);
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
  .error {
    color: #f87171;
    font-size: 12px;
    padding: 8px 10px;
    background: rgba(248, 113, 113, 0.08);
    border: 1px solid rgba(248, 113, 113, 0.35);
    border-radius: var(--radius-s);
  }
  .result {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
</style>
