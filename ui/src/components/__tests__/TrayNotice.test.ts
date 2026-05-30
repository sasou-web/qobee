/**
 * @vitest-environment jsdom
 *
 * Feature: qobee-beta-feedback-improvements
 *
 * Validates: Requirements 7.1 (bandeau « Qobee continue dans la zone de
 *            notification » affiché à la réception de l'événement backend),
 *            7.2 (bouton « Quitter complètement » → commande `quit_app`).
 *
 * TrayNotice (tâche 22.1) est un bandeau persistant monté au niveau du
 * shell. Il s'abonne, dans `onMount`, à l'événement Tauri
 * `tray:first-close-notice` via `listen` et n'affiche le bandeau qu'à la
 * réception de cet événement (le backend ne l'émet qu'une seule fois,
 * R7.3). Le bouton « Quitter complètement » appelle `quitApp()` (commande
 * `quit_app`, R7.2) et une croix masque l'avis.
 *
 * Stratégie de test :
 *  - `@tauri-apps/api/event` est mocké pour CAPTURER le callback passé à
 *    `listen("tray:first-close-notice", …)`. On le déclenche manuellement
 *    pour simuler l'événement backend, sans dépendre d'un runtime Tauri.
 *    `listen` rend une `Promise<UnlistenFn>` ; le mock enregistre le
 *    handler et rend une fonction de désabonnement no-op (espionnée).
 *  - `../../lib/api` est mocké pour que `quitApp` soit un `vi.fn()`
 *    assertable.
 *
 * Notes d'environnement :
 *  - Le composant utilise la transition `fly` ; jsdom n'implémente pas
 *    `Element.prototype.animate`. On pose un stub minimal qui déclenche
 *    `onfinish` afin que l'outro de transition se termine (et que le nœud
 *    soit retiré du DOM au dismiss).
 *  - `announce()` (R2.9) crée une région live ARIA `role="status"`
 *    rattachée à `document.body` (hors du conteneur du composant). On
 *    scope donc les requêtes du bandeau sur le `container` du composant
 *    pour éviter toute collision de rôle, et on nettoie la région entre
 *    les tests.
 *
 * Couvert :
 *  - Aucun bandeau au montage tant que l'événement n'est pas arrivé.
 *  - Après déclenchement du handler capturé, le bandeau s'affiche avec le
 *    message FR exact et `role="status"`.
 *  - Le clic sur « Quitter complètement » invoque `quitApp`.
 *  - Le clic sur la croix masque le bandeau.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";

// Shared mock state. `vi.hoisted` lets these values exist before the
// hoisted `vi.mock` factories run (top-level `let` would be in the
// temporal dead zone at mock-evaluation time).
const mocks = vi.hoisted(() => ({
  // Handlers registered for the `tray:first-close-notice` event. The SUT
  // registers exactly one in `onMount`; we invoke it to simulate the
  // backend emitting the event.
  capturedHandlers: [] as Array<(event: unknown) => void>,
  // No-op unlisten returned by `listen(...)`, spied so we can assert the
  // component cleans up on destroy.
  unlisten: vi.fn(),
  // `quitApp` resolves immediately so the click handler completes.
  quitApp: vi.fn(async () => undefined),
}));

// Mock the Tauri event surface BEFORE importing the component so the SUT
// binds to the mocked `listen`. We capture the handler synchronously at
// call time and return a resolved Promise<UnlistenFn>.
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    (event: string, handler: (e: unknown) => void): Promise<() => void> => {
      if (event === "tray:first-close-notice") {
        mocks.capturedHandlers.push(handler);
      }
      return Promise.resolve(mocks.unlisten);
    }
  ),
}));

// Mock the typed Tauri command surface used by the "Quitter
// complètement" button.
vi.mock("../../lib/api", () => ({
  quitApp: mocks.quitApp,
}));

import TrayNotice from "../TrayNotice.svelte";

const MESSAGE = "Qobee continue dans la zone de notification";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Render the component and wait for `onMount` to register the event
 * listener, returning a helper that fires the captured handler to
 * simulate the backend `tray:first-close-notice` event.
 */
async function renderAndCapture() {
  const result = render(TrayNotice);
  // Let onMount run and the `listen(...).then(...)` microtask resolve so
  // the handler is registered.
  await tick();
  await Promise.resolve();

  function fireTrayEvent(): void {
    expect(mocks.capturedHandlers.length).toBeGreaterThan(0);
    for (const handler of mocks.capturedHandlers) {
      handler({});
    }
  }

  /** The component's banner element (scoped to its own container). */
  function banner(): HTMLElement | null {
    return result.container.querySelector(".tray-notice");
  }

  return { ...result, fireTrayEvent, banner };
}

beforeAll(() => {
  // jsdom doesn't implement the Web Animations API used by Svelte's
  // `fly` transition. Provide a stub that fires `onfinish` so the outro
  // transition completes and the node is removed from the DOM.
  if (typeof Element.prototype.animate !== "function") {
    Element.prototype.animate = function animateStub() {
      const animation: {
        onfinish: ((this: unknown) => void) | null;
        oncancel: ((this: unknown) => void) | null;
        cancel: () => void;
        finished: Promise<void>;
      } = {
        onfinish: null,
        oncancel: null,
        cancel() {},
        finished: Promise.resolve(),
      };
      // Resolve the transition on the next macrotask.
      setTimeout(() => animation.onfinish?.call(animation), 0);
      return animation as unknown as Animation;
    } as typeof Element.prototype.animate;
  }
});

beforeEach(() => {
  mocks.capturedHandlers.length = 0;
  vi.clearAllMocks();
});

afterEach(() => {
  cleanup();
  // Drop the shared a11y live region created by `announce` so it doesn't
  // leak across tests (it's a module singleton attached to document.body).
  document
    .querySelectorAll("[data-a11y-live-region]")
    .forEach((el) => el.remove());
});

// ---------------------------------------------------------------------------
// R7.1 — banner hidden until the event arrives
// ---------------------------------------------------------------------------

describe("TrayNotice — affichage piloté par l'événement (R7.1)", () => {
  it("ne rend AUCUN bandeau au montage tant que l'événement n'est pas reçu", async () => {
    const { banner } = await renderAndCapture();

    expect(banner()).toBeNull();
  });

  it("affiche le bandeau (role=status + message FR) à la réception de tray:first-close-notice", async () => {
    const { fireTrayEvent, banner } = await renderAndCapture();

    fireTrayEvent();
    await tick();

    const el = banner();
    expect(el).not.toBeNull();
    expect(el).toBeInTheDocument();
    expect(el).toHaveAttribute("role", "status");
    expect(el).toHaveAttribute("aria-live", "polite");
    expect(el).toHaveTextContent(MESSAGE);
  });
});

// ---------------------------------------------------------------------------
// R7.2 — "Quitter complètement" invokes quit_app
// ---------------------------------------------------------------------------

describe("TrayNotice — quitter complètement (R7.2)", () => {
  it("invoque quitApp au clic sur « Quitter complètement »", async () => {
    const { fireTrayEvent, getByRole } = await renderAndCapture();

    fireTrayEvent();
    await tick();

    const quitBtn = getByRole("button", { name: "Quitter complètement" });
    await fireEvent.click(quitBtn);

    expect(mocks.quitApp).toHaveBeenCalledTimes(1);
  });
});

// ---------------------------------------------------------------------------
// Dismiss — close button hides the banner
// ---------------------------------------------------------------------------

describe("TrayNotice — fermeture de l'avis", () => {
  it("masque le bandeau au clic sur la croix de fermeture", async () => {
    const { fireTrayEvent, getByRole, banner } = await renderAndCapture();

    fireTrayEvent();
    await tick();
    expect(banner()).not.toBeNull();

    const closeBtn = getByRole("button", { name: "Fermer l'avis" });
    await fireEvent.click(closeBtn);

    // `visible` becomes false; the fly outro transition delays DOM
    // removal, so wait for the banner to actually leave the document.
    await waitFor(() => {
      expect(banner()).toBeNull();
    });
  });
});
