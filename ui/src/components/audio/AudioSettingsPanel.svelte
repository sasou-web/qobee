<script lang="ts">
  // Page Settings → Audio (R12).
  //
  // Single panel grouping every `audio.*` knob persisted by
  // `AudioSettingsStore`. Each section is two-way bound through the
  // `audioSettings` runes store: the controls read from the cached
  // snapshot and write through `updateAudio(key, value)` which
  // calls `setAudioSetting` and patches the local state on success.
  //
  // The Convolver, Bit-Perfect Health, and Null-Test sub-panels keep
  // owning their own state (file pickers, async runs, debounced
  // events) so we just embed them here.

  import { onMount } from "svelte";
  import { audioSettings, loadAudioSettings } from "../../lib/audioSettings.svelte";
  import BitPerfectHealthPanel from "./BitPerfectHealthPanel.svelte";
  import ConvolverIrPicker from "./ConvolverIrPicker.svelte";
  import NullTestDiagnostic from "./NullTestDiagnostic.svelte";

  let lastError = $state<string | null>(null);

  onMount(() => {
    void loadAudioSettings();
  });

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

  // ----- ReplayGain --------------------------------------------------------

  const rgHeadroomCommit = debounced((v) =>
    commit("audio.rg_safety_headroom_db", v)
  );

  // ----- Limiter -----------------------------------------------------------

  const ceilingCommit = debounced((v) =>
    commit("audio.peak_limiter_ceiling_dbfs", v)
  );
  const lookaheadCommit = debounced((v) =>
    commit("audio.peak_limiter_lookahead_ms", v)
  );

  // ----- Volume ------------------------------------------------------------

  const volumeFloorCommit = debounced((v) =>
    commit("audio.volume_floor_db", v)
  );

  // ----- Crossfeed ---------------------------------------------------------

  const crossfeedDelayCommit = debounced((v) =>
    commit("audio.crossfeed_delay_us", v)
  );
  const crossfeedCutoffCommit = debounced((v) =>
    commit("audio.crossfeed_lp_cutoff_hz", v)
  );

  // ----- Balance / Trim ----------------------------------------------------

  const balanceCommit = debounced((v) => commit("audio.balance", v));

  // The engine stores per-channel trim as a flat array. The L/R
  // sliders in this panel address index 0 and 1; we keep any
  // remaining channels untouched so multi-channel presets configured
  // elsewhere survive a stereo edit.
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

  <!-- ReplayGain ---------------------------------------------------------- -->
  <section>
    <h3>ReplayGain</h3>
    <div class="row">
      <label class="toggle">
        <input
          type="checkbox"
          checked={audioSettings.rg_peak_protection}
          onchange={(e) =>
            commit(
              "audio.rg_peak_protection",
              (e.target as HTMLInputElement).checked
            )}
        />
        <span>Protection des pics (true-peak aware)</span>
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
      Quand un titre est très chaud, ReplayGain peut demander un boost qui
      ferait dépasser 0 dBFS. La protection retire automatiquement assez de
      gain pour rester sous le plafond du limiteur, et la marge ajoute un
      coussin pour absorber les pics inter-échantillon reconstruits.
    </p>
  </section>

  <!-- Dither -------------------------------------------------------------- -->
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
            (e.target as HTMLSelectElement).value
          )}
      >
        <option value="tpdf">TPDF (bruit blanc)</option>
        <option value="shaped_hp">Passe-haut</option>
        <option value="shaped_f_weighted">F-weighted (recommandé)</option>
      </select>
    </div>
    <p class="hint">
      Actif uniquement pour les sorties ≤ 16 bits. Le profil shaped
      F-weighted déplace le bruit de quantification hors de la bande
      de sensibilité auditive maximale (2–5 kHz), ce qui améliore le
      SNR perçu sans augmenter l'énergie totale du bruit.
    </p>
  </section>

  <!-- Limiter ------------------------------------------------------------- -->
  <section>
    <h3>Limiteur</h3>
    <div class="row">
      <label for="limiter-mode">Mode</label>
      <select
        id="limiter-mode"
        value={audioSettings.peak_limiter_mode}
        onchange={(e) =>
          commit(
            "audio.peak_limiter_mode",
            (e.target as HTMLSelectElement).value
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
      Le limiteur look-ahead garantit qu'aucun échantillon ne dépasse
      le plafond, même quand l'EQ pousse fort. Plus le look-ahead est
      long, plus l'attaque peut être douce mais plus la latence du
      curseur augmente. Désactiver le limiteur est obligatoire pour
      atteindre le badge bit-perfect en mode Exclusive.
    </p>
  </section>

  <!-- Volume -------------------------------------------------------------- -->
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
            (e.target as HTMLSelectElement).value
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
      La courbe logarithmique répartit la résolution audiophile sur
      toute la course du curseur (un pas équivaut à un même ΔdB),
      avec une butée basse au plancher. Position 0 mute exactement,
      position 1 produit l'unité parfaite.
    </p>
  </section>

  <!-- Resampler ----------------------------------------------------------- -->
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
            (e.target as HTMLSelectElement).value
          )}
      >
        <option value="standard">Standard (CPU léger)</option>
        <option value="best">Best (recommandé)</option>
      </select>
    </div>
    <p class="hint">
      « Best » utilise un sinc long (256 taps, oversampling 256) avec
      un SNR ≥ 140 dB dans la bande audible. « Standard » descend à
      ≤ 128 taps pour soulager les CPU portables tout en restant
      au-dessus de 120 dB de SNR. Le préréglage s'applique à la piste
      suivante.
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
              (e.target as HTMLInputElement).checked
            )}
        />
        <span>Activé</span>
      </label>
    </div>
    <div class="row">
      <label for="crossfeed-preset">Préréglage</label>
      <select
        id="crossfeed-preset"
        value={audioSettings.crossfeed_preset}
        onchange={(e) =>
          commit(
            "audio.crossfeed_preset",
            (e.target as HTMLSelectElement).value
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
    <p class="hint">
      Mélange une fraction filtrée et retardée du canal opposé pour
      simuler une écoute en haut-parleurs. Particulièrement utile sur
      les enregistrements anciens à panoramique extrême. Bypass
      automatique pour les flux mono ou multicanaux.
    </p>
  </section>

  <!-- Convolver ----------------------------------------------------------- -->
  <section>
    <h3>Convolveur (réponse impulsionnelle)</h3>
    <ConvolverIrPicker />
    <p class="hint">
      Charge un fichier WAV mono ou stéréo (jusqu'à 100 000 taps). L'IR
      est rééchantillonnée au taux de l'appareil de sortie et un gain
      compensatoire évite l'écrêtage causé par la convolution. La
      latence reste sous 20 ms grâce à la convolution FFT partitionnée.
    </p>
  </section>

  <!-- Balance / Trim ------------------------------------------------------ -->
  <section>
    <h3>Balance / Trim</h3>
    <label class="slider">
      <span class="slider-label">
        Balance
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
    <label class="slider">
      <span class="slider-label">
        Trim canal gauche
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
        Trim canal droit
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
      Compense une asymétrie auditive ou un casque légèrement
      déséquilibré. La balance déplace l'image stéréo sans changer
      le niveau global ; le trim atténue chaque canal indépendamment.
      Aucun effet quand tous les paramètres sont à zéro.
    </p>
  </section>

  <!-- Bit-Perfect Health -------------------------------------------------- -->
  <section>
    <h3>Santé bit-perfect</h3>
    <BitPerfectHealthPanel />
    <p class="hint">
      Le badge passe au vert uniquement si la sortie est en Exclusive,
      tous les étages DSP sont à l'unité (limiteur off, EQ flat, RG sans
      atténuation, volume = 1.0, balance = 0, IR déchargée), et le SR
      du périphérique correspond au SR source.
    </p>
  </section>

  <!-- Null-Test ----------------------------------------------------------- -->
  <section>
    <h3>Diagnostic null-test</h3>
    <NullTestDiagnostic />
    <p class="hint">
      Vérifie le bit-perfect de bout en bout en comparant la sortie
      capturée au signal source par corrélation croisée. En Shared,
      la capture passe par WASAPI loopback ; en Exclusive, le
      moteur pré-rend la chaîne DSP dans un fichier 24 bits avant la
      comparaison.
    </p>
  </section>
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
</style>
