/**
 * @vitest-environment jsdom
 *
 * Tâche 19.2 — Tests Vitest du mode immersif (`NowPlayingFullscreen.svelte`).
 *
 * Feature: qobee-beta-feedback-improvements
 *
 * Validates: Requirements 4.2, 4.4, 4.5, 4.6
 *
 * On asservit ici la découvrabilité et le feedback du mode immersif
 * câblés en tâche 19.1 :
 *
 *  - R4.2 — Le contrôle de sortie expose un `aria-label`
 *    « Quitter le mode immersif » ET un indice clavier visible « Esc »
 *    (élément `<kbd>`).
 *  - R4.4 / R4.6 — Le toggle favori :
 *      * part de `aria-pressed="false"` + libellé « Ajouter aux favoris »
 *        quand `isFavorite` résout `false` ;
 *      * un clic appelle `addFavorite` + `toasts.success("Ajouté aux
 *        favoris")` et bascule `aria-pressed="true"` + libellé
 *        « Retirer des favoris » ;
 *      * quand `isFavorite` résout `true`, un clic appelle
 *        `removeFavorite` + `toasts.info("Retiré des favoris")`.
 *  - R4.5 — Le bouton de file appelle `nowPlayingFullscreen.hide` et
 *    `app.setView("queue")`.
 *  - Le bouton de sortie appelle `nowPlayingFullscreen.hide`.
 *
 * On mocke `../../lib/api` via `importOriginal` : on conserve les vraies
 * fonctions utilitaires (`coverUrl`, types) — utilisées par `Cover` —
 * et on ne pilote que les commandes de favori/transport. Les méthodes du
 * store `toasts` sont espionnées (`vi.spyOn`) pour observer succès/info
 * sans planifier de vrais timers de dismiss. Le `$effect` qui résout
 * l'état favori est attendu via `waitFor`.
 */
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type MockInstance,
} from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";

// jsdom ne fournit pas la Web Animations API. L'overlay du mode immersif
// utilise des transitions Svelte (`transition:fade` / `transition:fly`)
// qui appellent `element.animate()` / `element.getAnimations()` au
// montage et lèveraient sinon. Des stubs no-op suffisent : on n'asserte
// pas l'animation, seulement le DOM et le feedback (toasts).
if (typeof Element.prototype.animate !== "function") {
  Element.prototype.animate = (() => ({
    cancel() {},
    finish() {},
    play() {},
    pause() {},
    onfinish: null,
    finished: Promise.resolve(),
  })) as unknown as Element["animate"];
}
if (typeof Element.prototype.getAnimations !== "function") {
  Element.prototype.getAnimations = (() => []) as unknown as Element["getAnimations"];
}

// Override only the favorite/transport command surface; keep everything
// else (types, `coverUrl`, …) from the real module so transitive
// consumers such as `Cover.svelte` / `AnimatedAmbientBackground.svelte`
// keep working without touching Tauri internals.
vi.mock("../../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/api")>();
  return {
    ...actual,
    isFavorite: vi.fn(async () => false),
    addFavorite: vi.fn(async () => undefined),
    removeFavorite: vi.fn(async () => undefined),
    nextTrack: vi.fn(async () => undefined),
    prevTrack: vi.fn(async () => undefined),
    pause: vi.fn(async () => undefined),
    resume: vi.fn(async () => undefined),
    seek: vi.fn(async () => undefined),
  };
});

import NowPlayingFullscreen from "../NowPlayingFullscreen.svelte";
import * as api from "../../lib/api";
import { app } from "../../lib/stores.svelte";
import { nowPlayingFullscreen } from "../../lib/nowPlayingFullscreen.svelte";
import { toasts } from "../../lib/toasts.svelte";
import type { Track } from "../../lib/api";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const isFavorite = vi.mocked(api.isFavorite);
const addFavorite = vi.mocked(api.addFavorite);
const removeFavorite = vi.mocked(api.removeFavorite);

/** Minimal `Track`. `cover_key: null` keeps `Cover` /
 *  `AnimatedAmbientBackground` on their placeholder branch so they never
 *  call `convertFileSrc` (no Tauri internals). */
function track(id: number): Track {
  return {
    id,
    track_uid: `uid-${id}`,
    path: `/music/${id}.flac`,
    title: "Titre courant",
    artist: "Artiste courant",
    album: "Album courant",
    album_artist: null,
    track_number: null,
    disc_number: null,
    year: null,
    genre: null,
    duration_seconds: 180,
    sample_rate: null,
    bit_depth: null,
    channels: null,
    cover_key: null,
  };
}

let successSpy: MockInstance<(message: string, durationMs?: number) => number>;
let infoSpy: MockInstance<(message: string, durationMs?: number) => number>;
let hideSpy: MockInstance<() => void>;
let setViewSpy: MockInstance<(typeof app)["setView"]>;

beforeEach(() => {
  app.lastError = null;

  // Spy on toast outcomes. mockImplementation avoids scheduling the real
  // dismiss timeout (window.setTimeout) so teardown stays clean.
  successSpy = vi.spyOn(toasts, "success").mockImplementation(() => 0);
  infoSpy = vi.spyOn(toasts, "info").mockImplementation(() => 0);

  // Spy on the singleton navigation side effects so the immersive view's
  // exit / queue actions are observable without mutating shared state.
  hideSpy = vi.spyOn(nowPlayingFullscreen, "hide").mockImplementation(() => {});
  setViewSpy = vi.spyOn(app, "setView").mockImplementation(() => {});
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

// ---------------------------------------------------------------------------
// R4.2 — explicit exit control with a visible "Esc" hint
// ---------------------------------------------------------------------------

describe("NowPlayingFullscreen — sortie explicite (R4.2)", () => {
  it("expose un bouton de sortie « Quitter le mode immersif » + indice « Esc »", async () => {
    isFavorite.mockResolvedValue(false);
    const { getByRole, getByText } = render(NowPlayingFullscreen, {
      props: { track: track(1) },
    });

    // The exit control's accessible name comes from PLAYER_LABELS (R4.2).
    const exitBtn = getByRole("button", { name: "Quitter le mode immersif" });
    expect(exitBtn).toBeInTheDocument();

    // Visible keyboard hint rendered as a <kbd> element carrying "Esc".
    const kbd = getByText("Esc");
    expect(kbd.tagName).toBe("KBD");
    expect(exitBtn).toContainElement(kbd);
  });

  it("cliquer le bouton de sortie appelle nowPlayingFullscreen.hide", async () => {
    isFavorite.mockResolvedValue(false);
    const { getByRole } = render(NowPlayingFullscreen, {
      props: { track: track(1) },
    });

    await fireEvent.click(
      getByRole("button", { name: "Quitter le mode immersif" })
    );
    expect(hideSpy).toHaveBeenCalledTimes(1);
  });
});

// ---------------------------------------------------------------------------
// R4.4 / R4.6 — favorite toggle feedback + dynamic aria-pressed/label
// ---------------------------------------------------------------------------

describe("NowPlayingFullscreen — toggle favori (R4.4/R4.6)", () => {
  it("isFavorite=false → aria-pressed=false + « Ajouter aux favoris » ; clic ajoute + toast succès et bascule l'état", async () => {
    isFavorite.mockResolvedValue(false);
    const { getByRole } = render(NowPlayingFullscreen, {
      props: { track: track(7) },
    });

    // Let the favorite-resolving $effect settle.
    await waitFor(() => expect(isFavorite).toHaveBeenCalledWith(7));

    const favBtn = getByRole("button", { name: "Ajouter aux favoris" });
    expect(favBtn).toHaveAttribute("aria-pressed", "false");

    await fireEvent.click(favBtn);

    // Add command + success toast (R4.4).
    await waitFor(() => expect(addFavorite).toHaveBeenCalledWith(7));
    expect(successSpy).toHaveBeenCalledWith("Ajouté aux favoris");
    expect(infoSpy).not.toHaveBeenCalled();

    // State flips: aria-pressed=true + label « Retirer des favoris » (R4.6).
    await waitFor(() => {
      const pressed = getByRole("button", { name: "Retirer des favoris" });
      expect(pressed).toHaveAttribute("aria-pressed", "true");
    });
  });

  it("isFavorite=true → clic retire + toast info « Retiré des favoris »", async () => {
    isFavorite.mockResolvedValue(true);
    const { getByRole } = render(NowPlayingFullscreen, {
      props: { track: track(9) },
    });

    // Wait for the $effect to resolve the favorite state to "on".
    const favBtn = await waitFor(() =>
      getByRole("button", { name: "Retirer des favoris" })
    );
    expect(favBtn).toHaveAttribute("aria-pressed", "true");

    await fireEvent.click(favBtn);

    await waitFor(() => expect(removeFavorite).toHaveBeenCalledWith(9));
    expect(infoSpy).toHaveBeenCalledWith("Retiré des favoris");
    expect(successSpy).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// R4.5 — queue / "Up next" action opens the dedicated queue view
// ---------------------------------------------------------------------------

describe("NowPlayingFullscreen — action file d'attente (R4.5)", () => {
  it("cliquer le bouton de file appelle hide puis app.setView(\"queue\")", async () => {
    isFavorite.mockResolvedValue(false);
    const { getByRole } = render(NowPlayingFullscreen, {
      props: { track: track(1) },
    });

    await fireEvent.click(getByRole("button", { name: "File d'attente" }));

    expect(hideSpy).toHaveBeenCalledTimes(1);
    expect(setViewSpy).toHaveBeenCalledWith("queue");
  });
});
