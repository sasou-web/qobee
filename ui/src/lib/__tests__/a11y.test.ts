/**
 * Tests des helpers d'accessibilité partagés et de la cohérence des
 * libellés FR (tâche 3.3).
 *
 * Feature: qobee-beta-feedback-improvements
 *
 * Validates: Requirements 2.8 (activation clavier Entrée/Espace),
 *            2.9 (annonce via région live ARIA),
 *            5.5 (libellés non vides, source unique aria-label/title).
 *
 * Couvert :
 *  - `activateOnKey` déclenche le callback sur Entrée et Espace,
 *    appelle `preventDefault`, pose `tabindex=0` quand absent, et
 *    nettoie son écouteur au `destroy` (un événement clavier ultérieur
 *    ne déclenche plus le callback).
 *  - `announce` écrit le message dans une région live ARIA partagée
 *    (`role="status"`, `aria-live="polite"`) à la frame suivante.
 *  - Chaque entrée de `SIDEBAR_LABELS` / `PLAYER_LABELS` porte un
 *    `label` non vide.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { activateOnKey, announce } from "../a11y";
import { PLAYER_LABELS, SIDEBAR_LABELS, type UiLabel } from "../labels";

// ---------------------------------------------------------------------------
// Helpers locaux au fichier de test.
// ---------------------------------------------------------------------------

/** Attend la prochaine frame d'animation (pour `announce`, qui écrit sur rAF). */
function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

/** Construit un événement `keydown` annulable et propageable pour une touche. */
function keydown(key: string): KeyboardEvent {
  return new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
}

// ---------------------------------------------------------------------------
// activateOnKey — activation clavier (R2.8)
// ---------------------------------------------------------------------------

describe("activateOnKey", () => {
  let node: HTMLElement;

  beforeEach(() => {
    node = document.createElement("div");
    document.body.appendChild(node);
  });

  afterEach(() => {
    node.remove();
  });

  it("déclenche le callback sur la touche Entrée et appelle preventDefault", () => {
    const onActivate = vi.fn();
    activateOnKey(node, onActivate);

    const event = keydown("Enter");
    node.dispatchEvent(event);

    expect(onActivate).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });

  it("déclenche le callback sur la touche Espace et appelle preventDefault", () => {
    const onActivate = vi.fn();
    activateOnKey(node, onActivate);

    const event = keydown(" ");
    node.dispatchEvent(event);

    expect(onActivate).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });

  it("ignore les touches non activantes (ne déclenche pas le callback)", () => {
    const onActivate = vi.fn();
    activateOnKey(node, onActivate);

    for (const key of ["Tab", "a", "ArrowDown", "Escape"]) {
      const event = keydown(key);
      node.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
    }

    expect(onActivate).not.toHaveBeenCalled();
  });

  it("pose tabindex=0 quand l'élément n'est pas déjà focusable", () => {
    activateOnKey(node, vi.fn());
    expect(node.getAttribute("tabindex")).toBe("0");
  });

  it("préserve un tabindex déjà présent", () => {
    node.setAttribute("tabindex", "-1");
    activateOnKey(node, vi.fn());
    expect(node.getAttribute("tabindex")).toBe("-1");
  });

  it("nettoie son écouteur au destroy : un événement ultérieur ne déclenche plus le callback", () => {
    const onActivate = vi.fn();
    const action = activateOnKey(node, onActivate);

    node.dispatchEvent(keydown("Enter"));
    expect(onActivate).toHaveBeenCalledTimes(1);

    action.destroy();

    node.dispatchEvent(keydown("Enter"));
    node.dispatchEvent(keydown(" "));
    // Toujours 1 : aucun appel supplémentaire après destroy.
    expect(onActivate).toHaveBeenCalledTimes(1);
  });
});

// ---------------------------------------------------------------------------
// announce — région live ARIA partagée (R2.9)
// ---------------------------------------------------------------------------

describe("announce", () => {
  afterEach(() => {
    // Nettoie la région live partagée entre les tests (singleton module).
    document
      .querySelectorAll("[data-a11y-live-region]")
      .forEach((el) => el.remove());
  });

  it("écrit le message dans une région live ARIA (role=status, aria-live=polite)", async () => {
    announce("Ajouté à la file");
    await nextFrame();

    const region = document.querySelector("[data-a11y-live-region]");
    expect(region).not.toBeNull();
    expect(region!.getAttribute("role")).toBe("status");
    expect(region!.getAttribute("aria-live")).toBe("polite");
    expect(region!.getAttribute("aria-atomic")).toBe("true");
    expect(region!.textContent).toBe("Ajouté à la file");
  });

  it("réutilise la même région live pour des annonces successives", async () => {
    announce("Premier message");
    await nextFrame();
    announce("Second message");
    await nextFrame();

    const regions = document.querySelectorAll("[data-a11y-live-region]");
    expect(regions.length).toBe(1);
    expect(regions[0]?.textContent).toBe("Second message");
  });
});

// ---------------------------------------------------------------------------
// labels — cohérence des libellés FR (R2.1 / R2.4, R5.5)
// ---------------------------------------------------------------------------

describe("libellés FR (labels.ts)", () => {
  const allEntries: ReadonlyArray<[string, Record<string, UiLabel>]> = [
    ["SIDEBAR_LABELS", SIDEBAR_LABELS],
    ["PLAYER_LABELS", PLAYER_LABELS],
  ];

  for (const [tableName, table] of allEntries) {
    describe(tableName, () => {
      for (const [key, entry] of Object.entries(table)) {
        it(`l'entrée « ${key} » a un label non vide`, () => {
          expect(typeof entry.label).toBe("string");
          expect(entry.label.trim().length).toBeGreaterThan(0);
        });
      }
    });
  }
});
