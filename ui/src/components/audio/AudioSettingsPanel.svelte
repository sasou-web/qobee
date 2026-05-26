<script lang="ts">
  // Page Settings → Audio (R12).
  //
  // Two-tier UI on top of the same `AudioSettingsStore` backend:
  //
  //   • Mode "Standard" (par défaut) : seulement les contrôles
  //     directement utiles à un utilisateur lambda — santé
  //     bit-perfect, limiteur on/off, crossfeed casque, convolveur,
  //     balance, et un bouton "Réinitialiser".
  //
  //   • Mode "Avancé" (déroulé sur demande, persisté en
  //     localStorage) : expose les knobs experts laissés par la
  //     spec (headroom RG, profil de dither, plafond et look-ahead
  //     du limiteur, courbe et plancher de volume, qualité du
  //     rééchantillonneur, crossfeed personnalisé, gain
  //     compensatoire du convolveur, trim L/R, null-test).
  //
  // Aucun changement côté Rust : tous les défauts déjà persistés
  // restent valides et continuent d'alimenter le moteur. Cacher
  // un contrôle ne le réinitialise pas.

  import { onMount } from "svelte";
  import {
    audioSettings,
    loadAudioSettings,
  } from "../../lib/audioSettings.svelte";
  import { resetAudioSettings } from "../../lib/api";
  import BitPerfectHealthPanel from "./BitPerfectHealthPanel.svelte";
  import ConvolverIrPicker from "./ConvolverIrPicker.svelte";
  import NullTestDiagnostic from "./NullTestDiagnostic.svelte";

  const ADVANCED_KEY = "qobee.audio.showAdvanced";

  let lastError = $state<string | null>(null);
  let resetting = $state(false);
  let showAdvanced = $state(false);

  onMount(() => {
    void loadAudioSettings();
    try {
      showAdvanced = localStorage.getItem(ADVANCED_KEY) === "1";
    } catch {
      /* localStorage indisponible : on reste sur Standard. */
    }
  });

  function toggleAdvanced() {
    showAdvanced = !showAdvanced;
    try {
      localStorage.setItem(ADVANCED_KEY, showAdvanced ? "1" : "0");
    } catch {
      /* idem : best-effort */
    }
  }

  /** Push a setting and surface engine errors at the top of the panel. */
  async function commit(key: string, value: unknown) {
    const err = await audioSettings.update(key, value);
    lastError = err;
  }

  // Debouncer factory for sliders so dragging doesn't flood the
  // engine with hundreds of `setAudioSetting` round-trips. 80 ms is
  // imperceptible but reliably coalesces a typical drag burst.
  function debounced(fn: (v: number) => void, delay = 80) {
    let timer: ReturnType<typeof setTimeout> | null = null;
    return (v: number) => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => fn(v), delay);
    };
  }

  // --- Limiter on/off shortcut (Standard) ---------------------------------
  //
  // The store accepts three modes (off / soft_clip / lookahead_limiter).
  // For the Standard view we expose a single toggle: ON maps to
  // lookahead_limiter (the recommended default), OFF maps to "off".
  // Users who want soft_clip flip into Advanced.
  let limiterOn = $derived(audioSettings.peak_limiter_mode !== "off");

  async function onToggleLimiter(e: Event) {
    const next = (e.target as HTMLInputElement).checked
      ? "lookahead_limiter"
      : "off";
    await commit("audio.peak_limiter_mode", next);
  }

  async function onResetAll() {
    if (
      !window.confirm(
        "Restaurer tous les paramètres audio aux valeurs par défaut ?",
      )
    ) {
      return;
    }
    resetting = true;
    try {
      await resetAudioSettings();
      await loadAudioSettings(true);
      lastError = null;
    } catch (e) {
      lastError = String(e);
    } finally {
      resetting = false;
    }
  }

  // ----- Sliders (Advanced) -----------------------------------------------

  const rgHeadroomCommit = debounced((v) =>
    commit("audio.rg_safety_headroom_db", v),
  );
  const ceilingCommit = debounced((v) =>
    commit("audio.peak_limiter_ceiling_dbfs", v),
  );
  const lookaheadCommit = debounced((v) =>
    commit("audio.peak_limiter_lookahead_ms", v),
  );
  const volumeFloorCommit = debounced((v) =>
    commit("audio.volume_floor_db", v),
  );
  const crossfeedDelayCommit = debounced((v) =>
    commit("audio.crossfeed_delay_us", v),
  );
  const crossfeedCutoffCommit = debounced((v) =>
    commit("audio.crossfeed_lp_cutoff_hz", v),
  );
  const balanceCommit = debounced((v) => commit("audio.balance", v));

  // The engine stores per-channel trim as a flat array. The L/R
  // sliders address index 0 and 1; remaining channels (multicanal
  // presets) survive a stereo edit.
  function setTrimChannel(index: 0 | 1, value: number) {
    const current = audioSettings.trim_db_per_channel.slice();
    while (current.length <= index) current.push(0);
    current[index] = value;
    void commit("audio.trim_db_per_channel", current);
  }
  const leftTrimCommit = debounced((v) => setTrimChannel(0, v));
  const rightTrimCommit = debounced((v) => setTrimChannel(1, v));

  // Local mirrors — bound directly to the input elements. Whenever
  // the store changes (load, reset, refresh) `$effect` syncs them.
  let rgHeadroomLocal = $state(audioSettings.rg_safety_headroom_db);
  let ceilingLocal = $state(audioSettings.peak_limiter_ceiling_dbfs);
  let lookaheadLocal = $state(audioSettings.peak_limiter_lookahead_ms);
  let volumeFloorLocal = $state(audioSettings.volume_floor_db);
  let crossfeedDelayLocal = $state(audioSettings.crossfeed_delay_us);
  let crossfeedCutoffLocal = $state(audioSettings.crossfeed_lp_cutoff_hz);
  let balanceLocal = $state(audioSettings.balance);
  let leftTrimLocal = $state(audioSettings.trim_db_per_channel[0] ?? 0);
  let rightTrimLocal = $state(audioSettings.trim_db_per_channel[1] ?? 0);

  $effect(() => {
    rgHeadroomLocal = audioSettings.rg_safety_headroom_db;
    ceilingLocal = audioSettings.peak_limiter_ceiling_dbfs;
    lookaheadLocal = audioSettings.peak_limiter_lookahead_ms;
    volumeFloorLocal = audioSettings.volume_floor_db;
    crossfeedDelayLocal = audioSettings.crossfeed_delay_us;
    crossfeedCutoffLocal = audioSettings.crossfeed_lp_cutoff_hz;
    balanceLocal = audioSettings.balance;
    leftTrimLocal = audioSettings.trim_db_per_channel[0] ?? 0;
    rightTrimLocal = audioSettings.trim_db_per_channel[1] ?? 0;
  });
</script>

<div class="panel">
  {#if lastError}
    <div class="error">{lastError}</div>
  {/if}

  <!-- =============================================================== -->
  <!-- Standard — l'essentiel                                          -->
  <!-- =============================================================== -->

  <!-- Bit-Perfect Health -------------------------------------------------- -->
  <section>
    <h3>Santé bit-perfect</h3>
    <BitPerfectHealthPanel />
    <p class="hint">
      Le badge passe au vert quand la sortie atteint le bit-perfect
      strict (Exclusive, sans DSP, SR du périphérique = SR source).
    </p>
  </section>

  <!-- Limiteur ------------------------------------------------------------ -->
  <section>
    <h3>Limiteur de pics</h3>
    <div class="row">
      <label class="toggle">
        <input
          type="checkbox"
          checked={limiterOn}
          onchange={onToggleLimiter}
        />
        <span>Activé (recommandé)</span>
      </label>
    </div>
    <p class="hint">
      Empêche les pics de dépasser le 0 dBFS et protège vos enceintes.
      À désactiver uniquement pour viser le bit-perfect strict en mode
      Exclusive.
    </p>
  </section>

  <!-- Crossfeed ----------------------------------------------------------- -->
  <section>
    <h3>Crossfeed casque</h3>
    <div class="row">
      <label class="toggle">
        <input
          type="checkbox"
          checked={audioSettings.crossfeed_enabled}
          onchange={(e) =>
            commit(
              "audio.crossfeed_enabled",
              (e.target as HTMLInputElement).checked,
            )}
        />
        <span>Activé</span>
      </label>
    </div>
    <p class="hint">
      Adoucit l'image stéréo extrême pour une écoute au casque plus
      naturelle, en simulant le couplage des deux oreilles avec les
      enceintes. Bypass automatique pour le mono ou le multicanal.
    </p>
  </section>

  <!-- Convolveur ---------------------------------------------------------- -->
  <section>
    <h3>Convolveur (réponse impulsionnelle)</h3>
    <ConvolverIrPicker />
    <p class="hint">
      Charge un fichier WAV mono ou stéréo pour appliquer une
      correction de pièce ou un profil de casque. Optionnel.
    </p>
  </section>

  <!-- Balance ------------------------------------------------------------- -->
  <section>
    <h3>Balance</h3>
    <label class="slider">
      <span class="slider-label">
        Balance gauche / droite
        <em>
          {balanceLocal === 0
            ? "centré"
            : balanceLocal < 0
              ? `G ${Math.abs(balanceLocal * 100).toFixed(0)}%`
              : `D ${(balanceLocal * 100).toFixed(0)}%`}
        </em>
      </span>
      <input
        type="range"
        min="-1"
        max="1"
        step="0.01"
        bind:value={balanceLocal}
        oninput={() => balanceCommit(balanceLocal)}
      />
    </label>
  </section>

  <!-- Reset + Advanced toggle --------------------------------------------- -->
  <section class="actions">
    <button
      type="button"
      class="ghost"
      onclick={onResetAll}
      disabled={resetting}
    >
      {resetting ? "Réinitialisation…" : "Réinitialiser tous les réglages audio"}
    </button>
    <button type="button" class="link" onclick={toggleAdvanced}>
      {showAdvanced ? "Masquer" : "Afficher"} les réglages avancés
      <span class="caret" class:open={showAdvanced}>▾</span>
    </button>
  </section>

  <!-- =============================================================== -->
  <!-- Avancé — pour audiophiles                                       -->
  <!-- =============================================================== -->

  {#if showAdvanced}
    <!-- ReplayGain ------------------------------------------------------ -->
    <section>
      <h3>ReplayGain — protection des pics</h3>
      <div class="row">
        <label class="toggle">
          <input
            type="checkbox"
            checked={audioSettings.rg_peak_protection}
            onchange={(e) =>
              commit(
                "audio.rg_peak_protection",
                (e.target as HTMLInputElement).checked,
              )}
          />
          <span>Activée (true-peak aware)</span>
        </label>
      </div>
      <label class="slider">
        <span class="slider-label">
          Marge de sécurité
          <em>{rgHeadroomLocal.toFixed(1)} dB</em>
        </span>
        <input
          type="range"
          min="0"
          max="3"
          step="0.1"
          bind:value={rgHeadroomLocal}
          oninput={() => rgHeadroomCommit(rgHeadroomLocal)}
        />
      </label>
      <p class="hint">
        Quand ReplayGain demande un boost qui ferait dépasser 0 dBFS,
        la protection retire automatiquement assez de gain pour rester
        sous le plafond du limiteur. La marge ajoute un coussin pour
        absorber les pics inter-échantillon reconstruits.
      </p>
    </section>

    <!-- Dither --------------------------------------------------------- -->
    <section>
      <h3>Dither</h3>
      <div class="row">
        <label for="dither-profile">Profil</label>
        <select
          id="dither-profile"
          value={audioSettings.dither_profile}
          onchange={(e) =>
            commit(
              "audio.dither_profile",
              (e.target as HTMLSelectElement).value,
            )}
        >
          <option value="tpdf">TPDF (bruit blanc)</option>
          <option value="shaped_hp">Passe-haut</option>
          <option value="shaped_f_weighted">F-weighted (recommandé)</option>
        </select>
      </div>
      <p class="hint">
        Actif uniquement pour les sorties ≤ 16 bits. Le profil
        F-weighted déplace le bruit de quantification hors de la bande
        de sensibilité auditive maximale (2–5 kHz).
      </p>
    </section>

    <!-- Limiter (advanced) -------------------------------------------- -->
    <section>
      <h3>Limiteur — réglages fins</h3>
      <div class="row">
        <label for="limiter-mode">Mode</label>
        <select
          id="limiter-mode"
          value={audioSettings.peak_limiter_mode}
          onchange={(e) =>
            commit(
              "audio.peak_limiter_mode",
              (e.target as HTMLSelectElement).value,
            )}
        >
          <option value="off">Désactivé</option>
          <option value="soft_clip">Soft-clip (legacy)</option>
          <option value="lookahead_limiter">Look-ahead (recommandé)</option>
        </select>
      </div>
      <label class="slider">
        <span class="slider-label">
          Plafond
          <em>{ceilingLocal.toFixed(1)} dBFS</em>
        </span>
        <input
          type="range"
          min="-3"
          max="0"
          step="0.1"
          bind:value={ceilingLocal}
          oninput={() => ceilingCommit(ceilingLocal)}
        />
      </label>
      <label class="slider">
        <span class="slider-label">
          Look-ahead
          <em>{lookaheadLocal.toFixed(1)} ms</em>
        </span>
        <input
          type="range"
          min="2"
          max="10"
          step="0.1"
          bind:value={lookaheadLocal}
          oninput={() => lookaheadCommit(lookaheadLocal)}
        />
      </label>
      <p class="hint">
        Le look-ahead garantit qu'aucun échantillon ne dépasse le
        plafond, même quand l'EQ pousse fort. Un look-ahead plus long
        adoucit l'attaque mais ajoute de la latence au curseur.
      </p>
    </section>

    <!-- Volume -------------------------------------------------------- -->
    <section>
      <h3>Volume</h3>
      <div class="row">
        <label for="volume-curve">Courbe</label>
        <select
          id="volume-curve"
          value={audioSettings.volume_curve}
          onchange={(e) =>
            commit(
              "audio.volume_curve",
              (e.target as HTMLSelectElement).value,
            )}
        >
          <option value="logarithmic">Logarithmique (recommandée)</option>
          <option value="quadratic">Quadratique (legacy)</option>
        </select>
      </div>
      <label class="slider">
        <span class="slider-label">
          Plancher
          <em>{volumeFloorLocal.toFixed(0)} dB</em>
        </span>
        <input
          type="range"
          min="-80"
          max="-30"
          step="1"
          bind:value={volumeFloorLocal}
          oninput={() => volumeFloorCommit(volumeFloorLocal)}
        />
      </label>
      <p class="hint">
        La courbe logarithmique répartit la résolution sur toute la
        course du curseur (un pas équivaut à un même ΔdB). Position 0
        mute exactement, position 1 produit l'unité parfaite.
      </p>
    </section>

    <!-- Resampler ----------------------------------------------------- -->
    <section>
      <h3>Rééchantillonneur</h3>
      <div class="row">
        <label for="resampler-quality">Qualité</label>
        <select
          id="resampler-quality"
          value={audioSettings.resampler_quality}
          onchange={(e) =>
            commit(
              "audio.resampler_quality",
              (e.target as HTMLSelectElement).value,
            )}
        >
          <option value="standard">Standard (CPU léger)</option>
          <option value="best">Best (recommandé)</option>
        </select>
      </div>
      <p class="hint">
        « Best » utilise un sinc long (256 taps, SNR ≥ 140 dB).
        « Standard » descend à ≤ 128 taps pour soulager les CPU
        portables tout en restant au-dessus de 120 dB de SNR.
      </p>
    </section>

    <!-- Crossfeed (advanced) ------------------------------------------ -->
    <section>
      <h3>Crossfeed — réglages fins</h3>
      <div class="row">
        <label for="crossfeed-preset">Préréglage</label>
        <select
          id="crossfeed-preset"
          value={audioSettings.crossfeed_preset}
          onchange={(e) =>
            commit(
              "audio.crossfeed_preset",
              (e.target as HTMLSelectElement).value,
            )}
          disabled={!audioSettings.crossfeed_enabled}
        >
          <option value="bauer">Bauer (modéré)</option>
          <option value="bauer_strong">Bauer fort</option>
          <option value="custom">Personnalisé</option>
        </select>
      </div>
      {#if audioSettings.crossfeed_preset === "custom"}
        <label class="slider">
          <span class="slider-label">
            Délai inter-canal
            <em>{Math.round(crossfeedDelayLocal)} µs</em>
          </span>
          <input
            type="range"
            min="200"
            max="400"
            step="5"
            bind:value={crossfeedDelayLocal}
            oninput={() => crossfeedDelayCommit(crossfeedDelayLocal)}
            disabled={!audioSettings.crossfeed_enabled}
          />
        </label>
        <label class="slider">
          <span class="slider-label">
            Coupure passe-bas
            <em>{Math.round(crossfeedCutoffLocal)} Hz</em>
          </span>
          <input
            type="range"
            min="500"
            max="1500"
            step="10"
            bind:value={crossfeedCutoffLocal}
            oninput={() => crossfeedCutoffCommit(crossfeedCutoffLocal)}
            disabled={!audioSettings.crossfeed_enabled}
          />
        </label>
      {/if}
    </section>

    <!-- Trim L/R ------------------------------------------------------ -->
    <section>
      <h3>Trim par canal</h3>
      <label class="slider">
        <span class="slider-label">
          Canal gauche
          <em>{leftTrimLocal.toFixed(1)} dB</em>
        </span>
        <input
          type="range"
          min="-12"
          max="0"
          step="0.5"
          bind:value={leftTrimLocal}
          oninput={() => leftTrimCommit(leftTrimLocal)}
        />
      </label>
      <label class="slider">
        <span class="slider-label">
          Canal droit
          <em>{rightTrimLocal.toFixed(1)} dB</em>
        </span>
        <input
          type="range"
          min="-12"
          max="0"
          step="0.5"
          bind:value={rightTrimLocal}
          oninput={() => rightTrimCommit(rightTrimLocal)}
        />
      </label>
      <p class="hint">
        Compense un casque ou une asymétrie auditive en atténuant
        chaque canal indépendamment. Aucun effet quand les deux trims
        sont à zéro.
      </p>
    </section>

    <!-- Null-Test ----------------------------------------------------- -->
    <section>
      <h3>Diagnostic null-test</h3>
      <NullTestDiagnostic />
      <p class="hint">
        Vérifie le bit-perfect de bout en bout en comparant la sortie
        capturée au signal source par corrélation croisée.
      </p>
    </section>
  {/if}
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 16px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-m);
  }
  section.actions {
    background: transparent;
    border: none;
    padding: 0;
    flex-direction: row;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 12px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-1);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .row label[for] {
    flex: 0 0 160px;
    color: var(--fg-1);
    font-size: 13px;
  }
  .row select {
    flex: 1 1 200px;
    background: var(--bg-3);
    color: var(--fg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-s);
    padding: 4px 8px;
    cursor: pointer;
  }
  .row select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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
  .slider input[type="range"]:disabled {
    opacity: 0.5;
  }
  .hint {
    color: var(--fg-2);
    font-size: 12px;
    margin: 4px 0 0;
    line-height: 1.5;
  }
  .error {
    color: #f87171;
    font-size: 12px;
    padding: 8px 10px;
    background: rgba(248, 113, 113, 0.08);
    border: 1px solid rgba(248, 113, 113, 0.35);
    border-radius: var(--radius-s);
  }
  button.ghost {
    background: var(--bg-3);
    color: var(--fg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-s);
    padding: 6px 12px;
    font-size: 12px;
    cursor: pointer;
  }
  button.ghost:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
  button.ghost:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  button.link {
    background: transparent;
    border: none;
    color: var(--accent);
    font-size: 12px;
    cursor: pointer;
    padding: 4px 0;
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  button.link:hover {
    text-decoration: underline;
  }
  .caret {
    display: inline-block;
    transition: transform 0.15s ease;
  }
  .caret.open {
    transform: rotate(180deg);
  }
</style>
