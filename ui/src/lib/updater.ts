// Auto-update from GitHub Releases.
//
// On startup (and on demand from Settings) we ask the Tauri updater
// plugin whether a newer release is published. The plugin fetches
// `latest.json` from the GitHub `releases/latest` endpoint
// configured in `tauri.conf.json`, compares its `version` against
// the running app version, and — if newer — exposes the matching
// platform artifact plus its minisign signature. We verify the
// signature against the public key pinned in `tauri.conf.json`,
// download + install in place, then relaunch into the new version.
//
// The user never re-downloads the app by hand: a single toast
// announces the new version and applies it.

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { toasts } from "./toasts.svelte";

/** Guard so a manual check and the startup check never run the
 *  download twice for the same session. */
let updateInFlight = false;

/**
 * Check GitHub for a newer release and, if one exists, download +
 * install it and relaunch.
 *
 * @param opts.silent  When `true` (the startup path), stays quiet if
 *                      the app is already up to date or the network
 *                      is unreachable. When `false` (the Settings
 *                      "Check for updates" button), reports both the
 *                      "up to date" and the error cases so the user
 *                      gets feedback from their click.
 * @returns `true` when an update was found and the install/relaunch
 *          sequence started, `false` otherwise.
 */
export async function checkForUpdates({ silent }: { silent: boolean }): Promise<boolean> {
  if (updateInFlight) return false;
  updateInFlight = true;

  let update: Update | null = null;
  try {
    update = await check();
  } catch (e) {
    // `check()` throws when running outside Tauri (e.g. `vite
    // preview`) or when the endpoint is unreachable. Only surface
    // it on an explicit user action.
    if (!silent) {
      toasts.error(`Impossible de vérifier les mises à jour : ${errMessage(e)}`);
    }
    updateInFlight = false;
    return false;
  }

  if (!update) {
    if (!silent) {
      toasts.success("Qobee est à jour.");
    }
    updateInFlight = false;
    return false;
  }

  // A newer version is available. Announce it, then download +
  // install. The download runs in the background; for a desktop
  // music player the artifact is small enough that a progress bar
  // would add more noise than value, so we settle for a single
  // "installing" toast and a relaunch prompt.
  toasts.info(`Mise à jour ${update.version} disponible — installation…`, 6000);

  try {
    await update.downloadAndInstall();
  } catch (e) {
    toasts.error(`Échec de la mise à jour : ${errMessage(e)}`);
    updateInFlight = false;
    return false;
  }

  // On Windows the passive NSIS installer may have already closed
  // us; on macOS / Linux we relaunch explicitly so the user lands
  // back in Qobee on the new version.
  toasts.success(`Qobee ${update.version} installé — redémarrage…`, 4000);
  try {
    await relaunch();
  } catch (e) {
    toasts.warn(
      `Mise à jour installée. Redémarre Qobee pour l'appliquer (${errMessage(e)}).`,
      8000
    );
  }
  updateInFlight = false;
  return true;
}

function errMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return String(e);
}
