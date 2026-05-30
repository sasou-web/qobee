<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    addLibraryRoot,
    clearCoverCache,
    clearHistory,
    clearSettings,
    clearUploadedCoverLinks,
    driveSync,
    getSelectedOutputDevice,
    getWindowSettings,
    libraryStats,
    listLibraryRoots,
    listLibrarySources,
    listOutputDevices,
    removeLibraryRoot,
    removeLibrarySource,
    resetLibrary,
    scanAllRoots,
    scanLibrary,
    setCloseBehavior,
    setEqGains,
    setNotifyOnTrackChange,
    setOutputDevice,
    setTrayEnabled,
    getReplayGainMode,
    setReplayGainMode,
    getCrossfadeMs,
    setCrossfadeMs,
    type CloseBehavior,
    type ReplayGainMode,
    getUserOutputMode,
    setOutputMode,
    type OutputMode,
    setVolume,
    type LibraryRoot,
    type LibrarySource,
    type LibraryStats,
    type OutputDevice,
    type WindowSettings,
  } from "../lib/api";
  import { formatGainDb, formatFrequency } from "../lib/format";
  import { settings, type Theme, BG_THEMES, type BgTheme } from "../lib/settings.svelte";
  import { app } from "../lib/stores.svelte";
  import { discordPresence, type DiscordStatus } from "../lib/discordPresence";
  import { toasts } from "../lib/toasts.svelte";
  import SettingsAudio from "./audio/SettingsAudio.svelte";
  import SettingsWindowsIntegration from "./SettingsWindowsIntegration.svelte";
  import ShortcutsHelp from "./ShortcutsHelp.svelte";
  import DriveWizard from "./DriveWizard.svelte";
  import Icon from "./Icon.svelte";
  import { shellOpen } from "../lib/api";
  import { GOOGLE_CLOUD_SETUP_DOC_URL } from "../lib/driveDocs";

  // Settings is laid out as a sidebar of categories + a content
  // pane (Spotify / Apple Music / Cider style). Each category
  // shows only the inputs that belong to it; long-form prose
  // explanations are gone. The nav itself is searchable so the
  // user can jump straight to a setting by name without scrolling
  // a 9-section accordion.
  type Category =
    | "library"
    | "playback"
    | "audio"
    | "windows"
    | "window"
    | "appearance"
    | "equalizer"
    | "integrations"
    | "shortcuts"
    | "advanced";

  interface CategoryDef {
    id: Category;
    label: string;
    icon:
      | "library"
      | "music"
      | "volume"
      | "shield"
      | "expand"
      | "settings"
      | "shuffle"
      | "quote"
      | "zap";
    /** Words used by the search filter, in addition to the label. */
    keywords: string;
  }

  const CATEGORIES: CategoryDef[] = [
    { id: "library", label: "Library", icon: "library", keywords: "folder root drive scan source remote google" },
    { id: "playback", label: "Playback", icon: "music", keywords: "device volume replaygain output exclusive shared crossfade fade" },
    { id: "audio", label: "Audio", icon: "volume", keywords: "bit perfect dsp convolver resampler null test" },
    { id: "windows", label: "Windows integration", icon: "shield", keywords: "smtc taskbar jumplist aumid" },
    { id: "window", label: "Window", icon: "expand", keywords: "close tray notify minimize background" },
    { id: "appearance", label: "Appearance", icon: "settings", keywords: "theme dark light accent color background follow cover" },
    { id: "equalizer", label: "Equalizer", icon: "shuffle", keywords: "eq band gain preset bass treble vocal flat" },
    { id: "integrations", label: "Integrations", icon: "zap", keywords: "discord rich presence cover upload" },
    { id: "shortcuts", label: "Keyboard shortcuts", icon: "quote", keywords: "raccourcis clavier keyboard shortcuts hotkeys touches accessibility" },
    { id: "advanced", label: "Advanced", icon: "settings", keywords: "reset clear cover history database wipe" },
  ];

  let activeCategory = $state<Category>("library");
  let navQuery = $state("");

  let filteredCategories = $derived(
    navQuery.trim() === ""
      ? CATEGORIES
      : CATEGORIES.filter((c) => {
          const q = navQuery.trim().toLowerCase();
          return (
            c.label.toLowerCase().includes(q) ||
            c.keywords.toLowerCase().includes(q)
          );
        }),
  );

  let roots = $state<LibraryRoot[]>([]);
  let sources = $state<LibrarySource[]>([]);
  let driveWizardOpen = $state(false);
  let devices = $state<OutputDevice[]>([]);
  let selectedDeviceId = $state<string>("");
  let stats = $state<LibraryStats | null>(null);
  let replayGainMode = $state<ReplayGainMode>("off");
  let outputMode = $state<OutputMode>("auto");
  let outputModeError = $state<string | null>(null);
  let crossfadeMs = $state<number>(0);
  let discordStatus = $state<DiscordStatus>("disabled");
  let discordPoll: number | null = null;

  // -------- Window_Manager preferences (R7 — task 10.1) --------
  // Three keys persisted in the `settings` SQLite table:
  // `windows.close_behavior`, `windows.tray_enabled`,
  // `windows.notify_on_track_change`. Loaded on mount; each
  // control persists through a dedicated Tauri command which
  // returns the resulting full snapshot so the UI never drifts
  // from the backend.
  let windowSettings = $state<WindowSettings | null>(null);
  let windowBusyKey = $state<string | null>(null);
  let windowError = $state<string | null>(null);

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
      listLibrarySources(),
      getWindowSettings(),
      getCrossfadeMs(),
    ]);
    const [rs, ds, sel, st, rg, om, ss, ws, xf] = results;

    if (rs.status === "fulfilled") roots = rs.value;
    else app.lastError = `Library roots: ${String(rs.reason)}`;
    if (ds.status === "fulfilled") devices = ds.value;
    else app.lastError = `Output devices: ${String(ds.reason)}`;
    if (sel.status === "fulfilled") selectedDeviceId = sel.value ?? "";
    if (st.status === "fulfilled") stats = st.value;
    if (rg.status === "fulfilled") replayGainMode = rg.value;
    if (om.status === "fulfilled") outputMode = om.value;
    if (ss.status === "fulfilled") sources = ss.value;
    if (ws.status === "fulfilled") windowSettings = ws.value;
    else windowError = String(ws.reason);
    if (xf.status === "fulfilled") crossfadeMs = xf.value;
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

  function handleAddDriveSource(): void {
    // The wizard handles every step (OAuth, token persistence,
    // about-the-account check). We just open it; on success it
    // dispatches a callback that re-loads the source list.
    driveWizardOpen = true;
  }

  /** Open `docs/google-cloud-setup.md` (FR walkthrough, R5.5) in
   *  the user's default browser via the `shell_open` Tauri
   *  command. Falls back to `window.open` if the command fails. */
  async function openDriveSetupDoc(event: MouseEvent): Promise<void> {
    event.preventDefault();
    try {
      await shellOpen(GOOGLE_CLOUD_SETUP_DOC_URL);
    } catch (e) {
      window.open(GOOGLE_CLOUD_SETUP_DOC_URL, "_blank", "noopener,noreferrer");
      // eslint-disable-next-line no-console
      console.warn("shell_open failed, used window.open fallback:", e);
    }
  }

  async function handleDriveWizardSuccess(): Promise<void> {
    sources = await listLibrarySources();
  }

  async function handleRemoveSource(id: number, name: string): Promise<void> {
    if (!window.confirm(`Remove source "${name}"?`)) return;
    try {
      await removeLibrarySource(id);
      sources = await listLibrarySources();
    } catch (e) {
      app.lastError = String(e);
    }
  }

  let syncingSourceId = $state<number | null>(null);
  async function handleSyncSource(id: number, name: string): Promise<void> {
    syncingSourceId = id;
    try {
      const r = await driveSync(id);
      window.alert(
        `Synced "${name}".\n` +
          `${r.pulled} favorites pulled from Drive, ${r.pushed} pushed back.`,
      );
    } catch (e) {
      app.lastError = `Sync ${name}: ${String(e)}`;
    } finally {
      syncingSourceId = null;
    }
  }

  async function handleRescanAll(): Promise<void> {
    if (roots.length === 0) {
      app.lastError = "No library folder configured.";
      return;
    }
    // R6.1: immediate feedback at trigger time. `app.beginScan()` sets
    // `app.scanRunning = true`, which disables the Rescan button
    // (`disabled={app.scanRunning}`) and guards against a concurrent
    // rescan (R6.4).
    toasts.info("Scan démarré…");
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

  // Crossfade is debounced so dragging the slider doesn't spam the
  // backend (each call persists a settings row + touches the engine).
  let crossfadeDebounce: ReturnType<typeof setTimeout> | null = null;
  function handleCrossfadeChange(e: Event): void {
    const ms = Number((e.target as HTMLInputElement).value);
    crossfadeMs = ms;
    if (crossfadeDebounce) clearTimeout(crossfadeDebounce);
    crossfadeDebounce = setTimeout(() => {
      void setCrossfadeMs(ms).catch((err) => (app.lastError = String(err)));
    }, 200);
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

  async function handleBgTheme(id: BgTheme): Promise<void> {
    await settings.set("bgTheme", id);
  }

  async function handleAccent(e: Event): Promise<void> {
    const v = (e.target as HTMLInputElement).value;
    await settings.set("accent", v);
  }

  // ------- Equalizer -------

  // Display-only frequency labels for the 10 EQ bands. The engine
  // uses ISO centers 31.25 / 62.5 for the lowest two; we round to
  // 32 / 64 here to match the labels users expect on commercial
  // equalizers (R4.4). The gain array index (0..=9) is what actually
  // routes to `qobee-engine::eq::FREQS_HZ`.
  const EQ_FREQS = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
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
  // Retained as a quick-revert label format. Unused by the EQ
  // template, which now uses `safeFreqLabel` (R4.4 / R4.5).
  void eqLabel;

  // Safe wrappers around the formatters so a transient runtime error
  // (e.g. NaN gain after a corrupted settings file) cannot blank out
  // the slider. The catch returns an empty string; the slider stays
  // fully interactive (R4.5).
  function safeGainLabel(v: unknown): string {
    try {
      const n = typeof v === "number" ? v : Number(v);
      if (!Number.isFinite(n)) return "";
      // Round to one decimal so half-steps display as "+1.5 dB"
      // while integer gains stay clean ("+3 dB").
      const rounded = Math.round(n * 10) / 10;
      return formatGainDb(rounded);
    } catch {
      return "";
    }
  }

  function safeFreqLabel(hz: unknown): string {
    try {
      const n = typeof hz === "number" ? hz : Number(hz);
      if (!Number.isFinite(n)) return "";
      return formatFrequency(n);
    } catch {
      return "";
    }
  }

  async function handleEqReset(): Promise<void> {
    const zeros = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    await settings.set("eqGains", zeros);
    try {
      await setEqGains(zeros);
    } catch (e) {
      app.lastError = String(e);
    }
  }

  // ------- Window_Manager (R7 — task 10.1) -------

  async function handleCloseBehaviorChange(e: Event): Promise<void> {
    if (!windowSettings) return;
    const v = (e.target as HTMLSelectElement).value as CloseBehavior;
    const previous = windowSettings;
    // Optimistic update for snappy UI; rollback on error.
    windowSettings = { ...previous, close_behavior: v };
    windowBusyKey = "close_behavior";
    windowError = null;
    try {
      windowSettings = await setCloseBehavior(v);
    } catch (err) {
      windowSettings = previous;
      windowError = String(err);
      app.lastError = `Window close behavior: ${String(err)}`;
    } finally {
      windowBusyKey = null;
    }
  }

  async function handleTrayEnabledChange(e: Event): Promise<void> {
    if (!windowSettings) return;
    const v = (e.target as HTMLInputElement).checked;
    const previous = windowSettings;
    windowSettings = { ...previous, tray_enabled: v };
    windowBusyKey = "tray_enabled";
    windowError = null;
    try {
      windowSettings = await setTrayEnabled(v);
    } catch (err) {
      windowSettings = previous;
      windowError = String(err);
      app.lastError = `Tray: ${String(err)}`;
    } finally {
      windowBusyKey = null;
    }
  }

  async function handleNotifyOnTrackChangeChange(e: Event): Promise<void> {
    if (!windowSettings) return;
    const v = (e.target as HTMLInputElement).checked;
    const previous = windowSettings;
    windowSettings = { ...previous, notify_on_track_change: v };
    windowBusyKey = "notify_on_track_change";
    windowError = null;
    try {
      windowSettings = await setNotifyOnTrackChange(v);
    } catch (err) {
      windowSettings = previous;
      windowError = String(err);
      app.lastError = `Track change notifications: ${String(err)}`;
    } finally {
      windowBusyKey = null;
    }
  }

  // ------- Integrations (R8 — task 23.1) -------

  // Cover_Upload privacy. The toggle is OFF by default (opt-in, R8.1):
  // `discordCoverUpload` defaults to `false` in the settings store, so
  // the first time a user configures Discord here the cover upload is
  // presented as an explicit, disabled option. Flipping it both
  // persists the preference (`integrations.discord_cover_upload`, R8.6)
  // and tells the backend cover-host worker to start/stop uploading
  // (R8.2/R8.3 — disabling stops new uploads).
  async function handleCoverUploadChange(e: Event): Promise<void> {
    const v = (e.target as HTMLInputElement).checked;
    await settings.set("discordCoverUpload", v);
    await discordPresence.setCoverUploadEnabled(v);
  }

  // Explicit purge of every cover link already uploaded for Discord
  // (R8.4). The backend deletes the `discord.cover_url::*` settings
  // rows, resets `discord.cover_url_count` to 0 and clears the in-memory
  // CoverHost cache, returning the number of links removed so we can
  // confirm it in a toast (R8.5).
  let clearingCoverLinks = $state(false);
  async function handleClearCoverLinks(): Promise<void> {
    clearingCoverLinks = true;
    try {
      const n = await clearUploadedCoverLinks();
      toasts.success(`${n} lien(s) supprimé(s)`);
    } catch (e) {
      app.lastError = `Clear cover links: ${String(e)}`;
    } finally {
      clearingCoverLinks = false;
    }
  }

  // ------- Maintenance -------

  async function handleClearHistory(): Promise<void> {    if (!window.confirm("Clear listening history? This cannot be undone.")) return;
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
  <aside class="nav">
    <div class="nav-search">
      <Icon name="search" size={14} />
      <input
        type="text"
        placeholder="Search settings…"
        bind:value={navQuery}
        spellcheck="false"
      />
    </div>
    <nav>
      {#each filteredCategories as c (c.id)}
        <button
          type="button"
          class="nav-item"
          class:active={activeCategory === c.id}
          onclick={() => (activeCategory = c.id)}
        >
          <Icon name={c.icon} size={16} />
          <span>{c.label}</span>
        </button>
      {/each}
      {#if filteredCategories.length === 0}
        <p class="nav-empty">No match.</p>
      {/if}
    </nav>
  </aside>

  <main class="pane">
    <h1>{CATEGORIES.find((c) => c.id === activeCategory)?.label ?? "Settings"}</h1>

    {#if activeCategory === "library"}
      <div class="block">
        <h3>Folders</h3>
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
            <Icon name="folder-plus" size={14} />
            <span>Add folder</span>
          </button>
          <button class="secondary" onclick={handleRescanAll} disabled={app.scanRunning}>
            Rescan all
          </button>
        </div>
      </div>

      <div class="block">
        <h3>Remote sources</h3>
        {#if sources.length === 0}
          <p class="empty">No remote source yet.</p>
        {:else}
          <ul class="roots">
            {#each sources as s (s.id)}
              <li>
                <span class="source-kind">{s.kind === "google_drive" ? "Drive" : s.kind}</span>
                <span class="path" title={s.name}>{s.name}</span>
                {#if !s.enabled}
                  <span class="badge subtle">disabled</span>
                {/if}
                {#if s.kind === "google_drive"}
                  <button
                    class="ghost"
                    onclick={() => handleSyncSource(s.id, s.name)}
                    disabled={syncingSourceId === s.id}
                    title="Two-way sync of favorites with the Drive folder"
                  >
                    {syncingSourceId === s.id ? "Syncing…" : "Sync"}
                  </button>
                {/if}
                <button class="ghost" onclick={() => handleRemoveSource(s.id, s.name)}>Remove</button>
              </li>
            {/each}
          </ul>
        {/if}
        <div class="actions">
          <button class="secondary" onclick={handleAddDriveSource}>
            <Icon name="plus" size={14} />
            <span>Connect Google Drive</span>
          </button>
          <a
            href={GOOGLE_CLOUD_SETUP_DOC_URL}
            target="_blank"
            rel="noopener noreferrer"
            class="link-doc"
            onclick={openDriveSetupDoc}
          >
            Setup guide
          </a>
        </div>
      </div>

      {#if stats}
        <div class="block">
          <h3>Stats</h3>
          <div class="stats">
            <div><span>Tracks</span><strong>{stats.track_count.toLocaleString()}</strong></div>
            <div><span>Albums</span><strong>{stats.album_count.toLocaleString()}</strong></div>
            <div><span>Artists</span><strong>{stats.artist_count.toLocaleString()}</strong></div>
            <div><span>Playlists</span><strong>{stats.playlist_count.toLocaleString()}</strong></div>
            <div><span>History</span><strong>{stats.history_count.toLocaleString()}</strong></div>
            <div><span>Database</span><strong>{formatBytes(stats.db_size_bytes)}</strong></div>
            <div><span>Cover cache</span><strong>{formatBytes(stats.covers_size_bytes)}</strong></div>
          </div>
        </div>
      {/if}
    {/if}

    {#if activeCategory === "playback"}
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

      <div class="row">
        <label for="crossfade">Crossfade</label>
        <input
          id="crossfade"
          type="range"
          min="0"
          max="12000"
          step="500"
          value={crossfadeMs}
          oninput={handleCrossfadeChange}
        />
        <span class="value">
          {crossfadeMs === 0 ? "Off" : `${(crossfadeMs / 1000).toFixed(1)} s`}
        </span>
      </div>

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

      {#if outputModeError}
        <p class="error-line">{outputModeError}</p>
      {/if}
    {/if}

    {#if activeCategory === "audio"}
      <SettingsAudio />
    {/if}

    {#if activeCategory === "windows"}
      <SettingsWindowsIntegration />
    {/if}

    {#if activeCategory === "window"}
      {#if !windowSettings}
        {#if windowError}
          <p class="error-line">{windowError}</p>
        {:else}
          <p class="empty">Loading…</p>
        {/if}
      {:else}
        {#if windowError}
          <p class="error-line">{windowError}</p>
        {/if}

        <div class="row">
          <label for="close-behavior">On close</label>
          <select
            id="close-behavior"
            value={windowSettings.close_behavior}
            disabled={windowBusyKey === "close_behavior"}
            onchange={handleCloseBehaviorChange}
          >
            <option value="quit">Quit Qobee</option>
            <option value="minimize_to_tray">Minimize to tray</option>
            <option value="keep_running_in_background">Keep in background</option>
          </select>
        </div>

        <div class="row toggle-row">
          <span>System tray icon</span>
          <label class="switch">
            <input
              type="checkbox"
              checked={windowSettings.tray_enabled}
              disabled={windowBusyKey === "tray_enabled"}
              onchange={handleTrayEnabledChange}
            />
            <span class="slider"></span>
          </label>
        </div>

        <div class="row toggle-row">
          <span>Track-change notifications</span>
          <label class="switch">
            <input
              type="checkbox"
              checked={windowSettings.notify_on_track_change}
              disabled={windowBusyKey === "notify_on_track_change"}
              onchange={handleNotifyOnTrackChangeChange}
            />
            <span class="slider"></span>
          </label>
        </div>
      {/if}
    {/if}

    {#if activeCategory === "appearance"}
      <div class="row">
        <label for="theme">Theme</label>
        <select id="theme" value={settings.values.theme} onchange={handleTheme}>
          <option value="system">Follow system</option>
          <option value="dark">Dark</option>
          <option value="light">Light</option>
        </select>
      </div>

      <div class="row bg-theme-row">
        <span>Background</span>
        <div class="bg-theme-list" role="radiogroup" aria-label="Background theme">
          {#each BG_THEMES as t (t.id)}
            <button
              type="button"
              class="bg-swatch"
              class:active={settings.values.bgTheme === t.id}
              style:background={t.sample}
              onclick={() => handleBgTheme(t.id)}
              title={t.label}
              role="radio"
              aria-checked={settings.values.bgTheme === t.id}
            >
              <span class="bg-swatch-label">{t.label}</span>
            </button>
          {/each}
        </div>
      </div>

      <div class="row">
        <label for="accent">Accent</label>
        <input
          id="accent"
          type="color"
          value={settings.values.accent}
          oninput={handleAccent}
        />
        <span class="value">{settings.values.accent}</span>
      </div>

      <div class="row toggle-row">
        <span>Follow cover</span>
        <label class="switch">
          <input
            type="checkbox"
            checked={settings.values.accentFollowsCover}
            onchange={(e) =>
              settings.set("accentFollowsCover", (e.target as HTMLInputElement).checked)}
          />
          <span class="slider"></span>
        </label>
      </div>
    {/if}

    {#if activeCategory === "equalizer"}
      <div class="row toggle-row">
        <span>Enabled</span>
        <label class="switch">
          <input
            type="checkbox"
            checked={settings.values.eqEnabled}
            onchange={handleEqEnable}
          />
          <span class="slider"></span>
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
              {safeGainLabel(settings.values.eqGains[i] ?? 0)}
            </div>
            <input
              type="range"
              min="-12"
              max="12"
              step="0.5"
              value={settings.values.eqGains[i] ?? 0}
              oninput={(e) => handleEqBand(i, e)}
              class="vert"
              aria-label={`${safeFreqLabel(hz) || `Band ${i + 1}`} gain`}
            />
            <div class="hz">{safeFreqLabel(hz)}</div>
          </div>
        {/each}
      </div>

      <div class="actions">
        <button class="secondary" onclick={handleEqReset}>Reset</button>
      </div>
    {/if}

    {#if activeCategory === "integrations"}
      <div class="row toggle-row">
        <span>
          Discord Rich Presence
          <small class="status" data-status={discordStatus}>
            {#if discordStatus === "connected"}
              Connected
            {:else if discordStatus === "connecting"}
              Connecting…
            {:else if discordStatus === "no_client_id"}
              No application ID
            {:else}
              Off
            {/if}
          </small>
        </span>
        <label class="switch">
          <input
            type="checkbox"
            checked={settings.values.discordRichPresence}
            onchange={(e) =>
              settings.set(
                "discordRichPresence",
                (e.target as HTMLInputElement).checked,
              )}
          />
          <span class="slider"></span>
        </label>
      </div>

      <div class="row">
        <label for="discord-client-id">Application ID</label>
        <input
          id="discord-client-id"
          type="text"
          inputmode="numeric"
          pattern="[0-9]*"
          placeholder="Public Qobee app"
          value={settings.values.discordClientId}
          onchange={(e) =>
            settings.set(
              "discordClientId",
              (e.target as HTMLInputElement).value.trim(),
            )}
          spellcheck="false"
        />
      </div>

      <div class="row toggle-row">
        <span>
          Upload covers for Discord presence
          <small class="cover-hint">Off by default — covers are uploaded to an external host only if you opt in.</small>
        </span>
        <label class="switch">
          <input
            type="checkbox"
            checked={settings.values.discordCoverUpload}
            onchange={handleCoverUploadChange}
          />
          <span class="slider"></span>
        </label>
      </div>

      <div class="block">
        <div class="actions">
          <button onclick={handleClearCoverLinks} disabled={clearingCoverLinks}>
            {clearingCoverLinks ? "Clearing…" : "Clear uploaded cover links"}
          </button>
        </div>
      </div>
    {/if}

    {#if activeCategory === "shortcuts"}
      <ShortcutsHelp />
    {/if}

    {#if activeCategory === "advanced"}
      {#if stats}
        <div class="block">
          <h3>Paths</h3>
          <div class="paths">
            <div><span>Data folder</span><code title={stats.data_dir}>{stats.data_dir}</code></div>
            <div><span>Covers folder</span><code title={stats.covers_dir}>{stats.covers_dir}</code></div>
          </div>
        </div>
      {/if}

      <div class="block">
        <h3>Maintenance</h3>
        <div class="actions vertical">
          <button onclick={handleClearHistory}>Clear listening history</button>
          <button onclick={handleClearCovers}>Clear cover cache</button>
          <button onclick={handleResetSettings}>Reset preferences</button>
          <button class="danger" onclick={handleResetLibrary}>Wipe all library data…</button>
        </div>
      </div>
    {/if}
  </main>
</section>

<DriveWizard
  open={driveWizardOpen}
  onclose={() => (driveWizardOpen = false)}
  onsuccess={() => {
    void handleDriveWizardSuccess();
  }}
/>

<style>
  /* ============================================================
     Two-column layout: nav (left) + content pane (right). Mirrors
     the Cider / Spotify / iTunes preferences pattern. The nav is
     a simple list with a search field on top; the pane scrolls
     independently when the active section is long (Equalizer,
     Library stats…).
     ============================================================ */
  .settings {
    display: grid;
    grid-template-columns: 220px 1fr;
    gap: var(--space-6);
    max-width: 980px;
    align-items: start;
  }

  .nav {
    position: sticky;
    top: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: 4px 0 var(--space-2);
  }

  .nav-search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 10px;
    height: 32px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    color: var(--fg-2);
    transition: border-color var(--dur-fast) var(--ease-out),
      background var(--dur-fast) var(--ease-out);
  }
  .nav-search:focus-within {
    border-color: var(--accent-soft);
    background: var(--bg-1);
    color: var(--fg-1);
  }
  .nav-search input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    color: var(--fg-0);
    font-size: 12px;
    font-family: inherit;
  }
  .nav-search input::placeholder {
    color: var(--fg-3, var(--fg-2));
  }

  .nav nav {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--fg-1);
    cursor: pointer;
    padding: 7px 10px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 500;
    transition:
      background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .nav-item:hover {
    background: var(--bg-2);
    color: var(--fg-0);
  }
  .nav-item.active {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .nav-item :global(svg) {
    flex-shrink: 0;
    opacity: 0.85;
  }
  .nav-item.active :global(svg) {
    opacity: 1;
  }
  .nav-empty {
    color: var(--fg-2);
    font-size: 12px;
    padding: 8px 10px;
    margin: 0;
  }

  /* ------------------------------------------------------------
     Content pane.
     ------------------------------------------------------------ */
  .pane {
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
    min-width: 0;
  }
  .pane h1 {
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.01em;
    margin: 0 0 var(--space-2);
    color: var(--fg-0);
  }
  .pane h3 {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-2);
    margin: 0 0 var(--space-3);
  }

  .block {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding-bottom: var(--space-4);
    border-bottom: 1px solid var(--border);
  }
  .block:last-child {
    border-bottom: none;
    padding-bottom: 0;
  }

  /* Plain key/value row: label on the left, control on the right. */
  .row {
    display: grid;
    grid-template-columns: 160px 1fr auto;
    align-items: center;
    gap: 12px;
    margin: 0;
  }
  .row label {
    color: var(--fg-1);
    font-size: 13px;
    font-weight: 500;
  }
  .row select,
  .row input[type="text"],
  .row input[type="color"] {
    background: var(--bg-2);
    color: var(--fg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-s);
    padding: 6px 10px;
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
    width: 100%;
    transition: border-color var(--dur-fast) var(--ease-out),
      background var(--dur-fast) var(--ease-out);
  }
  .row select:hover,
  .row input[type="text"]:hover {
    border-color: var(--accent-soft);
  }
  .row select:focus,
  .row input[type="text"]:focus {
    outline: none;
    border-color: var(--accent);
    background: var(--bg-1);
  }
  .row input[type="text"] {
    cursor: text;
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }
  .row input[type="color"] {
    height: 32px;
    padding: 2px;
    width: 60px;
  }
  .row input[type="range"] {
    accent-color: var(--accent);
  }
  .row .value {
    color: var(--fg-2);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    min-width: 50px;
    text-align: right;
  }

  /* A row that uses a single span on the left + a switch on the
     right (no middle control). The switch is the iOS / macOS
     style we use everywhere a boolean toggle is needed. */
  .row.toggle-row {
    grid-template-columns: 1fr auto;
  }
  .row.toggle-row > span {
    color: var(--fg-1);
    font-size: 13px;
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .row.toggle-row .status {
    font-size: 11px;
    color: var(--fg-2);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-weight: 500;
  }
  .row.toggle-row .status[data-status="connected"] {
    color: #4ade80;
  }
  .row.toggle-row .status[data-status="connecting"] {
    color: var(--accent);
  }
  .row.toggle-row .status[data-status="no_client_id"] {
    color: var(--danger);
  }

  /* iOS-style switch. The native checkbox is hidden but kept for
     keyboard / accessibility; the visible state is the .slider
     pill. */
  .switch {
    position: relative;
    display: inline-block;
    width: 36px;
    height: 20px;
    flex-shrink: 0;
  }
  .switch input {
    opacity: 0;
    width: 0;
    height: 0;
  }
  .switch .slider {
    position: absolute;
    inset: 0;
    background: var(--bg-3);
    border: 1px solid var(--border);
    border-radius: 999px;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .switch .slider::before {
    content: "";
    position: absolute;
    left: 2px;
    top: 2px;
    width: 14px;
    height: 14px;
    background: #fff;
    border-radius: 50%;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.4);
    transition: transform var(--dur-base) var(--ease-spring);
  }
  .switch input:checked + .slider {
    background: var(--accent);
    border-color: var(--accent);
  }
  .switch input:checked + .slider::before {
    transform: translateX(16px);
  }
  .switch input:disabled + .slider {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .switch input:focus-visible + .slider {
    box-shadow: 0 0 0 2px var(--accent-soft);
  }

  /* ------------------------------------------------------------
     Library: paths list + actions + stats grid.
     ------------------------------------------------------------ */
  .empty {
    color: var(--fg-2);
    font-size: 13px;
    margin: 0;
    font-style: italic;
  }
  .cover-hint {
    display: block;
    margin-top: 2px;
    color: var(--fg-2);
    font-size: 11px;
    font-weight: 400;
    max-width: 42ch;
    line-height: 1.4;
  }
  .roots {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .roots li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: var(--bg-2);
    border-radius: var(--radius-s);
  }
  .path {
    flex: 1;
    font-family: ui-monospace, monospace;
    font-size: 12px;
    color: var(--fg-1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .source-kind {
    display: inline-flex;
    align-items: center;
    height: 18px;
    padding: 0 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--fg-2);
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    flex-shrink: 0;
  }
  .badge.subtle {
    background: transparent;
    color: var(--fg-3, var(--fg-2));
    border: 1px dashed var(--border);
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 4px;
    text-transform: lowercase;
  }

  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .actions.vertical {
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
  }
  .actions button,
  .pane button:not(.bg-swatch):not(.nav-item) {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: var(--bg-2);
    color: var(--fg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-s);
    padding: 6px 12px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .actions button:hover:not(:disabled),
  .pane button:not(.bg-swatch):not(.nav-item):hover:not(:disabled) {
    background: var(--bg-3);
    border-color: var(--accent-soft);
  }
  .actions button:disabled,
  .pane button:not(.bg-swatch):not(.nav-item):disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .primary {
    background: var(--accent) !important;
    color: #fff !important;
    border-color: var(--accent) !important;
  }
  .primary:hover:not(:disabled) {
    filter: brightness(1.08);
  }
  .danger {
    background: transparent !important;
    color: var(--danger) !important;
    border-color: var(--danger) !important;
  }
  .danger:hover {
    background: var(--danger) !important;
    color: #fff !important;
  }
  .ghost {
    background: transparent !important;
    border: 1px solid transparent !important;
    color: var(--fg-2) !important;
    padding: 4px 8px !important;
    font-size: 12px !important;
  }
  .ghost:hover {
    color: var(--fg-0) !important;
    background: var(--bg-3) !important;
  }
  .link-doc {
    color: var(--fg-2);
    font-size: 12px;
    text-decoration: underline;
    text-decoration-color: rgba(255, 255, 255, 0.18);
    text-underline-offset: 2px;
    align-self: center;
  }
  .link-doc:hover {
    color: var(--fg-0);
  }

  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 8px 14px;
  }
  .stats div {
    display: flex;
    flex-direction: column;
    gap: 2px;
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
    font-weight: 600;
  }
  .paths {
    display: flex;
    flex-direction: column;
    gap: 6px;
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

  .error-line {
    color: var(--danger);
    font-size: 12px;
    margin: 0;
    padding: 8px 10px;
    background: rgba(248, 113, 113, 0.08);
    border: 1px solid rgba(248, 113, 113, 0.25);
    border-radius: var(--radius-s);
  }

  /* ------------------------------------------------------------
     Appearance — background swatches.
     ------------------------------------------------------------ */
  .bg-theme-row {
    grid-template-columns: 160px 1fr;
  }
  .bg-theme-list {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .bg-swatch {
    position: relative;
    height: 36px;
    min-width: 64px;
    flex: 1 1 auto;
    border-radius: var(--radius-md);
    border: 1px solid rgba(255, 255, 255, 0.08);
    cursor: pointer;
    color: var(--fg-2);
    font-size: 11px;
    font-weight: 500;
    padding: 0 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    transition:
      border-color var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .bg-swatch:hover {
    border-color: rgba(255, 255, 255, 0.18);
    color: var(--fg-0);
  }
  .bg-swatch.active {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-soft);
    color: var(--fg-0);
  }
  .bg-swatch-label {
    text-shadow: 0 1px 4px rgba(0, 0, 0, 0.5);
  }

  /* ------------------------------------------------------------
     Equalizer — vertical sliders + presets.
     ------------------------------------------------------------ */
  .eq-presets {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .eq-presets button {
    background: var(--bg-2) !important;
    border-radius: 999px !important;
    padding: 4px 12px !important;
    font-size: 12px !important;
    font-weight: 500;
  }
  .eq {
    display: grid;
    grid-template-columns: repeat(10, 1fr);
    gap: 8px;
    padding: var(--space-3);
    background: var(--bg-2);
    border-radius: var(--radius-md);
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

  /* ------------------------------------------------------------
     Narrow-window adjustment — fold the nav above the pane on
     skinny windows so neither column gets crushed.
     ------------------------------------------------------------ */
  @media (max-width: 720px) {
    .settings {
      grid-template-columns: 1fr;
    }
    .nav {
      position: static;
    }
  }
</style>
