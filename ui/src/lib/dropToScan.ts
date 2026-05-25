// Drag-and-drop a folder anywhere on the window to add it as a
// library root and trigger a scan. The Tauri webview surfaces native
// drop events with absolute paths, which the regular HTML drag-drop
// API cannot do.

import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { addLibraryRoot, scanLibrary } from "./api";
import { app } from "./stores.svelte";
import { toasts } from "./toasts.svelte";

/**
 * Install the drop listener. Returns a cleanup function the caller
 * is expected to invoke on unmount. Multiple paths in one drop are
 * scanned sequentially.
 */
export async function installFolderDrop(): Promise<UnlistenFn> {
  const webview = getCurrentWebview();
  return webview.onDragDropEvent(async (event) => {
    const payload = event.payload;
    if (payload.type !== "drop") return;
    const paths = (payload.paths as string[] | undefined) ?? [];
    if (paths.length === 0) return;

    let added = 0;
    let failed = 0;
    for (const p of paths) {
      try {
        await addLibraryRoot(p);
        app.beginScan();
        await scanLibrary(p);
        added += 1;
      } catch (e) {
        failed += 1;
        // Likely the user dropped a file or a non-existent path.
        // We don't want a single bad entry to abort the others, so
        // just count and report at the end.
        console.warn("dropToScan: failed for", p, e);
      }
    }
    if (added > 0) {
      toasts.success(
        added === 1 ? "Added folder to library." : `Added ${added} folders to library.`
      );
      await app.refreshAll();
    }
    if (failed > 0) {
      toasts.warn(
        failed === 1
          ? "One drop was ignored (not a folder?)."
          : `${failed} drops were ignored (not folders?).`
      );
    }
    app.scanRunning = false;
  });
}
