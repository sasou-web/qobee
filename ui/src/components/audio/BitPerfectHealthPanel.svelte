<script lang="ts">
  // Bit-Perfect Health panel (R5).
  //
  // Subscribes to both `player:bit-perfect` (the engine debounces
  // updates to ~5 Hz) and `player:state` so the snapshot tracks the
  // active stream without round-tripping `getBitPerfectHealth` on
  // every tick. The initial snapshot is fetched on mount so the
  // panel renders correctly even when the user opens the settings
  // page while a track is already playing.
  import { onMount } from "svelte";
  import {
    getBitPerfectHealth,
    onBitPerfectChanged,
    onPlayerState,
    openWindowsSoundSettings,
    type BitPerfectHealth,
  } from "../../lib/api";
  import Icon from "../Icon.svelte";

  let health = $state<BitPerfectHealth | null>(null);
  let unsubA: (() => void) | null = null;
  let unsubB: (() => void) | null = null;

  onMount(() => {
    void getBitPerfectHealth().then((h) => {
      health = h;
    });
    void onBitPerfectChanged((h) => {
      health = h;
    }).then((u) => (unsubA = u));
    void onPlayerState((s) => {
      // PlayerState.bit_perfect is `Some(...)` once a device is
      // open. When `None`, the panel hides the source/track block
      // (R5.7) but keeps any device fields the engine still knows.
      health = s.bit_perfect ?? null;
    }).then((u) => (unsubB = u));
    return () => {
      unsubA?.();
      unsubB?.();
    };
  });

  let badgeClass = $derived(
    health
      ? health.status === "green"
        ? "ok"
        : health.status === "amber"
          ? "warn"
          : "err"
      : "idle"
  );

  let statusLabel = $derived.by(() => {
    if (!health) return "Aucun appareil ouvert";
    switch (health.status) {
      case "green":
        return "Bit-perfect";
      case "amber":
        return "Lossless";
      case "red":
        return "DSP actif";
    }
  });

  // Friendly one-liner explaining the badge state to a normal user.
  // The detailed engine messages stay accessible through the
  // "Détails techniques" disclosure below.
  let summaryLabel = $derived.by(() => {
    if (!health) return "";
    switch (health.status) {
      case "green":
        return "Le flux atteint la sortie sans aucune transformation.";
      case "amber":
        return "Le flux passe par le mixeur Windows mais reste lossless.";
      case "red":
        return "Le moteur applique une correction (limiteur, EQ, volume…) avant la sortie.";
    }
  });

  let hasResampleMismatch = $derived(
    !!health &&
      health.messages.some((m) => m.includes("Double rééchantillonnage caché"))
  );

  let showSourceBlock = $derived(
    !!health && health.source_sample_rate != null
  );

  // Technical message list collapsed by default — most users don't
  // need to read the engine's bullet point list to understand the
  // badge.
  let showDetails = $state(false);
</script>

<div class="bp-panel" data-status={health?.status ?? "idle"}>
  <header class="head">
    <span class="badge {badgeClass}" aria-live="polite">
      <Icon
        name={health?.status === "green"
          ? "shield"
          : health?.status === "amber"
            ? "music"
            : "zap"}
        size={14}
      />
      <span>{statusLabel}</span>
    </span>
    <span class="mode">
      {health?.effective_output_mode === "exclusive"
        ? "Exclusive"
        : health?.effective_output_mode === "shared"
          ? "Shared"
          : "—"}
    </span>
  </header>

  <dl class="grid">
    <div>
      <dt>Appareil</dt>
      <dd>
        {health?.device_sample_rate
          ? `${health.device_sample_rate} Hz`
          : "—"}
      </dd>
    </div>
    {#if showSourceBlock}
      <div>
        <dt>Source</dt>
        <dd>
          {health?.source_sample_rate} Hz
          {#if health?.source_bit_depth}
            · {health.source_bit_depth} bits
          {/if}
        </dd>
      </div>
    {/if}
  </dl>

  {#if health && summaryLabel}
    <p class="summary">{summaryLabel}</p>
  {/if}

  {#if health && health.messages.length > 0}
    <button
      class="details-toggle"
      type="button"
      onclick={() => (showDetails = !showDetails)}
    >
      {showDetails ? "Masquer" : "Afficher"} les détails techniques
      <span class="caret" class:open={showDetails}>▾</span>
    </button>
    {#if showDetails}
      <ul class="messages">
        {#each health.messages as msg, i (i)}
          <li>{msg}</li>
        {/each}
      </ul>
    {/if}
  {/if}

  {#if hasResampleMismatch}
    <button
      class="settings-btn"
      onclick={() => void openWindowsSoundSettings()}
    >
      Ouvrir les réglages son Windows
    </button>
  {/if}
</div>

<style>
  .bp-panel {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 14px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-m);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 999px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-weight: 600;
    border: 1px solid var(--border);
    background: var(--bg-3);
    color: var(--fg-1);
  }
  .badge.ok {
    color: #4ade80;
    border-color: rgba(74, 222, 128, 0.35);
    background: rgba(74, 222, 128, 0.08);
  }
  .badge.warn {
    color: #fbbf24;
    border-color: rgba(251, 191, 36, 0.35);
    background: rgba(251, 191, 36, 0.08);
  }
  .badge.err {
    color: #f87171;
    border-color: rgba(248, 113, 113, 0.35);
    background: rgba(248, 113, 113, 0.08);
  }
  .badge.idle {
    color: var(--fg-2);
  }
  .mode {
    font-size: 11px;
    color: var(--fg-2);
    text-transform: uppercase;
    letter-spacing: 0.06em;
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
  .messages {
    margin: 0;
    padding: 0 0 0 18px;
    color: var(--fg-1);
    font-size: 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .summary {
    margin: 0;
    color: var(--fg-1);
    font-size: 12px;
    line-height: 1.5;
  }
  .details-toggle {
    align-self: flex-start;
    background: transparent;
    border: none;
    color: var(--accent);
    font-size: 11px;
    cursor: pointer;
    padding: 0;
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .details-toggle:hover {
    text-decoration: underline;
  }
  .caret {
    display: inline-block;
    transition: transform 0.15s ease;
  }
  .caret.open {
    transform: rotate(180deg);
  }
  .settings-btn {
    align-self: flex-start;
    background: var(--bg-3);
    color: var(--fg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-s);
    padding: 6px 12px;
    font-size: 12px;
    cursor: pointer;
  }
  .settings-btn:hover {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
