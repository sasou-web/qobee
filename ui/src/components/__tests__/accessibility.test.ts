/**
 * @vitest-environment jsdom
 *
 * Tâche 20.4 — Tests Vitest ARIA / toasts / clavier / infobulles.
 *
 * Feature: qobee-beta-feedback-improvements
 *
 * Validates: Requirements 2.1 (rôles + noms accessibles non vides),
 *            2.5 (aria-pressed mis à jour au toggle),
 *            2.6 (écran d'aide des raccourcis présent),
 *            2.8 (activation clavier Entrée/Espace au niveau composant),
 *            2.9 (région live `role="status"` du ToastsRoot + announce),
 *            5.1 / 5.2 / 5.5 (`title === aria-label`, source unique des
 *            libellés Sidebar/Player), 5.4 (persistance de `sidebarExpanded`).
 *
 * ---------------------------------------------------------------------------
 * Stratégie
 * ---------------------------------------------------------------------------
 * On asservit le CÂBLAGE d'accessibilité réellement livré par les tâches
 * 3.x / 20.1 / 20.2, en rendant les vrais composants (`Sidebar`,
 * `PlayerBar`, `ToastsRoot`, `ShortcutsHelp`, `QueuePanel`) dans jsdom.
 *
 *  - Source unique des libellés (R2.4/R5.5) : on prouve que chaque bouton
 *    de nav Sidebar et chaque contrôle iconographique du PlayerBar
 *    dérivent `aria-label` ET `title` de la MÊME chaîne
 *    (`SIDEBAR_LABELS` / `PLAYER_LABELS`) — donc aucune divergence
 *    possible entre lecteur d'écran et infobulle.
 *  - `aria-pressed` (R2.5) : les toggles favori / shuffle / file du
 *    PlayerBar exposent `aria-pressed` reflétant l'état, et le mettent à
 *    jour au clic.
 *  - Région live (R2.9) : le `ToastsRoot` enveloppe la pile dans
 *    `role="status" aria-live="polite"` et un toast poussé y apparaît ;
 *    `announce` recopie le message dans la région live partagée.
 *  - Activation clavier (R2.8) : `activateOnKey` est déjà couvert en
 *    isolation par `lib/__tests__/a11y.test.ts` ; on ajoute UN test de
 *    niveau composant (une rangée de `QueuePanel` s'active sur Entrée →
 *    `queueJumpTo`) qui prouve le câblage de l'action dans un vrai
 *    composant.
 *
 * Les composants `PlayerBar` / `Sidebar` ont un graphe d'import lourd
 * (commandes Tauri, stores). On mocke `../../lib/api` via `importOriginal`
 * (on garde `coverUrl`, les types, etc. et on ne pilote que les commandes
 * appelées au montage / au clic), suivant le pattern des fichiers de test
 * existants. Les vrais stores (`app`, `settings`, `toasts`,
 * `queuePopover`, `audioSettings`) sont conservés.
 *
 * Limites jsdom honnêtes :
 *  - jsdom n'a pas la Web Animations API : on stub `Element.prototype.
 *    animate` / `getAnimations` (transitions Svelte des composants).
 *  - jsdom n'a pas `ResizeObserver` (utilisé par `PlayerBar` pour le
 *    relayout R1.5) : on pose un stub no-op.
 *  - Les contrastes / rendu pixel (R2.2 focus visible) ne sont PAS
 *    vérifiables ici — ils relèvent des contrôles manuels documentés
 *    dans `docs/accessibility-validation.md`.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";

// jsdom ne fournit pas la Web Animations API. Les transitions Svelte
// (`fly`/`fade`/`scale`, `animate:flip`) appellent `element.animate()` /
// `getAnimations()` et lèveraient sinon. Des stubs no-op suffisent : on
// n'asserte pas l'animation, seulement les attributs ARIA et le DOM.
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

// jsdom n'implémente pas `ResizeObserver`. `PlayerBar` en instancie un
// dans `onMount` (relayout R1.5) ; un stub no-op évite un throw au montage.
if (typeof (globalThis as { ResizeObserver?: unknown }).ResizeObserver === "undefined") {
  class ResizeObserverStub {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  (globalThis as { ResizeObserver?: unknown }).ResizeObserver =
    ResizeObserverStub as unknown as typeof ResizeObserver;
}

// Mock the typed Tauri command surface BEFORE importing the components so
// the SUT and the real stores it pulls in bind to the mock. We keep the
// real module (types, `coverUrl`, …) via `importOriginal` and only pilot
// the commands touched at mount / on click.
vi.mock("../../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/api")>();
  return {
    ...actual,
    // --- PlayerBar mount / interaction surface ---
    getTrack: vi.fn(async () => null),
    albumIdForTrack: vi.fn(async () => null),
    isFavorite: vi.fn(async () => false),
    getShuffle: vi.fn(async () => false),
    getRepeatMode: vi.fn(async () => "off"),
    setShuffle: vi.fn(async () => undefined),
    setRepeatMode: vi.fn(async () => undefined),
    addFavorite: vi.fn(async () => undefined),
    removeFavorite: vi.fn(async () => undefined),
    seek: vi.fn(async () => undefined),
    setVolume: vi.fn(async () => undefined),
    // --- audioSettings store (volume curve warm-up) ---
    listAudioSettings: vi.fn(async () => ({})),
    // --- SleepTimer (polls on an interval) ---
    getSleepTimer: vi.fn(async () => ({
      remaining_secs: null,
      stop_after_track: false,
    })),
    // --- Sidebar ---
    createPlaylist: vi.fn(),
    // --- QueuePanel ---
    getQueue: vi.fn(async () => ({ items: [], cursor: null })),
    getTracks: vi.fn(async () => []),
    queueJumpTo: vi.fn(async () => undefined),
    queueMove: vi.fn(async () => undefined),
    queueRemoveAt: vi.fn(async () => undefined),
    clearQueue: vi.fn(async () => undefined),
  };
});

import Sidebar from "../Sidebar.svelte";
import PlayerBar from "../PlayerBar.svelte";
import ToastsRoot from "../ToastsRoot.svelte";
import ShortcutsHelp from "../ShortcutsHelp.svelte";
import QueuePanel from "../QueuePanel.svelte";
import * as api from "../../lib/api";
import { app } from "../../lib/stores.svelte";
import { settings } from "../../lib/settings.svelte";
import { toasts } from "../../lib/toasts.svelte";
import { queuePopover } from "../../lib/queuePopover.svelte";
import { SIDEBAR_LABELS, PLAYER_LABELS } from "../../lib/labels";
import type { QueueView, Track } from "../../lib/api";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/** Drop the shared a11y live region created by `announce` so it doesn't
 *  leak across tests (module singleton attached to `document.body`). */
function clearLiveRegion(): void {
  document
    .querySelectorAll("[data-a11y-live-region]")
    .forEach((el) => el.remove());
}

beforeEach(() => {
  // Reset singleton stores mutated by individual tests.
  app.player = { ...app.player, current_track_id: null };
  app.lastError = null;
  app.playlists = [];
  queuePopover.open = false;
  settings.values = { ...settings.values, sidebarExpanded: false };
  toasts.clear();
});

afterEach(() => {
  cleanup();
  clearLiveRegion();
  vi.clearAllMocks();
});

// ---------------------------------------------------------------------------
// R5.1 / R5.5 — Sidebar : title === aria-label === SIDEBAR_LABELS[…].label
// ---------------------------------------------------------------------------

describe("Sidebar — libellés issus de la source unique (R5.1, R5.5)", () => {
  it("chaque destination de nav dérive aria-label ET title de SIDEBAR_LABELS", () => {
    const { container } = render(Sidebar);

    // The primary nav lists the destinations in DOM order = visual order
    // (R2.3) and in the same order as the label table.
    const order: (keyof typeof SIDEBAR_LABELS)[] = [
      "home",
      "search",
      "albums",
      "artists",
      "genres",
      "favorites",
      "queue",
    ];

    const buttons = container.querySelectorAll<HTMLButtonElement>(
      "nav.primary > button"
    );
    expect(buttons.length).toBe(order.length);

    order.forEach((key, i) => {
      const btn = buttons[i];
      expect(btn).toBeDefined();
      const label = SIDEBAR_LABELS[key].label;
      // Non-empty accessible name (R2.1) …
      expect(label.length).toBeGreaterThan(0);
      // … sourced identically for the screen reader (aria-label) and the
      // tooltip (title) — single source, zero divergence (R5.5).
      expect(btn?.getAttribute("aria-label")).toBe(label);
      expect(btn?.getAttribute("title")).toBe(label);
    });
  });

  it("le conteneur <nav> de navigation porte un nom accessible (R2.1)", () => {
    const { container } = render(Sidebar);
    const aside = container.querySelector("aside");
    expect(aside).not.toBeNull();
    expect(aside).toHaveAttribute("aria-label", "Navigation");
  });
});

// ---------------------------------------------------------------------------
// R5.4 — persistance de l'option « Sidebar étendue »
// ---------------------------------------------------------------------------

describe("Sidebar — persistance de sidebarExpanded (R5.4)", () => {
  it("basculer l'option appelle settings.set('sidebarExpanded', true)", async () => {
    // Spy with a no-op impl so we don't hit `setSetting`/`applyVisual`.
    const setSpy = vi.spyOn(settings, "set").mockResolvedValue(undefined);

    const { getByLabelText } = render(Sidebar);

    // Default OFF → the toggle exposes the "Sidebar étendue" name.
    const toggle = getByLabelText("Sidebar étendue") as HTMLButtonElement;
    // The toggle also exposes its state via aria-pressed (R2.5).
    expect(toggle).toHaveAttribute("aria-pressed", "false");

    await fireEvent.click(toggle);

    expect(setSpy).toHaveBeenCalledWith("sidebarExpanded", true);
  });
});

// ---------------------------------------------------------------------------
// R2.5 / R5.2 / R5.5 — PlayerBar : aria-pressed + title===aria-label
// ---------------------------------------------------------------------------

describe("PlayerBar — aria-pressed des toggles (R2.5)", () => {
  it("le bouton shuffle reflète l'état backend et le met à jour au clic", async () => {
    vi.mocked(api.getShuffle).mockResolvedValue(true);

    const { container, getByLabelText } = render(PlayerBar);

    // Backend says shuffle is on → aria-pressed reflects it.
    await waitFor(() => expect(getByLabelText("Shuffle on")).toBeInTheDocument());
    const shuffle = container.querySelector(
      ".left > button.ctrl[aria-pressed]"
    ) as HTMLButtonElement;
    expect(shuffle).not.toBeNull();
    expect(shuffle).toHaveAttribute("aria-pressed", "true");
    // Single source for SR + tooltip (R5.5): title mirrors aria-label.
    expect(shuffle.getAttribute("title")).toBe(shuffle.getAttribute("aria-label"));

    // Toggling flips the reflected state immediately (R2.5).
    await fireEvent.click(shuffle);
    await tick();
    expect(shuffle).toHaveAttribute("aria-pressed", "false");
    expect(api.setShuffle).toHaveBeenCalledWith(false);
  });

  it("le bouton file expose aria-pressed lié au popover et bascule au clic", async () => {
    const { container } = render(PlayerBar);

    const queueBtn = container.querySelector(
      ".right button.ctrl[aria-pressed]"
    ) as HTMLButtonElement;
    expect(queueBtn).not.toBeNull();
    // Closed popover → not pressed; label + title from PLAYER_LABELS (R5.2).
    expect(queueBtn).toHaveAttribute("aria-pressed", "false");
    expect(queueBtn.getAttribute("aria-label")).toBe(PLAYER_LABELS.queue.label);
    expect(queueBtn.getAttribute("title")).toBe(PLAYER_LABELS.queue.label);

    await fireEvent.click(queueBtn);
    await tick();
    expect(queueBtn).toHaveAttribute("aria-pressed", "true");
    expect(queuePopover.open).toBe(true);
  });

  it("le bouton favori expose aria-pressed + nom accessible dynamiques (R2.5/R5.5)", async () => {
    // A track is loaded and already favorited.
    app.player = { ...app.player, current_track_id: "10" };
    vi.mocked(api.getTrack).mockResolvedValue(track(10, "Titre", "Artiste"));
    vi.mocked(api.isFavorite).mockResolvedValue(true);

    const { container } = render(PlayerBar);

    const heart = container.querySelector("button.heart") as HTMLButtonElement;
    expect(heart).not.toBeNull();

    // Favorited → pressed + "unfavorite" name, mirrored on title (R5.5).
    await waitFor(() => expect(heart).toHaveAttribute("aria-pressed", "true"));
    expect(heart.getAttribute("aria-label")).toBe(PLAYER_LABELS.unfavorite.label);
    expect(heart.getAttribute("title")).toBe(PLAYER_LABELS.unfavorite.label);

    // Un-favoriting flips both the pressed state and the accessible name.
    await fireEvent.click(heart);
    await tick();
    expect(heart).toHaveAttribute("aria-pressed", "false");
    expect(heart.getAttribute("aria-label")).toBe(PLAYER_LABELS.favorite.label);
    expect(heart.getAttribute("title")).toBe(PLAYER_LABELS.favorite.label);
    expect(api.removeFavorite).toHaveBeenCalledWith(10);
  });

  it("les contrôles précédent/suivant dérivent leurs libellés de PLAYER_LABELS (R5.2/R5.5)", () => {
    const { container } = render(PlayerBar);

    const cases: { label: string; expected: string }[] = [
      { label: PLAYER_LABELS.previous.label, expected: PLAYER_LABELS.previous.label },
      { label: PLAYER_LABELS.next.label, expected: PLAYER_LABELS.next.label },
    ];

    for (const c of cases) {
      const btn = container.querySelector(
        `button[aria-label="${c.label}"]`
      ) as HTMLButtonElement | null;
      expect(btn).not.toBeNull();
      // aria-label and title share the single source string.
      expect(btn?.getAttribute("aria-label")).toBe(c.expected);
      expect(btn?.getAttribute("title")).toBe(c.expected);
    }
  });
});

// ---------------------------------------------------------------------------
// R2.9 — ToastsRoot : région live + toast poussé annoncé
// ---------------------------------------------------------------------------

describe("ToastsRoot — région live ARIA (R2.9)", () => {
  it("expose la pile dans role=status aria-live=polite aria-atomic", () => {
    const { container } = render(ToastsRoot);
    const root = container.querySelector(".root");
    expect(root).not.toBeNull();
    expect(root).toHaveAttribute("role", "status");
    expect(root).toHaveAttribute("aria-live", "polite");
    expect(root).toHaveAttribute("aria-atomic", "true");
  });

  it("un toast poussé apparaît dans la région et est recopié via announce", async () => {
    const { container } = render(ToastsRoot);
    const message = "Ajouté à la file";

    // durationMs=0 → pas de timer de dismiss programmé (teardown propre).
    toasts.push(message, "info", 0);
    await tick();

    // Le message est rendu dans la région live visible du ToastsRoot.
    await waitFor(() =>
      expect(container.querySelector(".root")?.textContent).toContain(message)
    );

    // `announce` (R2.9) le recopie dans la région live partagée hors-écran
    // (écriture différée d'une frame). Couvre le câblage ToastsRoot→announce
    // sans dupliquer les assertions unitaires de `lib/__tests__/a11y.test.ts`.
    await waitFor(() => {
      const live = document.querySelector("[data-a11y-live-region]");
      expect(live?.textContent).toBe(message);
    });
  });
});

// ---------------------------------------------------------------------------
// R2.6 — ShortcutsHelp : écran d'aide des raccourcis présent
// ---------------------------------------------------------------------------

describe("ShortcutsHelp — liste des raccourcis (R2.6)", () => {
  it("rend le titre et des libellés dérivés de labels.ts", () => {
    const { getByText, getAllByRole } = render(ShortcutsHelp);

    expect(getByText("Raccourcis clavier")).toBeInTheDocument();

    // Action labels are sourced from `labels.ts` (R5.5 alignment).
    expect(getByText(PLAYER_LABELS.exitImmersive.label)).toBeInTheDocument();
    expect(getByText(SIDEBAR_LABELS.search.label)).toBeInTheDocument();

    // The grouped tables actually carry rows.
    expect(getAllByRole("table").length).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------------
// R2.8 — Activation clavier au niveau composant (QueuePanel)
// ---------------------------------------------------------------------------
// `activateOnKey` est couvert en isolation par `lib/__tests__/a11y.test.ts`.
// Ce test prouve le câblage de l'action DANS un composant : une rangée non
// courante de la file s'active sur Entrée et déclenche `queueJumpTo`.

describe("QueuePanel — activation clavier d'une rangée (R2.8)", () => {
  it("Entrée sur une rangée à venir déclenche queueJumpTo", async () => {
    vi.mocked(api.getQueue).mockResolvedValue({
      items: [10, 20],
      cursor: 0,
    } satisfies QueueView);
    vi.mocked(api.getTracks).mockResolvedValue([
      track(10, "Premier titre", "Artiste A"),
      track(20, "Second titre", "Artiste B"),
    ]);

    const { container, getByText } = render(QueuePanel);
    await waitFor(() => expect(getByText("Second titre")).toBeInTheDocument());

    const rows = container.querySelectorAll<HTMLElement>(
      ".row-body[role='button']"
    );
    expect(rows.length).toBe(2);

    // Row index 1 is upcoming (cursor is 0 → row 0 is current). Pressing
    // Enter on it should jump there via the wired `activateOnKey` action.
    const upcoming = rows[1];
    expect(upcoming).toBeDefined();
    await fireEvent.keyDown(upcoming as HTMLElement, { key: "Enter" });

    await waitFor(() => expect(api.queueJumpTo).toHaveBeenCalledWith(1));
  });
});
