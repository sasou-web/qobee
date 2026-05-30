/**
 * @vitest-environment jsdom
 *
 * Tests du feedback de rescan de bibliothèque (tâche 21.2).
 *
 * Feature: qobee-beta-feedback-improvements
 *
 * Validates: Requirements 6.1 (toast « démarré » au déclenchement),
 *            6.2 (toast « terminé · N modifiés » à la fin),
 *            6.3 (toast d'erreur descriptif FR sur échec),
 *            6.4 (garde anti-concurrence : `scanRunning` vrai pendant un scan).
 *
 * ---------------------------------------------------------------------------
 * Surface testée
 * ---------------------------------------------------------------------------
 * Le câblage du feedback de rescan vit dans `AppStore.wire()`
 * (`stores.svelte.ts`) : les écouteurs `onScanStarted` / `onScanFinished` /
 * `onScanError` (de simples enveloppes Tauri exposées par `api.ts`)
 * basculent `app.scanRunning` et émettent les toasts FR. Le toast « Scan
 * démarré… » du déclenchement (R6.1) est levé dans `Settings.handleRescanAll`
 * mais reflété ici par la garde `scanRunning = true` (R6.4) que l'événement
 * `library:scan-started` pose.
 *
 * On isole donc ce câblage en :
 *  - mockant `../api` pour CAPTURER le callback de chaque écouteur que
 *    `wire()` enregistre (et en renvoyant un unlisten no-op), et pour faire
 *    résoudre `getPlayerState` + les `refresh*` appelés par `wire()` /
 *    `refreshAll()` ;
 *  - espionnant les méthodes du singleton `toasts` (`success`/`error`/`info`).
 *
 * On invoque ensuite les callbacks capturés et on asserte l'effet sur
 * `app.scanRunning` et sur les toasts.
 */
import {
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type MockInstance,
} from "vitest";

// ---------------------------------------------------------------------------
// Conteneur de callbacks capturés. `vi.hoisted` le rend disponible dans la
// factory `vi.mock` (hoistée au-dessus des imports) tout en le gardant
// accessible depuis les tests.
// ---------------------------------------------------------------------------

type ScanResultLike = {
  files_visited: number;
  files_indexed: number;
  errors: string[];
};

const captured = vi.hoisted(() => {
  return {
    scanStarted: undefined as undefined | ((p: { roots: number }) => void),
    scanFinished: undefined as undefined | ((r: ScanResultLike) => void),
    scanError: undefined as undefined | ((message: string) => void),
    scanProgress: undefined as undefined | ((p: unknown) => void),
    playerState: undefined as undefined | ((s: unknown) => void),
    playerPosition: undefined as undefined | ((p: number) => void),
    playerEndOfTrack: undefined as undefined | (() => void),
    playerError: undefined as undefined | ((m: string) => void),
  };
});

// ---------------------------------------------------------------------------
// Mock du module `../api`. Chaque écouteur capture son callback et renvoie un
// unlisten no-op. Les commandes/lectures utilisées par `wire()` et
// `refreshAll()` résolvent vers des valeurs vides afin que le câblage tourne
// sans toucher le backend Tauri.
// ---------------------------------------------------------------------------

vi.mock("../api", () => {
  const noop = () => {};
  const idlePlayerState = {
    status: "idle",
    current_track_id: null,
    position_seconds: 0,
    duration_seconds: 0,
    volume: 1,
    output_mode: "shared",
    sample_rate: null,
    bit_depth: null,
    channels: null,
    is_bit_perfect: false,
    rg_attenuation_db: null,
    error: null,
  };

  return {
    // Lecture initiale dans wire().
    getPlayerState: vi.fn(async () => idlePlayerState),

    // Lectures déclenchées par refreshAll() après un scan terminé.
    listAlbums: vi.fn(async () => []),
    listArtists: vi.fn(async () => []),
    listGenres: vi.fn(async () => []),
    listLibraryRoots: vi.fn(async () => []),
    listPlaylists: vi.fn(async () => []),
    recentlyPlayedTracks: vi.fn(async () => []),
    recentlyPlayedAlbums: vi.fn(async () => []),
    recentlyPlayedArtists: vi.fn(async () => []),

    // Écouteurs de lecteur (capturés pour ne rien fuiter, non utilisés ici).
    onPlayerState: vi.fn(async (cb: (s: unknown) => void) => {
      captured.playerState = cb;
      return noop;
    }),
    onPlayerPosition: vi.fn(async (cb: (p: number) => void) => {
      captured.playerPosition = cb;
      return noop;
    }),
    onPlayerEndOfTrack: vi.fn(async (cb: () => void) => {
      captured.playerEndOfTrack = cb;
      return noop;
    }),
    onPlayerError: vi.fn(async (cb: (m: string) => void) => {
      captured.playerError = cb;
      return noop;
    }),

    // Écouteurs de scan : la cible de ces tests.
    onScanProgress: vi.fn(async (cb: (p: unknown) => void) => {
      captured.scanProgress = cb;
      return noop;
    }),
    onScanStarted: vi.fn(async (cb: (p: { roots: number }) => void) => {
      captured.scanStarted = cb;
      return noop;
    }),
    onScanFinished: vi.fn(async (cb: (r: ScanResultLike) => void) => {
      captured.scanFinished = cb;
      return noop;
    }),
    onScanError: vi.fn(async (cb: (message: string) => void) => {
      captured.scanError = cb;
      return noop;
    }),
  };
});

// Imports APRÈS le mock (qui est hoisté de toute façon) : `app` est le
// singleton du store, `toasts` le singleton de notifications à espionner.
import { app } from "../stores.svelte";
import { toasts } from "../toasts.svelte";

// ---------------------------------------------------------------------------
// Setup : on câble le store une seule fois (garde `wired`), puis on
// (ré)espionne les toasts avant chaque test. `scanRunning` est remis à false
// pour repartir d'un état connu.
// ---------------------------------------------------------------------------

let successSpy: MockInstance<(message: string, durationMs?: number) => number>;
let errorSpy: MockInstance<(message: string, durationMs?: number) => number>;
let infoSpy: MockInstance<(message: string, durationMs?: number) => number>;

beforeAll(async () => {
  // Câble les écouteurs (capture les callbacks via le mock de `../api`).
  await app.wire();
});

beforeEach(() => {
  // Implémentations no-op pour éviter d'écrire dans le $state des toasts et
  // de programmer des setTimeout réels pendant le test.
  successSpy = vi.spyOn(toasts, "success").mockImplementation(() => 0);
  errorSpy = vi.spyOn(toasts, "error").mockImplementation(() => 0);
  infoSpy = vi.spyOn(toasts, "info").mockImplementation(() => 0);
  app.scanRunning = false;
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// R6.4 — garde anti-concurrence : scan-started pose scanRunning = true.
// ---------------------------------------------------------------------------

describe("library:scan-started → garde anti-concurrence (R6.4)", () => {
  it("câble bien un écouteur onScanStarted", () => {
    expect(captured.scanStarted).toBeTypeOf("function");
  });

  it("bascule app.scanRunning à true au démarrage du scan", () => {
    expect(app.scanRunning).toBe(false);
    captured.scanStarted?.({ roots: 2 });
    expect(app.scanRunning).toBe(true);
  });

  it("ne lève pas de toast de succès/erreur au seul démarrage (R6.1 levé côté Settings)", () => {
    captured.scanStarted?.({ roots: 1 });
    expect(successSpy).not.toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
    // R6.1 — le toast « Scan démarré… » est levé dans
    // Settings.handleRescanAll, jamais par le câblage du store : aucun
    // `toasts.info` ne doit partir de l'événement `library:scan-started`.
    expect(infoSpy).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// R6.2 — scan-finished : toast « terminé · N modifiés » + relâche la garde.
// ---------------------------------------------------------------------------

describe("library:scan-finished → toast de fin (R6.2)", () => {
  it("câble bien un écouteur onScanFinished", () => {
    expect(captured.scanFinished).toBeTypeOf("function");
  });

  it("affiche un toast de succès mentionnant le nombre de titres modifiés", async () => {
    // Pré-condition : un scan est en cours.
    app.scanRunning = true;

    await captured.scanFinished?.({
      files_visited: 10,
      files_indexed: 7,
      errors: [],
    });

    expect(successSpy).toHaveBeenCalledTimes(1);
    const message = successSpy.mock.calls[0]?.[0] as string;
    expect(message).toContain("7");
    expect(message).toContain("modifiés");
    // R6.4 : la fin du scan relâche la garde.
    expect(app.scanRunning).toBe(false);
    // Aucun toast d'erreur sur un succès.
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it("reporte le compteur files_indexed même quand il vaut 0", async () => {
    app.scanRunning = true;

    await captured.scanFinished?.({
      files_visited: 4,
      files_indexed: 0,
      errors: [],
    });

    expect(successSpy).toHaveBeenCalledTimes(1);
    const message = successSpy.mock.calls[0]?.[0] as string;
    expect(message).toContain("0");
    expect(message).toContain("modifiés");
    expect(app.scanRunning).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// R6.3 — scan-error : toast d'erreur descriptif FR + relâche la garde.
// ---------------------------------------------------------------------------

describe("library:scan-error → toast d'erreur (R6.3)", () => {
  it("câble bien un écouteur onScanError", () => {
    expect(captured.scanError).toBeTypeOf("function");
  });

  it("affiche un toast d'erreur contenant le message du backend et relâche la garde", () => {
    app.scanRunning = true;

    captured.scanError?.("boom");

    expect(errorSpy).toHaveBeenCalledTimes(1);
    const message = errorSpy.mock.calls[0]?.[0] as string;
    expect(message).toContain("boom");
    // Message FR descriptif (préfixe « Échec du scan »).
    expect(message).toContain("Échec du scan");
    // R6.4 : un échec global relâche aussi la garde.
    expect(app.scanRunning).toBe(false);
    // Pas de toast de succès sur un échec.
    expect(successSpy).not.toHaveBeenCalled();
  });
});
