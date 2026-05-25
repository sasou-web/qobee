<script lang="ts">
  import { fly } from "svelte/transition";
  import { open } from "@tauri-apps/plugin-dialog";
  import { addLibraryRoot, scanLibrary } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import { toasts } from "../lib/toasts.svelte";
  import Icon from "./Icon.svelte";

  // The Welcome screen is the very first impression: a fresh install
  // with no library root looks like a broken empty app otherwise.
  // We provide a single, obvious call to action to add a music
  // folder, then trigger the same scan flow as Settings → Library.

  let pickingFolder = $state(false);
  let dragOver = $state(false);

  async function pickFolder(): Promise<string | null> {
    try {
      const r = await open({ directory: true, multiple: false });
      return typeof r === "string" ? r : null;
    } catch {
      // Fall back to a manual path entry on platforms where the
      // native picker is unavailable (e.g. CI / headless).
      const typed = window.prompt("Path to your music folder");
      return typed && typed.trim().length > 0 ? typed.trim() : null;
    }
  }

  async function startScan(folder: string): Promise<void> {
    pickingFolder = true;
    try {
      await addLibraryRoot(folder);
      app.beginScan();
      await scanLibrary(folder);
      toasts.success("Library scanned. Welcome to Qobee.");
      // Refresh data so the app immediately leaves the welcome
      // screen and shows the freshly populated home page.
      await app.refreshAll();
      app.setView("home");
    } catch (e) {
      app.lastError = String(e);
      app.scanRunning = false;
    } finally {
      pickingFolder = false;
    }
  }

  async function onAddFolder(): Promise<void> {
    const folder = await pickFolder();
    if (!folder) return;
    await startScan(folder);
  }

  // Window-level drag-drop: handled in App.svelte, but the welcome
  // screen also accepts drops directly so the user gets a visible
  // affordance.
  function onDragEnter(e: DragEvent): void {
    e.preventDefault();
    dragOver = true;
  }
  function onDragLeave(): void {
    dragOver = false;
  }
  function onDragOver(e: DragEvent): void {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
  }
  function onDrop(e: DragEvent): void {
    e.preventDefault();
    dragOver = false;
    // Browser-side drop carries paths only when the webview was
    // configured to surface them. On Tauri this is handled at the
    // native level, so the App.svelte handler does the work; we
    // keep this as a UX placeholder.
  }
</script>

<section
  class="welcome"
  class:over={dragOver}
  ondragenter={onDragEnter}
  ondragleave={onDragLeave}
  ondragover={onDragOver}
  ondrop={onDrop}
  aria-label="Welcome to Qobee"
  in:fly={{ y: 12, duration: 280 }}
>
  <div class="hero">
    <div class="logo" aria-hidden="true">
      <span class="ring r1"></span>
      <span class="ring r2"></span>
      <span class="dot"></span>
    </div>
    <h1>Welcome to Qobee</h1>
    <p class="lede">
      A local audio player built around your own files. Pick a folder
      and Qobee will index your albums, artists and covers — nothing
      leaves your machine unless you explicitly say so.
    </p>

    <button class="cta" onclick={onAddFolder} disabled={pickingFolder}>
      <Icon name="folder-plus" />
      {pickingFolder ? "Scanning…" : "Choose your music folder"}
    </button>
    <p class="hint">
      You can add more folders later from <strong>Settings → Library</strong>.
      Tip: drop a folder anywhere on this window.
    </p>
  </div>

  <ul class="features">
    <li>
      <Icon name="zap" />
      <div>
        <strong>Fast, local</strong>
        <span>Indexed in SQLite, covers cached on disk.</span>
      </div>
    </li>
    <li>
      <Icon name="music" />
      <div>
        <strong>Lossless first</strong>
        <span>FLAC, ALAC, WAV, plus the usual lossy formats.</span>
      </div>
    </li>
    <li>
      <Icon name="shield" />
      <div>
        <strong>Private by default</strong>
        <span>No cloud, no telemetry, no account.</span>
      </div>
    </li>
  </ul>
</section>

<style>
  .welcome {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 36px;
    padding: 56px 24px;
    max-width: 720px;
    margin: 0 auto;
    border-radius: var(--radius-lg);
    transition: background var(--dur-base) var(--ease-out),
      box-shadow var(--dur-base) var(--ease-out);
  }
  .welcome.over {
    background: var(--accent-soft);
    box-shadow: inset 0 0 0 2px var(--accent);
  }
  .hero {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 14px;
  }
  .logo {
    position: relative;
    width: 72px;
    height: 72px;
    margin-bottom: 8px;
  }
  .logo .ring {
    position: absolute;
    inset: 0;
    border: 2px solid var(--accent);
    border-radius: 50%;
    opacity: 0.3;
    animation: ring 2.4s var(--ease-in-out) infinite;
  }
  .logo .r2 {
    animation-delay: 1.2s;
  }
  .logo .dot {
    position: absolute;
    inset: 26px;
    background: var(--accent);
    border-radius: 50%;
    box-shadow: 0 0 24px var(--accent-glow);
  }
  @keyframes ring {
    0% {
      transform: scale(0.6);
      opacity: 0.6;
    }
    100% {
      transform: scale(1.4);
      opacity: 0;
    }
  }
  h1 {
    font-size: 30px;
    font-weight: 700;
    letter-spacing: -0.015em;
    margin: 0;
    color: var(--fg-0);
  }
  .lede {
    color: var(--fg-1);
    line-height: 1.6;
    max-width: 540px;
    margin: 0;
  }
  .cta {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    background: var(--accent);
    color: #fff;
    border: none;
    padding: 12px 22px;
    border-radius: var(--radius-md);
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    margin-top: 12px;
    transition: filter var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out);
  }
  .cta:hover:not(:disabled) {
    filter: brightness(1.1);
    transform: translateY(-1px);
  }
  .cta:disabled {
    opacity: 0.7;
    cursor: progress;
  }
  .hint {
    color: var(--fg-2);
    font-size: 12px;
    margin: 0;
  }
  .features {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 14px;
    width: 100%;
  }
  .features li {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 14px;
    color: var(--fg-1);
  }
  .features strong {
    display: block;
    color: var(--fg-0);
    font-size: 13px;
    margin-bottom: 2px;
  }
  .features span {
    color: var(--fg-2);
    font-size: 12px;
    line-height: 1.4;
  }
  @media (max-width: 720px) {
    .features {
      grid-template-columns: 1fr;
    }
  }
</style>
