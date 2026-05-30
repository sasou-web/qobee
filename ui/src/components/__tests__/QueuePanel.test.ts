/**
 * @vitest-environment jsdom
 *
 * Tâche 18.3 — Tests Vitest de `QueuePanel.svelte`.
 *
 * Feature: qobee-beta-feedback-improvements
 *
 * Validates: Requirements 3.5, 3.6
 *
 * Le `QueuePanel` est la page/panneau dédié de la file d'attente. On
 * asservit ici :
 *
 *  - R3.6 — État vide explicite FR : le texte « Votre file est vide. »
 *    + l'indice « Ajouter à la file » s'affiche quand la file est vide,
 *    et le bouton « Vider la file » est alors désactivé.
 *  - R3.7 (adjacent) — Avec une file non vide, cliquer « Vider la file »
 *    appelle bien `clearQueue` (commande tâche 7.1).
 *  - R3.5/R2 (structurel) — Accessibilité : la section porte
 *    `aria-label="File d'attente"`, le bouton de vidage porte un
 *    `aria-label`, et chaque rangée expose un `role="button"` avec un
 *    nom accessible (`aria-label`) décrivant l'action de lecture, ce qui
 *    rend le panneau atteignable et pilotable au clavier / lecteur
 *    d'écran.
 *
 * On mocke `../../lib/api` via `importOriginal` : on garde les vraies
 * fonctions utilitaires (`coverUrl`, types) et on ne pilote que les
 * commandes de file (`getQueue`/`getTracks`/`clearQueue`/…). Le chargement
 * asynchrone du `$effect` est attendu via `waitFor`.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";

// jsdom ne fournit pas la Web Animations API. Les transitions Svelte
// (`animate:flip` sur les rangées) appellent `element.animate()` /
// `element.getAnimations()` et lèveraient sinon. Des stubs no-op
// suffisent : on n'asserte pas l'animation, seulement le DOM résultant.
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

// Override only the queue command surface; keep everything else
// (types, `coverUrl`, …) from the real module so transitive consumers
// such as `Cover.svelte` keep working.
vi.mock("../../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/api")>();
  return {
    ...actual,
    getQueue: vi.fn(),
    getTracks: vi.fn(async () => []),
    queueJumpTo: vi.fn(async () => undefined),
    queueMove: vi.fn(async () => undefined),
    queueRemoveAt: vi.fn(async () => undefined),
    clearQueue: vi.fn(async () => undefined),
  };
});

import QueuePanel from "../QueuePanel.svelte";
import * as api from "../../lib/api";
import { app } from "../../lib/stores.svelte";
import type { QueueView, Track } from "../../lib/api";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const getQueue = vi.mocked(api.getQueue);
const getTracks = vi.mocked(api.getTracks);
const clearQueue = vi.mocked(api.clearQueue);

/** Minimal `Track`. `cover_key: null` keeps `Cover` on its placeholder
 *  branch so it never calls `convertFileSrc` (no Tauri internals). */
function track(id: number, title: string, artist: string): Track {
  return {
    id,
    track_uid: `uid-${id}`,
    path: `/music/${id}.flac`,
    title,
    artist,
    album: "Album",
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

beforeEach(() => {
  app.lastError = null;
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

// ---------------------------------------------------------------------------
// R3.6 — explicit empty state + disabled clear button
// ---------------------------------------------------------------------------

describe("QueuePanel — état vide (R3.6)", () => {
  it("affiche l'état vide explicite FR quand la file est vide", async () => {
    getQueue.mockResolvedValue({ items: [], cursor: null } satisfies QueueView);
    const { getByText, container } = render(QueuePanel);

    await waitFor(() =>
      expect(getByText("Votre file est vide.")).toBeInTheDocument()
    );
    // The hint explains how to add tracks (R3.6).
    expect(
      getByText(/Ajoutez des titres via le menu d'une piste/)
    ).toBeInTheDocument();

    // Empty state is exposed as a status region.
    expect(container.querySelector(".empty[role='status']")).not.toBeNull();
  });

  it("désactive « Vider la file » et n'appelle pas clearQueue quand la file est vide", async () => {
    getQueue.mockResolvedValue({ items: [], cursor: null } satisfies QueueView);
    const { getByLabelText } = render(QueuePanel);

    await waitFor(() => expect(getQueue).toHaveBeenCalled());

    const clearBtn = getByLabelText("Vider la file") as HTMLButtonElement;
    expect(clearBtn).toBeDisabled();

    // Even if forced, the handler guards on length === 0.
    await fireEvent.click(clearBtn);
    expect(clearQueue).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// R3.7 (adjacent) — clear button calls clearQueue on a non-empty queue
// ---------------------------------------------------------------------------

describe("QueuePanel — « Vider la file » (R3.7)", () => {
  it("clique sur « Vider la file » → appelle clearQueue", async () => {
    getQueue.mockResolvedValue({ items: [10, 20], cursor: 0 } satisfies QueueView);
    getTracks.mockResolvedValue([
      track(10, "Premier titre", "Artiste A"),
      track(20, "Second titre", "Artiste B"),
    ]);

    const { getByLabelText, getByText } = render(QueuePanel);

    // Wait for the rows to render (load() resolved).
    await waitFor(() => expect(getByText("Premier titre")).toBeInTheDocument());

    const clearBtn = getByLabelText("Vider la file") as HTMLButtonElement;
    expect(clearBtn).not.toBeDisabled();

    await fireEvent.click(clearBtn);
    await waitFor(() => expect(clearQueue).toHaveBeenCalledTimes(1));
  });
});

// ---------------------------------------------------------------------------
// R3.5 / R2 — accessibility: roles and labels
// ---------------------------------------------------------------------------

describe("QueuePanel — accessibilité rôles/labels (R3.5)", () => {
  it("expose la section avec aria-label « File d'attente »", async () => {
    getQueue.mockResolvedValue({ items: [], cursor: null } satisfies QueueView);
    const { container } = render(QueuePanel);

    await waitFor(() => expect(getQueue).toHaveBeenCalled());
    const section = container.querySelector("section.queue-panel");
    expect(section).not.toBeNull();
    expect(section).toHaveAttribute("aria-label", "File d'attente");
  });

  it("rend chaque rangée comme un contrôle nommé (role=button + aria-label de lecture)", async () => {
    getQueue.mockResolvedValue({ items: [10, 20], cursor: 0 } satisfies QueueView);
    getTracks.mockResolvedValue([
      track(10, "Premier titre", "Artiste A"),
      track(20, "Second titre", "Artiste B"),
    ]);

    const { container, getByText } = render(QueuePanel);
    await waitFor(() => expect(getByText("Premier titre")).toBeInTheDocument());

    // One row body per queue item, each a named button-role control.
    const rowBodies = container.querySelectorAll(".row-body[role='button']");
    expect(rowBodies.length).toBe(2);

    const labels = Array.from(rowBodies).map((el) =>
      el.getAttribute("aria-label")
    );
    // Upcoming (non-current) rows carry the "Lire « … »" action label.
    expect(labels).toContain("Lire « Second titre » — Artiste B");

    // Per-row remove control is also labelled (R2.4).
    expect(
      container.querySelector("button[aria-label='Retirer de la file']")
    ).not.toBeNull();
  });
});
