<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    addLibraryRoot,
    clearCoverCache,
    clearHistory,
    clearSettings,
    getSelectedOutputDevice,
    libraryStats,
    listLibraryRoots,
    listOutputDevices,
    removeLibraryRoot,
    resetLibrary,
    scanAllRoots,
    scanLibrary,
    setEqGains,
    setOutputDevice,
    getReplayGainMode,
    setReplayGainMode,
    type ReplayGainMode,
    getUserOutputMode,
    setOutputMode,
    type OutputMode,
    setVolume,
    type LibraryRoot,
    type LibraryStats,
    type OutputDevice,
  } from "../lib/api";
  import { settings, type Theme } from "../lib/settings.svelte";
  import { app } from "../lib/stores.svelte";
  import { discordPresence, type DiscordStatus } from "../lib/discordPresence";
  import { toasts } from "../lib/toasts.svelte";
  import SettingsAudio from "./audio/SettingsAudio.svelte";
  import SettingsWindowsIntegration from "./SettingsWindowsIntegration.svelte";

  let roots = $state<LibraryRoot[]>([]);
  let devices = $state<OutputDevice[]>([]);
  let selectedDeviceId = $state<string>("");
  let stats = $state<LibraryStats | null>(null);
  let replayGainMode = $state<ReplayGainMode>("off");
  let outputMode = $state<OutputMode>("auto");
  let outputModeError = $state<string | null>(null);
  let discordStatus = $state<DiscordStatus>("disabled");
  let discordPoll: number | null = null;

  // Defensive unstick at module load: a previous scan attempt that
  // missed its `scan-finished` event would otherwise leave every
  // scan-related button disabled forever. Settings is never opened
  // mid-scan as part of normal flow, so clearing here is safe.
  app.scanRunning = false;

  async function load(): Promise<void> {
    // Each sub-call is awaited independently so a single failure
    // doesn't blank out the whole settings page. Errors surface
    // through `app.lastError` instead.
    const results = await Promise.allSettled([
      listLibraryRoots(),
      listOutputDevices(),
      getSelectedOutputDevice(),
      libraryStats(),
      getReplayGainMode(),
      getUserOutputMode(),
    ]);
    const [rs, ds, sel, st, rg, om] = results;

    if (rs.status === "fulfilled") roots = rs.value;
    else app.lastError = `Library roots: ${String(rs.reason)}`;
    if (ds.status === "fulfilled") devices = ds.value;
    else app.lastError = `Output devices: ${String(ds.reason)}`;
    if (sel.status === "fulfilled") selectedDeviceId = sel.value ?? "";
    if (st.status === "fulfilled") stats = st.value;
    if (rg.status === "fulfilled") replayGainMode = rg.value;
    if (om.status === "fulfilled") outputMode = om.value;
  }

  $effect(() => {
    void load();
  });

  // Poll Discord status while Settings is open so the badge reflects
  // reconnect events live (Discord opening/closing in the
  // background). The interval is conservative: 1s is more than fast
  // enough for a status indicator and never approaches the IPC rate
  // limit because it doesn't poke Discord, only reads a Mutex on the
  // Rust side.
  $effect(() => {
    void discordPresence.getStatus().then((s) => (discordStatus = s));
    const id = window.setInterval(async () => {
      discordStatus = await discordPresence.getStatus();
    }, 1000);
    discordPoll = id;
    return () => {
      if (discordPoll !== null) {
        window.clearInterval(discordPoll);
        discordPoll = null;
      }
    };
  });

  async function pickFolder(): Promise<string | null> {
    try {
      const r = await open({ directory: true, multiple: false });
      return typeof r === "string" ? r : null;
    } catch {
      const typed = window.prompt("Folder path");
      return typed && typed.trim().length > 0 ? typed.trim() : null;
    }
  }

  async function handleAddRoot(): Promise<void> {
    const folder = await pickFolder();
    if (!folder) return;
    app.beginScan();
    try {
      await addLibraryRoot(folder);
      await scanLibrary(folder);
    } catch (e) {
      app.lastError = String(e);
    } finally {
      // Reset locally even if the `scan-finished` event was missed
      // (listener race, dropped event, backend short-circuit…). The
      // event handler in the store also clears it, so doing both is
      // safe.
      app.scanRunning = false;
      await load();
    }
  }

  async function handleRemoveRoot(id: number, path: string): Promise<void> {
    if (!window.confirm(`Stop indexing "${path}"? Tracks already in the library are kept.`)) return;
    try {
      await removeLibraryRoot(id);
      await load();
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function handleRescanAll(): Promise<void> {
    if (roots.length === 0) {
      app.lastError = "No library folder configured.";
      return;
    }
    app.beginScan();
    try {
      await scanAllRoots();
    } catch (e) {
      app.lastError = String(e);
    } finally {
      // Don't rely solely on the `scan-finished` event to unstick the
      // UI: if the event listener isn't attached yet or the event is
      // dropped, the button would stay disabled forever.
      app.scanRunning = false;
      await load();
    }
  }

  // ------- Audio settings -------

  async function handleReplayGainChange(e: Event): Promise<void> {
    const v = (e.target as HTMLSelectElement).value as ReplayGainMode;
    replayGainMode = v;
    try {
      await setReplayGainMode(v);
    } catch (err) {
      app.lastError = String(err);
    }
  }

  async function handleOutputModeChange(e: Event): Promise<void> {
    const v = (e.target as HTMLSelectElement).value as OutputMode;
    const previous = outputMode;
    outputMode = v;
    outputModeError = null;
    try {
      await setOutputMode(v);
    } catch (err) {
      // Reverting in the UI keeps the dropdown honest if the engine
      // refused (e.g. the device doesn't accept Exclusive).
      outputMode = previous;
      outputModeError = String(err);
      app.lastError = `Output mode: ${String(err)}`;
    }
  }

  async function handleDeviceChange(e: Event): Promise<void> {
    const v = (e.target as HTMLSelectElement).value;
    selectedDeviceId = v;
    try {
      await setOutputDevice(v === "" ? null : v);
      await settings.set("outputDeviceId", v === "" ? null : v);
    } catch (err) {
      app.lastError = String(err);
    }
  }

  async function handleDefaultVolume(e: Event): Promise<void> {
    const v = Number((e.target as HTMLInputElement).value) / 100;
    await settings.set("defaultVolume", v);
    try {
      await setVolume(v);
    } catch {
      // not blocking
    }
  }

  // ------- Appearance -------

  async function handleTheme(e: Event): Promise<void> {
    const v = (e.target as HTMLSelectElement).value as Theme;
    await settings.set("theme", v);
  }

  async function handleAccent(e: Event): Promise<void> {
    const v = (e.target as HTMLInputElement).value;
    await settings.set("accent", v);
  }

  // ------- Equalizer -------

  const EQ_FREQS = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
  const EQ_PRESETS: Record<string, number[]> = {
    Flat: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    "Bass boost": [6, 5, 4, 2, 0, 0, 0, 0, 0, 0],
    "Treble boost": [0, 0, 0, 0, 0, 1, 3, 4, 5, 6],
    "V-shape": [5, 4, 2, 0, -2, -2, 0, 2, 4, 5],
    Vocal: [-2, -2, -1, 1, 3, 4, 4, 3, 1, 0],
    "Loudness (low vol)": [5, 4, 2, 0, 0, 0, 1, 3, 4, 4],
    Soft: [-1, -1, -1, 0, 0, 0, 0, -1, -2, -2],
  };

  async function pushEqToEngine(): Promise<void> {
    try {
      const gains = settings.values.eqEnabled
        ? settings.values.eqGains
        : [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
      await setEqGains(gains);
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function handleEqEnable(e: Event): Promise<void> {
    const v = (e.target as HTMLInputElement).checked;
    await settings.set("eqEnabled", v);
    await pushEqToEngine();
  }

  async function handleEqBand(idx: number, e: Event): Promise<void> {
    const v = Number((e.target as HTMLInputElement).value);
    const next = settings.values.eqGains.slice();
    next[idx] = v;
    await settings.set("eqGains", next);
    await pushEqToEngine();
  }

  async function applyPreset(name: string): Promise<void> {
    const gains = EQ_PRESETS[name];
    if (!gains) return;
    await settings.set("eqGains", gains);
    if (!settings.values.eqEnabled) {
      await settings.set("eqEnabled", true);
    }
    await pushEqToEngine();
  }

  function eqLabel(hz: number): string {
    return hz >= 1000 ? `${hz / 1000}k` : `${hz}`;
  }

  // ------- Maintenance -------

  async function handleClearHistory(): Promise<void> {
    if (!window.confirm("Clear listening history? This cannot be undone.")) return;
    try {
      await clearHistory();
      await load();
      await app.refreshHome();
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function handleClearCovers(): Promise<void> {
    if (!window.confirm("Delete the cover cache? Covers will be re-extracted on the next scan."))
      return;
    try {
      const freed = await clearCoverCache();
      await load();
      toasts.success(`Freed ${formatBytes(freed)}.`);
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function handleResetSettings(): Promise<void> {
    if (!window.confirm("Reset all preferences to defaults?")) return;
    try {
      await clearSettings();
      // Reload defaults into the store + reapply visuals.
      await settings.load();
    } catch (e) {
      app.lastError = String(e);
    }
  }

  async function handleResetLibrary(): Promise<void> {
    const msg =
      "Wipe ALL library data? This deletes tracks, albums, playlists, history, " +
      "library roots, and the cover cache. Audio files on disk are NOT touched.\n\n" +
      "This cannot be undone.";
    if (!window.confirm(msg)) return;
    if (!window.confirm("Really wipe everything?")) return;
    try {
      await resetLibrary(false);
      await load();
      await app.refreshAll();
    } catch (e) {
      app.lastError = String(e);
    }
  }

  function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }
</script>

<section class="settings">
  <h1>Settings</h1>

  <!-- Library -->
  <details class="card">
    <summary>
      <h2>Library</h2>
      <span class="caret">▾</span>
    </summary>
    <div class="card-body">
    <p class="hint">
      Folders Qobee indexes. Tracks are read-only — nothing is modified on disk.
    </p>

    {#if roots.length === 0}
      <p class="empty">No folder yet.</p>
    {:else}
      <ul class="roots">
        {#each roots as r (r.id)}
          <li>
            <span class="path" title={r.path}>{r.path}</span>
            <button class="ghost" onclick={() => handleRemoveRoot(r.id, r.path)}>Remove</button>
          </li>
        {/each}
      </ul>
    {/if}

    <div class="actions">
      <button class="primary" onclick={handleAddRoot} disabled={app.scanRunning}>
        + Add folder
      </button>
      <button class="secondary" onclick={handleRescanAll} disabled={app.scanRunning}>
        Rescan all
      </button>
    </div>

    {#if stats}
      <div class="stats">
        <div><span>Tracks</span><strong>{stats.track_count.toLocaleString()}</strong></div>
        <div><span>Albums</span><strong>{stats.album_count.toLocaleString()}</strong></div>
        <div><span>Artists</span><strong>{stats.artist_count.toLocaleString()}</strong></div>
        <div><span>Playlists</span><strong>{stats.playlist_count.toLocaleString()}</strong></div>
        <div><span>History entries</span><strong>{stats.history_count.toLocaleString()}</strong></div>
        <div><span>Database</span><strong>{formatBytes(stats.db_size_bytes)}</strong></div>
        <div><span>Cover cache</span><strong>{formatBytes(stats.covers_size_bytes)}</strong></div>
      </div>
    {/if}
    </div>
  </details>

  <!-- Playback -->
  <details class="card">
    <summary>
      <h2>Playback</h2>
      <span class="caret">▾</span>
    </summary>
    <div class="card-body">

    <div class="row">
      <label for="device">Output device</label>
      <select id="device" value={selectedDeviceId} onchange={handleDeviceChange}>
        <option value="">System default</option>
        {#each devices as d (d.id)}
          <option value={d.id}>
            {d.is_default ? "★ " : ""}{d.name}
          </option>
        {/each}
      </select>
    </div>

    <div class="row">
      <label for="volume">Default volume</label>
      <input
        id="volume"
        type="range"
        min="0"
        max="100"
        step="1"
        value={settings.values.defaultVolume * 100}
        oninput={handleDefaultVolume}
      />
      <span class="value">{Math.round(settings.values.defaultVolume * 100)}%</span>
    </div>

    <div class="row">
      <label for="replaygain">ReplayGain</label>
      <select
        id="replaygain"
        value={replayGainMode}
        onchange={handleReplayGainChange}
      >
        <option value="off">Off</option>
        <option value="track">Track gain</option>
        <option value="album">Album gain</option>
      </select>
    </div>

    <p class="hint subtle">
      ReplayGain normalizes loudness using tags written by tools like foobar2000
      or rsgain. Track mode evens out a shuffled queue; album mode preserves the
      relative dynamics inside an album. Files without RG tags are played at
      their original level.
    </p>

    <div class="row">
      <label for="output-mode">Output mode</label>
      <select
        id="output-mode"
        value={outputMode}
        onchange={handleOutputModeChange}
      >
        <option value="auto">Auto (Shared)</option>
        <option value="shared">Shared (OS mixer)</option>
        <option value="exclusive">Exclusive (bit-perfect)</option>
      </select>
    </div>

    <p class="hint subtle">
      Shared lets other apps play to the same device through the Windows
      mixer; the engine is lossless to the mixer but never strictly
      bit-perfect. Exclusive bypasses the mixer entirely and reaches a
      true bit-perfect chain when the device natively supports the
      file's sample rate. While Exclusive is active no other app can
      play to this device, and Windows notifications go silent. Pause
      or stop Qobee to release the device.
    </p>

    {#if outputModeError}
      <p class="hint subtle" style="color: var(--danger);">
        Could not switch: {outputModeError}
      </p>
    {/if}
    </div>
  </details>

  <!-- Audio (R5 — Bit-Perfect Health & friends) -->
  <details class="card">
    <summary>
      <h2>Audio</h2>
      <span class="caret">▾</span>
    </summary>
    <div class="card-body">
    <SettingsAudio />
    </div>
  </details>

  <!-- Windows Integration -->
  <details class="card">
    <summary>
      <h2>Windows Integration</h2>
      <span class="caret">▾</span>
    </summary>
    <div class="card-body">
      <SettingsWindowsIntegration />
    </div>
  </details>

  <!-- Appearance -->
  <details class="card">
    <summary>
      <h2>Appearance</h2>
      <span class="caret">▾</span>
    </summary>
    <div class="card-body">

    <div class="row">
      <label for="theme">Theme</label>
      <select id="theme" value={settings.values.theme} onchange={handleTheme}>
        <option value="system">Follow system</option>
        <option value="dark">Dark</option>
        <option value="light">Light</option>
      </select>
    </div>

    <div class="row">
      <label for="accent">Accent color</label>
      <input
        id="accent"
        type="color"
        value={settings.values.accent}
        oninput={handleAccent}
      />
      <span class="value">{settings.values.accent}</span>
    </div>

    <div class="row">
      <label for="accent-follow">Follow cover</label>
      <label class="eq-toggle" for="accent-follow">
        <input
          id="accent-follow"
          type="checkbox"
          checked={settings.values.accentFollowsCover}
          onchange={(e) =>
            settings.set("accentFollowsCover", (e.target as HTMLInputElement).checked)}
        />
        <span>Tint the UI from the playing cover</span>
      </label>
      <span></span>
    </div>
    </div>
  </details>

  <!-- Equalizer -->
  <details class="card">
    <summary>
      <h2>Equalizer</h2>
      <span class="caret">▾</span>
    </summary>
    <div class="card-body">
    <div class="eq-head">
      <label class="eq-toggle">
        <input
          type="checkbox"
          checked={settings.values.eqEnabled}
          onchange={handleEqEnable}
        />
        <span>Enabled</span>
      </label>
    </div>

    <div class="eq-presets">
      {#each Object.keys(EQ_PRESETS) as name (name)}
        <button onclick={() => applyPreset(name)}>{name}</button>
      {/each}
    </div>

    <div class="eq" class:disabled={!settings.values.eqEnabled}>
      {#each EQ_FREQS as hz, i (hz)}
        <div class="band">
          <div class="db">
            {settings.values.eqGains[i]?.toFixed(0) ?? 0} dB
          </div>
          <input
            type="range"
            min="-12"
            max="12"
            step="0.5"
            value={settings.values.eqGains[i] ?? 0}
            oninput={(e) => handleEqBand(i, e)}
            class="vert"
          />
          <div class="hz">{eqLabel(hz)}</div>
        </div>
      {/each}
    </div>
    <p class="hint subtle">
      Peaking biquad filters at ISO octave centers. Q=1.0. Range ±12 dB. Bypass
      mode kicks in automatically when every band is at 0 dB.
    </p>
    </div>
  </details>

  <!-- Integrations -->
  <details class="card">
    <summary>
      <h2>Integrations</h2>
      <span class="caret">▾</span>
    </summary>
    <div class="card-body">
    <p class="hint">
      Show what you're listening to elsewhere. Each integration falls back
      silently when its target is unavailable.
    </p>

    <div class="row">
      <label for="discord-rpc">Discord Rich Presence</label>
      <label class="eq-toggle" for="discord-rpc">
        <input
          id="discord-rpc"
          type="checkbox"
          checked={settings.values.discordRichPresence}
          onchange={(e) =>
            settings.set(
              "discordRichPresence",
              (e.target as HTMLInputElement).checked
            )}
        />
        <span>Display the current track on your Discord profile</span>
      </label>
      <span class="discord-status" data-status={discordStatus}>
        {#if discordStatus === "connected"}
          ● Connected
        {:else if discordStatus === "connecting"}
          ● Connecting…
        {:else if discordStatus === "no_client_id"}
          ● No Application ID
        {:else}
          ● Off
        {/if}
      </span>
    </div>

    <div class="row">
      <label for="discord-client-id">Application ID</label>
      <input
        id="discord-client-id"
        type="text"
        inputmode="numeric"
        pattern="[0-9]*"
        placeholder="Leave empty to use the public Qobee app"
        value={settings.values.discordClientId}
        onchange={(e) =>
          settings.set(
            "discordClientId",
            (e.target as HTMLInputElement).value.trim()
          )}
        spellcheck="false"
      />
      <span></span>
    </div>

    <p class="hint subtle">
      Discord requires every Rich Presence to be backed by a registered
      application. Create one at
      <a
        href="https://discord.com/developers/applications"
        target="_blank"
        rel="noreferrer noopener"
        class="discord-link"
      >discord.com/developers/applications</a>
      , copy its <em>Application ID</em> here, and (optionally) upload Rich
      Presence assets named <code>qobee</code>, <code>playing</code>, and
      <code>paused</code> for the cover and play/pause icons. Cover art is
      replaced by the Qobee logo unless the file is reachable from a
      public URL.
    </p>
    <p class="hint subtle">
      Reconnects automatically when Discord starts or restarts. Nothing is
      shown when Discord is closed.
    </p>

    <div class="row">
      <label for="discord-cover-upload">Show local covers</label>
      <label class="eq-toggle" for="discord-cover-upload">
        <input
          id="discord-cover-upload"
          type="checkbox"
          checked={settings.values.discordCoverUpload}
          onchange={(e) =>
            settings.set(
              "discordCoverUpload",
              (e.target as HTMLInputElement).checked
            )}
        />
        <span>Upload album art so Discord can display it</span>
      </label>
      <span></span>
    </div>
    <p class="hint subtle privacy">
      <strong>Privacy:</strong> Discord cannot read files on your
      machine. To display the actual album cover (instead of the
      Qobee logo), Qobee uploads the image to
      <a
        href="https://litterbox.catbox.moe/"
        target="_blank"
        rel="noreferrer noopener"
        class="discord-link"
      >litterbox.catbox.moe</a>,
      a public anonymous file host, and stores the resulting URL.
      Each unique cover is uploaded only once. <strong>This is off by
      default.</strong> When off, the default Qobee art is used and
      no image bytes leave your machine.
    </p>
    </div>
  </details>

  <!-- Advanced -->
  <details class="card">
    <summary>
      <h2>Advanced</h2>
      <span class="caret">▾</span>
    </summary>
    <div class="card-body">
    {#if stats}
      <div class="paths">
        <div><span>Data folder</span><code title={stats.data_dir}>{stats.data_dir}</code></div>
        <div><span>Covers folder</span><code title={stats.covers_dir}>{stats.covers_dir}</code></div>
      </div>
    {/if}

    <div class="actions">
      <button onclick={handleClearHistory}>Clear listening history</button>
      <button onclick={handleClearCovers}>Clear cover cache</button>
      <button onclick={handleResetSettings}>Reset preferences</button>
      <button class="danger" onclick={handleResetLibrary}>Wipe all library data…</button>
    </div>
    </div>
  </details>
</section>

<style>
  .settings {
    display: flex;
    flex-direction: column;
    gap: 18px;
    max-width: 820px;
  }
  h1 {
    font-size: 24px;
    font-weight: 700;
    letter-spacing: -0.01em;
    margin: 4px 0 var(--space-2);
  }
  h2 {
    font-size: 14px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    margin: 0;
    color: var(--fg-1);
  }
  /* Each card is a <details> with the title + caret in <summary>.
     Closed by default — clicking the summary toggles the body open
     so the page never feels overwhelming on first visit. */
  .card {
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-m);
    overflow: hidden;
  }
  .card > summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    list-style: none;
    cursor: pointer;
    padding: 14px 18px;
    user-select: none;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .card > summary::-webkit-details-marker {
    display: none;
  }
  .card > summary:hover {
    background: var(--bg-2);
  }
  .card[open] > summary {
    border-bottom: 1px solid var(--border);
  }
  .card[open] > summary h2 {
    color: var(--fg-0);
  }
  .card .caret {
    color: var(--fg-2);
    font-size: 12px;
    transition: transform 0.2s var(--ease-out);
  }
  .card[open] > summary .caret {
    transform: rotate(180deg);
    color: var(--accent);
  }
  .card-body {
    padding: 16px 18px;
  }
  .hint {
    color: var(--fg-2);
    font-size: 12px;
    margin: 0 0 10px;
  }
  .hint.subtle {
    color: var(--fg-3);
    margin-top: 8px;
  }
  .empty {
    color: var(--fg-2);
    font-style: italic;
  }
  .roots {
    list-style: none;
    margin: 0 0 10px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .roots li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    background: var(--bg-2);
    border-radius: var(--radius-s);
  }
  .roots .path {
    flex: 1;
    font-family: ui-monospace, monospace;
    font-size: 12px;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 8px;
  }
  .primary {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }
  .primary:hover:not(:disabled) {
    filter: brightness(1.1);
  }
  .secondary {
    background: var(--bg-2);
    color: var(--fg-0);
    border: 1px solid var(--border);
  }
  .secondary:hover:not(:disabled) {
    background: var(--bg-3);
  }
  .danger {
    background: var(--danger);
    color: #fff;
    border-color: var(--danger);
  }
  .danger:hover {
    filter: brightness(1.05);
  }
  .ghost {
    background: transparent;
    color: var(--fg-2);
  }
  .ghost:hover {
    color: var(--fg-0);
    background: var(--bg-3);
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 8px 14px;
    margin-top: 14px;
    padding-top: 14px;
    border-top: 1px solid var(--border);
  }
  .stats div {
    display: flex;
    flex-direction: column;
  }
  .stats span {
    color: var(--fg-2);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .stats strong {
    color: var(--fg-0);
    font-variant-numeric: tabular-nums;
  }
  .row {
    display: grid;
    grid-template-columns: 160px 1fr auto;
    align-items: center;
    gap: 12px;
    margin: 10px 0;
  }
  .row label {
    color: var(--fg-1);
    font-size: 13px;
  }
  .row select,
  .row input[type="color"],
  .row input[type="text"] {
    background: var(--bg-2);
    color: var(--fg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-s);
    padding: 4px 8px;
    width: 100%;
    cursor: pointer;
  }
  .row input[type="text"] {
    cursor: text;
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }
  .row input[type="color"] {
    height: 30px;
    padding: 2px;
    width: 60px;
  }
  .discord-status {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-variant-numeric: tabular-nums;
    color: var(--fg-2);
    text-align: right;
    min-width: 130px;
  }
  .discord-status[data-status="connected"] {
    color: #4ade80;
  }
  .discord-status[data-status="connecting"] {
    color: var(--accent);
  }
  .discord-status[data-status="no_client_id"] {
    color: var(--danger);
  }
  .discord-link {
    color: var(--accent);
    text-decoration: none;
  }
  .discord-link:hover {
    text-decoration: underline;
  }
  .hint.privacy {
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-s);
    padding: 10px 12px;
    margin-top: 10px;
    line-height: 1.5;
  }
  .row .value {
    color: var(--fg-2);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    min-width: 60px;
    text-align: right;
  }
  .paths {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 12px;
  }
  .paths div {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .paths span {
    color: var(--fg-2);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .paths code {
    font-family: ui-monospace, monospace;
    font-size: 12px;
    color: var(--fg-0);
    background: var(--bg-2);
    padding: 4px 8px;
    border-radius: var(--radius-s);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .eq-head {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    margin-bottom: 12px;
  }
  .eq-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--fg-1);
    cursor: pointer;
  }
  .eq-presets {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 14px;
  }
  .eq-presets button {
    background: var(--bg-2);
    color: var(--fg-1);
    border: 1px solid var(--border);
    padding: 4px 10px;
    font-size: 12px;
    border-radius: 999px;
    cursor: pointer;
  }
  .eq-presets button:hover {
    background: var(--bg-3);
    color: var(--fg-0);
  }
  .eq {
    display: grid;
    grid-template-columns: repeat(10, 1fr);
    gap: 8px;
    padding: 10px;
    background: var(--bg-2);
    border-radius: var(--radius-m);
    align-items: end;
  }
  .eq.disabled {
    opacity: 0.5;
    pointer-events: none;
  }
  .band {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
  }
  .band .db {
    color: var(--fg-2);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .band .hz {
    color: var(--fg-1);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .vert {
    appearance: none;
    -webkit-appearance: slider-vertical;
    writing-mode: vertical-lr;
    direction: rtl;
    width: 18px;
    height: 130px;
    background: transparent;
  }
</style>
