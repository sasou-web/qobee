/**
 * @vitest-environment jsdom
 *
 * Tâche 18.3 — Tests Vitest de `PlaylistPickerDialog.svelte`.
 *
 * Feature: qobee-beta-feedback-improvements
 *
 * Validates: Requirements 3.1, 3.3, 3.8
 *
 * Trois invariants de feedback de persistance d'ajout à une playlist,
 * câblés sur la valeur de retour `n` de `addToPlaylist` (cf. correctif
 * couche 2, tâche 1.2) :
 *
 *  - R3.1 — Le sous-menu liste les playlists existantes (rangées
 *    cliquables) + l'action « Nouvelle playlist ».
 *  - R3.2/R3.3 — `n > 0` ⇒ toast succès « Ajouté à «P» » et AUCUN toast
 *    d'erreur ; `n === 0` ou exception ⇒ toast d'erreur descriptif et
 *    AUCUN toast de succès.
 *  - R3.8 — La touche Entrée dans le champ « nouvelle playlist » valide
 *    la création (appelle `createPlaylist`) comme le bouton de
 *    confirmation.
 *
 * Le surface de commandes Tauri (`lib/api`) est mockée pour piloter la
 * valeur retournée par `addToPlaylist`/`createPlaylist`. Les méthodes du
 * store `toasts` sont espionnées (`vi.spyOn`) pour observer succès vs
 * erreur sans planifier de vrais timers de dismiss.
 */
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";

// jsdom ne fournit pas la Web Animations API. Les transitions Svelte
// (`transition:fade` / `transition:scale` de la modale) appellent
// `element.animate()` au montage/démontage et lèveraient sinon. Des
// stubs no-op suffisent : on n'asserte pas l'animation, seulement le
// comportement de feedback (toasts) et le DOM.
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

// Mock the typed Tauri command surface BEFORE importing the component so
// the SUT (and the real `stores.svelte` it pulls in) bind to the mock.
vi.mock("../../lib/api", () => ({
  addToPlaylist: vi.fn(),
  createPlaylist: vi.fn(),
  // `app.refreshPlaylists()` runs after a successful add; keep it cheap.
  listPlaylists: vi.fn(async () => []),
}));

import PlaylistPickerDialog from "../PlaylistPickerDialog.svelte";
import * as api from "../../lib/api";
import { playlistPicker } from "../../lib/playlistPicker.svelte";
import { app } from "../../lib/stores.svelte";
import { toasts } from "../../lib/toasts.svelte";
import type { Playlist } from "../../lib/api";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const addToPlaylist = vi.mocked(api.addToPlaylist);
const createPlaylist = vi.mocked(api.createPlaylist);

/** Minimal `Playlist` row for the picker list. */
function playlist(id: number, name: string, trackCount = 0): Playlist {
  return {
    id,
    name,
    created_at: 0,
    updated_at: 0,
    track_count: trackCount,
    cover_key: null,
  };
}

let successSpy: MockInstance<(message: string, durationMs?: number) => number>;
let errorSpy: MockInstance<(message: string, durationMs?: number) => number>;

beforeEach(() => {
  // Singleton stores leak across tests: reset the picker + app state.
  playlistPicker.state = { open: true, trackIds: [1, 2] };
  app.playlists = [playlist(1, "Rock"), playlist(2, "Jazz")];
  app.lastError = null;

  // Spy on toast outcomes. mockImplementation avoids scheduling the
  // real dismiss timeout (window.setTimeout) so teardown stays clean.
  successSpy = vi.spyOn(toasts, "success").mockImplementation(() => 0);
  errorSpy = vi.spyOn(toasts, "error").mockImplementation(() => 0);

  createPlaylist.mockResolvedValue(playlist(99, "Nouvelle"));
});

afterEach(() => {
  cleanup();
  playlistPicker.state = { open: false, trackIds: [] };
  app.playlists = [];
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

// ---------------------------------------------------------------------------
// R3.1 — sub-menu lists existing playlists + "Nouvelle playlist"
// ---------------------------------------------------------------------------

describe("PlaylistPickerDialog — liste des playlists (R3.1)", () => {
  it("affiche une rangée par playlist existante et l'action « Nouvelle playlist »", () => {
    const { getByText, container } = render(PlaylistPickerDialog);

    // One clickable `.row` per existing playlist.
    const rows = container.querySelectorAll("button.row");
    expect(rows.length).toBe(2);
    expect(getByText("Rock")).toBeInTheDocument();
    expect(getByText("Jazz")).toBeInTheDocument();

    // The "+ Nouvelle playlist" entry is always present when not creating.
    expect(getByText("+ Nouvelle playlist")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// R3.2 / R3.3 — success vs error feedback keyed on the returned count
// ---------------------------------------------------------------------------

describe("PlaylistPickerDialog — feedback de persistance (R3.2/R3.3)", () => {
  it("n > 0 → toast succès, AUCUN toast d'erreur", async () => {
    addToPlaylist.mockResolvedValue(2);
    const { container } = render(PlaylistPickerDialog);

    const firstRow = container.querySelector("button.row") as HTMLElement;
    await fireEvent.click(firstRow);

    await waitFor(() => expect(successSpy).toHaveBeenCalledTimes(1));
    expect(successSpy).toHaveBeenCalledWith("Ajouté à «Rock»");
    expect(errorSpy).not.toHaveBeenCalled();
    expect(addToPlaylist).toHaveBeenCalledWith(1, [1, 2]);
  });

  it("n === 0 → toast d'erreur, AUCUN toast de succès", async () => {
    addToPlaylist.mockResolvedValue(0);
    const { container } = render(PlaylistPickerDialog);

    const firstRow = container.querySelector("button.row") as HTMLElement;
    await fireEvent.click(firstRow);

    await waitFor(() => expect(errorSpy).toHaveBeenCalledTimes(1));
    expect(successSpy).not.toHaveBeenCalled();
    // Descriptive French error mentioning the playlist name.
    expect(errorSpy.mock.calls[0]?.[0]).toContain("Rock");
  });

  it("exception → toast d'erreur, AUCUN toast de succès", async () => {
    addToPlaylist.mockRejectedValue(new Error("db locked"));
    const { container } = render(PlaylistPickerDialog);

    const firstRow = container.querySelector("button.row") as HTMLElement;
    await fireEvent.click(firstRow);

    await waitFor(() => expect(errorSpy).toHaveBeenCalledTimes(1));
    expect(successSpy).not.toHaveBeenCalled();
    expect(errorSpy.mock.calls[0]?.[0]).toContain("Rock");
  });
});

// ---------------------------------------------------------------------------
// R3.8 — Enter in the "new playlist" field triggers creation
// ---------------------------------------------------------------------------

describe("PlaylistPickerDialog — création par Entrée (R3.8)", () => {
  it("la touche Entrée dans le champ « nouvelle playlist » appelle createPlaylist", async () => {
    addToPlaylist.mockResolvedValue(2);
    const { getByText, getByPlaceholderText } = render(PlaylistPickerDialog);

    // Open the create form.
    await fireEvent.click(getByText("+ Nouvelle playlist"));
    await tick();

    const input = getByPlaceholderText("New playlist name") as HTMLInputElement;
    // Update the bound value, then press Enter.
    await fireEvent.input(input, { target: { value: "Chill" } });
    await fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(createPlaylist).toHaveBeenCalledTimes(1));
    expect(createPlaylist).toHaveBeenCalledWith("Chill");
  });

  it("Entrée sur un champ vide ne crée rien (garde-fou)", async () => {
    const { getByText, getByPlaceholderText } = render(PlaylistPickerDialog);

    await fireEvent.click(getByText("+ Nouvelle playlist"));
    await tick();

    const input = getByPlaceholderText("New playlist name") as HTMLInputElement;
    await fireEvent.keyDown(input, { key: "Enter" });
    await tick();

    expect(createPlaylist).not.toHaveBeenCalled();
  });
});
