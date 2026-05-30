/**
 * @vitest-environment jsdom
 *
 * Tests Vitest de confidentialité des covers Discord — tâche 23.2.
 *
 * Feature: qobee-beta-feedback-improvements
 *
 * Validates: Requirements 8.1 (Cover_Upload opt-in, désactivé par défaut),
 *            8.2 (toggle reflète / pilote `integrations.discord_cover_upload`),
 *            8.4 (bouton de purge appelle `clearUploadedCoverLinks`),
 *            8.5 (toast confirmant le nombre de liens supprimés).
 *
 * ---------------------------------------------------------------------------
 * Stratégie (approche « a » du design — rendu du vrai composant)
 * ---------------------------------------------------------------------------
 * On monte le vrai `Settings.svelte` puis on navigue vers la catégorie
 * « Integrations » via la nav `CATEGORIES`, et on interagit avec le toggle
 * « Upload covers for Discord presence » et le bouton « Clear uploaded cover
 * links ». C'est le test le plus proche de l'intention de la tâche
 * (« confidentialité des covers » câblée dans la page Settings réelle).
 *
 * `Settings.svelte` est lourd : son `$effect` `load()` appelle ~9 commandes
 * Tauri et il interroge le statut Discord en boucle. On isole le composant
 * du backend en mockant :
 *   - `../../lib/api`            → toutes les commandes de `load()` résolvent
 *                                  proprement ; `clearUploadedCoverLinks`
 *                                  résout sur un compte connu (3).
 *   - `../../lib/discordPresence`→ `getStatus` résout, `setCoverUploadEnabled`
 *                                  est un espion (R8.2/R8.3).
 *
 * On garde les VRAIS stores `settings` et `toasts` : `settings.values`
 * démarre sur ses `DEFAULTS` (dont `discordCoverUpload: false`, l'opt-in
 * R8.1), ce qui rend le toggle non coché sans artifice ; on espionne
 * `settings.set` (R8.2) et `toasts.success` (R8.5) pour vérifier le câblage
 * sans effet de bord réel.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { tick } from "svelte";

// ---------------------------------------------------------------------------
// Mocks de modules — déclarés AVANT l'import du composant pour que le SUT et
// ses dépendances transitives (le store `app`, le store `settings`) résolvent
// bien la version mockée de `../../lib/api`.
// ---------------------------------------------------------------------------

// Surface `../../lib/api`. On ne mocke que ce que `Settings.svelte` touche au
// rendu / dans son `$effect` `load()`, plus la commande de purge. Les autres
// fonctions importées par le composant ne sont appelées que dans des
// gestionnaires non déclenchés ici (rester `undefined` est sans danger en ESM
// tant qu'on ne les invoque pas).
vi.mock("../../lib/api", () => ({
  // --- appels de `load()` (Promise.allSettled) : tous résolvent ---
  listLibraryRoots: vi.fn(async () => []),
  listOutputDevices: vi.fn(async () => []),
  getSelectedOutputDevice: vi.fn(async () => null),
  libraryStats: vi.fn(async () => null),
  getReplayGainMode: vi.fn(async () => "off"),
  getUserOutputMode: vi.fn(async () => "auto"),
  listLibrarySources: vi.fn(async () => []),
  getWindowSettings: vi.fn(async () => ({
    close_behavior: "quit",
    tray_enabled: false,
    notify_on_track_change: false,
    tray_notice_shown: false,
  })),
  getCrossfadeMs: vi.fn(async () => 0),
  // --- R8.4 : purge des liens de covers (compte connu) ---
  clearUploadedCoverLinks: vi.fn(async () => 3),
  // --- persistance bas niveau utilisée par le vrai store `settings` ---
  getSetting: vi.fn(async () => null),
  setSetting: vi.fn(async () => undefined),
  // --- divers, importés mais non invoqués pendant ce test ---
  shellOpen: vi.fn(async () => undefined),
}));

// Surface `../../lib/discordPresence`. `getStatus` est interrogé par un
// `$effect`; `setCoverUploadEnabled` est l'espion clé de R8.2/R8.3.
// La factory est hoistée au sommet du fichier : on ne peut pas y référencer
// une variable de module. On crée donc les espions à l'intérieur et on
// récupère une référence via l'import mocké plus bas.
vi.mock("../../lib/discordPresence", () => ({
  discordPresence: {
    getStatus: vi.fn(async () => "disabled"),
    setCoverUploadEnabled: vi.fn(async () => undefined),
  },
}));

import Settings from "../Settings.svelte";
import * as api from "../../lib/api";
import { discordPresence } from "../../lib/discordPresence";
import { settings } from "../../lib/settings.svelte";
import { toasts } from "../../lib/toasts.svelte";

// Référence typée vers l'espion `setCoverUploadEnabled` du module mocké.
const setCoverUploadEnabled = vi.mocked(discordPresence.setCoverUploadEnabled);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Laisse les microtâches (promesses awaitées) et le tick Svelte se vider. */
async function flush(): Promise<void> {
  await Promise.resolve();
  await tick();
  await new Promise((r) => setTimeout(r, 0));
  await tick();
}

/** Bascule sur la catégorie « Integrations » via le bouton de nav. */
async function openIntegrations(
  getByRole: (role: string, opts?: { name?: string | RegExp }) => HTMLElement,
): Promise<void> {
  const navButton = getByRole("button", { name: "Integrations" });
  await fireEvent.click(navButton);
  await tick();
}

/** Localise la ligne du toggle « Upload covers » et son `input` checkbox. */
function coverUploadCheckbox(container: HTMLElement): HTMLInputElement {
  const row = Array.from(container.querySelectorAll<HTMLElement>(".toggle-row")).find(
    (r) => r.textContent?.includes("Upload covers for Discord presence"),
  );
  if (!row) throw new Error("Cover-upload toggle row not found");
  const input = row.querySelector<HTMLInputElement>('input[type="checkbox"]');
  if (!input) throw new Error("Cover-upload checkbox not found");
  return input;
}

beforeEach(() => {
  // Réinitialise le singleton `settings` à l'état opt-in par défaut entre les
  // tests (un test précédent a pu muter `discordCoverUpload`).
  settings.values = { ...settings.values, discordCoverUpload: false };
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// R8.1 — opt-in : le toggle est désactivé par défaut
// ---------------------------------------------------------------------------

describe("Settings → Integrations — Cover_Upload opt-in (R8.1)", () => {
  it("le toggle « Upload covers » est non coché par défaut (discordCoverUpload=false)", async () => {
    const { container, getByRole } = render(Settings);
    await flush();
    await openIntegrations(getByRole);

    const checkbox = coverUploadCheckbox(container);
    // Reflète `settings.values.discordCoverUpload` (défaut `false`).
    expect(checkbox).not.toBeChecked();
    expect(settings.values.discordCoverUpload).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// R8.2 / R8.3 — basculer le toggle persiste ET pilote le backend
// ---------------------------------------------------------------------------

describe("Settings → Integrations — bascule du Cover_Upload (R8.2, R8.3)", () => {
  it("activer le toggle appelle settings.set(..., true) ET setCoverUploadEnabled(true)", async () => {
    const setSpy = vi.spyOn(settings, "set").mockResolvedValue(undefined);

    const { container, getByRole } = render(Settings);
    await flush();
    await openIntegrations(getByRole);

    const checkbox = coverUploadCheckbox(container);
    // `fireEvent.change` avec `target.checked` est déterministe : le handler
    // lit `e.target.checked` indépendamment de la liaison contrôlée.
    await fireEvent.change(checkbox, { target: { checked: true } });
    await flush();

    expect(setSpy).toHaveBeenCalledWith("discordCoverUpload", true);
    expect(setCoverUploadEnabled).toHaveBeenCalledWith(true);
  });

  it("désactiver le toggle appelle settings.set(..., false) ET setCoverUploadEnabled(false)", async () => {
    const setSpy = vi.spyOn(settings, "set").mockResolvedValue(undefined);

    const { container, getByRole } = render(Settings);
    await flush();
    await openIntegrations(getByRole);

    const checkbox = coverUploadCheckbox(container);
    await fireEvent.change(checkbox, { target: { checked: false } });
    await flush();

    expect(setSpy).toHaveBeenCalledWith("discordCoverUpload", false);
    expect(setCoverUploadEnabled).toHaveBeenCalledWith(false);
  });
});

// ---------------------------------------------------------------------------
// R8.4 / R8.5 — purge des liens + toast du compte retourné
// ---------------------------------------------------------------------------

describe("Settings → Integrations — purge des liens de covers (R8.4, R8.5)", () => {
  it("« Clear uploaded cover links » appelle clearUploadedCoverLinks et affiche le toast du compte", async () => {
    const successSpy = vi.spyOn(toasts, "success");

    const { getByRole } = render(Settings);
    await flush();
    await openIntegrations(getByRole);

    const clearButton = getByRole("button", { name: "Clear uploaded cover links" });
    await fireEvent.click(clearButton);
    await flush();

    // R8.4 : la commande backend de purge est appelée.
    expect(vi.mocked(api.clearUploadedCoverLinks)).toHaveBeenCalledTimes(1);
    // R8.5 : un toast de succès confirme le nombre (3) de liens supprimés.
    expect(successSpy).toHaveBeenCalledTimes(1);
    const message = successSpy.mock.calls[0]?.[0] ?? "";
    expect(message).toContain("3");
  });
});
