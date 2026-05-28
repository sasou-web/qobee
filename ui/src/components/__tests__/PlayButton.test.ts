/**
 * @vitest-environment jsdom
 *
 * Validates: Requirements R3.1, R3.5, R3.6
 *
 * R3 Play_Button — three behaviour invariants:
 *
 *  1. R3.1 — Clicking the button MUST NOT bubble navigation: the
 *     PlayButton is hosted inside clickable rows / cards / cover
 *     overlays whose own onclick triggers a route change. The
 *     button calls e.stopPropagation() so a click never reaches
 *     that handler, regardless of `target.kind`.
 *
 *  2. R3.5 — The visible state MUST derive deterministically from
 *     the (matches, status) pair held in `playerStore`: each
 *     combination maps to a specific class / aria-label / icon.
 *
 *  3. R3.6 — The error state MUST be scoped to the button's
 *     target track id. An `Errored` for another track must not
 *     light up this button.
 *
 * The Tauri command surface (`lib/api`) is mocked so click
 * handlers never actually reach the backend; the singleton
 * `playerStore` is reset between tests via a manual reset helper.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { tick } from "svelte";

// Mock the typed Tauri command surface used by PlayButton. Each
// command resolves immediately. Doing this BEFORE the component
// import ensures the SUT picks up the mocked module.
vi.mock("../../lib/api", () => ({
  playTrack: vi.fn(async () => undefined),
  playAlbum: vi.fn(async () => undefined),
  playPlaylist: vi.fn(async () => undefined),
  pause: vi.fn(async () => undefined),
  resume: vi.fn(async () => undefined),
}));

import PlayButton from "../PlayButton.svelte";
import { playerStore } from "../../lib/playerStore.svelte";
import type { TrackMetaDto } from "../../lib/playerEvent";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Reset the singleton player store so tests don't leak state. */
function resetPlayerStore(): void {
  playerStore.status = "idle";
  playerStore.currentTrack = null;
  playerStore.positionMs = 0;
  playerStore.durationMs = 0;
  playerStore.lastError = null;
}

/** Build a minimal `TrackMetaDto` for a given id. */
function trackMeta(id: number): TrackMetaDto {
  return {
    id,
    title: `Track ${id}`,
    artist: "Artist",
    album: "Album",
    duration_ms: 60_000,
  };
}

// `Icon.svelte` renders the play glyph as a single `<path>` and
// the pause glyph as two `<rect>` elements. We probe those raw
// SVG elements rather than relying on a name attribute because
// the component does not expose one to the DOM.
function hasPlayIcon(root: ParentNode): boolean {
  return root.querySelector("svg path") !== null;
}
function hasPauseIcon(root: ParentNode): boolean {
  return root.querySelector("svg rect") !== null;
}

beforeEach(() => {
  resetPlayerStore();
  // The click handler in PlayButton schedules a setTimeout(4000)
  // safety ceiling per click that drops the `pending` guard if
  // the backend never responds. Fake timers prevent that from
  // leaking across tests and keep teardown synchronous.
  vi.useFakeTimers();
});

afterEach(() => {
  cleanup();
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
  vi.clearAllMocks();
});

// ---------------------------------------------------------------------------
// R3.1 — click never navigates
// ---------------------------------------------------------------------------

describe("PlayButton — click never navigates (R3.1)", () => {
  // We attach a navigation spy ABOVE the test's mount target so
  // we can observe whether the click bubbles past it. Note that
  // we cannot use `container.addEventListener(...)` here:
  // `@testing-library/svelte` mounts the component inside that
  // container, and Svelte 5 wires its event delegation onto the
  // same container — so `e.stopPropagation()` inside the
  // component's click handler does NOT stop a sibling listener
  // attached to the very same node. In production the
  // delegation root is App.svelte's root and the row/card's
  // navigation handler sits on a node further up the bubble
  // path; `document.body` plays that role here.
  //
  // The invariant must hold for every `target.kind` (track /
  // album / playlist) because the click handler routes
  // differently per kind (playTrack / playAlbum / playPlaylist)
  // yet always calls `stopPropagation()` up-front.
  for (const kind of ["track", "album", "playlist"] as const) {
    it(`stops propagation for kind=${kind}`, async () => {
      const navSpy = vi.fn();
      document.body.addEventListener("click", navSpy);
      try {
        const { getByRole } = render(PlayButton, {
          props: { target: { kind, id: 42 } },
        });
        const btn = getByRole("button");
        await fireEvent.click(btn);
        expect(navSpy).not.toHaveBeenCalled();
      } finally {
        document.body.removeEventListener("click", navSpy);
      }
    });
  }
});

// ---------------------------------------------------------------------------
// R3.5 — state derivation across (matches, status) combinations
// ---------------------------------------------------------------------------

describe("PlayButton — state derivation (R3.5)", () => {
  // matches=false (no track loaded), status=idle.
  it("matches=false, status=idle → state-idle, aria 'Play', play icon", async () => {
    const { getByRole, container } = render(PlayButton, {
      props: { target: { kind: "track", id: 1 } },
    });
    await tick();
    const btn = getByRole("button");
    expect(btn).toHaveClass("state-idle");
    expect(btn).toHaveAttribute("aria-label", "Play");
    expect(hasPlayIcon(container)).toBe(true);
    expect(hasPauseIcon(container)).toBe(false);
  });

  // matches=true (this is the loaded track), status=playing.
  it("matches=true, status=playing → state-playing, aria 'Pause', pause icon", async () => {
    playerStore.currentTrack = trackMeta(1);
    playerStore.status = "playing";
    const { getByRole, container } = render(PlayButton, {
      props: { target: { kind: "track", id: 1 } },
    });
    await tick();
    const btn = getByRole("button");
    expect(btn).toHaveClass("state-playing");
    expect(btn).toHaveAttribute("aria-label", "Pause");
    expect(hasPauseIcon(container)).toBe(true);
    expect(hasPlayIcon(container)).toBe(false);
  });

  // matches=true, status=paused.
  it("matches=true, status=paused → state-paused, aria 'Play', play icon", async () => {
    playerStore.currentTrack = trackMeta(1);
    playerStore.status = "paused";
    const { getByRole, container } = render(PlayButton, {
      props: { target: { kind: "track", id: 1 } },
    });
    await tick();
    const btn = getByRole("button");
    expect(btn).toHaveClass("state-paused");
    expect(btn).toHaveAttribute("aria-label", "Play");
    expect(hasPlayIcon(container)).toBe(true);
    expect(hasPauseIcon(container)).toBe(false);
  });

  // matches=false (a *different* track is loaded), status=playing.
  // The button must NOT pretend to be the active one — it stays
  // idle so the user can start its own target.
  it("matches=false (other track playing), status=playing → state-idle", async () => {
    playerStore.currentTrack = trackMeta(99);
    playerStore.status = "playing";
    const { getByRole, container } = render(PlayButton, {
      props: { target: { kind: "track", id: 1 } },
    });
    await tick();
    const btn = getByRole("button");
    expect(btn).toHaveClass("state-idle");
    expect(btn).toHaveAttribute("aria-label", "Play");
    expect(hasPlayIcon(container)).toBe(true);
    expect(hasPauseIcon(container)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// R3.6 — error scoping to the target track id
// ---------------------------------------------------------------------------

describe("PlayButton — error scoping (R3.6)", () => {
  // Engine reported a failure for track id 99; this button is
  // for track id 1. The error state is scoped to the target's
  // own track id so this button must stay idle.
  it("does NOT enter state-error when the failed trackId is a different id", async () => {
    playerStore.currentTrack = trackMeta(99);
    playerStore.lastError = {
      trackId: 99,
      kind: "decode_failed",
      message: "boom",
    };
    const { getByRole } = render(PlayButton, {
      props: { target: { kind: "track", id: 1 } },
    });
    await tick();
    const btn = getByRole("button");
    expect(btn).not.toHaveClass("state-error");
    expect(btn).toHaveClass("state-idle");
    expect(btn).not.toHaveAttribute("title");
    expect(btn).not.toHaveAttribute("aria-describedby");
  });

  // Same store error mapped to OUR track id — the button DOES
  // light up, surfaces the message via `title`, and wires
  // aria-describedby to the sr-only span carrying the cause.
  it("enters state-error when the failed trackId matches our target", async () => {
    playerStore.currentTrack = trackMeta(1);
    playerStore.lastError = {
      trackId: 1,
      kind: "decode_failed",
      message: "boom",
    };
    const { getByRole } = render(PlayButton, {
      props: { target: { kind: "track", id: 1 } },
    });
    await tick();
    const btn = getByRole("button");
    expect(btn).toHaveClass("state-error");
    expect(btn).toHaveAttribute("title", "boom");
    expect(btn).toHaveAttribute("aria-describedby");
  });

  // An error for a track that isn't loaded as `currentTrack`
  // (lastError.trackId points at someone we never matched).
  // Still must not blow up our button: it stays idle.
  it("does NOT enter state-error when there is no current track and the error is unrelated", async () => {
    playerStore.currentTrack = null;
    playerStore.lastError = {
      trackId: 7,
      kind: "decode_failed",
      message: "elsewhere",
    };
    const { getByRole } = render(PlayButton, {
      props: { target: { kind: "track", id: 1 } },
    });
    await tick();
    const btn = getByRole("button");
    expect(btn).not.toHaveClass("state-error");
    expect(btn).toHaveClass("state-idle");
  });
});
